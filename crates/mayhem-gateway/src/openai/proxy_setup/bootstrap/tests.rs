use super::*;
#[path="../../../../../mayhem-proxy/tests/support/setup_discovery.rs"]
mod canonical_fixture;
#[path="guided_tests.rs"]
mod guided;
use mayhem_proxy::{
    attempts::{Digest, Identity},
    setup::{self, LaunchBinding, LifecycleObservation, RunFuture},
};
use std::{os::unix::fs::PermissionsExt, path::Path};

struct NoLifecycle;
impl RunLifecycle for NoLifecycle {
    fn binding(&self, _: &Identity, _: &Path, _: &Digest) -> setup::Result<LaunchBinding> {
        panic!("bootstrap must not create a Run")
    }
    fn inspect<'a>(&'a self, _: &'a LaunchBinding) -> RunFuture<'a, LifecycleObservation> {
        panic!("bootstrap must not inspect a process")
    }
    fn install<'a>(
        &'a self,
        _: &'a LaunchBinding,
        _: &'a Path,
        _: &'a Digest,
    ) -> RunFuture<'a, ()> {
        panic!("bootstrap must not launch a process")
    }
}
fn private(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn config(dir: &Path) -> BootstrapConfig {
    let bridge = dir.join("private-bridge-key");
    private(&bridge, b"synthetic-bridge-only");
    let token = dir.join("private-tokenizer.json");
    let data=serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,"model":{"type":"WordLevel","vocab":{"[UNK]":0,"hello":1,"world":2},"unk_token":"[UNK]"}})).unwrap();
    private(&token, &data);
    BootstrapConfig{destination:dir.join("proxy-setup"),host:Host{
        network:serde_json::from_value(json!({"network_id":"dashboard-fixture","msb_bootstrap":"03".repeat(32),"subnet_bootstrap":"04".repeat(32),"contract_version":mayhem_proto::CONTRACT_VERSION})).unwrap(),
        provider_pubkey:Digest::new(hex::encode(ed25519_dalek::SigningKey::from_bytes(&[201;32]).verifying_key().to_bytes())).unwrap(),
        peer_rpc:std::fs::read_to_string(dir.join("fixture-peer-url")).unwrap_or_else(|_|"http://127.0.0.1:9/".into()),bridge_url:"ws://127.0.0.1:9/".into(),bridge_token_file:bridge,
        worker_program:std::env::current_exe().unwrap(),wallet_password_file:None,admission_origin:None,declaration_registry:None},
        tokenizers:BTreeMap::from([("approved".into(),Tokenizer{file:token,digest:Digest::new(blake3::hash(&data).to_hex().to_string()).unwrap(),limits:mayhem_proxy::health::native::Limits{artifact_bytes:1024*1024,output_bytes:1024*1024,channels:16,workers:1,minimum_tokens:2}})]),
        credentials:BTreeMap::new(),lifecycle:Arc::new(NoLifecycle)}
}
fn input() -> Value {
    json!({"schema_version":1,"base_url":"http://127.0.0.1:9/v1/","network_policy":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},"credential":{"kind":"bearer_value","value":"synthetic-dashboard-only-key"},"endpoint":"openai_chat_completions","upstream_model":"fixture-model","market":{"action":"create_market","slug":"dashboard-fixture","model":{"family_id":"fixture","model_id":"declared-model","revision":"","quantization":""}},"served_context":4096,"concurrency":2,"accepted_rails":["fiat"],"sequence":1,"offers":[{"revision":1,"ctx_bracket":"ctx4096","outcome_class":"","rates":[{"unit":"input_token","granularity":1000,"per_unit_au":"123"},{"unit":"output_token","granularity":1000,"per_unit_au":"456"}],"per_request_au":"2","min_session_au":"3","accepted_rails":["fiat"]}],"settlement_policy":{"schema_version":1,"lane":"proxy","payable_outcomes":["complete"],"allow_checkpoints":false},"probe_budget":{"max_attempts":2,"max_cost_microusd":20,"per_attempt_cost_microusd":10},"probe_output_limit":32,"probe_timeout_ms":3000,"allow_recovery_probes":false,"tokenizer_id":"approved","closed_retention_ms":86400000})
}
struct Server {
    peer: Option<canonical_fixture::Server>,
    control: Arc<Option<Control>>,
    cookie: std::sync::Mutex<Option<String>>,
    origin: String,
    token: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn start(dir: &Path) -> Server {
    let peer=if dir.join("proxy-setup/wizard.json").exists(){None}else{
        let peer=canonical_fixture::Server::start(serde_json::to_value(config(dir).host.network).unwrap()).await;
        private(&dir.join("fixture-peer-url"),peer.url.as_bytes());Some(peer)
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let state = GatewayState::fixture()
        .with_receipt_user_seed([201; 32])
        .with_proxy_setup_bootstrap(config(dir), &origin)
        .unwrap();
    let token = state.dashboard_session.bootstrap_token.clone();
    let control = state.proxy_setup.clone();
    let task =
        tokio::spawn(async move { axum::serve(listener, openai_router(state)).await.unwrap() });
    Server {
        peer,
        control,
        cookie: std::sync::Mutex::new(None),
        origin,
        token,
        task,
    }
}
async fn session(c: &reqwest::Client, s: &Server) -> (String, Value) {
    let request = c.get(format!(
        "{}/mayhem/dashboard/provider/setup/state",
        s.origin
    ));
    let cookie = s.cookie.lock().unwrap().clone();
    let request = if let Some(cookie) = cookie {
        request.header(header::COOKIE, cookie)
    } else {
        request.header("x-mayhem-dashboard-token", &s.token)
    };
    let r = request.send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
    let cookie = r.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    *s.cookie.lock().unwrap() = Some(cookie.clone());
    (cookie, r.json().await.unwrap())
}
fn post(
    c: &reqwest::Client,
    s: &Server,
    cookie: &str,
    csrf: &str,
    path: &str,
) -> reqwest::RequestBuilder {
    c.post(format!(
        "{}/mayhem/dashboard/provider/setup/{path}",
        s.origin
    ))
    .header(header::COOKIE, cookie)
    .header(header::ORIGIN, &s.origin)
    .header("x-mayhem-setup-csrf", csrf)
}
#[tokio::test]
async fn dashboard_bootstrap_actual_http_private_factory_original_recovery_and_existing_wizard() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let client = reqwest::Client::new();
    let (cookie, state) = session(&client, &s).await;
    let csrf = state["csrf"].as_str().unwrap();
    assert!(state["view"].is_null());
    assert_eq!(state["bootstrap"]["tokenizers"][0]["id"], "approved");
    for hidden in [
        "private-tokenizer",
        "private-bridge",
        "127.0.0.1:9",
        dir.path().to_str().unwrap(),
    ] {
        assert!(!state.to_string().contains(hidden));
    }
    let page = client
        .get(format!("{}/mayhem/dashboard/provider/setup", s.origin))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Connect your model server"));
    let response = post(&client, &s, &cookie, csrf, "bootstrap")
        .json(&input())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let result: Value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["created"], true);
    assert_eq!(result["authorizes_run"], false);
    let dest = dir.path().join("proxy-setup");
    for file in [
        "connection.json",
        "wizard.json",
        "probe.json",
        "runtime-policy.json",
        "upstream-key",
        "tokenizer.json",
    ] {
        assert_eq!(
            std::fs::metadata(dest.join(file))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::read(dest.join("upstream-key")).unwrap(),
        b"synthetic-dashboard-only-key"
    );
    assert!(!dest.join("runtime/capacity.redb").exists());
    assert!(!dest.join("state/draft.json").exists());
    let original = std::fs::read(dest.join("wizard.json")).unwrap();
    let (_, view) = session(&client, &s).await;
    assert!(view["view"].is_object());
    assert!(!view.to_string().contains("synthetic-dashboard-only-key"));
    let cfg = FlowConfig::load(&dest.join("wizard.json")).unwrap();
    let p = cfg.profile;
    let select = json!({"action":"select","expected_revision":null,"choice":{"upstream_model":p.upstream_model,"market":p.market,"membership":p.membership,"offers":p.offers}});
    let r = post(&client, &s, &cookie, csrf, "action")
        .json(&select)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK, "{}", r.text().await.unwrap());
    let r = post(&client, &s, &cookie, csrf, "action")
        .json(&json!({"action":"check","expected_revision":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK, "{}", r.text().await.unwrap());
    let mut altered = input();
    altered["upstream_model"] = json!("replacement-forbidden");
    let r = post(&client, &s, &cookie, csrf, "bootstrap")
        .json(&altered)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    // Drop the HTTP response/state and restart. Original prices/config/draft survive.
    drop(s);
    let restarted = start(dir.path()).await;
    let (cookie2, state2) = session(&client, &restarted).await;
    assert!(state2["view"].is_object());
    assert_ne!(state2["csrf"], state["csrf"]);
    assert_eq!(std::fs::read(dest.join("wizard.json")).unwrap(), original);
    let r = post(
        &client,
        &restarted,
        &cookie2,
        state2["csrf"].as_str().unwrap(),
        "bootstrap",
    )
    .json(&altered)
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    let cfg = FlowConfig::load(&dest.join("wizard.json")).unwrap();
    assert_eq!(cfg.profile.offers[0].rates[0].per_unit_au, 123);
    if let Some(path) = std::env::var_os("MAYHEM_DASHBOARD_BOOTSTRAP_EVIDENCE") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"schema_version":1,"fixture":"actual_authenticated_loopback_gateway_and_private_factory","created":result,"restarted_view":state2["view"],"private_files_mode":"0600","no_upstream_calls":true,"no_capacity_db":true,"no_publication_or_run":true,"original_prices_preserved":true})).unwrap()).unwrap();
    }
}
#[tokio::test]
async fn dashboard_bootstrap_prebody_auth_csrf_strict_choices_and_host_authority() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let c = reqwest::Client::new();
    let (cookie, state) = session(&c, &s).await;
    let csrf = state["csrf"].as_str().unwrap();
    let url = format!("{}/mayhem/dashboard/provider/setup/bootstrap", s.origin);
    assert_eq!(
        c.post(&url).body("bad").send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    for (host, origin, token) in [
        ("evil.invalid".to_owned(), s.origin.clone(), csrf.to_owned()),
        (
            s.origin[7..].into(),
            "http://evil.invalid".into(),
            csrf.into(),
        ),
        (s.origin[7..].into(), s.origin.clone(), "wrong".into()),
    ] {
        assert_eq!(
            c.post(&url)
                .header(header::HOST, host)
                .header(header::ORIGIN, origin)
                .header(header::COOKIE, &cookie)
                .header("x-mayhem-setup-csrf", token)
                .header(header::CONTENT_TYPE, "application/json")
                .body("malformed")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for field in [
        "destination",
        "provider_pubkey",
        "network",
        "worker_program",
        "admission_origin",
        "wallet_password_file",
    ] {
        let mut body = input();
        body[field] = json!("forbidden");
        assert_eq!(
            post(&c, &s, &cookie, csrf, "bootstrap")
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for patch in [
        json!({"credential":{"kind":"reference","id":"unknown"}}),
        json!({"tokenizer_id":"unknown"}),
        json!({"settlement_policy":{"schema_version":1,"lane":"native","payable_outcomes":["complete"],"allow_checkpoints":false}}),
    ] {
        let mut body = input();
        for (k, v) in patch.as_object().unwrap() {
            body[k] = v.clone();
        }
        assert!(!post(&c, &s, &cookie, csrf, "bootstrap")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }
    assert!(!dir.path().join("proxy-setup").exists());
    let mut other = config(dir.path());
    other.host.provider_pubkey = Digest::new("01".repeat(32)).unwrap();
    assert!(GatewayState::fixture()
        .with_receipt_user_seed([201; 32])
        .with_proxy_setup_bootstrap(other, &s.origin)
        .is_err());
    // An invalid retained path never becomes an empty first-create state.
    std::fs::create_dir(dir.path().join("proxy-setup")).unwrap();
    assert!(Control::new_bootstrap(config(dir.path()), &s.origin, &[201; 32]).is_err());
}

#[tokio::test]
async fn dashboard_bootstrap_get_recovers_commit_without_ack_and_rejects_host_change() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let c = reqwest::Client::new();
    let (_, initial) = session(&c, &s).await;
    assert!(initial["view"].is_null());
    // The factory committed, but no handler acknowledged or installed its Flow.
    // GET must find the original, including after a canceled HTTP handler.
    let cfg = config(dir.path());
    let chosen = cfg
        .choices(serde_json::from_value(input()).unwrap())
        .unwrap();
    bootstrap::create(&cfg.destination, cfg.host.clone(), chosen).unwrap();
    let (_, recovered) = session(&c, &s).await;
    assert!(recovered["view"].is_object());
    let mut changed = config(dir.path());
    changed.host.network.network_id = "another-network".into();
    assert!(Control::new_bootstrap(changed, &s.origin, &[201; 32]).is_err());
}

#[tokio::test]
#[ignore = "explicit disposable local browser coordinator only"]
async fn dashboard_bootstrap_browser_fixture() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let ready = std::path::PathBuf::from(
        std::env::var_os("MAYHEM_DASHBOARD_BOOTSTRAP_READY").expect("explicit ready path"),
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&ready)
        .unwrap();
    file.write_all(json!({"url":format!("{}/mayhem/dashboard/provider/setup?token={}",s.origin,s.token),"origin":s.origin,"upstream_url":format!("{}upstream/",s.peer.as_ref().unwrap().url)}).to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    tokio::time::timeout(Duration::from_secs(180), async {
        while !ready.with_extension("done").exists() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("browser coordinator deadline");
    let cfg = FlowConfig::load(&dir.path().join("proxy-setup/wizard.json")).unwrap();
    assert_eq!(cfg.profile.upstream_model, "browser-fixture-model");
    assert_eq!(cfg.profile.offers[0].rates[0].per_unit_au, 123);
    assert!(!dir
        .path()
        .join("proxy-setup/runtime/capacity.redb")
        .exists());
}

#[tokio::test]
async fn dashboard_bootstrap_cancellation_retains_gate_until_blocking_factory_finishes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = Arc::new(config(dir.path()));
    let choices = config
        .choices(serde_json::from_value(input()).unwrap())
        .unwrap();
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = gate.clone().try_acquire_owned().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let worker_config = config.clone();
    // The awaiter models the HTTP handler. Only the production spawn boundary
    // owns the permit; the work itself does not retain or reacquire it.
    let handler = tokio::spawn(async move {
        spawn_creation(permit, move || {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let result = bootstrap::create(
                &worker_config.destination,
                worker_config.host.clone(),
                choices,
            );
            let _ = finished_tx.send(result.is_ok());
            result
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    handler.abort();
    assert!(matches!(handler.await, Err(error) if error.is_cancelled()));
    // Disconnected callers cannot accumulate overlapping detached disk tasks.
    for _ in 0..8 {
        assert!(gate.clone().try_acquire_owned().is_err());
    }
    assert!(!config.destination.exists());
    release_tx.send(()).unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), finished_rx)
        .await
        .unwrap()
        .unwrap());
    let recovered_permit =
        tokio::time::timeout(Duration::from_secs(5), gate.clone().acquire_owned())
            .await
            .unwrap()
            .unwrap();
    let flow = config
        .restore()
        .unwrap()
        .expect("original committed without HTTP ACK");
    assert_eq!(flow.provider(), &config.host.provider_pubkey);
    let stored = FlowConfig::load(&config.destination.join("wizard.json")).unwrap();
    assert_eq!(stored.profile.offers[0].rates[0].per_unit_au, 123);
    drop(recovered_permit);
    assert_eq!(gate.available_permits(), 1);
}

#[tokio::test]
async fn dashboard_bootstrap_restore_http_shares_gate_with_detached_disk_work() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let server = start(dir.path()).await;
    let client = reqwest::Client::new();
    let csrf = server.control.as_ref().as_ref().unwrap().csrf.clone();
    let gate = server
        .control
        .as_ref()
        .as_ref()
        .unwrap()
        .create_gate
        .clone();
    let permit = gate.clone().try_acquire_owned().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let cfg = config(dir.path());
    let choices = cfg
        .choices(serde_json::from_value(input()).unwrap())
        .unwrap();
    let handler = tokio::spawn(async move {
        spawn_creation(permit, move || {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            bootstrap::create(&cfg.destination, cfg.host, choices).unwrap();
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    handler.abort();
    assert!(matches!(handler.await, Err(error) if error.is_cancelled()));
    let first = client
        .get(format!(
            "{}/mayhem/dashboard/provider/setup/state",
            server.origin
        ))
        .header("x-mayhem-dashboard-token", &server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
    // Consuming the one-time bootstrap token on a busy read must not lose ACK
    // of the resulting authenticated browser session.
    let cookie = first.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    *server.cookie.lock().unwrap() = Some(cookie.clone());
    assert_eq!(first.json::<Value>().await.unwrap()["error"], "setup_busy");
    // Both authenticated GET routes must fail before opening retained files or
    // scheduling another restore. Busy is distinct from an invalid original.
    for _ in 0..4 {
        for suffix in ["", "/state"] {
            let response = client
                .get(format!(
                    "{}/mayhem/dashboard/provider/setup{suffix}",
                    server.origin
                ))
                .header(header::COOKIE, &cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            if suffix.is_empty() {
                let html = response.text().await.unwrap();
                assert!(html.contains("data-state=\"setup_busy\""));
                assert!(html.contains("Retry setup status"));
            } else {
                assert_eq!(
                    response.json::<Value>().await.unwrap()["error"],
                    "setup_busy"
                );
            }
            assert_eq!(gate.available_permits(), 0);
        }
    }
    let response = post(&client, &server, &cookie, &csrf, "bootstrap")
        .json(&input())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"],
        "setup_creation_in_progress"
    );
    release_tx.send(()).unwrap();
    let permit = tokio::time::timeout(Duration::from_secs(5), gate.clone().acquire_owned())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    let (_, recovered) = session(&client, &server).await;
    assert!(recovered["view"].is_object());
    assert_eq!(
        FlowConfig::load(&dir.path().join("proxy-setup/wizard.json"))
            .unwrap()
            .profile
            .offers[0]
            .rates[0]
            .per_unit_au,
        123
    );
}
