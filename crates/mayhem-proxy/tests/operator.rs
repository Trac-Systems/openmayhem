use mayhem_proxy::{
    attempts::Digest,
    discovery::{Identity, Proof},
    operator::{Reader, Status, MAX_AGE_MS},
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn h(n: u8) -> String {
    format!("{n:064x}")
}
fn provider() -> Digest {
    Digest::new(h(1)).unwrap()
}
fn network() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: h(2),
        subnet_bootstrap: h(3),
        contract_version: 30,
    }
}
fn proof(n: u64) -> Proof {
    Proof {
        view_key: h(4),
        fork: 0,
        signed_length: n,
        tree_hash: h(n as u8),
    }
}
#[derive(Clone, Default)]
struct Mode {
    patch: Value,
    status: Option<&'static str>,
    delay: Duration,
}
struct Server {
    url: String,
    mode: Arc<Mutex<Mode>>,
    calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let mode = Arc::new(Mutex::new(Mode::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let state = mode.clone();
        let count = calls.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let state = state.clone();
                let count = count.clone();
                tasks.spawn(async move {
                    let mut bytes = Vec::new(); let mut buffer = [0; 2048];
                    let request = loop {
                        let n = socket.read(&mut buffer).await.unwrap();
                        if n == 0 { return; }
                        bytes.extend_from_slice(&buffer[..n]); assert!(bytes.len() < 8192);
                        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                            let header = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                            assert!(header.starts_with("post /v1/proxy/operator-state "));
                            let length: usize = header.lines().find_map(|l| l.strip_prefix("content-length: ")).unwrap().parse().unwrap();
                            if bytes.len() >= end + 4 + length {
                                break serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length]).unwrap();
                            }
                        }
                    };
                    assert_eq!(request.as_object().unwrap().len(), 2);
                    assert_eq!(request["provider_pubkey"], h(1));
                    let mode = state.lock().unwrap().clone();
                    count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(mode.delay).await;
                    let mut value = json!({"ok":true,"schema_version":1,"lane":"proxy","requester":h(9),
                        "provider_pubkey":request["provider_pubkey"],"request_nonce":request["request_nonce"],
                        "context":{"network_id":"918","msb_bootstrap":h(2),"subnet_bootstrap":h(3),"contract_version":30,"epoch":10},
                        "proof":proof(10),"operator":{"status":"verified","proof_hash":h(8)}});
                    if let Some(patch) = mode.patch.as_object() {
                        for (key, value_patch) in patch { value[key] = value_patch.clone(); }
                    }
                    let body = serde_json::to_vec(&value).unwrap();
                    let header = format!("HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        mode.status.unwrap_or("200 OK"), body.len());
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        Self {
            url,
            mode,
            calls,
            task,
        }
    }
    fn patch(&self, patch: Value) {
        *self.mode.lock().unwrap() = Mode {
            patch,
            ..Default::default()
        };
    }
    fn reader(&self) -> Reader {
        Reader::new(&self.url, network(), Duration::from_secs(2)).unwrap()
    }
}

#[tokio::test]
async fn exact_provider_observation_is_fresh_and_negative_canonical_status_never_permits() {
    let server = Server::start().await;
    let reader = server.reader();
    let observed = reader.read(&provider(), &proof(1), 1).await.unwrap();
    assert_eq!(observed.status(), Status::Verified);
    assert!(observed.permits(&provider()));
    assert!(!observed.permits(&Digest::new(h(2)).unwrap()));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!((now..=now + MAX_AGE_MS).contains(&observed.expires_at_ms()));
    for status in ["not_verified", "not_registered", "revoked", "inactive"] {
        server.patch(json!({"operator":{"status":status,"proof_hash":null}}));
        let negative = reader.read(&provider(), &proof(1), 1).await.unwrap();
        assert_ne!(negative.status(), Status::Verified);
        assert!(!negative.permits(&provider()));
    }
}

#[tokio::test]
async fn foreign_stale_forked_unknown_and_oversized_observations_fail_closed() {
    let server = Server::start().await;
    for patch in [
        json!({"provider_pubkey":h(2)}),
        json!({"request_nonce":h(7)}),
        json!({"schema_version":2}),
        json!({"context":{"network_id":"foreign","msb_bootstrap":h(2),"subnet_bootstrap":h(3),"contract_version":30,"epoch":10}}),
        json!({"context":{"network_id":"918","msb_bootstrap":h(2),"subnet_bootstrap":h(3),"contract_version":30,"epoch":0}}),
        json!({"proof":proof(1)}),
        json!({"proof":{"view_key":h(4),"fork":1,"signed_length":10,"tree_hash":h(10)}}),
        json!({"operator":{"status":"self_reported","proof_hash":h(8)}}),
        json!({"operator":{"status":"verified","proof_hash":null}}),
        json!({"operator":{"status":"revoked","proof_hash":h(8)}}),
        json!({"extra":"invalid"}),
        json!({"extra":"x".repeat(4096)}),
    ] {
        server.patch(patch.clone());
        assert!(
            server
                .reader()
                .read(&provider(), &proof(2), 1)
                .await
                .is_err(),
            "{patch}"
        );
    }
    server.patch(json!({}));
    let reader = server.reader();
    reader.read(&provider(), &proof(1), 1).await.unwrap();
    server.patch(json!({"proof":proof(9)}));
    assert!(
        reader.read(&provider(), &proof(1), 1).await.is_err(),
        "high-water rejects regression even with older catalog"
    );
}

#[tokio::test]
async fn unavailable_old_peer_redirect_and_timed_out_reads_never_become_negative_proof() {
    let server = Server::start().await;
    for status in ["404 Not Found", "503 Service Unavailable", "302 Found"] {
        server.mode.lock().unwrap().status = Some(status);
        assert!(server
            .reader()
            .read(&provider(), &proof(1), 1)
            .await
            .is_err());
    }
    *server.mode.lock().unwrap() = Mode {
        delay: Duration::from_millis(200),
        ..Default::default()
    };
    let reader = Reader::new(&server.url, network(), Duration::from_millis(20)).unwrap();
    assert!(reader.read(&provider(), &proof(1), 1).await.is_err());
}

#[tokio::test]
async fn reads_use_four_permits_and_do_not_queue_unbounded_work() {
    let server = Server::start().await;
    server.mode.lock().unwrap().delay = Duration::from_millis(200);
    let reader = Arc::new(server.reader());
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let reader = reader.clone();
        tasks.push(tokio::spawn(async move {
            reader.read(&provider(), &proof(1), 1).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.calls.load(Ordering::SeqCst) < 4 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(reader.read(&provider(), &proof(1), 1).await.is_err());
    assert_eq!(server.calls.load(Ordering::SeqCst), 4);
    for task in tasks {
        assert!(task.await.unwrap().is_ok());
    }
    server.patch(json!({}));
    assert!(reader
        .read(&provider(), &proof(1), 1)
        .await
        .unwrap()
        .permits(&provider()));
}
