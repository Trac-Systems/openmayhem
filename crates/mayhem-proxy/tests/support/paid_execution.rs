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
    buyer_client: Arc<financial::Client>,
}
impl Peer {
    async fn start(
        rail: ProxyRail,
        f: &Fixture,
        bytes: &[u8],
        streaming: bool,
        policy: Option<Value>,
    ) -> Self {
        let request = if streaming {
            f.adapter.prepare_stream(bytes).unwrap()
        } else {
            f.adapter.prepare_json(bytes).unwrap()
        };
        let execution = json!({"endpoint":request.endpoint(),"endpoint_contract":f.adapter.contract_hash(),
            "recipe_hash":f.adapter.recipe_hash(),"connection_revision":f.connection.revision(),
            "connection_digest":f.connection.fingerprint(),"capacity_group":d(200),"request_hash":request.request_hash(),
            "metering":mayhem_proxy::metering::Policy::for_endpoint(request.endpoint()).contract(),"settlement_policy":policy});
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
            financial::Client::new(
                v["url"].as_str().unwrap(),
                network.clone(),
                requester.into(),
                4,
            )
            .unwrap(),
        );
        Self {
            stdin: child.stdin.take().unwrap(),
            child,
            lines,
            identity,
            client,
            buyer_client: Arc::new(
                financial::Client::new(
                    v["buyer_url"].as_str().unwrap(),
                    network,
                    v["buyer"].as_str().unwrap().into(),
                    4,
                )
                .unwrap(),
            ),
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
    fn verifier(&self) -> Pool {
        Pool::new(
            env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
            self._fixture._work.path(),
            PoolLimits {
                max_children: 2,
                max_buffer_bytes: 64 * 1024 * 1024,
                startup_timeout: Duration::from_secs(5),
                processing_timeout: Duration::from_secs(3),
            },
        )
        .unwrap()
    }
    fn reopen(self) -> Self {
        self.reopen_format(false)
    }
    fn reopen_format(self, schema_five: bool) -> Self {
        let Self {
            _fixture: f,
            peer,
            executor,
            journal,
            authority,
            authorization,
            record,
            lease,
        } = self;
        drop(executor);
        drop(journal);
        drop(authority);
        if schema_five {
            // Reconstruct the exact prior on-disk format using this real signed
            // receipt, not an invented empty database or a copied test result.
            use redb::ReadableTable;
            let db = redb::Database::open(f._store.path().join("paid-journal")).unwrap();
            let tx = db.begin_write().unwrap();
            {
                let mut meta = tx
                    .open_table(redb::TableDefinition::<&str, &[u8]>::new(
                        "proxy_attempt_meta_v1",
                    ))
                    .unwrap();
                let mut value: Value =
                    serde_json::from_slice(meta.get("state").unwrap().unwrap().value()).unwrap();
                value["schema"] = json!(5);
                meta.insert("state", serde_json::to_vec(&value).unwrap().as_slice())
                    .unwrap();
                let mut table = tx
                    .open_table(redb::TableDefinition::<&str, &[u8]>::new(
                        "proxy_attempt_outcomes_v1",
                    ))
                    .unwrap();
                let key = format!("{}:{:020}", record.invocation.as_str(), record.attempt);
                let mut slot: Value =
                    serde_json::from_slice(table.get(key.as_str()).unwrap().unwrap().value())
                        .unwrap();
                assert!(slot["waiver"].is_null() && slot["closure"].is_null());
                slot.as_object_mut().unwrap().remove("waiver");
                slot.as_object_mut().unwrap().remove("closure");
                table
                    .insert(key.as_str(), serde_json::to_vec(&slot).unwrap().as_slice())
                    .unwrap();
            }
            tx.commit().unwrap();
            drop(db);
        }
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
    async fn start(
        base: &str,
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        bytes: &[u8],
        streaming: bool,
    ) -> Self {
        Self::start_with_policy(base, endpoint, rail, bytes, streaming, None).await
    }
    async fn start_with_policy(
        base: &str,
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        bytes: &[u8],
        streaming: bool,
        policy: Option<Value>,
    ) -> Self {
        Self::start_configured(base, endpoint, rail, bytes, streaming, policy, false).await
    }
    async fn start_configured(
        base: &str,
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        bytes: &[u8],
        streaming: bool,
        policy: Option<Value>,
        publish: bool,
    ) -> Self {
        let f = Fixture::new(base, endpoint);
        let mut peer = Peer::start(rail, &f, bytes, streaming, policy).await;
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
        let operation = if publish { "bind_lease" } else { "reserve" };
        let v = peer.command(&json!({operation:lease}).to_string()).await;
        assert_eq!(v["done"], operation);
        let authorization: ProxySpendAuthorization =
            serde_json::from_value(v["authorization"].clone()).unwrap();
        if publish {
            use financial::recovery::{BuyerRecovery, Limits, Store};
            let mut buyer_identity = peer.identity.clone();
            buyer_identity.controller_pubkey =
                Digest::new(&authorization.terms.buyer_pubkey).unwrap();
            let buyer = BuyerRecovery::new(
                Arc::new(
                    Store::open(
                        f._store.path().join("publication-buyer"),
                        buyer_identity,
                        Limits {
                            max_records: 8,
                            closed_retention_ms: 1000,
                        },
                    )
                    .unwrap(),
                ),
                peer.buyer_client.clone(),
                4,
            )
            .unwrap();
            let key = buyer
                .retain_reservation(
                    authorization.clone(),
                    serde_json::from_value(v["policy"].clone()).unwrap(),
                    1000,
                )
                .await
                .unwrap();
            assert!(
                !buyer
                    .recover(key.clone())
                    .await
                    .unwrap()
                    .reservation
                    .unwrap()
                    .confirmed
            );
            buyer
                .publish_reservation(key, 1001)
                .await
                .unwrap()
                .initial_binding()
                .unwrap();
        }
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
#[tokio::test]
async fn buyer_published_reservations_enable_exact_paid_execution_and_settlement_on_all_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in cases() {
            let backend = backend(200, response, Duration::ZERO).await;
            let mut p =
                Paid::start_configured(&backend.base, endpoint, rail, &bytes, false, None, true)
                    .await;
            assert_eq!(
                backend.calls.load(Ordering::SeqCst),
                0,
                "reservation publication does not infer"
            );
            assert_eq!(p.peer.command("status").await["publications"], 1);
            p.executor
                .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                .await
                .unwrap();
            let receipt = signed_terminal(&mut p).await;
            p.executor
                .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
                .await
                .unwrap();
            assert!(p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(p.peer.command("status").await["publications"], 2);
            assert_eq!(
                p.peer.command("state").await["summary"]["reserved_au"],
                (50 + receipt.body.au_owed_cum).to_string(),
                "native hold plus verified proxy liability remain until epoch settlement"
            );
            p.peer.stop().await;
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

fn terminal_response(kind: &str) -> Value {
    let mut value = answer();
    if kind == "partial" {
        value["choices"][0]["finish_reason"] = json!("length");
    }
    if kind == "refused" {
        value["choices"][0]["message"]["content"] = json!("");
        value["choices"][0]["message"]["refusal"] = json!("Cannot comply");
    }
    value
}
fn cancel_before_outcome(p: &Paid) {
    let r = p.journal.get(&p.record.invocation).unwrap().unwrap();
    p.journal
        .advance(
            &r.invocation,
            r.generation,
            attempts::Event::CancelRequested,
            r.updated_at_ms + 1,
        )
        .unwrap();
}
async fn signed_waiver(p: &mut Paid) -> mayhem_proto::proxy::finance::ProxyReservationClosure {
    let draft = p
        .executor
        .prepare_waiver(&p.record.invocation, p.record.attempt)
        .await
        .unwrap();
    let sigs = p
        .peer
        .command(&json!({"sign_waiver":draft.body}).to_string())
        .await;
    let saved = p
        .journal
        .recover(&p.record.invocation, p.record.attempt)
        .unwrap();
    let approval = mayhem_proxy::receipts::approve_waiver(
        &draft,
        sigs["provider_sig"].as_str().unwrap(),
        &p.authorization,
        &saved.request.as_ref().unwrap().body,
        saved.result.as_ref().map(|v| &v.reply),
        saved.record.cancellation_requested,
    )
    .unwrap();
    assert!(mayhem_proxy::receipts::verify_signature(
        sigs["buyer_sig"].as_str().unwrap(),
        approval.signing_bytes(),
        &p.authorization.terms.buyer_pubkey
    ));
    mayhem_proto::proxy::finance::ProxyReservationClosure {
        body: draft.body,
        buyer_sig: sigs["buyer_sig"].as_str().unwrap().into(),
        provider_sig: sigs["provider_sig"].as_str().unwrap().into(),
    }
}

#[tokio::test]
async fn paid_terminal_partial_refusal_and_cancelled_charge_only_under_the_original_explicit_policy(
) {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for kind in ["partial", "refused", "cancelled"] {
            let backend = backend(200, terminal_response(kind), Duration::ZERO).await;
            let policy = json!({"schema_version":1,"lane":"proxy","payable_outcomes":["cancelled","complete","partial","refused"],"allow_checkpoints":false});
            let mut p = Paid::start_with_policy(
                &backend.base,
                ProxyEndpoint::Chat,
                rail,
                &chat(),
                false,
                Some(policy),
            )
            .await;
            p.executor
                .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
                .await
                .unwrap();
            if kind == "cancelled" {
                cancel_before_outcome(&p);
            }
            let receipt = signed_terminal(&mut p).await;
            assert_eq!(
                serde_json::to_value(receipt.body.outcome).unwrap(),
                json!(kind)
            );
            p.executor
                .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
                .await
                .unwrap();
            assert!(p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert!(p
                .peer
                .client
                .observe(&p.authorization)
                .await
                .unwrap()
                .confirms_receipt(&receipt)
                .unwrap());
            assert_eq!(p.peer.command("status").await["publications"], 1);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn paid_terminal_disallowed_outcomes_require_explicit_mutual_waiver_on_every_rail() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for kind in ["partial", "refused", "cancelled"] {
            let backend = backend(200, terminal_response(kind), Duration::ZERO).await;
            let mut p = Paid::start(&backend.base, ProxyEndpoint::Chat, rail, &chat(), false).await;
            p.executor
                .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
                .await
                .unwrap();
            if kind == "cancelled" {
                cancel_before_outcome(&p);
            }
            assert!(p
                .executor
                .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            assert!(p
                .journal
                .terminal_draft(&p.record.invocation, p.record.attempt)
                .unwrap()
                .is_none());
            assert!(p
                .journal
                .waiver_draft(&p.record.invocation, p.record.attempt)
                .unwrap()
                .is_none());
            assert_eq!(p.peer.command("status").await["publications"], 0);
            let closure = signed_waiver(&mut p).await;
            p.executor
                .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
                .await
                .unwrap();
            assert!(p
                .executor
                .publish_waiver(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            let observed = p.peer.client.observe(&p.authorization).await.unwrap();
            assert!(observed.confirms_waiver(&closure).unwrap());
            assert!(observed.receipt_head().unwrap().is_none());
            assert!(p.authority.lease(&p.lease).unwrap().is_none());
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn paid_waiver_unsent_cancellation_releases_money_without_inference_and_recovers_after_restart(
) {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let mut p = Paid::start(&backend.base, ProxyEndpoint::Chat, rail, &chat(), false).await;
        p.executor
            .cancel_unsent(&p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        let closure = signed_waiver(&mut p).await;
        assert_eq!(
            closure.body.outcome,
            mayhem_proto::proxy::finance::ProxyClosureOutcome::NotExecuted
        );
        p.executor
            .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
            .await
            .unwrap();
        p.peer.command("publish_pending").await;
        assert!(!p
            .executor
            .publish_waiver(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        p.peer.command("flush_publication").await;
        let mut p = p.reopen();
        assert!(p
            .executor
            .publish_waiver(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        assert_eq!(
            p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
            Phase::Closed
        );
        let stats = p.peer.command("status").await;
        assert_eq!(stats["publications"], 1);
        assert_eq!(stats["submissions"], 1);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn paid_terminal_intents_are_exclusive_and_late_cancellation_cannot_relabel_them() {
    for waive in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let mut p = Paid::start(
            &backend.base,
            ProxyEndpoint::Chat,
            ProxyRail::Fiat,
            &chat(),
            false,
        )
        .await;
        p.executor
            .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
            .await
            .unwrap();
        if waive {
            let closure = signed_waiver(&mut p).await;
            assert!(p
                .executor
                .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            cancel_before_outcome(&p);
            assert!(
                !p.journal
                    .get(&p.record.invocation)
                    .unwrap()
                    .unwrap()
                    .cancellation_requested
            );
            let mut altered = closure.clone();
            altered.body.at_ms += 1;
            let sigs = p
                .peer
                .command(&json!({"sign_waiver":altered.body}).to_string())
                .await;
            altered.buyer_sig = sigs["buyer_sig"].as_str().unwrap().into();
            altered.provider_sig = sigs["provider_sig"].as_str().unwrap().into();
            assert!(p
                .executor
                .retain_waiver(&p.record.invocation, p.record.attempt, &altered)
                .await
                .is_err());
            p.executor
                .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
                .await
                .unwrap();
            p.peer.command("publish_lost_ack").await;
            let (a, b) = tokio::join!(
                p.executor
                    .publish_waiver(&p.record.invocation, p.record.attempt),
                p.executor
                    .publish_waiver(&p.record.invocation, p.record.attempt)
            );
            assert!(a.unwrap() && b.unwrap());
        } else {
            let receipt = signed_terminal(&mut p).await;
            assert!(p
                .executor
                .prepare_waiver(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            cancel_before_outcome(&p);
            assert!(
                !p.journal
                    .get(&p.record.invocation)
                    .unwrap()
                    .unwrap()
                    .cancellation_requested
            );
            p.executor
                .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
                .await
                .unwrap();
            assert!(p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
        }
        assert_eq!(p.peer.command("status").await["publications"], 1);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn paid_unknown_execution_cannot_be_signed_off_as_terminal_or_waived() {
    let backend = backend_raw(
        200,
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"prefix\"}}]}\n\n".into(),
        "text/event-stream",
        Duration::ZERO,
    )
    .await;
    let bytes = stream_request();
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &bytes,
        true,
    )
    .await;
    assert!(p
        .executor
        .execute_stream(
            &p.record.invocation,
            &bytes,
            &Cancellation::default(),
            |_| async { Ok(()) }
        )
        .await
        .is_err());
    assert!(p
        .executor
        .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
        .await
        .is_err());
    assert!(p
        .executor
        .prepare_waiver(&p.record.invocation, p.record.attempt)
        .await
        .is_err());
    assert!(p
        .executor
        .reconcile_capacity(&p.record.invocation, p.record.attempt)
        .await
        .is_err());
    let state = p.peer.client.observe(&p.authorization).await.unwrap();
    assert!(!state.is_closed());
    assert!(!state.has_receipt());
    assert!(p.authority.lease(&p.lease).unwrap().is_some());
    assert_eq!(p.peer.command("status").await["publications"], 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_terminal_other_llm_endpoints_and_stream_keep_partial_refusal_and_cancellation_semantics(
) {
    let policy = json!({"schema_version":1,"lane":"proxy","payable_outcomes":["cancelled","complete","partial","refused"],"allow_checkpoints":false});
    for (endpoint, bytes, response) in cases()
        .into_iter()
        .filter(|(e, _, _)| *e != ProxyEndpoint::Chat)
    {
        let kinds: &[&str] = if endpoint == ProxyEndpoint::Decisions {
            &["cancelled"]
        } else {
            &["partial", "refused", "cancelled"]
        };
        for kind in kinds {
            let mut response = response.clone();
            match (endpoint, *kind) {
                (ProxyEndpoint::Completions, "partial") => {
                    response["choices"][0]["finish_reason"] = json!("length")
                }
                (ProxyEndpoint::Completions, "refused") => {
                    response["choices"][0]["finish_reason"] = json!("content_filter")
                }
                (ProxyEndpoint::Responses, "partial") => {
                    response["status"] = json!("incomplete");
                    response["incomplete_details"] = json!({"reason":"max_output_tokens"});
                }
                (ProxyEndpoint::Responses, "refused") => {
                    response["output"][0]["content"] =
                        json!([{"type":"refusal","refusal":"Cannot comply"}])
                }
                _ => {}
            }
            let backend = backend(200, response, Duration::ZERO).await;
            let mut p = Paid::start_with_policy(
                &backend.base,
                endpoint,
                ProxyRail::Fiat,
                &bytes,
                false,
                Some(policy.clone()),
            )
            .await;
            p.executor
                .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                .await
                .unwrap();
            if *kind == "cancelled" {
                cancel_before_outcome(&p);
            }
            let receipt = signed_terminal(&mut p).await;
            assert_eq!(
                serde_json::to_value(receipt.body.outcome).unwrap(),
                json!(kind)
            );
            p.executor
                .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
                .await
                .unwrap();
            assert!(p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            p.peer.stop().await;
        }
    }
    for kind in ["partial", "refused", "cancelled"] {
        let finish = match kind {
            "partial" => "length",
            "refused" => "content_filter",
            _ => "stop",
        };
        let backend = backend_raw(
            200,
            sse(
                &[delta("visible", json!(null)), delta("", json!(finish))],
                true,
            ),
            "text/event-stream",
            Duration::ZERO,
        )
        .await;
        let bytes = stream_request();
        let mut p = Paid::start_with_policy(
            &backend.base,
            ProxyEndpoint::Chat,
            ProxyRail::Tap,
            &bytes,
            true,
            Some(policy.clone()),
        )
        .await;
        p.executor
            .execute_stream(
                &p.record.invocation,
                &bytes,
                &Cancellation::default(),
                |_| async { Ok(()) },
            )
            .await
            .unwrap();
        if kind == "cancelled" {
            cancel_before_outcome(&p);
        }
        let receipt = signed_terminal(&mut p).await;
        assert_eq!(
            serde_json::to_value(receipt.body.outcome).unwrap(),
            json!(kind)
        );
        p.executor
            .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
            .await
            .unwrap();
        assert!(p
            .executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn paid_terminal_receipt_and_waiver_race_cannot_create_two_financial_intents() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &chat(),
        false,
    )
    .await;
    p.executor
        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
        .await
        .unwrap();
    let (receipt, waiver) = tokio::join!(
        p.executor
            .prepare_terminal_receipt(&p.record.invocation, p.record.attempt),
        p.executor
            .prepare_waiver(&p.record.invocation, p.record.attempt)
    );
    assert_ne!(receipt.is_ok(), waiver.is_ok());
    assert_ne!(
        p.journal
            .terminal_draft(&p.record.invocation, p.record.attempt)
            .unwrap()
            .is_some(),
        p.journal
            .waiver_draft(&p.record.invocation, p.record.attempt)
            .unwrap()
            .is_some()
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_schema_five_signed_receipt_migrates_without_changing_authorization_or_publishing_twice(
) {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &chat(),
        false,
    )
    .await;
    p.executor
        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
        .await
        .unwrap();
    let receipt = signed_terminal(&mut p).await;
    p.executor
        .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
        .await
        .unwrap();
    let allocated = p.journal.allocated_payload_bytes().unwrap();
    let mut p = p.reopen_format(true);
    assert_eq!(p.journal.allocated_payload_bytes().unwrap(), allocated);
    assert_eq!(
        p.journal
            .terminal_receipt(&p.record.invocation, p.record.attempt)
            .unwrap(),
        Some(receipt.clone())
    );
    assert!(p
        .journal
        .waiver_draft(&p.record.invocation, p.record.attempt)
        .unwrap()
        .is_none());
    assert!(p
        .executor
        .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
        .await
        .unwrap());
    assert_eq!(p.peer.command("status").await["publications"], 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_waiver_buyer_rejects_false_nonexecution_altered_evidence_and_bad_signatures() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        &chat(),
        false,
    )
    .await;
    p.executor
        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
        .await
        .unwrap();
    let closure = signed_waiver(&mut p).await;
    let draft = p
        .journal
        .waiver_draft(&p.record.invocation, p.record.attempt)
        .unwrap()
        .unwrap();
    let saved = p
        .journal
        .recover(&p.record.invocation, p.record.attempt)
        .unwrap();
    for fault in ["unsent", "evidence", "signature", "cancellation"] {
        let mut draft = draft.clone();
        if fault == "unsent" {
            draft.body.outcome = mayhem_proto::proxy::finance::ProxyClosureOutcome::NotExecuted;
        }
        if fault == "evidence" {
            draft.body.evidence_hash = "0".repeat(64);
        }
        let sigs = p
            .peer
            .command(&json!({"sign_waiver":draft.body}).to_string())
            .await;
        let signature = if fault == "signature" {
            "0".repeat(128)
        } else {
            sigs["provider_sig"].as_str().unwrap().into()
        };
        assert!(
            mayhem_proxy::receipts::approve_waiver(
                &draft,
                &signature,
                &p.authorization,
                &saved.request.as_ref().unwrap().body,
                saved.result.as_ref().map(|v| &v.reply),
                fault == "cancellation"
            )
            .is_err(),
            "{fault}"
        );
    }
    p.executor
        .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
        .await
        .unwrap();
    assert!(p
        .executor
        .publish_waiver(&p.record.invocation, p.record.attempt)
        .await
        .unwrap());
    p.peer.stop().await;
}

async fn signed_terminal(p: &mut Paid) -> mayhem_proto::proxy::finance::ProxyUsageReceipt {
    let draft = p
        .executor
        .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
        .await
        .unwrap();
    let sigs = p
        .peer
        .command(&json!({"sign_receipt":draft.body}).to_string())
        .await;
    let saved = p
        .journal
        .recover(&p.record.invocation, p.record.attempt)
        .unwrap();
    let accepted = saved.financial.as_ref().unwrap().accepted();
    let approval = mayhem_proxy::receipts::approve_terminal(
        &p.verifier(),
        &draft,
        sigs["provider_sig"].as_str().unwrap(),
        &accepted.authorization,
        &accepted.settlement_policy,
        &saved.acceptance.as_ref().unwrap().snapshot,
        &saved.request.as_ref().unwrap().body,
        &saved.result.as_ref().unwrap().reply,
        saved.record.cancellation_requested,
    )
    .await
    .unwrap();
    assert!(mayhem_proxy::receipts::verify_signature(
        sigs["buyer_sig"].as_str().unwrap(),
        approval.signing_bytes(),
        &p.authorization.terms.buyer_pubkey
    ));
    mayhem_proto::proxy::finance::ProxyUsageReceipt {
        body: draft.body,
        buyer_sig: sigs["buyer_sig"].as_str().unwrap().into(),
        provider_sig: sigs["provider_sig"].as_str().unwrap().into(),
    }
}

#[tokio::test]
async fn paid_terminal_receipts_recount_sign_publish_and_close_each_endpoint_and_rail() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in cases() {
            let backend = backend(200, response, Duration::ZERO).await;
            let mut p = Paid::start(&backend.base, endpoint, rail, &bytes, false).await;
            assert!(p
                .executor
                .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            p.executor
                .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                .await
                .unwrap();
            let receipt = signed_terminal(&mut p).await;
            p.executor
                .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
                .await
                .unwrap();
            assert_eq!(
                p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
                Phase::Resolved
            );
            assert!(p.authority.lease(&p.lease).unwrap().is_some());
            assert!(p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert!(p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            let observed = p.peer.client.observe(&p.authorization).await.unwrap();
            assert!(observed.confirms_receipt(&receipt).unwrap());
            assert!(observed.initial_binding().is_err());
            let stats = p.peer.command("status").await;
            assert_eq!(stats["submissions"], 1);
            assert_eq!(stats["publications"], 1);
            assert!(p.authority.lease(&p.lease).unwrap().is_none());
            let closed = p.journal.get(&p.record.invocation).unwrap().unwrap();
            assert_eq!(closed.phase, Phase::Closed);
            assert!(p.journal.allocated_payload_bytes().unwrap() > 0);
            assert_eq!(
                p.journal
                    .prune_closed(closed.expires_at_ms.unwrap(), 1)
                    .unwrap(),
                1
            );
            assert_eq!(p.journal.allocated_payload_bytes().unwrap(), 0);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn paid_receipt_http_ack_is_not_canonical_confirmation_and_ack_loss_recovers_same_receipt() {
    for mode in ["publish_pending", "publish_lost_ack"] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let mut p = Paid::start(
            &backend.base,
            ProxyEndpoint::Chat,
            ProxyRail::Fiat,
            &chat(),
            false,
        )
        .await;
        p.executor
            .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
            .await
            .unwrap();
        let receipt = signed_terminal(&mut p).await;
        p.executor
            .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
            .await
            .unwrap();
        p.peer.command(mode).await;
        let done = p
            .executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        assert_eq!(done, mode == "publish_lost_ack");
        if !done {
            assert_eq!(
                p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
                Phase::Resolved
            );
            assert!(p.authority.lease(&p.lease).unwrap().is_some());
            p.peer.command("flush_publication").await;
        }
        assert!(p
            .executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        let stats = p.peer.command("status").await;
        assert_eq!(stats["submissions"], 1);
        assert_eq!(stats["publications"], 1);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn paid_receipt_buyer_rejects_altered_output_usage_body_signature_and_identity() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tap,
        &chat(),
        false,
    )
    .await;
    p.executor
        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
        .await
        .unwrap();
    let receipt = signed_terminal(&mut p).await;
    let draft = p
        .journal
        .terminal_draft(&p.record.invocation, p.record.attempt)
        .unwrap()
        .unwrap();
    let saved = p
        .journal
        .recover(&p.record.invocation, p.record.attempt)
        .unwrap();
    let accepted = saved.financial.as_ref().unwrap().accepted();
    let snapshot = &saved.acceptance.as_ref().unwrap().snapshot;
    let input = &saved.request.as_ref().unwrap().body;
    let output = &saved.result.as_ref().unwrap().reply;
    for fault in [
        "output",
        "usage",
        "result",
        "observation",
        "invocation",
        "attempt",
        "request",
        "signature",
        "authorization",
    ] {
        let mut d = draft.clone();
        let mut reply: mayhem_proxy::endpoint::ProtocolReply =
            serde_json::from_value(serde_json::to_value(output).unwrap()).unwrap();
        let mut request = input.clone();
        let mut authorization = accepted.authorization.clone();
        match fault {
            "output" => {
                reply.body["choices"][0]["message"]["content"] = json!("changed received answer")
            }
            "usage" => *d.body.usage.values_mut().next().unwrap() += 1,
            "result" => d.body.result_hash = "0".repeat(64),
            "observation" => d.body.observation_hash = "0".repeat(64),
            "invocation" => d.invocation = super::d(8),
            "attempt" => d.attempt += 1,
            "request" => request = serde_json::to_vec(
                &json!({"model":"public-model","messages":[{"role":"user","content":"different"}]}),
            )
            .unwrap(),
            "authorization" => authorization.terms.buyer_pubkey = "0".repeat(64),
            _ => {}
        }
        // Re-sign changed provider bodies: the buyer must independently detect
        // incorrect evidence, not merely rely on rejecting an invalid signature.
        let sigs = p
            .peer
            .command(&json!({"sign_receipt":d.body}).to_string())
            .await;
        let signature = if fault == "signature" {
            "0".repeat(128)
        } else {
            sigs["provider_sig"].as_str().unwrap().into()
        };
        assert!(
            mayhem_proxy::receipts::approve_terminal(
                &p.verifier(),
                &d,
                &signature,
                &authorization,
                &accepted.settlement_policy,
                snapshot,
                &request,
                &reply,
                false,
            )
            .await
            .is_err(),
            "{fault}"
        );
    }
    let mut altered = receipt.clone();
    altered.body.at_ms += 1;
    let sigs = p
        .peer
        .command(&json!({"sign_receipt":altered.body}).to_string())
        .await;
    altered.provider_sig = sigs["provider_sig"].as_str().unwrap().into();
    altered.buyer_sig = sigs["buyer_sig"].as_str().unwrap().into();
    assert!(p
        .executor
        .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &altered)
        .await
        .is_err());
    p.executor
        .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
        .await
        .unwrap();
    assert_eq!(p.peer.command("status").await["submissions"], 0);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_terminal_receipt_concurrent_signer_recovery_keeps_one_body() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &chat(),
        false,
    )
    .await;
    p.executor
        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        p.executor
            .prepare_terminal_receipt(&p.record.invocation, p.record.attempt),
        p.executor
            .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    let receipt = signed_terminal(&mut p).await;
    p.executor
        .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        p.executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt),
        p.executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
    );
    assert!(a.unwrap() && b.unwrap());
    assert_eq!(p.peer.command("status").await["publications"], 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_terminal_receipt_recovers_draft_signatures_and_canonical_ack_across_restart() {
    for stage in ["draft", "signed", "canonical"] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let mut p = Paid::start(
            &backend.base,
            ProxyEndpoint::Chat,
            ProxyRail::Tnk,
            &chat(),
            false,
        )
        .await;
        p.executor
            .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
            .await
            .unwrap();
        let receipt = signed_terminal(&mut p).await;
        let allocated = p.journal.allocated_payload_bytes().unwrap();
        if stage != "draft" {
            p.executor
                .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
                .await
                .unwrap();
        }
        if stage == "canonical" {
            p.peer.command("publish_pending").await;
            assert!(!p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            p.peer.command("flush_publication").await;
        }
        let mut p = p.reopen();
        assert_eq!(p.journal.allocated_payload_bytes().unwrap(), allocated);
        assert_eq!(
            p.authority.lease(&p.lease).unwrap().unwrap().phase,
            capacity::Phase::Uncertain
        );
        let draft = p
            .executor
            .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        assert_eq!(draft.body, receipt.body);
        p.executor
            .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
            .await
            .unwrap();
        assert!(p
            .executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        let stats = p.peer.command("status").await;
        assert_eq!(stats["submissions"], 1);
        assert_eq!(stats["publications"], 1);
        assert_eq!(
            p.authority.status(&d(201)).unwrap().state,
            capacity::Readiness::Checking
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn paid_terminal_receipt_conflicting_canonical_final_never_overwrites_or_closes_local_outcome(
) {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        &chat(),
        false,
    )
    .await;
    p.executor
        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
        .await
        .unwrap();
    let receipt = signed_terminal(&mut p).await;
    p.executor
        .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
        .await
        .unwrap();
    p.peer.command("final").await;
    assert!(p
        .executor
        .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
        .await
        .is_err());
    assert_eq!(
        p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
        Phase::Resolved
    );
    assert_eq!(p.peer.command("status").await["submissions"], 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
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
        let receipt = signed_terminal(&mut p).await;
        p.executor
            .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
            .await
            .unwrap();
        assert!(p
            .executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        assert_eq!(p.peer.command("status").await["publications"], 1);
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

#[tokio::test]
async fn paid_buyer_expiry_of_unknown_stream_never_frees_capacity_or_retries_execution() {
    use financial::recovery::{BuyerRecovery, FinancialOutcome, Limits, Store};
    use mayhem_proto::proxy::finance::ProxyReservationExpiry;
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let backend = backend_raw(
            200,
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"prefix\"}}]}\n\n".into(),
            "text/event-stream",
            Duration::ZERO,
        )
        .await;
        let bytes = stream_request();
        let policy = json!({"schema_version":1,"lane":"proxy","payable_outcomes":["complete"],
            "allow_checkpoints":false,"hold_expiry":"release_unfinalized_and_block_retry"});
        let mut p = Paid::start_with_policy(
            &backend.base,
            ProxyEndpoint::Chat,
            rail,
            &bytes,
            true,
            Some(policy),
        )
        .await;
        let mut buyer_identity = p.peer.identity.clone();
        buyer_identity.controller_pubkey =
            Digest::new(&p.authorization.terms.buyer_pubkey).unwrap();
        let store = Arc::new(
            Store::open(
                p._fixture._store.path().join("buyer-recovery"),
                buyer_identity,
                Limits {
                    max_records: 8,
                    closed_retention_ms: 1000,
                },
            )
            .unwrap(),
        );
        let buyer = BuyerRecovery::new(store, p.peer.buyer_client.clone(), 2).unwrap();
        buyer.refresh(&p.authorization, 1).await.unwrap();
        assert!(p
            .executor
            .execute_stream(
                &p.record.invocation,
                &bytes,
                &Cancellation::default(),
                |_| async { Ok(()) }
            )
            .await
            .is_err());
        let deadline = p.authorization.terms.reservation_expires_after_epoch
            + p.authorization.terms.reservation_receipt_grace_epochs;
        p.peer
            .command(&json!({"epoch":deadline+1}).to_string())
            .await;
        let body = buyer.prepare_expiry(&p.authorization, 5).await.unwrap();
        let signature = p
            .peer
            .command(&json!({"sign_expiry":body}).to_string())
            .await;
        let expiry = ProxyReservationExpiry {
            body,
            buyer_sig: signature["buyer_sig"].as_str().unwrap().into(),
        };
        let terms = Digest::new(p.authorization.terms.digest().unwrap()).unwrap();
        buyer.retain_expiry(terms.clone(), expiry).await.unwrap();
        assert!(matches!(
            buyer.publish_expiry(terms, 6).await.unwrap(),
            Some(FinancialOutcome::ExpiredUnknown { .. })
        ));
        assert!(p.authority.lease(&p.lease).unwrap().is_some());
        assert_eq!(
            p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
            Phase::Dispatched
        );
        assert!(p
            .executor
            .reconcile_capacity(&p.record.invocation, p.record.attempt)
            .await
            .is_err());
        assert!(p
            .executor
            .prepare_waiver(&p.record.invocation, p.record.attempt)
            .await
            .is_err());
        assert!(p
            .executor
            .execute_stream(
                &p.record.invocation,
                &bytes,
                &Cancellation::default(),
                |_| async { Ok(()) }
            )
            .await
            .is_err());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        let p = p.reopen();
        assert!(p.authority.lease(&p.lease).unwrap().is_some());
        assert!(p
            .executor
            .reconcile_capacity(&p.record.invocation, p.record.attempt)
            .await
            .is_err());
        p.peer.stop().await;
    }
}

#[path = "proxy_signing.rs"]
mod protected_signing;
