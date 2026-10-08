use mayhem_proxy::{
    catalog::{Catalog, RefreshOutcome},
    discovery::*,
    Error,
};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

#[derive(clap::Parser)]
struct ProxyCli {
    #[command(subcommand)]
    command: mayhem_proxy::cli::Command,
}

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: 30,
    }
}

async fn mock(response: Vec<u8>, delay: Duration) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            bytes.extend_from_slice(&buf[..n]);
            assert!(bytes.len() < 16000);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                assert!(headers.starts_with("post /v1/proxy/discovery "));
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    let value: Value =
                        serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                    assert_eq!(value["query"]["kind"], "catalog");
                    break;
                }
            }
        }
        tokio::time::sleep(delay).await;
        let _ = socket.write_all(&response).await;
    });
    (base, task)
}

#[test]
fn serialized_query_matches_rpc_and_roundtrips() {
    let q = Query::catalog();
    let value = serde_json::to_value(&q).unwrap();
    assert_eq!(
        value,
        json!({"kind":"catalog","filter":{},"lookup":null,"limit":100,"cursor":null,"since":null})
    );
    assert_eq!(serde_json::from_value::<Query>(value).unwrap(), q);
    for url in [
        "file:///tmp/catalog",
        "https://user:secret@example.invalid/v1",
        "https://example.invalid/v1?key=secret",
    ] {
        assert!(DiscoveryClient::new(url, identity()).is_err());
    }
}

#[tokio::test]
async fn redirects_are_not_followed_and_backend_errors_are_sanitized() {
    let (url, task) = mock(
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/internal\r\nContent-Length: 0\r\n\r\n"
            .to_vec(),
        Duration::ZERO,
    )
    .await;
    let error = DiscoveryClient::new(&url, identity())
        .unwrap()
        .page(&Query::catalog())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Http { status: 302, .. }));
    task.await.unwrap();
    let body = br#"{"error":"secret upstream detail","code":"arbitrary-secret"}"#;
    let response = format!(
        "HTTP/1.1 503 Unavailable\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    );
    let (url, task) = mock(response.into_bytes(), Duration::ZERO).await;
    let error = DiscoveryClient::new(&url, identity())
        .unwrap()
        .page(&Query::catalog())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "proxy discovery HTTP status 503: proxy_discovery_unavailable"
    );
    task.await.unwrap();
}

#[tokio::test]
async fn declared_and_chunked_response_bounds_and_timeout_are_enforced() {
    let huge = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        MAX_RESPONSE_BYTES + 1
    );
    let chunk = "x".repeat(MAX_RESPONSE_BYTES + 1);
    let chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
        chunk.len(),
        chunk
    );
    for response in [huge, chunked] {
        let (url, task) = mock(response.into_bytes(), Duration::ZERO).await;
        let error = DiscoveryClient::new(&url, identity())
            .unwrap()
            .page(&Query::catalog())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error}");
        task.await.unwrap();
    }
    let (url, task) = mock(Vec::new(), Duration::from_secs(2)).await;
    let error = DiscoveryClient::with_timeout(&url, identity(), Duration::from_millis(100))
        .unwrap()
        .page(&Query::catalog())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Transport(_)));
    task.abort();
}

#[tokio::test]
async fn duplicate_refresh_does_not_queue_or_send_another_http_request() {
    let (url, task) = mock(Vec::new(), Duration::from_secs(2)).await;
    let client =
        DiscoveryClient::with_timeout(&url, identity(), Duration::from_millis(100)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(dir.path().join("catalog"), identity()).unwrap());
    let (left, right) = tokio::join!(
        catalog.refresh_page(&client, 1),
        catalog.refresh_page(&client, 1)
    );
    assert!(matches!(
        (&left, &right),
        (Err(Error::Transport(_)), Err(Error::RefreshBusy))
            | (Err(Error::RefreshBusy), Err(Error::Transport(_)))
    ));
    assert!(catalog.read().unwrap().status().committed.is_none());
    task.abort();
}

