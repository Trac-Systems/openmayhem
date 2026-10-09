//! Shared gateway integration fixture: real buyer/provider controllers, worker,
//! encrypted/durable stores supplied by each test, and the existing signed local
//! canonical ledger fixture. The SC-Bridge transport below is a bounded protocol
//! double, not evidence of a real Noise relay or production deployment.
use axum::{response::IntoResponse, routing::post, Json, Router};
mod streaming;
use ed25519_dalek::SigningKey;
use mayhem_proto::{
    endpoint_family_contract_template,
    proxy::{ProxyEndpoint, ProxyRail},
};
use mayhem_proxy::{
    attempts::{Digest, Identity, Journal},
    buyer_controller as buyer, capacity,
    connector::{config::ConnectionConfig, http::HttpConnection},
    discovery,
    endpoint::{Adapter, Limits},
    financial::{
        self,
        negotiation::BuyerNegotiation,
        quote::{Lifetimes, PreparedPurchase, PriceLimits, PurchaseRequest, Query, SessionBinding},
        recovery::BuyerRecovery,
    },
    negotiation, serving,
    signing::Authority,
    worker::host::{Pool, PoolLimits},
};
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
pub(crate) use streaming::StreamBackend;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::watch,
    task::{JoinHandle, JoinSet},
};

#[path = "../../../../../mayhem-proxy/tests/support/exchange_bridge.rs"]
mod exchange_bridge;
use exchange_bridge::Bridge;

