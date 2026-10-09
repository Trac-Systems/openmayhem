//! Explicit, bounded local HTTP integration lab. Only an ignored test can start
//! it; normal binaries never expose fixture keys, balances, relay or model output.
use super::*;
use serde::Deserialize;
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiReady {
    schema_version: u32,
    test_only: bool,
    ready: bool,
    callback_url: String,
    callback_credential: String,
}

fn write_private(path: &Path, value: &Value) {
    // NamedTempFile is mode 0600 on Unix. Publish atomically so an API polling
    // for readiness never observes an empty or partially written manifest.
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
    file.write_all(&serde_json::to_vec_pretty(value).unwrap())
        .unwrap();
    file.as_file().sync_all().unwrap();
    file.persist_noclobber(path).unwrap();
}

#[tokio::test]
#[ignore = "manual loopback integration; requires an already-running isolated API callback"]
async fn serve_proxy_loopback_lab() {
    let ready_path = std::env::var_os("MAYHEM_TEST_PROXY_LAB_API_READY")
        .map(PathBuf::from)
        .expect("explicit private API-ready manifest required");
    let ready: ApiReady = serde_json::from_slice(
        &mayhem_proxy::connector::config::private_file(&ready_path, 16 * 1024).unwrap(),
    )
    .unwrap();
    assert!(ready.schema_version == 1 && ready.test_only && ready.ready);
    let callback = url::Url::parse(&ready.callback_url).unwrap();
    assert_eq!(
        callback.scheme(),
        "http",
        "lab callback must use literal loopback HTTP"
    );
    assert!(callback.host_str().is_some_and(|host| host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())));
    assert_eq!(callback.path(), "/internal/proxy-retail/authorize");
    let config = RetailAuthorizationConfig {
        url: ready.callback_url,
        credential: ready.callback_credential,
        owner_token_ids: vec!["owner".into()],
        timeout_ms: 5000,
    };
    config.validate().unwrap();
    let manifest_path = std::env::var_os("MAYHEM_TEST_PROXY_LAB_MANIFEST")
        .map(PathBuf::from)
        .expect("explicit private output manifest required");
    assert!(manifest_path.is_absolute());
    let parent = std::fs::symlink_metadata(manifest_path.parent().unwrap()).unwrap();
    assert!(
        parent.is_dir()
            && !parent.file_type().is_symlink()
            && parent.permissions().mode() & 0o077 == 0,
        "lab output directory must already exist with private permissions"
    );
    let stop_file = manifest_path.with_extension("stop");
    let result_file = manifest_path.with_extension("result.json");
    assert!(
        !manifest_path.exists() && !stop_file.exists() && !result_file.exists(),
        "use new lab paths"
    );
    let duration: u64 = std::env::var("MAYHEM_TEST_PROXY_LAB_DURATION_SECONDS")
        .unwrap_or_else(|_| "90".into())
        .parse()
        .unwrap();
    assert!(
        (1..=90).contains(&duration),
        "lab serving duration is bounded to 90 seconds"
    );

    // All stores, wallet keys and balances originate in this disposable fixture.
    // This uses real controllers/discovery/settlement with explicit test doubles
    // for transport attribution and model text, never production peer services.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_url = format!("http://{}", listener.local_addr().unwrap());
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).unwrap();
    let gateway_token = format!("proxy-lab-{}", hex::encode(random));
    let mut f = Fixture::start_with_retail_key(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        Some(config),
        &gateway_token,
    )
    .await;
    f.tokens.tokens.retain(|token| token.token_id == "owner");
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    let (status, policy) = f.get("/v1/proxy/buyer-policy", &gateway_token).await;
    assert_eq!(status, StatusCode::OK);
    let (status, offers) = f.get("/v1/proxy/offers?rail=fiat", &gateway_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(offers["entries"].as_array().unwrap().len(), 1);
    let (shutdown, stopped) = oneshot::channel::<()>();
    let router = f.router.clone();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let started = crate::openai::now_millis_u64();
    write_private(
        &manifest_path,
        &json!({
            "schema_version":1,"test_only":true,"ready":true,
            "gateway_url":gateway_url,"gateway_token":gateway_token,"rail":"fiat",
            "model":model(&f.harness),"body":f.body(),"buyer_policy":policy,"offers":offers,
            "initial_purchase_deadline_ms":started + 45_000,
            "expires_at_ms":started + duration * 1000,"stop_file":stop_file,"result_file":result_file,
            "test_doubles":["local model output", "SC-Bridge peer transport attribution", "synthetic ledger funding"],
            "network":f.harness.network,
        }),
    );
    eprintln!(
        "Isolated proxy lab ready; private manifest: {}",
        manifest_path.display()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(duration);
    while tokio::time::Instant::now() < deadline && !stop_file.exists() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Stop admission/listening, then join the real purchase owner before dropping
    // the provider or its signed ledger. Disconnection is never a refund proof.
    let _ = shutdown.send(());
    f.stopped.send_replace(true);
    f.task.await.unwrap().unwrap();
    assert!(!f.runtime.running.load(Ordering::Acquire));
    // The owner has joined all durable completion/budget work before counters
    // are exported; a final settlement cannot race this shutdown summary.
    let financial = f.harness.status().await;
    let summary = json!({"schema_version":1,"test_only":true,
        "backend_calls":f.harness.backend_calls(),"canonical_publications":financial["publications"],
        "financial":financial,
        "spent_total_au":f.state.access_control.summary()["tokens"][0]["spent_total_au"],
        "pending_budgets":f.state.access_control.pending_key_budgets(None,64).unwrap().len(),
        "pending_jobs":f.state.jobs.lock().unwrap().pending_proxy(None,64).unwrap().len(),
    });
    f.control.stop().await;
    f.harness.stop().await;
    tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .unwrap()
        .unwrap();
    write_private(&result_file, &summary);
}
