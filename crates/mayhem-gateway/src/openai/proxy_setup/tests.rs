use super::*;
use mayhem_proxy::setup::FlowConfig;
use std::os::unix::fs::PermissionsExt;

fn fixture() -> (tempfile::TempDir, FlowConfig) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = dir.path().join("draft");
    std::fs::create_dir(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let connection = dir.path().join("connection.json");
    let data = json!({"schema_version":1,"id":"synthetic-local","revision":1,"base_url":"http://127.0.0.1:9/v1/","authentication":{"type":"bearer","secret":{"source":"file","path":"never-read-secret"}},"network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},"paths":{"chat_completions":"chat/completions"}});
    std::fs::write(&connection, data.to_string()).unwrap();
    std::fs::set_permissions(&connection, std::fs::Permissions::from_mode(0o600)).unwrap();
    let provider = hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&[201; 32])
            .verifying_key()
            .to_bytes(),
    );
    let config:FlowConfig=serde_json::from_value(json!({"schema_version":1,"directory":store,"profile":{"schema_version":1,"network":{"network_id":"fixture","msb_bootstrap":"03".repeat(32),"subnet_bootstrap":"04".repeat(32),"contract_version":mayhem_proto::CONTRACT_VERSION},"provider_pubkey":provider,"connection_file":connection,"profile":{"kind":"standard","endpoint":"openai_chat_completions"},"upstream_model":"private-model","limits":{"request_bytes":65536,"response_bytes":65536,"choices":4,"tools":4,"questions":4,"decision_options":4},"market":{"action":"create_market","slug":"local-wizard","model":{"family_id":"fixture","model_id":"declared","revision":"","quantization":""}},"membership":{"revision":1,"served_context":4096,"max_concurrency":2,"capacity_group":"02".repeat(32),"accepted_rails":["fiat","tap","tnk"]},"offers":[{"revision":1,"ctx_bracket":"ctx4k","outcome_class":"","rates":[{"unit":"input_token","per_unit_au":"10","granularity":1},{"unit":"output_token","per_unit_au":"20","granularity":1}],"per_request_au":"1","min_session_au":"2","accepted_rails":["fiat","tap","tnk"]}],"sequence":1,"settlement_policy":{"schema_version":1,"lane":"proxy","payable_outcomes":["complete"],"allow_checkpoints":false}},"probe_plan":null,"peer_rpc":null,"admission_origin":null,"timeout_ms":2000})).unwrap();
    (dir, config)
}
#[test]
fn activation_requires_exact_loopback_and_existing_wallet_and_debug_never_exports_csrf() {
    let (_dir, cfg) = fixture();
    for origin in [
        "http://0.0.0.0:9000",
        "http://localhost:9000",
        "https://127.0.0.1:9000",
        "http://127.0.0.1:9000/",
        "http://127.0.0.1:9000?q=1",
        "http://user@127.0.0.1:9000",
    ] {
        assert!(Control::new(Flow::open(cfg.clone()).unwrap(), origin, &[201; 32]).is_err());
    }
    assert!(Control::new(
        Flow::open(cfg.clone()).unwrap(),
        "http://127.0.0.1:9000",
        &[202; 32]
    )
    .is_err());
    let control = Control::new(
        Flow::open(cfg).unwrap(),
        "http://127.0.0.1:9000",
        &[201; 32],
    )
    .unwrap();
    assert!(control
        .validate_bind("0.0.0.0:9000".parse().unwrap())
        .is_err());
    assert!(control
        .validate_bind("127.0.0.1:9001".parse().unwrap())
        .is_err());
    assert!(control
        .validate_bind("127.0.0.1:9000".parse().unwrap())
        .is_ok());
    assert!(!format!("{control:?}").contains(&control.csrf));
}
#[tokio::test]
async fn real_http_existing_session_prebody_csrf_cas_restart_and_private_projection() {
    let (dir, cfg) = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let state = GatewayState::fixture()
        .with_receipt_user_seed([201; 32])
        .with_proxy_setup(Flow::open(cfg.clone()).unwrap(), &origin)
        .unwrap();
    let bootstrap = state.dashboard_session.bootstrap_token.clone();
    let old_csrf = state.proxy_setup.as_ref().as_ref().unwrap().csrf.clone();
    let task =
        tokio::spawn(async move { axum::serve(listener, openai_router(state)).await.unwrap() });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("{origin}/mayhem/dashboard/provider/setup");
    assert_eq!(
        client
            .get(format!("{base}/state"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(format!("{base}/action"))
            .body("invalid JSON")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let redirect = client
        .get(format!(
            "{origin}/mayhem/dashboard/provider?token={bootstrap}"
        ))
        .header(
            header::HOST,
            format!("localhost:{}", origin.rsplit(':').next().unwrap()),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        redirect.headers()[header::LOCATION],
        format!("{origin}/mayhem/dashboard/provider?token={bootstrap}")
    );
    let response = client
        .get(format!("{base}/state"))
        .header("x-mayhem-dashboard-token", &bootstrap)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let payload: Value = response.json().await.unwrap();
    let csrf = payload["csrf"].as_str().unwrap();
    let view = &payload["view"];
    let serialized = view.to_string();
    for private in [
        "never-read-secret",
        "connection.json",
        "127.0.0.1:9",
        "authentication",
    ] {
        assert!(!serialized.contains(private));
    }
    assert_eq!(view["review"], Value::Null);
    let page = client
        .get(&base)
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("Admission and fee"));
    let provider = client
        .get(format!("{origin}/mayhem/dashboard/provider"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(provider.contains("Open proxy setup wizard"));
    for (host, request_origin, token) in [
        ("evil.invalid".to_owned(), origin.clone(), csrf),
        (
            origin.trim_start_matches("http://").into(),
            "http://evil.invalid".into(),
            csrf,
        ),
        (
            origin.trim_start_matches("http://").into(),
            origin.clone(),
            "wrong",
        ),
    ] {
        let response = client
            .post(format!("{base}/action"))
            .header(header::HOST, host)
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, request_origin)
            .header("x-mayhem-setup-csrf", token)
            .header(header::CONTENT_TYPE, "application/json")
            .body("invalid JSON")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    // A valid authenticated request that stalls its body cannot occupy a slot indefinitely.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stalled = tokio::net::TcpStream::connect(origin.trim_start_matches("http://"))
        .await
        .unwrap();
    stalled.write_all(format!("POST /mayhem/dashboard/provider/setup/action HTTP/1.1\r\nHost: {}\r\nCookie: {cookie}\r\nOrigin: {origin}\r\nx-mayhem-setup-csrf: {csrf}\r\nContent-Type: application/json\r\nContent-Length: 30\r\n\r\n{{", origin.trim_start_matches("http://")).as_bytes()).await.unwrap();
    let mut response = [0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(7), stalled.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.1 408"));
    drop(stalled);
    let send = |body: Value| {
        client
            .post(format!("{base}/action"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, &origin)
            .header("x-mayhem-setup-csrf", csrf)
            .json(&body)
    };
    assert_eq!(
        send(json!({"action":"connect","path":"/etc/passwd"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let selected =
        send(json!({"action":"select","expected_revision":null,"choice":view["selection"]}))
            .send()
            .await
            .unwrap();
    assert_eq!(selected.status(), StatusCode::OK);
    let selected: Value = selected.json().await.unwrap();
    assert_eq!(selected["view"]["review"]["revision"], 1);
    assert_eq!(
        send(json!({"action":"check","expected_revision":1}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(json!({"action":"check","expected_revision":1}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let review = Flow::open(cfg.clone())
        .unwrap()
        .view()
        .unwrap()
        .review
        .unwrap();
    assert_eq!(review.revision, 2);
    assert_eq!(review.serving_status, "not_started");
    assert_eq!(review.admission_status, "not_checked");
    assert!(!dir.path().join("never-read-secret").exists());
    task.abort();
    let state = GatewayState::fixture()
        .with_receipt_user_seed([201; 32])
        .with_proxy_setup(Flow::open(cfg).unwrap(), &origin)
        .unwrap();
    assert_ne!(state.proxy_setup.as_ref().as_ref().unwrap().csrf, old_csrf);
    assert!(state
        .dashboard_session
        .authorize_browser(cookie.split('=').nth(1).unwrap())
        .is_none());
    // Existing disabled gateways have no setup data or mutation authority.
    assert!(state.proxy_setup.is_some());
    assert!(GatewayState::fixture().proxy_setup.is_none());
}

/// Opt-in short-lived real server for desktop/mobile and actual CLI acceptance.
/// The ready file contains only disposable fixture session material, owner-only.
#[tokio::test]
#[ignore = "requires an explicit local browser/CLI acceptance coordinator"]
async fn local_browser_and_cli_fixture() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let ready = std::path::PathBuf::from(
        std::env::var("MAYHEM_SETUP_BROWSER_READY").expect("explicit ready path"),
    );
    let (dir, cfg) = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let config_path = dir.path().join("wizard.json");
    std::fs::write(&config_path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let state = GatewayState::fixture()
        .with_receipt_user_seed([201; 32])
        .with_proxy_setup(Flow::open(cfg.clone()).unwrap(), &origin)
        .unwrap();
    let url = format!(
        "{origin}/mayhem/dashboard/provider/setup?token={}",
        state.dashboard_session.bootstrap_token
    );
    let task =
        tokio::spawn(async move { axum::serve(listener, openai_router(state)).await.unwrap() });
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&ready)
        .unwrap();
    file.write_all(
        json!({"url":url,"origin":origin,"config":config_path})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    file.sync_all().unwrap();
    let done = ready.with_extension("done");
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        while !done.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("local acceptance coordinator did not complete");
    let view = Flow::open(cfg).unwrap().view().unwrap();
    assert!(view.review.unwrap().revision >= 2);
    task.abort();
}
