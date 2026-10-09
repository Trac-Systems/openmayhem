use super::*;
use mayhem_proxy::{
    attempts, capacity, managed,
    setup::{
        LaunchBinding, LifecycleObservation, ProcessObservation, RunFuture, RunLifecycle,
        RunTemplate,
    },
    signing::Authority,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

struct Host {
    retained: Mutex<Option<LaunchBinding>>,
    installs: AtomicUsize,
    mode: Mutex<&'static str>,
}
impl Host {
    fn new() -> Self {
        Self {
            retained: Mutex::new(None),
            installs: AtomicUsize::new(0),
            mode: Mutex::new("normal"),
        }
    }
}
impl RunLifecycle for Host {
    fn binding(
        &self,
        identity: &attempts::Identity,
        _: &Path,
        digest: &Digest,
    ) -> mayhem_proxy::setup::Result<LaunchBinding> {
        Ok(LaunchBinding {
            schema_version: 1,
            authority_digest: d(60),
            child_name: format!("provider-proxy-{}", identity.controller_pubkey.as_str()),
            child_config_hash: digest.clone(),
        })
    }
    fn inspect<'a>(&'a self, binding: &'a LaunchBinding) -> RunFuture<'a, LifecycleObservation> {
        Box::pin(async move {
            let mode = *self.mode.lock().unwrap();
            if mode == "offline" {
                return Err(Error::RunUnavailable);
            }
            let retained = self.retained.lock().unwrap();
            let matches = retained
                .as_ref()
                .map(|v| v == binding && mode != "mismatch");
            Ok(LifecycleObservation {
                schema_version: 1,
                name: binding.child_name.clone(),
                state: match matches {
                    None => "missing",
                    Some(true) => "matched",
                    Some(false) => "mismatch",
                }
                .into(),
                persistent: matches.is_some(),
                config_matches: matches,
                lifecycle: matches.map(|_| ProcessObservation {
                    running: mode != "stopped",
                    pid: None,
                    restart_pending: false,
                    restarts: 0,
                    crash_loop: false,
                }),
            })
        })
    }
    fn install<'a>(
        &'a self,
        binding: &'a LaunchBinding,
        path: &'a Path,
        digest: &'a Digest,
    ) -> RunFuture<'a, ()> {
        Box::pin(async move {
            // Validate real loader and durable intent before simulated ACK loss.
            let prepared = managed::Prepared::load_supervised_pinned(path, digest).unwrap();
            assert_eq!(
                &self.binding(prepared.identity(), path, digest).unwrap(),
                binding
            );
            assert!(path.parent().unwrap().join("wizard-run.json").is_file());
            if *self.mode.lock().unwrap() == "reject" {
                return Err(Error::RunPrerequisite);
            }
            self.installs.fetch_add(1, Ordering::SeqCst);
            *self.retained.lock().unwrap() = Some(binding.clone());
            Err(Error::RunUnavailable)
        })
    }
}
fn template(f: &Fixture) -> RunTemplate {
    let schedule = json!({"interval_ms":1000,"page_pause_ms":1,"retry_initial_ms":100,"retry_max_ms":1000,"jitter_percent":0});
    let pool = mayhem_proxy::worker::host::PoolLimits {
        max_children: 2,
        max_buffer_bytes: 64 * 1024 * 1024,
        startup_timeout: Duration::from_secs(5),
        processing_timeout: Duration::from_secs(3),
    };
    private(
        &f.dir.path().join("bridge-token"),
        b"synthetic-bridge-token\n",
    );
    serde_json::from_value(json!({"schema_version":1,"bridge":{"url":"ws://127.0.0.1:1","token_file":f.dir.path().join("bridge-token"),"operation_timeout_ms":5000,"frame_bytes":65536,"queue_events":16,"queue_bytes":2097152,"logical_message_bytes":4194304},
        "health":{"max_routes":8,"max_classes_per_route":8,"evidence_ttl_ms":60000,"successes_to_increase":2,"bad_samples_to_reduce":2,"latency_baseline_samples":3,"latency_multiplier":4,"latency_increase_ms":1000,"min_native_tok_s":5,"recovery":schedule},
        "limits":{"max_groups":8,"max_routes":8,"max_leases":16,"tokenizer_bytes":4194304,"tokenizer_workers":4,"financial_reads":4,"sessions":8,"registrations":8,"observation_ms":50,"recovery_interval_ms":50,
            "serving":{"sessions":4,"per_buyer":2,"outbound_messages":16,"outbound_bytes":4194304,"control_wait":Duration::from_secs(5),"proposals":{"pending":4,"per_buyer":2,"request_bytes":65536,"total_request_bytes":131072,"storage_operations":4,"unsigned_lifetime":Duration::from_secs(30)}},
            "worker":pool,"recovery_worker":pool,"settlement_worker":pool,"journal":{"max_records":50,"max_unfinished":16,"closed_retention_ms":1000,"max_payload_bytes":134217728},"maintenance":{"page_size":4,"schedule":schedule}},
        "tokenizer":null,"allow_recovery_probes":true})).unwrap()
}
fn identity(f: &Fixture) -> attempts::Identity {
    attempts::Identity {
        network_id: f.input.network.network_id.clone(),
        msb_bootstrap: Digest::new(&f.input.network.msb_bootstrap).unwrap(),
        subnet_bootstrap: Digest::new(&f.input.network.subnet_bootstrap).unwrap(),
        controller_pubkey: f.input.provider_pubkey.clone(),
    }
}
fn limits() -> capacity::Limits {
    capacity::Limits {
        max_groups: 8,
        max_routes: 8,
        max_leases: 16,
        max_evidence_age: Duration::from_secs(60),
    }
}
#[tokio::test]
async fn run_exact_canonical_publication_lost_ack_restart_and_spent_budget() {
    let mut f = publication::owned(ProxyEndpoint::Decisions, 122);
    let peer = publication::Peer::start(&mut f).await;
    let backend = probes::backend(
        &mut f,
        200,
        serde_json::to_vec(&json!({"id":"u","answers":{"q":{"type":"noul","noul":0.7}}})).unwrap(),
        false,
        Duration::ZERO,
    );
    let probe = || {
        probes::plan(
            &f,
            json!({"model":"public-model","state":"synthetic","questions":{"q":{"type":"noul","instructions":"hello?"}}}),
        )
    };
    let host = Host::new();
    let created = f.store().create(f.input.clone()).unwrap();
    let checked = f.store().check(created.revision).unwrap();
    assert!(matches!(
        f.store()
            .run_plan(checked.revision, template(&f), probe(), &peer.rpc(), &host),
        Err(Error::RunPrerequisite)
    ));
    let checked = f.store().probe(checked.revision, probe()).await.unwrap();
    assert_eq!(checked.probe_status, "protocol_validated");
    let publication = f.store().publication_plan(checked.revision, false).unwrap();
    let permit = peer.permit(&publication, "fiat").await;
    let published = f
        .store()
        .publish(
            checked.revision,
            &peer.rpc(),
            10000,
            publication
                .authorize(&publication::signer(122), Some(permit))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        published.publication_status,
        "canonical_operations_confirmed"
    );
    let plan = f
        .store()
        .run_plan(
            published.revision,
            template(&f),
            probe(),
            &peer.rpc(),
            &host,
        )
        .unwrap();
    assert_eq!(plan.probe_budget, probe().budget);
    assert!(!f.store.join("wizard-run.json").exists());
    let before = capacity::Authority::open_existing(
        f.dir.path().join("capacity.redb"),
        identity(&f),
        limits(),
    )
    .unwrap()
    .probe_budget(&d(2))
    .unwrap()
    .unwrap();
    assert_eq!(before.used_attempts, 1);
    assert!(matches!(
        f.store()
            .start_run(
                published.revision,
                template(&f),
                probe(),
                &peer.rpc(),
                &d(99),
                &host
            )
            .await,
        Err(Error::RunConflict)
    ));
    assert_eq!(host.installs.load(Ordering::SeqCst), 0);
    *host.mode.lock().unwrap() = "reject";
    assert!(matches!(
        f.store()
            .start_run(
                published.revision,
                template(&f),
                probe(),
                &peer.rpc(),
                &plan.plan_digest,
                &host
            )
            .await,
        Err(Error::RunPrerequisite)
    ));
    assert_eq!(f.store().inspect_run().unwrap().unwrap().state, "missing");
    assert_eq!(host.installs.load(Ordering::SeqCst), 0);
    *host.mode.lock().unwrap() = "normal";
    let installed = f
        .store()
        .start_run(
            published.revision,
            template(&f),
            probe(),
            &peer.rpc(),
            &plan.plan_digest,
            &host,
        )
        .await
        .unwrap();
    assert_eq!(installed.state, "installed");
    assert!(!installed.capacity_advertised_by_setup);
    assert_eq!(host.installs.load(Ordering::SeqCst), 1);
    let report = f.store().recover_run(&host).await.unwrap();
    assert_eq!(report.plan.plan_digest, plan.plan_digest);
    f.store()
        .start_run(
            published.revision,
            template(&f),
            probe(),
            &peer.rpc(),
            &plan.plan_digest,
            &host,
        )
        .await
        .unwrap();
    assert_eq!(host.installs.load(Ordering::SeqCst), 1);
    *host.mode.lock().unwrap() = "stopped";
    let report = Store::open(&f.store)
        .unwrap()
        .recover_run(&host)
        .await
        .unwrap();
    assert_eq!(report.state, "installed");
    assert!(!report.observation.unwrap().lifecycle.unwrap().running);
    *host.mode.lock().unwrap() = "mismatch";
    assert_eq!(
        f.store().recover_run(&host).await.unwrap().state,
        "conflict"
    );
    *host.mode.lock().unwrap() = "offline";
    assert_eq!(
        f.store().recover_run(&host).await.unwrap().state,
        "unavailable"
    );
    *host.mode.lock().unwrap() = "normal";
    let path = f.store.join(format!(
        "wizard-managed-{}.json",
        plan.config_digest.as_str()
    ));
    let bytes = std::fs::read(&path).unwrap();
    for _ in 0..2 {
        let prepared =
            managed::Prepared::load_supervised_pinned(&path, &plan.config_digest).unwrap();
        let signer = Arc::new(
            Authority::from_unlocked_wallet(publication::signer(122), prepared.identity().clone())
                .unwrap(),
        );
        let provider = prepared.open(signer).unwrap();
        assert_eq!(
            provider.route_status(&d(50)).unwrap().1.available,
            0,
            "restart health must remain unknown"
        );
        drop(provider);
        let after = capacity::Authority::open_existing(
            f.dir.path().join("capacity.redb"),
            identity(&f),
            limits(),
        )
        .unwrap()
        .probe_budget(&d(2))
        .unwrap()
        .unwrap();
        assert_eq!(
            after, before,
            "same original cumulative budget after managed restart"
        );
    }
    let mut config: Value = serde_json::from_slice(&bytes).unwrap();
    config["connections"][0]["probe_budget"]["max_attempts"] = json!(99);
    private(&path, &serde_json::to_vec(&config).unwrap());
    assert!(managed::Prepared::load_supervised_pinned(&path, &plan.config_digest).is_err());
    assert!(matches!(
        f.store().recover_run(&host).await,
        Err(Error::RunConflict)
    ));
    private(&path, &bytes);
    let connection_original = std::fs::read(&f.input.connection_file).unwrap();
    let mut changed: Value = serde_json::from_slice(&connection_original).unwrap();
    changed["revision"] = json!(2);
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&changed).unwrap(),
    );
    assert!(
        managed::Prepared::load_supervised_pinned(&path, &plan.config_digest).is_err(),
        "mutable external config must preserve exact fingerprint"
    );
    private(&f.input.connection_file, &connection_original);
    // Changed editable draft does not relaunch or replace the retained original.
    let mut next = f.input.clone();
    next.offers[0].revision += 1;
    f.store().update(published.revision, next).unwrap();
    let recovered = f.store().recover_run(&host).await.unwrap();
    assert!(!recovered.for_current_configuration);
    assert_eq!(recovered.plan.plan_digest, plan.plan_digest);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.installs.load(Ordering::SeqCst), 1);
    let capacity = f.dir.path().join("capacity.redb");
    std::fs::rename(&capacity, f.dir.path().join("capacity-original.redb")).unwrap();
    let prepared = managed::Prepared::load_supervised_pinned(&path, &plan.config_digest).unwrap();
    let signer = Arc::new(
        Authority::from_unlocked_wallet(publication::signer(122), prepared.identity().clone())
            .unwrap(),
    );
    assert!(prepared.open(signer).is_err());
    assert!(!capacity.exists(), "missing state is not recreated");
    if let Some(path) = std::env::var_os("MAYHEM_SETUP_RUN_EVIDENCE") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"schema_version":1,"fixture":"actual_canonical_publication_worker_and_managed_runtime_with_lifecycle_ack_loss_double","installed":installed,"recovered":recovered,"upstream_calls":1,"installs":1,"budget":before,"missing_capacity_refused":true})).unwrap()).unwrap()
    }
    peer.close().await;
}

