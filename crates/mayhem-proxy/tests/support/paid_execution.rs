use super::*;
use mayhem_proto::proxy::finance::ProxySpendAuthorization;
use mayhem_proxy::{capacity, discovery, execution::PaidExecutor, financial};
use std::{path::Path, process::Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

struct Peer {
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
    identity: Identity,
    client: Arc<financial::Client>,
}
impl Peer {
    async fn start(rail: ProxyRail, f: &Fixture, bytes: &[u8], streaming: bool) -> Self {
        let request = if streaming {
            f.adapter.prepare_stream(bytes).unwrap()
        } else {
            f.adapter.prepare_json(bytes).unwrap()
        };
        let execution = json!({"endpoint":request.endpoint(),"endpoint_contract":f.adapter.contract_hash(),
            "recipe_hash":f.adapter.recipe_hash(),"connection_revision":f.connection.revision(),
            "connection_digest":f.connection.fingerprint(),"capacity_group":d(200),"request_hash":request.request_hash(),
            "metering":mayhem_proxy::metering::Policy::for_endpoint(request.endpoint()).contract()});
        let family = if request.endpoint() == ProxyEndpoint::Decisions {
            "decisions"
        } else {
            "llm"
        };
        let rail = serde_json::to_value(rail).unwrap();
        let mut child = tokio::process::Command::new("node")
            .arg("intercom/tests/helpers/proxy-financial-rpc-fixture.mjs")
            .arg(rail.as_str().unwrap())
            .arg(family)
            .arg(execution.to_string())
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let v: Value = serde_json::from_str(
            &tokio::time::timeout(Duration::from_secs(20), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .expect("peer ready"),
        )
        .unwrap();
        let network: discovery::Identity = serde_json::from_value(v["identity"].clone()).unwrap();
        let requester = v["requester"].as_str().unwrap();
        let identity = Identity {
            network_id: network.network_id.clone(),
            msb_bootstrap: Digest::new(&network.msb_bootstrap).unwrap(),
            subnet_bootstrap: Digest::new(&network.subnet_bootstrap).unwrap(),
            controller_pubkey: Digest::new(requester).unwrap(),
        };
        let client = Arc::new(
            financial::Client::new(v["url"].as_str().unwrap(), network, requester.into(), 4)
                .unwrap(),
        );
        Self {
            stdin: child.stdin.take().unwrap(),
            child,
            lines,
            identity,
            client,
        }
    }
    async fn command(&mut self, command: &str) -> Value {
        self.stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
        serde_json::from_str(
            &tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .expect("peer response"),
        )
        .unwrap()
    }
    async fn stop(mut self) {
        self.stdin.write_all(b"stop\n").await.unwrap();
        self.stdin.flush().await.unwrap();
        drop(self.stdin);
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
struct Paid {
    _fixture: Fixture,
    peer: Peer,
    executor: PaidExecutor,
    journal: Arc<Journal>,
    authority: Arc<capacity::Authority>,
    authorization: ProxySpendAuthorization,
    record: attempts::Record,
    lease: Digest,
}
impl Paid {
    async fn start(
        base: &str,
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        bytes: &[u8],
        streaming: bool,
    ) -> Self {
        let f = Fixture::new(base, endpoint);
        let mut peer = Peer::start(rail, &f, bytes, streaming).await;
        let journal = Arc::new(
            Journal::open(
                f._store.path().join("paid-journal"),
                peer.identity.clone(),
                attempts::Limits {
                    max_records: 50,
                    max_unfinished: 50,
                    closed_retention_ms: 1000,
                    max_payload_bytes: 128 * 1024 * 1024,
                },
            )
            .unwrap(),
        );
        let authority = Arc::new(
            capacity::Authority::open(
                f._store.path().join("paid-capacity"),
                peer.identity.clone(),
                capacity::Limits {
                    max_groups: 4,
                    max_routes: 8,
                    max_leases: 50,
                    max_evidence_age: Duration::from_secs(60),
                },
            )
            .unwrap(),
        );
        authority.configure_group(d(200), 2).unwrap();
        authority
            .configure_route(capacity::Route {
                id: d(201),
                group: d(200),
                lane: capacity::Lane::Proxy,
                max_concurrency: 2,
            })
            .unwrap();
        for s in [
            capacity::Scope::Group(d(200)),
            capacity::Scope::Route(d(201)),
        ] {
            capacity_ready(&authority, s);
        }
        let request = if streaming {
            f.adapter.prepare_stream(bytes).unwrap()
        } else {
            f.adapter.prepare_json(bytes).unwrap()
        };
        let reservation = authority
            .reserve(
                &d(201),
                capacity::Work {
                    invocation: d(1),
                    request_hash: request.request_hash().clone(),
                },
            )
            .unwrap();
        let lease = reservation.lease().id.clone();
        let v = peer.command(&json!({"reserve":lease}).to_string()).await;
        assert_eq!(v["done"], "reserve");
        let authorization: ProxySpendAuthorization =
            serde_json::from_value(v["authorization"].clone()).unwrap();
        let pool = Arc::new(
            Pool::new(
                env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
                f._work.path(),
                PoolLimits {
                    max_children: 2,
                    max_buffer_bytes: 64 * 1024 * 1024,
                    startup_timeout: Duration::from_secs(5),
                    processing_timeout: Duration::from_secs(3),
                },
            )
            .unwrap(),
        );
        let executor = Executor::new(
            f.connection.clone(),
            f.adapter.clone(),
            pool,
            Arc::new(Storage::new(journal.clone(), 8).unwrap()),
        )
        .unwrap();
        let executor =
            PaidExecutor::new(executor, peer.client.clone(), authority.clone(), d(201)).unwrap();
        let record = executor
            .prepare_accepted(d(1), &authorization, bytes, streaming)
            .await
            .unwrap();
        Self {
            _fixture: f,
            peer,
            executor,
            journal,
            authority,
            authorization,
            record,
            lease,
        }
    }
}
fn cases() -> Vec<(ProxyEndpoint, Vec<u8>, Value)> {
    vec![
    (ProxyEndpoint::Chat,chat(),answer()),
    (ProxyEndpoint::Completions,serde_json::to_vec(&json!({"model":"public-model","prompt":"hi"})).unwrap(),json!({"id":"u","choices":[{"index":0,"text":"hello","finish_reason":"stop"}]})),
    (ProxyEndpoint::Responses,serde_json::to_vec(&json!({"model":"public-model","input":"hi"})).unwrap(),json!({"id":"u","status":"completed","output":[{"id":"i","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}]})),
    (ProxyEndpoint::Decisions,serde_json::to_vec(&json!({"model":"public-model","state":"It is raining","questions":{"action":{"type":"choice","instructions":"Choose an action","criteria":{"umbrella":"Take an umbrella","sunglasses":"Take sunglasses"}}}})).unwrap(),json!({"answers":{"action":{"type":"choice","choice":"umbrella","probabilities":{"umbrella":0.9,"sunglasses":0.1}}}})),
]
}

#[tokio::test]
async fn canonical_paid_gate_executes_all_four_endpoints_all_three_rails_with_real_http_and_owned_recovery(
) {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in cases() {
            let backend = backend(200, response, Duration::ZERO).await;
            let p = Paid::start(&backend.base, endpoint, rail, &bytes, false).await;
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            let duplicate = p
                .executor
                .prepare_accepted(d(1), &p.authorization, &bytes, false)
                .await
                .unwrap();
            assert_eq!(duplicate, p.record);
            let result = p
                .executor
                .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                .await
                .unwrap();
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(result.attempt.binding.rail, rail);
            let saved = p
                .executor
                .recover(&p.record.invocation, p.record.attempt)
                .await
                .unwrap();
            assert_eq!(
                saved.financial.unwrap().accepted().authorization,
                p.authorization
            );
            assert_eq!(saved.result.unwrap().digest, result.result_digest);
            assert_eq!(p.authority.status(&d(201)).unwrap().group_occupied, 1);
            assert!(p
                .executor
                .reconcile_capacity(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert!(!p
                .executor
                .reconcile_capacity(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert_eq!(p.authority.status(&d(201)).unwrap().available, 2);
            assert_eq!(
                p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
                Phase::Dispatched,
                "capacity closure is not financial/attempt closure"
            );

            assert!(matches!(
                p.executor
                    .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                    .await,
                Err(Error::RecoveryRequired)
            ));
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn canonical_paid_gate_streams_on_each_rail_without_per_chunk_financial_reads() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let backend = backend_raw(
            200,
            sse(
                &[
                    delta("hello", json!(null)),
                    delta(" world", json!(null)),
                    delta("", json!("stop")),
                ],
                true,
            ),
            "text/event-stream",
            Duration::ZERO,
        )
        .await;
        let bytes = stream_request();
        let mut p = Paid::start(&backend.base, ProxyEndpoint::Chat, rail, &bytes, true).await;
        let before = p.peer.command("reset").await["calls"].as_u64().unwrap();
        let mut chunks = 0;
        p.executor
            .execute_stream(
                &p.record.invocation,
                &bytes,
                &Cancellation::default(),
                |_| {
                    chunks += 1;
                    async { Ok(()) }
                },
            )
            .await
            .unwrap();
        let after = p.peer.command("reset").await["calls"].as_u64().unwrap();
        assert!(chunks >= 2);
        assert_eq!(
            after - before,
            1,
            "one exact canonical read per dispatch, never per chunk"
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn canonical_final_or_close_after_local_preparation_prevents_the_first_post() {
    for (mutation, endpoint, rail) in [
        ("close", ProxyEndpoint::Chat, ProxyRail::Fiat),
        ("final", ProxyEndpoint::Decisions, ProxyRail::Tnk),
    ] {
        let (_, bytes, response) = cases().into_iter().find(|c| c.0 == endpoint).unwrap();
        let backend = backend(200, response, Duration::ZERO).await;
        let mut p = Paid::start(&backend.base, endpoint, rail, &bytes, false).await;
        p.peer.command(mutation).await;
        assert!(matches!(
            p.executor
                .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                .await,
            Err(Error::Financial(_))
        ));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
            Phase::Prepared
        );
        assert_eq!(
            p.authority.lease(&p.lease).unwrap().unwrap().phase,
            capacity::Phase::Reserved
        );
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn invalid_fresh_financial_evidence_cannot_fall_back_to_stored_acceptance() {
    for mutation in ["hold", "signature", "nonce", "network"] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let bytes = chat();
        let mut p = Paid::start(
            &backend.base,
            ProxyEndpoint::Chat,
            ProxyRail::Tap,
            &bytes,
            false,
        )
        .await;
        p.peer.command(mutation).await;
        assert!(p
            .executor
            .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
            .await
            .is_err());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
            Phase::Prepared
        );
        assert_eq!(
            p.authority.lease(&p.lease).unwrap().unwrap().phase,
            capacity::Phase::Reserved
        );
        p.peer.command("reset").await;
        p.executor
            .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap();
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn paid_guard_binds_original_authorization_body_and_capacity_not_a_current_offer() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &bytes,
        false,
    )
    .await;
    let original = p
        .executor
        .recover(&p.record.invocation, p.record.attempt)
        .await
        .unwrap()
        .financial
        .unwrap();
    for key in ["capacity", "body", "offer"] {
        let mut auth = p.authorization.clone();
        match key {
            "capacity" => auth.terms.capacity_lease = d(999).as_str().into(),
            "body" => auth.terms.request_hash = d(999).as_str().into(),
            _ => auth.terms.offer.revision += 1,
        };
        assert!(p
            .executor
            .prepare_accepted(d(1), &auth, &bytes, false)
            .await
            .is_err());
    }
    assert_eq!(
        p.executor
            .recover(&p.record.invocation, p.record.attempt)
            .await
            .unwrap()
            .financial
            .unwrap()
            .accepted()
            .authorization,
        original.accepted().authorization
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_unsent_cancellation_reconciles_before_send_and_between_store_commits() {
    for gap in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let bytes = chat();
        let p = Paid::start(
            &backend.base,
            ProxyEndpoint::Chat,
            ProxyRail::Fiat,
            &bytes,
            false,
        )
        .await;
        if gap {
            p.authority
                .dispatch_accepted(
                    &p.lease,
                    &capacity::Work {
                        invocation: p.record.invocation.clone(),
                        request_hash: p.record.binding.request_hash.clone(),
                    },
                    &d(201),
                )
                .unwrap();
        }
        p.executor
            .cancel_unsent(&p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        p.executor
            .cancel_unsent(&p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        assert_eq!(p.authority.status(&d(201)).unwrap().available, 2);
        let saved = p
            .executor
            .recover(&p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        assert_eq!(saved.record.phase, Phase::Resolved);
        assert!(saved.record.closure.is_none());
        assert!(saved.financial.is_some());
        assert!(p
            .executor
            .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
            .await
            .is_err());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn paid_dispatched_unknown_cannot_be_cancelled_as_unsent_or_freed_without_output() {
    let backend = backend(200, answer(), Duration::from_millis(250)).await;
    let bytes = chat();
    let p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tap,
        &bytes,
        false,
    )
    .await;
    let cancel = Cancellation::default();
    {
        let task = p
            .executor
            .execute_json(&p.record.invocation, &bytes, &cancel);
        tokio::pin!(task);
        tokio::select! { result=&mut task=>panic!("unexpected early completion {result:?}"),_=async{while backend.calls.load(Ordering::SeqCst)==0 {tokio::time::sleep(Duration::from_millis(2)).await;}}=>() }
    }
    assert!(matches!(
        p.executor
            .cancel_unsent(&p.record.invocation, p.record.attempt)
            .await,
        Err(Error::RecoveryRequired)
    ));
    assert!(matches!(
        p.executor
            .reconcile_capacity(&p.record.invocation, p.record.attempt)
            .await,
        Err(Error::RecoveryRequired)
    ));
    assert_eq!(p.authority.status(&d(201)).unwrap().group_occupied, 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_constructor_rejects_a_different_journal_owner() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &bytes,
        false,
    )
    .await;
    assert!(matches!(
        PaidExecutor::new(
            p._fixture.executor,
            p.peer.client.clone(),
            p.authority.clone(),
            d(201)
        ),
        Err(Error::Binding)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    p.peer.stop().await;
}

#[tokio::test]
async fn cancellation_during_canonical_read_is_durable_unsent_evidence_and_reconciles_capacity() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &bytes,
        false,
    )
    .await;
    let before = p.peer.command("delay").await["calls"].as_u64().unwrap();
    let cancel = Cancellation::default();
    {
        let run = p
            .executor
            .execute_json(&p.record.invocation, &bytes, &cancel);
        tokio::pin!(run);
        tokio::select! {
            value=&mut run=>panic!("returned before the observed financial read {value:?}"),
            _=async {
                loop {
                    if p.peer.command("status").await["calls"].as_u64().unwrap()>before {break;}
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                cancel.cancel();
            }=>()
        }
        assert!(matches!(run.await, Err(Error::Cancelled)));
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
        Phase::Resolved
    );
    assert!(p
        .executor
        .reconcile_capacity(&p.record.invocation, p.record.attempt)
        .await
        .unwrap());
    assert_eq!(p.authority.status(&d(201)).unwrap().available, 2);
    p.peer.stop().await;
}

#[tokio::test]
async fn retained_paid_result_reconciles_after_controller_restart_without_new_finance_or_inference_calls(
) {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        &bytes,
        false,
    )
    .await;
    p.executor
        .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
        .await
        .unwrap();
    let Paid {
        _fixture: f,
        mut peer,
        executor,
        journal,
        authority,
        authorization: _,
        record,
        lease,
    } = p;
    drop(executor);
    drop(journal);
    drop(authority);
    let journal = Arc::new(
        Journal::open(
            f._store.path().join("paid-journal"),
            peer.identity.clone(),
            attempts::Limits {
                max_records: 50,
                max_unfinished: 50,
                closed_retention_ms: 1000,
                max_payload_bytes: 128 * 1024 * 1024,
            },
        )
        .unwrap(),
    );
    let authority = Arc::new(
        capacity::Authority::open(
            f._store.path().join("paid-capacity"),
            peer.identity.clone(),
            capacity::Limits {
                max_groups: 4,
                max_routes: 8,
                max_leases: 50,
                max_evidence_age: Duration::from_secs(60),
            },
        )
        .unwrap(),
    );
    assert_eq!(
        authority.lease(&lease).unwrap().unwrap().phase,
        capacity::Phase::Uncertain
    );
    assert_eq!(
        authority.status(&d(201)).unwrap().state,
        capacity::Readiness::Checking
    );
    let pool = Arc::new(
        Pool::new(
            env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
            f._work.path(),
            PoolLimits {
                max_children: 2,
                max_buffer_bytes: 64 * 1024 * 1024,
                startup_timeout: Duration::from_secs(5),
                processing_timeout: Duration::from_secs(3),
            },
        )
        .unwrap(),
    );
    let executor = Executor::new(
        f.connection.clone(),
        f.adapter.clone(),
        pool,
        Arc::new(Storage::new(journal.clone(), 8).unwrap()),
    )
    .unwrap();
    let executor =
        PaidExecutor::new(executor, peer.client.clone(), authority.clone(), d(201)).unwrap();
    let before = peer.command("status").await["calls"].as_u64().unwrap();
    assert!(executor
        .reconcile_capacity(&record.invocation, record.attempt)
        .await
        .unwrap());
    assert!(executor
        .recover(&record.invocation, record.attempt)
        .await
        .unwrap()
        .result
        .is_some());
    assert!(matches!(
        executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await,
        Err(Error::RecoveryRequired)
    ));
    assert_eq!(
        peer.command("status").await["calls"].as_u64().unwrap(),
        before
    );
    assert_eq!(authority.status(&d(201)).unwrap().group_occupied, 0);
    assert_eq!(
        authority.status(&d(201)).unwrap().state,
        capacity::Readiness::Checking,
        "result recovery cannot invent fresh upstream readiness"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    peer.stop().await;
}