pub(crate) fn digest(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
pub(crate) fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
pub(crate) fn worker_path() -> PathBuf {
    // Cargo places the gateway test executable in the same target/debug/deps tree.
    let path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("mayhem-proxy-worker");
    assert!(
        path.is_file(),
        "build mayhem-proxy-worker in the shared target first"
    );
    path
}
pub(crate) fn chat() -> Vec<u8> {
    serde_json::to_vec(
        &json!({"model":"public-model","messages":[{"role":"user","content":"hello"}]}),
    )
    .unwrap()
}
pub(crate) fn request_body(endpoint: ProxyEndpoint) -> Vec<u8> {
    match endpoint {
        ProxyEndpoint::Chat => chat(),
        ProxyEndpoint::Completions => serde_json::to_vec(
            &json!({"model":"public-model","prompt":"hi","temperature":0.2,"max_tokens":11}),
        )
        .unwrap(),
        ProxyEndpoint::Responses => serde_json::to_vec(
            &json!({"model":"public-model","input":"hi","temperature":0.2,"max_output_tokens":11}),
        )
        .unwrap(),
        ProxyEndpoint::Decisions => {
            serde_json::to_vec(&json!({"model":"public-model","state":"hi",
                "questions":{"q":{"type":"noul","instructions":"hello?"}}}))
            .unwrap()
        }
    }
}
pub(crate) fn endpoint_path(endpoint: ProxyEndpoint) -> &'static str {
    match endpoint {
        ProxyEndpoint::Chat => "/v1/chat/completions",
        ProxyEndpoint::Completions => "/v1/completions",
        ProxyEndpoint::Responses => "/v1/responses",
        ProxyEndpoint::Decisions => "/v1/decisions",
    }
}
fn protocol() -> Limits {
    Limits {
        request_bytes: 512 * 1024,
        response_bytes: 512 * 1024,
        choices: 8,
        tools: 16,
        questions: 16,
        decision_options: 32,
    }
}
fn lifetimes() -> Lifetimes {
    Lifetimes {
        acceptance_epochs: 0,
        reservation_epochs: 19,
        receipt_grace_epochs: 2,
    }
}
struct Peer {
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
}
impl Peer {
    async fn request(&mut self, command: &str) -> Value {
        self.stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("local signed fixture reply");
        serde_json::from_str(&line).unwrap()
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
pub(crate) struct Harness {
    /// Ephemeral child-fixture key only, for matching the gateway wallet in tests.
    pub(crate) buyer_seed: [u8; 32],
    pub(crate) network: discovery::Identity,
    pub(crate) provider_rpc_url: String,
    provider_signer: Arc<Authority>,
    provider_financial: Arc<financial::Client>,
    capacity: Arc<capacity::Authority>,
    pub(crate) buyer: Arc<buyer::Controller>,
    pub(crate) negotiation: Arc<BuyerNegotiation>,
    pub(crate) recovery: Arc<BuyerRecovery>,
    pub(crate) financial: Arc<financial::Client>,
    pub(crate) adapter: Arc<Adapter>,
    pub(crate) policy: mayhem_proto::proxy::finance::ProxySettlementPolicy,
    pub(crate) template: mayhem_proto::proxy::finance::ProxySpendAuthorization,
    peer: Peer,
    calls: Arc<AtomicUsize>,
    pub(crate) stream_backend: Arc<StreamBackend>,
    provider_stop: watch::Sender<bool>,
    descriptors_enabled: Arc<AtomicBool>,
    provider_ended: watch::Receiver<usize>,
    provider_task: JoinHandle<()>,
    backend_task: JoinHandle<()>,
    _bridge: Bridge,
    _directory: tempfile::TempDir,
}
impl Harness {
    pub(crate) async fn start(worker: &Path) -> Self {
        Self::start_with(worker, ProxyEndpoint::Chat, ProxyRail::Tnk).await
    }
    pub(crate) async fn start_with(
        worker: &Path,
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
    ) -> Self {
        Self::start_with_contract(worker, endpoint, rail, None).await
    }
    pub(crate) async fn start_with_contract(
        worker: &Path,
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        contract: Option<mayhem_proto::EndpointFamilyContract>,
    ) -> Self {
        let directory = private_dir();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let stream_backend = Arc::new(StreamBackend::default());
        let backend_stream = stream_backend.clone();
        let path = endpoint_path(endpoint);
        let app = Router::new().route(path, post(move |Json(input): Json<Value>| {
            let count = count.clone();
            let backend_stream = backend_stream.clone();
            async move {
                assert_eq!(input["model"], "upstream-model");
                assert!(input.get("proxy").is_none());
                match endpoint {
                    ProxyEndpoint::Completions => {
                        assert_eq!(input["prompt"], "hi");
                        assert_eq!(input["temperature"], 0.2);
                        assert_eq!(input["max_tokens"], 11);
                    }
                    ProxyEndpoint::Responses => {
                        assert_eq!(input["input"], "hi");
                        assert_eq!(input["temperature"], 0.2);
                        assert_eq!(input["max_output_tokens"], 11);
                        assert_eq!(input["store"], false);
                    }
                    _ => (),
                }
                count.fetch_add(1, Ordering::SeqCst);
                if input["stream"] == true {
                    return streaming::response(endpoint, backend_stream);
                }
                Json(match endpoint {
                    ProxyEndpoint::Decisions => json!({"id":"private-upstream-id","answers":{"q":{"type":"noul","noul":0.7}}}),
                    ProxyEndpoint::Chat => json!({"id":"private-upstream-id","object":"chat.completion",
                    "choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
                    ProxyEndpoint::Completions => json!({"id":"private-upstream-id","object":"text_completion",
                        "choices":[{"index":0,"text":"hello","finish_reason":"stop"}],
                        "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
                    ProxyEndpoint::Responses => json!({"id":"private-upstream-id","status":"completed",
                        "output":[{"id":"private-output-id","type":"message","role":"assistant",
                            "content":[{"type":"output_text","text":"hello"}]}],
                        "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}),
                }).into_response()
            }
        }));
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1/", socket.local_addr().unwrap());
        let backend_task = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        let adapter = Arc::new(
            Adapter::new(
                endpoint,
                contract.unwrap_or_else(|| {
                    endpoint_family_contract_template(match endpoint {
                        ProxyEndpoint::Chat => mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
                        ProxyEndpoint::Completions => mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
                        ProxyEndpoint::Responses => mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
                        ProxyEndpoint::Decisions => mayhem_proto::ENDPOINT_MAYHEM_DECISIONS,
                    })
                    .unwrap()
                }),
                "upstream-model".into(),
                protocol(),
            )
            .unwrap(),
        );
        let connection = Arc::new(
            HttpConnection::new(
                serde_json::from_value::<ConnectionConfig>(json!({
            "schema_version":1,"id":"owner-fixture","revision":1,"base_url":base,
            "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},
            "paths":{"chat_completions":"chat/completions","completions":"completions","responses":"responses","decisions":"decisions"},"error_profile":"open_ai"}))
                .unwrap(),
            )
            .unwrap(),
        );
        let prepared = adapter.prepare_json(&request_body(endpoint)).unwrap();
        let execution = json!({"endpoint":prepared.endpoint(),"endpoint_contract":adapter.contract_hash(),
            "recipe_hash":adapter.recipe_hash(),"connection_revision":connection.revision(),
            "connection_digest":connection.fingerprint(),"capacity_group":digest(200),
            "request_hash":prepared.request_hash(),
            "metering":mayhem_proxy::metering::Policy::for_endpoint(endpoint).contract(),
            "settlement_policy":null});
        let mut child = tokio::process::Command::new("node")
            .arg("intercom/tests/helpers/proxy-financial-rpc-fixture.mjs")
            .arg(serde_json::to_value(rail).unwrap().as_str().unwrap())
            .arg(if endpoint != ProxyEndpoint::Decisions {
                "llm"
            } else {
                "decisions"
            })
            .arg(execution.to_string())
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let ready = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("local signed fixture ready");
        let ready: Value = serde_json::from_str(&ready).unwrap();
        let mut peer = Peer {
            stdin: child.stdin.take().unwrap(),
            child,
            lines,
        };
        let network: discovery::Identity =
            serde_json::from_value(ready["identity"].clone()).unwrap();
        let template: mayhem_proto::proxy::finance::ProxySpendAuthorization =
            serde_json::from_value(ready["authorization"].clone()).unwrap();
        let policy: mayhem_proto::proxy::finance::ProxySettlementPolicy =
            serde_json::from_value(ready["policy"].clone()).unwrap();
        let identity = |controller: &str| Identity {
            network_id: network.network_id.clone(),
            msb_bootstrap: Digest::new(&network.msb_bootstrap).unwrap(),
            subnet_bootstrap: Digest::new(&network.subnet_bootstrap).unwrap(),
            controller_pubkey: Digest::new(controller).unwrap(),
        };
        let provider_id = identity(&template.terms.offer.provider_pubkey);
        let buyer_id = identity(&template.terms.buyer_pubkey);
        let provider_client = Arc::new(
            financial::Client::new(
                ready["url"].as_str().unwrap(),
                network.clone(),
                template.terms.offer.provider_pubkey.clone(),
                4,
            )
            .unwrap(),
        );
        let financial = Arc::new(
            financial::Client::new(
                ready["buyer_url"].as_str().unwrap(),
                network.clone(),
                template.terms.buyer_pubkey.clone(),
                4,
            )
            .unwrap(),
        );
        // These keys are generated only by this child fixture, never read from
        // configuration or exposed through the public RPC.
        let ephemeral = peer.request("ephemeral_test_wallet_seeds").await;
        let buyer_seed: [u8; 32] = serde_json::from_value(ephemeral["buyer"].clone()).unwrap();
        let provider_seed: [u8; 32] =
            serde_json::from_value(ephemeral["provider"].clone()).unwrap();
        let buyer_signer = Arc::new(
            Authority::from_unlocked_wallet(SigningKey::from_bytes(&buyer_seed), buyer_id.clone())
                .unwrap(),
        );
        let provider_signer = Arc::new(
            Authority::from_unlocked_wallet(
                SigningKey::from_bytes(&provider_seed),
                provider_id.clone(),
            )
            .unwrap(),
        );
        let capacity = Arc::new(
            capacity::Authority::open(
                directory.path().join("capacity"),
                provider_id.clone(),
                capacity::Limits {
                    max_groups: 4,
                    max_routes: 8,
                    max_leases: 8,
                    max_evidence_age: Duration::from_secs(60),
                },
            )
            .unwrap(),
        );
        capacity.configure_group(digest(200), 2).unwrap();
        capacity
            .configure_route(capacity::Route {
                id: digest(201),
                group: digest(200),
                lane: capacity::Lane::Proxy,
                max_concurrency: 2,
            })
            .unwrap();
        for scope in [
            capacity::Scope::Group(digest(200)),
            capacity::Scope::Route(digest(201)),
        ] {
            let ticket = capacity.begin_observation(scope).unwrap();
            capacity
                .observe(
                    ticket,
                    capacity::Evidence {
                        state: capacity::Readiness::Ready,
                        allowance: 2,
                        age: Duration::ZERO,
                        valid_for: Duration::from_secs(60),
                    },
                )
                .unwrap();
        }
        let runtime = Arc::new(financial::provider::Runtime {
            adapter: adapter.clone(),
            connection,
            capacity: capacity.clone(),
            route: digest(201),
            approved_policy: policy.clone(),
        });
        let journal = Arc::new(
            Journal::open(
                directory.path().join("provider"),
                provider_id.clone(),
                mayhem_proxy::attempts::Limits {
                    max_records: 8,
                    max_unfinished: 8,
                    closed_retention_ms: 1000,
                    max_payload_bytes: 128 * 1024 * 1024,
                },
            )
            .unwrap(),
        );
        let work = directory.path().join("worker");
        std::fs::create_dir(&work).unwrap();
        std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
        let pool = Arc::new(
            Pool::new(
                worker,
                &work,
                PoolLimits {
                    max_children: 2,
                    max_buffer_bytes: 64 * 1024 * 1024,
                    startup_timeout: Duration::from_secs(5),
                    processing_timeout: Duration::from_secs(3),
                },
            )
            .unwrap(),
        );
        let provider = serving::Controller::new(
            runtime,
            journal,
            provider_signer.clone(),
            provider_client.clone(),
            pool.clone(),
            serving::Limits {
                sessions: 4,
                per_buyer: 2,
                outbound_messages: 16,
                outbound_bytes: 4 * 1024 * 1024,
                control_wait: Duration::from_secs(5),
                proposals: negotiation::provider::Limits {
                    pending: 4,
                    per_buyer: 2,
                    request_bytes: 512 * 1024,
                    total_request_bytes: 1024 * 1024,
                    storage_operations: 4,
                    unsigned_lifetime: Duration::from_secs(30),
                },
            },
        )
        .unwrap();
        let bridge = Bridge::start(
            &template.terms.buyer_pubkey,
            &template.terms.offer.provider_pubkey,
        )
        .await;
        let mut listener = negotiation::opening::Listener::connect(
            bridge.config(false),
            provider_id,
            mayhem_proxy::exchange::Limits {
                max_message_bytes: 2 * 1024 * 1024,
            },
        )
        .await
        .unwrap();
        let (provider_stop, mut stop) = watch::channel(false);
        let (ended, provider_ended) = watch::channel(0usize);
        let descriptors_enabled = Arc::new(AtomicBool::new(true));
        let descriptor_mode = descriptors_enabled.clone();
        let provider_task = tokio::spawn(async move {
            let mut sessions = JoinSet::new();
            let mut descriptions = JoinSet::new();
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    channel = listener.next_opening(Duration::from_secs(60)) => {
                        if let Ok(channel) = channel {
                            let channel = match channel {
                                negotiation::opening::Opening::Descriptor(incoming) => {
                                    if descriptor_mode.load(Ordering::Acquire)
                                        && descriptions.len() < mayhem_proxy::descriptor::READS {
                                        let reader = provider.proposals().clone();
                                        descriptions.spawn(async move { let _ = reader.describe(incoming).await; });
                                    }
                                    continue;
                                },
                                negotiation::opening::Opening::Negotiation(channel) => channel,
                            };
                            match provider.accept(channel) {
                                Ok(handle) => { sessions.spawn(async move { let _ = handle.wait().await; }); },
                                // The recovery supervisor may reconnect while
                                // the previous control session is closing. Match
                                // production dispatch: bounded admission rejects
                                // this connection, not the entire listener.
                                Err(serving::Error::Busy) => (),
                                Err(error) => panic!("provider fixture admission failed: {error}"),
                            }
                        }
                    },
                    Some(result) = descriptions.join_next(), if !descriptions.is_empty() => { result.unwrap(); },
                    Some(result) = sessions.join_next(), if !sessions.is_empty() => {
                        result.unwrap();
                        ended.send_modify(|count| *count += 1);
                    }
                }
            }
            while descriptions.join_next().await.is_some() {}
            sessions.abort_all();
            while sessions.join_next().await.is_some() {}
        });
        let negotiation = Arc::new(
            BuyerNegotiation::new(
                Arc::new(
                    financial::negotiation::Store::open(
                        directory.path().join("negotiation"),
                        buyer_id.clone(),
                        financial::negotiation::Limits {
                            max_records: 8,
                            max_payload_bytes: 1024 * 1024,
                            max_record_bytes: 128 * 1024,
                            closed_retention_ms: 1000,
                        },
                    )
                    .unwrap(),
                ),
                financial.clone(),
                4,
            )
            .unwrap(),
        );
        let recovery = Arc::new(
            BuyerRecovery::new(
                Arc::new(
                    financial::recovery::Store::open(
                        directory.path().join("recovery"),
                        buyer_id,
                        financial::recovery::Limits {
                            max_records: 8,
                            closed_retention_ms: 1000,
                        },
                    )
                    .unwrap(),
                ),
                financial.clone(),
                4,
            )
            .unwrap(),
        );
        let buyer = Arc::new(
            buyer::Controller::new(
                negotiation.clone(),
                recovery.clone(),
                financial.clone(),
                buyer_signer,
                pool,
                bridge.config(true),
                buyer::Limits {
                    sessions: 2,
                    buffer_bytes: 128 * 1024 * 1024,
                    protocol: protocol(),
                    message_bytes: 2 * 1024 * 1024,
                    control_wait: Duration::from_secs(5),
                },
            )
            .unwrap(),
        );
        Self {
            buyer_seed,
            network,
            provider_rpc_url: ready["url"].as_str().unwrap().into(),
            provider_signer,
            provider_financial: provider_client,
            capacity,
            buyer,
            negotiation,
            recovery,
            financial,
            adapter,
            policy,
            template,
            peer,
            calls,
            provider_stop,
            descriptors_enabled,
            provider_ended,
            provider_task,
            backend_task,
            stream_backend,
            _bridge: bridge,
            _directory: directory,
        }
    }
    pub(crate) fn context(&self) -> negotiation::Context {
        let t = &self.template.terms;
        let d = |s: &str| Digest::new(s).unwrap();
        negotiation::Context {
            schema_version: 1,
            network_id: t.network_id.clone(),
            msb_bootstrap: d(&t.msb_bootstrap),
            subnet_bootstrap: d(&t.subnet_bootstrap),
            contract_version: t.contract_version,
            session_id: d(&t.session_id),
            buyer: d(&t.buyer_pubkey),
            billing_id: d(&t.billing_id),
            billing_attempt: t.billing_attempt,
            request_hash: d(&t.request_hash),
            offer: t.offer.clone(),
            rail: t.rail,
            settlement_policy_hash: d(&t.settlement_policy_hash),
        }
    }
    fn prices(&self) -> PriceLimits {
        let t = &self.template.terms;
        PriceLimits {
            rates: t.offer.rates.clone(),
            per_request_au: t.offer.per_request_au,
            min_session_au: t.offer.min_session_au,
            max_total_spend_au: t.max_total_spend_au,
        }
    }
    pub(crate) fn request(&self, gate: Arc<dyn buyer::AuthorizationGate>) -> buyer::Request {
        buyer::Request {
            context: self.context(),
            body: request_body(self.adapter.endpoint()),
            gate,
            authorization: buyer::Authorization {
                prices: self.prices(),
                output_units: (self.adapter.endpoint() != ProxyEndpoint::Decisions).then_some(37),
                lifetimes: lifetimes(),
                settlement_policy: self.policy.clone(),
                endpoint_contract: self.adapter.contract_hash().clone(),
                recipe_hash: self.adapter.recipe_hash().clone(),
            },
        }
    }
    pub(crate) async fn prepare(&self) -> PreparedPurchase {
        self.prepare_connection(Digest::new(&self.template.terms.connection_digest).unwrap())
            .await
    }
    pub(crate) async fn prepare_connection(&self, connection_digest: Digest) -> PreparedPurchase {
        let t = &self.template.terms;
        let quote = self
            .financial
            .quote(&Query {
                offer: t.offer.clone(),
                rail: t.rail,
                settlement_policy_hash: t.settlement_policy_hash.clone(),
                billing_id: t.billing_id.clone(),
            })
            .await
            .unwrap();
        let request = PurchaseRequest::new(
            self.adapter.public_snapshot(),
            request_body(self.adapter.endpoint()),
            self.prices(),
            (self.adapter.endpoint() != ProxyEndpoint::Decisions).then_some(37),
            lifetimes(),
        )
        .unwrap();
        quote
            .prepare_purchase(
                &request,
                &SessionBinding {
                    session_id: Digest::new(&t.session_id).unwrap(),
                    reservation_id: Digest::new(&t.reservation_id).unwrap(),
                    connection_digest,
                    capacity_lease: Digest::new(&t.capacity_lease).unwrap(),
                },
            )
            .unwrap()
    }
    pub(crate) async fn status(&mut self) -> Value {
        self.peer.request("status").await
    }
    pub(crate) async fn command(&mut self, command: &str) -> Value {
        self.peer.request(command).await
    }
    pub(crate) fn backend_calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub(crate) fn capacity_status(&self) -> capacity::Status {
        self.capacity.status(&digest(201)).unwrap()
    }
    pub(crate) fn reserve_test_capacity(&self, n: u64) -> capacity::Reservation {
        self.capacity
            .reserve(
                &digest(201),
                capacity::Work {
                    invocation: digest(n),
                    request_hash: digest(n + 1),
                },
            )
            .unwrap()
    }
    pub(crate) fn release_test_capacity(&self, reservation: capacity::Reservation) {
        self.capacity.cancel_reserved(reservation).unwrap();
    }
    pub(crate) fn stop_descriptors(&self) {
        // Simulate a live older peer that ignores the new opening tag while
        // continuing to accept the unchanged paid negotiation protocol.
        self.descriptors_enabled.store(false, Ordering::Release);
    }
    pub(crate) async fn session_frame_tags(&self) -> Vec<String> {
        self._bridge
            .frames
            .lock()
            .await
            .iter()
            .filter_map(|frame| frame["frame"]["t"].as_str().map(str::to_owned))
            .collect()
    }
    pub(crate) async fn wait_provider_ends(&mut self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while *self.provider_ended.borrow() < expected {
                self.provider_ended.changed().await.unwrap();
            }
        })
        .await
        .expect("provider session must release before reconnect");
    }
    pub(crate) async fn stop(self) {
        self.buyer.shutdown().await.unwrap();
        self.provider_stop.send_replace(true);
        self.provider_task.await.unwrap();
        self.backend_task.abort();
        self.peer.stop().await;
    }
}

pub(crate) struct ControlFixture {
    pub(crate) control: Arc<crate::openai::proxy_control::ProxyControl>,
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    _directory: tempfile::TempDir,
}
impl ControlFixture {
    pub(crate) async fn pause(&mut self) {
        self.stop.send_replace(true);
        for task in self.tasks.drain(..) {
            task.await.unwrap();
        }
    }
    pub(crate) async fn stop(mut self) {
        self.pause().await;
    }
}
fn protected(path: &Path, bytes: &[u8]) {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
impl Harness {
    pub(crate) async fn control(&self) -> ControlFixture {
        self.control_with_registry(None).await
    }
    pub(crate) async fn control_with_registry(
        &self,
        registry: Option<crate::openai::proxy_control::RegistryConfig>,
    ) -> ControlFixture {
        self.control_with_evidence(registry, None).await
    }
    pub(crate) async fn control_with_evidence(
        &self,
        registry: Option<crate::openai::proxy_control::RegistryConfig>,
        conformance: Option<mayhem_proxy::conformance::Config>,
    ) -> ControlFixture {
        use crate::openai::proxy_control::{self, Prepared};
        use mayhem_proxy::{health, presence, supervisor::RefreshPolicy};
        let directory = private_dir();
        let bridge = self._bridge.config(true);
        protected(
            &directory.path().join("bridge-token"),
            bridge.token.as_bytes(),
        );
        let refresh = RefreshPolicy {
            interval_ms: 2000,
            page_pause_ms: 10,
            retry_initial_ms: 100,
            retry_max_ms: 1000,
            jitter_percent: 0,
        };
        let config = proxy_control::Config {
            schema_version: 1,
            network: self.network.clone(),
            peer_rpc_url: self.provider_rpc_url.clone(),
            state_dir: directory.path().join("state"),
            bridge: proxy_control::Bridge {
                url: bridge.url.as_str().into(),
                token_file: directory.path().join("bridge-token"),
                operation_timeout_ms: 1000,
                frame_bytes: 64 * 1024,
                queue_events: 16,
                queue_bytes: 2 * 1024 * 1024,
            },
            max_markets: 2,
            max_presence_routes: 8,
            selected_markets: vec![Digest::new(&self.template.terms.offer.market_id).unwrap()],
            refresh: refresh.clone(),
            rpc_timeout_ms: 2000,
            registry,
            conformance,
        };
        let path = directory.path().join("control.json");
        protected(&path, &serde_json::to_vec(&config).unwrap());
        let prepared = Prepared::load(&path, &self.network).unwrap();
        let (control, lifecycle) = tokio::task::spawn_blocking(move || prepared.open())
            .await
            .unwrap()
            .unwrap();
        let (stop, stopped) = watch::channel(false);
        let control_task = tokio::spawn(async move {
            lifecycle.run(stopped).await.unwrap();
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let health = control.health().unwrap();
                assert!(
                    health.failure.is_none(),
                    "canonical discovery fixture failed: {health:?}"
                );
                if health.catalog_fresh && health.presence.connected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real local discovery and selected presence subscription");
        let tokenizer = digest(801);
        let monitor = health::Monitor::new(
            health::Policy {
                max_routes: 8,
                max_classes_per_route: 8,
                evidence_ttl_ms: 60_000,
                successes_to_increase: 2,
                bad_samples_to_reduce: 2,
                latency_baseline_samples: 3,
                latency_multiplier: 4,
                latency_increase_ms: 1000,
                min_native_tok_s: 5,
                recovery: refresh,
            },
            2,
            1,
        )
        .unwrap();
        let measured = self.adapter.endpoint() != ProxyEndpoint::Decisions;
        if measured {
            monitor
                .register_measured(digest(201), 2, tokenizer.clone())
                .unwrap();
        } else {
            monitor.register(digest(201), 2, false).unwrap();
        }
        // Deterministic local tokenizer-count/timestamp fixture evidence. This
        // tests publishing/receiving evidence, not tokenizer model accuracy.
        for _ in 0..3 {
            let mut sample = monitor
                .observe_request(
                    &digest(201),
                    health::Class::new(1024, health::Thinking::Unknown, true),
                )
                .unwrap();
            if measured {
                sample.headers();
                sample.delta(&json!({"choices":[{"delta":{"content":"one "}}]}));
                sample.native_progress(tokenizer.clone(), 1).unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                sample.delta(&json!({"choices":[{"delta":{"content":"two three four five"}}]}));
                sample.native_progress(tokenizer.clone(), 5).unwrap();
            }
            sample.success(None);
        }
        if measured {
            assert!(monitor
                .snapshot(&digest(201))
                .unwrap()
                .meets_native_floor(5));
        }
        self.capacity
            .bind_live(
                capacity::Scope::Group(digest(200)),
                monitor.connection_source(),
            )
            .unwrap();
        self.capacity
            .bind_live(
                capacity::Scope::Route(digest(201)),
                monitor.route_source(&digest(201)).unwrap(),
            )
            .unwrap();
        let mut publisher = mayhem_proxy::test_support::PresencePublisher::new(
            self.provider_signer.clone(),
            self.capacity.clone(),
        )
        .unwrap();
        let financial = self.provider_financial.clone();
        let query = financial::offer::Query {
            offer: self.template.terms.offer.clone(),
            rail: self.template.terms.rail,
            settlement_policy_hash: self.template.terms.settlement_policy_hash.clone(),
        };
        let channel =
            presence::channel(&self.network, &Digest::new(&query.offer.market_id).unwrap())
                .unwrap();
        let mut transport = mayhem_bridge::ScBridgeClient::connect(self._bridge.config(false))
            .await
            .unwrap();
        transport.mute_sidechannel_events().await.unwrap();
        transport.join_many([&channel]).await.unwrap();
        let mut stopped = stop.subscribe();
        let presence_task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _=stopped.changed()=>break,
                    _=tick.tick()=>{
                        let observation=financial.offer_state(&query).await.unwrap();
                        let signed=publisher.issue(&observation,&digest(201),&monitor,60_000,None,true).unwrap().unwrap();
                        transport.send(&channel,&signed).await.unwrap();
                    }
                }
            }
        });
        let offer = &self.template.terms.offer;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if control
                    .presence()
                    .status(
                        &Digest::new(&offer.market_id).unwrap(),
                        &Digest::new(&offer.provider_pubkey).unwrap(),
                        &Digest::new(offer.slot_id().unwrap()).unwrap(),
                        None,
                    )
                    .unwrap()
                    == presence::Eligibility::Available
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("production publisher's signed presence must satisfy actual eligibility");
        ControlFixture {
            control,
            stop,
            tasks: vec![control_task, presence_task],
            _directory: directory,
        }
    }
}