#[test]
fn run_existing_capacity_refuses_missing_empty_or_unrelated_database() {
    let f = Fixture::new(ProxyEndpoint::Decisions);
    let path = f.dir.path().join("capacity.redb");
    assert!(capacity::Authority::open_existing(&path, identity(&f), limits()).is_err());
    assert!(!path.exists());
    private(&path, b"");
    assert!(capacity::Authority::open_existing(&path, identity(&f), limits()).is_err());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    std::fs::remove_file(&path).unwrap();
    let db = redb::Database::create(&path).unwrap();
    drop(db);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(capacity::Authority::open_existing(&path, identity(&f), limits()).is_err());
}

#[path = "../support/exchange_bridge.rs"]
mod bridge;

/// Explicit portable binary paths. Local synthetic wallet, canonical fixture,
/// decoder worker and protocol bridge double; no real funds or upstream service.
#[tokio::test]
#[ignore = "requires explicitly built MAYHEM_SETUP_CLI_BINARY and MAYHEM_SETUP_DAEMON_BINARY"]
async fn run_cli_real_supervisor_publication_restart() {
    use std::process::Stdio;
    use tokio::process::Command;
    let binary = PathBuf::from(std::env::var_os("MAYHEM_SETUP_CLI_BINARY").expect("CLI binary"));
    let daemon =
        PathBuf::from(std::env::var_os("MAYHEM_SETUP_DAEMON_BINARY").expect("daemon binary"));
    let mut f = publication::owned(ProxyEndpoint::Decisions, 123);
    let peer = publication::Peer::start(&mut f).await;
    let backend = probes::backend(
        &mut f,
        200,
        serde_json::to_vec(&json!({"id":"u","answers":{"q":{"type":"noul","noul":0.7}}})).unwrap(),
        false,
        Duration::ZERO,
    );
    let bridge = bridge::Bridge::start(d(71).as_str(), f.input.provider_pubkey.as_str()).await;
    let probe = probes::plan(
        &f,
        json!({"model":"public-model","state":"synthetic","questions":{"q":{"type":"noul","instructions":"hello?"}}}),
    );
    let rev = f.store().create(f.input.clone()).unwrap().revision;
    let rev = f.store().check(rev).unwrap().revision;
    let rev = f
        .store()
        .probe(rev, serde_json::from_value(json!(probe)).unwrap())
        .await
        .unwrap()
        .revision;
    let plan = f.store().publication_plan(rev, false).unwrap();
    let permit = peer.permit(&plan, "fiat").await;
    let rev = f
        .store()
        .publish(
            rev,
            &peer.rpc(),
            10000,
            plan.authorize(&publication::signer(123), Some(permit))
                .unwrap(),
        )
        .await
        .unwrap()
        .revision;
    let mut runtime = template(&f);
    runtime.bridge.url = bridge.config(false).url.to_string();
    private(&runtime.bridge.token_file, b"test-provider\n");
    let runtime_path = f.dir.path().join("runtime-template.json");
    private(&runtime_path, &serde_json::to_vec(&runtime).unwrap());
    let probe_path = f.dir.path().join("probe-plan.json");
    private(&probe_path, &serde_json::to_vec(&probe).unwrap());
    let mut flow = flow::config(&f);
    flow.probe_plan = Some(probe_path);
    flow.peer_rpc = Some(peer.rpc());
    let home = f.dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let password = home.join("synthetic-password");
    private(&password, b"synthetic-fixture-password\n");
    flow.run = Some(mayhem_proxy::setup::RunSettings {
        template: runtime_path.clone(),
        wallet_password_file: Some(password),
    });
    let flow_path = f.dir.path().join("flow.json");
    private(&flow_path, &serde_json::to_vec(&flow).unwrap());
    let keypair = home.join("fixture-wallet.json");
    // Use the wallet library's actual encrypted storage format, with only a
    // deterministic disposable fixture key. The normal CLI wallet helper reads it.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assets = f.dir.path().join("verified-assets");
    let assets_output = Command::new("node")
        .arg(root.join("crates/mayhem-proxy/tests/setup/run_assets.mjs"))
        .arg(std::fs::canonicalize(&root).unwrap())
        .arg(&assets)
        .output()
        .await
        .unwrap();
    assert!(
        assets_output.status.success(),
        "isolated candidate release verification: {}",
        String::from_utf8_lossy(&assets_output.stderr)
    );
    let wallet_script = r#"import fs from 'node:fs';import crypto from 'trac-crypto-api';const publicKey=process.argv[2];const data={publicKey,secretKey:'7b'.repeat(32)+publicKey,mnemonic:null,derivationPath:null};const e=crypto.data.encrypt(Buffer.from(JSON.stringify(data)),Buffer.from('synthetic-fixture-password'));fs.writeFileSync(process.argv[1],JSON.stringify({nonce:e.nonce.toString('hex'),salt:e.salt.toString('hex'),ciphertext:e.ciphertext.toString('hex')}),{mode:0o600});"#;
    let made = Command::new("node")
        .args(["--input-type=module", "-e", wallet_script])
        .arg(&keypair)
        .arg(f.input.provider_pubkey.as_str())
        .current_dir(root.join("intercom"))
        .output()
        .await
        .unwrap();
    assert!(made.status.success(), "synthetic wallet creation failed");
    let bind = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = bind.local_addr().unwrap();
    drop(bind);
    let token = "ac".repeat(32);
    private(&home.join("mayhemd-control-token"), token.as_bytes());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    async fn spawn_daemon(
        binary: &Path,
        home: &Path,
        address: std::net::SocketAddr,
        token: &str,
        client: &reqwest::Client,
        assets: &Path,
    ) -> tokio::process::Child {
        let child = Command::new(binary)
            .arg("--home")
            .arg(home)
            .arg("--bind")
            .arg(address.to_string())
            .arg("--exit-after-ms")
            .arg("60000")
            .env("MAYHEMD_CONTROL_TOKEN", token)
            .env("MAYHEM_ASSET_DIR", assets)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if client
                    .get(format!("http://{address}/status"))
                    .send()
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        child
    }
    async fn stop(child: &mut tokio::process::Child) {
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(child.id().unwrap() as i32).unwrap(),
            rustix::process::Signal::TERM,
        )
        .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
    }
    async fn action(
        binary: &Path,
        home: &Path,
        keypair: &Path,
        flow: &Path,
        action: Value,
        assets: &Path,
    ) -> Value {
        let path = home.join("action.json");
        private(&path, &serde_json::to_vec(&action).unwrap());
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            Command::new(binary)
                .args(["provider", "proxy", "setup", "wizard", "--config"])
                .arg(flow)
                .arg("--home")
                .arg(home)
                .arg("--keypair")
                .arg(keypair)
                .arg("--action-file")
                .arg(path)
                .env("MAYHEM_ASSET_DIR", assets)
                .env_remove("MAYHEM_WALLET_PASSWORD")
                .env_remove("MAYHEM_HOME")
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "wizard action {} failed: {}",
            action["action"],
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    let mut process = spawn_daemon(&daemon, &home, address, &token, &client, &assets).await;
    let reviewed = action(
        &binary,
        &home,
        &keypair,
        &flow_path,
        json!({"action":"run_plan","expected_revision":rev}),
        &assets,
    )
    .await;
    let started=action(&binary,&home,&keypair,&flow_path,json!({"action":"start_run","expected_revision":rev,"plan_digest":reviewed["action_result"]["plan_digest"]}),&assets).await;
    assert_eq!(started["action_result"]["state"], "installed");
    tokio::time::timeout(Duration::from_secs(20), async {
        while backend.calls.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let name = started["action_result"]["plan"]["launch"]["child_name"]
        .as_str()
        .unwrap();
    let status: Value = client
        .get(format!("http://{address}/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["children"].as_object().unwrap().len(), 1);
    assert_eq!(status["children"][name]["running"], true);
    stop(&mut process).await;
    let original = f.store.join(format!(
        "wizard-managed-{}.json",
        started["action_result"]["plan"]["config_digest"]
            .as_str()
            .unwrap()
    ));
    let original_bytes = std::fs::read(&original).unwrap();
    // Recovery needs the original retained config, not a new template or plan.
    std::fs::remove_file(runtime_path).unwrap();
    let mut process = spawn_daemon(&daemon, &home, address, &token, &client, &assets).await;
    let recovered = action(
        &binary,
        &home,
        &keypair,
        &flow_path,
        json!({"action":"recover_run"}),
        &assets,
    )
    .await;
    assert_eq!(recovered["action_result"]["state"], "installed");
    assert_eq!(
        recovered["action_result"]["plan"],
        started["action_result"]["plan"]
    );
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        2,
        "supervisor restart must not renew spent allowance"
    );
    let status: Value = client
        .get(format!("http://{address}/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["children"].as_object().unwrap().len(), 1);
    assert_eq!(status["children"][name]["running"], true);
    assert_eq!(std::fs::read(original).unwrap(), original_bytes);
    stop(&mut process).await;
    let budget = capacity::Authority::open_existing(
        f.dir.path().join("capacity.redb"),
        identity(&f),
        limits(),
    )
    .unwrap()
    .probe_budget(&d(2))
    .unwrap()
    .unwrap();
    assert_eq!(budget.used_attempts, 2);
    if let Some(path) = std::env::var_os("MAYHEM_SETUP_CLI_RUN_EVIDENCE") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"schema_version":1,"fixture":"actual_cli_mayhemd_encrypted_synthetic_wallet_canonical_publication_and_worker_with_loopback_bridge_double","reviewed":reviewed["action_result"],"started":started["action_result"],"recovered":recovered["action_result"],"upstream_calls":2,"child_count":1,"budget":budget,"native_children_created":0})).unwrap()).unwrap();
    }
    peer.close().await;
}
