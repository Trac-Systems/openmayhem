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
    let catalog = Arc::new(Catalog::open(&path, identity).unwrap());
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
    stdin.write_all(b"stop\n").await.unwrap();
    drop(stdin);
    assert!(tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