#[tokio::test]
async fn supervisor_retries_transient_failure_with_bounded_backoff_and_stops_cleanly() {
    use mayhem_proxy::supervisor::{self, Health, Phase, RefreshPolicy};
    let body = r#"{"code":"proxy_discovery_busy"}"#;
    let response = format!(
        "HTTP/1.1 503 Unavailable\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let (url, server) = mock(response.into_bytes(), Duration::ZERO).await;
    let client = DiscoveryClient::new(&url, identity()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(dir.path().join("catalog"), identity()).unwrap());
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let (updates, mut health) = tokio::sync::watch::channel(Health::default());
    let policy = RefreshPolicy {
        interval_ms: 100,
        page_pause_ms: 1,
        retry_initial_ms: 10,
        retry_max_ms: 40,
        jitter_percent: 0,
    };
    let task = tokio::spawn(supervisor::run(
        catalog.clone(),
        client,
        policy,
        1,
        shutdown,
        updates,
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            health.changed().await.unwrap();
            let current = health.borrow_and_update().clone();
            assert_eq!(current.phase, Phase::Degraded);
            assert!(current.retry_in_ms.unwrap() <= 40);
            if current.consecutive_failures >= 4 {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        catalog.read().unwrap().status().generation,
        0,
        "failed reads do not mutate cache/ledger"
    );
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    server.await.unwrap();
    assert_eq!(health.borrow().phase, Phase::Stopped);
}

#[tokio::test]
async fn supervisor_respects_preexisting_shutdown_without_any_connection() {
    use mayhem_proxy::supervisor::{self, Health, Phase, RefreshPolicy};
    let client = DiscoveryClient::new("http://127.0.0.1:1/v1", identity()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(dir.path().join("catalog"), identity()).unwrap());
    let (_stop, shutdown) = tokio::sync::watch::channel(true);
    let (updates, health) = tokio::sync::watch::channel(Health::default());
    supervisor::run(
        catalog,
        client,
        RefreshPolicy::default(),
        1,
        shutdown,
        updates,
    )
    .await
    .unwrap();
    assert_eq!(health.borrow().phase, Phase::Stopped);
    assert_eq!(health.borrow().consecutive_failures, 0);
}

#[test]
fn control_config_rejects_wrong_contract_secrets_and_zero_polling_and_resolves_local_paths() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("control.json");
    let config = json!({"schema_version":1,"identity":identity(),"peer_rpc_url":"http://127.0.0.1:1/v1","catalog_file":"public.redb"});
    std::fs::write(&file, serde_json::to_vec(&config).unwrap()).unwrap();
    let parsed = mayhem_proxy::cli::ControlConfig::load(&file).unwrap();
    assert_eq!(parsed.catalog_file, dir.path().join("public.redb"));
    for mutate in [0, 1, 2] {
        let mut invalid = config.clone();
        match mutate {
            0 => invalid["identity"]["contract_version"] = json!(1),
            1 => invalid["upstream_api_key"] = json!("test-only-sentinel"),
            _ => invalid["refresh"] = json!({"interval_ms":0}),
        }
        std::fs::write(&file, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(mayhem_proxy::cli::ControlConfig::load(&file).is_err());
        assert!(
            !dir.path().join("public.redb").exists(),
            "configuration errors precede opening storage"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn watch_child() {
    let Some(config) = std::env::var_os("MAYHEM_PROXY_WATCH_CHILD_TEST") else {
        return;
    };
    mayhem_proxy::cli::run(mayhem_proxy::cli::Command::Catalog {
        command: mayhem_proxy::cli::CatalogCommand::Watch(mayhem_proxy::cli::ConfigArgs {
            config: config.into(),
        }),
    })
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn supervised_watch_handles_sigterm_and_releases_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("control.json");
    let cache = dir.path().join("public.redb");
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({"schema_version":1,"identity":identity(),
        "peer_rpc_url":"http://127.0.0.1:1/v1","catalog_file":"public.redb"}))
        .unwrap(),
    )
    .unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "watch_child", "--nocapture"])
        .env("MAYHEM_PROXY_WATCH_CHILD_TEST", &file)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("watch child ended before reporting state");
            if line.contains("\"phase\":\"degraded\"") {
                break;
            }
        }
    })
    .await
    .unwrap();
    let pid = child.id().unwrap().to_string();
    assert!(tokio::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .await
        .unwrap()
        .success());
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    let catalog = Catalog::open(&cache, identity()).unwrap();
    assert_eq!(catalog.read().unwrap().status().generation, 0);
}

#[tokio::test]
async fn actual_signed_js_rpc_hydrates_restarts_applies_changes_and_recovers_expired_cursor() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = tokio::process::Command::new("node")
        .arg(root.join("intercom/tests/helpers/proxy-discovery-rpc-fixture.mjs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stdin = child.stdin.take().unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ready: Value = serde_json::from_str(&ready).unwrap();
    let identity: Identity = serde_json::from_value(ready["identity"].clone()).unwrap();
    let client = DiscoveryClient::new(ready["url"].as_str().unwrap(), identity.clone()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let catalog = Arc::new(Catalog::open(&path, identity.clone()).unwrap());
    assert!(matches!(
        catalog.refresh_page(&client, 10000).await.unwrap(),
        RefreshOutcome::Staged(_)
    ));
    assert!(catalog
        .read()
        .unwrap()
        .page(CATALOG_PREFIX, None, 100)
        .unwrap()
        .entries
        .is_empty());
    drop(catalog);
    let catalog = Arc::new(Catalog::open(&path, identity.clone()).unwrap());
    stdin.write_all(b"change\n").await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&lines.next_line().await.unwrap().unwrap()).unwrap()["done"],
        "change"
    );
    assert!(matches!(
        catalog.refresh_page(&client, 10000).await.unwrap(),
        RefreshOutcome::Committed(_)
    ));
    let key0 = format!("{CATALOG_PREFIX}families/f000");
    let key1 = format!("{CATALOG_PREFIX}families/f001");
    assert!(
        catalog.read().unwrap().get(&key0).unwrap().is_some(),
        "initial snapshot remained pinned during mutation"
    );
    let market_key = format!(
        "{CATALOG_PREFIX}markets/{}",
        ready["market_id"].as_str().unwrap()
    );
    assert!(catalog.read().unwrap().get(&market_key).unwrap().is_some());
    assert!(matches!(
        catalog.refresh_page(&client, 20000).await.unwrap(),
        RefreshOutcome::Committed(_)
    ));
    assert!(catalog.read().unwrap().get(&key0).unwrap().is_none());
    assert_eq!(
        catalog.read().unwrap().get(&key1).unwrap().unwrap()["enabled"],
        false
    );
    let offer = catalog
        .read()
        .unwrap()
        .page(&format!("{CATALOG_PREFIX}offers/"), None, 1)
        .unwrap();
    assert_eq!(offer.entries[0].value["active"], false);
    assert_eq!(offer.entries[0].value["revision"], 2);
    stdin.write_all(b"expire\n").await.unwrap();
    lines.next_line().await.unwrap().unwrap();
    assert!(matches!(
        catalog.refresh_page(&client, 30000).await.unwrap(),
        RefreshOutcome::Invalidated
    ));
    assert!(catalog.read().unwrap().status().invalidated);
    assert!(matches!(
        catalog.refresh_page(&client, 30000).await.unwrap(),
        RefreshOutcome::Staged(_)
    ));
    assert!(matches!(
        catalog.refresh_page(&client, 30000).await.unwrap(),
        RefreshOutcome::Committed(_)
    ));
    // Exercise the same command parser/implementation used by the Core CLI,
    // against the real signed RPC and a persisted cache, without native engines.
    use clap::Parser;
    let market = catalog.read().unwrap().get(&market_key).unwrap().unwrap();
    drop(catalog);
    let config_path = dir.path().join("proxy-control.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&json!({"schema_version":1,"identity":identity,
        "peer_rpc_url":ready["url"],"catalog_file":"catalog"}))
        .unwrap(),
    )
    .unwrap();
    for name in ["sync", "status", "markets"] {
        let cli = ProxyCli::try_parse_from([
            "mayhem-proxy",
            "catalog",
            name,
            "--config",
            config_path.to_str().unwrap(),
        ])
        .unwrap();
        mayhem_proxy::cli::run(cli.command).await.unwrap();
    }
    let report_path = dir.path().join("probe.json");
    std::fs::write(&report_path, serde_json::to_vec(&json!({"schema_version":1,"report_digest":"a".repeat(64),
        "family":market["family"],"model":market["model"],"endpoints":market["endpoints"],"metering":market["metering"],"aliases":[]})).unwrap()).unwrap();
    let cli = ProxyCli::try_parse_from([
        "mayhem-proxy",
        "catalog",
        "suggest",
        "--config",
        config_path.to_str().unwrap(),
        "--report",
        report_path.to_str().unwrap(),
    ])
    .unwrap();
    mayhem_proxy::cli::run(cli.command).await.unwrap();
    use mayhem_proxy::supervisor::{self, Health, Phase, RefreshPolicy};
    let catalog = Arc::new(Catalog::open(&path, identity.clone()).unwrap());
    let client = DiscoveryClient::new(ready["url"].as_str().unwrap(), identity).unwrap();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let (updates, mut health) = tokio::sync::watch::channel(Health::default());
    let runner = tokio::spawn(supervisor::run(
        catalog.clone(),
        client,
        RefreshPolicy {
            interval_ms: 20,
            page_pause_ms: 1,
            retry_initial_ms: 10,
            retry_max_ms: 40,
            jitter_percent: 0,
        },
        1,
        shutdown,
        updates,
    ));
    for (command, expected) in [
        (None, Phase::Ready),
        (Some("fail"), Phase::Degraded),
        (Some("recover"), Phase::Ready),
    ] {
        if let Some(command) = command {
            stdin
                .write_all(format!("{command}\n").as_bytes())
                .await
                .unwrap();
            lines.next_line().await.unwrap().unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                health.changed().await.unwrap();
                if health.borrow_and_update().phase == expected {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(
        health.borrow().consecutive_failures,
        0,
        "successful refresh resets failure backoff"
    );
    assert!(catalog.read().unwrap().get(&market_key).unwrap().is_some());
    stop.send(true).unwrap();
    runner.await.unwrap().unwrap();
    drop(catalog);
    stdin.write_all(b"stop\n").await.unwrap();
    drop(stdin);
    assert!(tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
