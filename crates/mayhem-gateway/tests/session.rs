use std::env;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mayhem_bridge::{BridgeError, ScBridgeClient, ScBridgeConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_session_streams_completion_frames_without_relay() -> anyhow::Result<()> {
    if env::var("MAYHEM_RUN_INTERCOM_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping live Intercom session test; set MAYHEM_RUN_INTERCOM_TESTS=1 to run");
        return Ok(());
    }

    let pear_runtime = pear_runtime_path()?;
    let intercom_dir = intercom_app_dir()?;
    let suffix = unique_suffix();
    let sidechannel = "0000intercom".to_owned();
    let subnet_channel = format!("p3-2-subnet-{suffix}");
    let token = format!("p3-2-token-{suffix}");
    let dht_port = free_port()?;
    let provider_port = free_port()?;
    let user_port = free_port()?;
    let mut dht = DhtBootstrapProcess::spawn(&intercom_dir, dht_port)?;
    dht.wait_for_contains(
        "Fully started Hyperswarm DHT bootstrap node",
        Duration::from_secs(30),
    )
    .await?;
    let dht_bootstrap = format!("127.0.0.1:{dht_port}");

    let mut provider = PeerProcess::spawn(PeerSpec {
        pear_runtime: pear_runtime.clone(),
        intercom_dir: intercom_dir.clone(),
        store: format!("p32-provider-{suffix}"),
        msb_store: format!("p32-provider-msb-{suffix}"),
        subnet_channel: subnet_channel.clone(),
        subnet_bootstrap: None,
        dht_bootstrap: Some(dht_bootstrap.clone()),
        sidechannel: sidechannel.clone(),
        bridge_port: provider_port,
        bridge_token: token.clone(),
    })?;
    let bootstrap = provider
        .wait_for_value("Peer subnet bootstrap:", Duration::from_secs(60))
        .await?;
    provider
        .wait_for_contains("Sidechannel: ready", Duration::from_secs(60))
        .await?;

    let mut user = PeerProcess::spawn(PeerSpec {
        pear_runtime,
        intercom_dir,
        store: format!("p32-user-{suffix}"),
        msb_store: format!("p32-user-msb-{suffix}"),
        subnet_channel,
        subnet_bootstrap: Some(bootstrap),
        dht_bootstrap: Some(dht_bootstrap),
        sidechannel,
        bridge_port: user_port,
        bridge_token: token.clone(),
    })?;
    user.wait_for_contains("Sidechannel: ready", Duration::from_secs(60))
        .await?;

    let mut provider_bridge = connect_bridge(provider_port, &token).await?;
    let mut user_bridge = connect_bridge(user_port, &token).await?;
    wait_for_connections(&mut provider_bridge, Duration::from_secs(120)).await?;
    wait_for_connections(&mut user_bridge, Duration::from_secs(120)).await?;

    let provider_key = peer_pubkey(provider_bridge.info().await?)?;
    let user_key = peer_pubkey(user_bridge.info().await?)?;
    let session_id = "b2".repeat(32);
    user_bridge.session_subscribe([session_id.as_str()]).await?;
    provider_bridge
        .session_subscribe([session_id.as_str()])
        .await?;

    let opened = user_bridge.session_open(&provider_key, &session_id).await?;
    assert_eq!(opened["direct"], true);
    assert_eq!(opened["relayed"], false);
    assert_eq!(opened["channel"], format!("mx/s/{session_id}"));
    let user_session_stats = user_bridge.session_stats().await?;
    assert_eq!(user_session_stats["rateBytesPerSecond"], 1_000_000);
    assert_eq!(user_session_stats["rateBurstBytes"], 1_000_000);

    let open_frame = json!({
        "t": "s.open",
        "v": 1,
        "session_id": session_id,
        "user": user_key,
        "enclave_id": "11".repeat(32),
        "price_ver": 7,
        "rules_ver": 1,
        "voucher": {
            "max_spend_au": "5000",
            "checkpoint": { "tokens": 8192, "ms": 30000 },
            "user_sig": "22".repeat(64)
        },
        "att_nonce": "33".repeat(32),
        "ts": now_millis(),
        "nonce": "44".repeat(32),
        "sig": "55".repeat(64)
    });
    user_bridge
        .session_send(&provider_key, &session_id, open_frame)
        .await?;
    expect_session_frame(&mut provider_bridge, &session_id, "s.open").await?;

    user_bridge
        .session_send(
            &provider_key,
            &session_id,
            json!({
                "t": "s.att_challenge",
                "v": 1,
                "session_id": session_id,
                "nonce_u": "66".repeat(32)
            }),
        )
        .await?;
    expect_session_frame(&mut provider_bridge, &session_id, "s.att_challenge").await?;

    provider_bridge
        .session_send(
            &user_key,
            &session_id,
            json!({
                "t": "s.att_report",
                "v": 1,
                "session_id": session_id,
                "att_report": { "enclave_id": "11".repeat(32), "report_ts": now_millis() }
            }),
        )
        .await?;
    expect_session_frame(&mut user_bridge, &session_id, "s.att_report").await?;

    provider_bridge
        .session_send(
            &user_key,
            &session_id,
            json!({
                "t": "s.accept",
                "v": 1,
                "session_id": session_id,
                "att_report": { "enclave_id": "11".repeat(32), "report_ts": now_millis() },
                "engine": { "ctx": 32768 },
                "ts": now_millis(),
                "nonce": "77".repeat(32),
                "sig": "88".repeat(64)
            }),
        )
        .await?;
    expect_session_frame(&mut user_bridge, &session_id, "s.accept").await?;

    let request_id = "99".repeat(16);
    user_bridge
        .session_send(
            &provider_key,
            &session_id,
            json!({
                "t": "s.req",
                "rid": request_id,
                "body": {
                    "messages": [{ "role": "user", "content": "Say hello" }],
                    "stream": true,
                    "max_tokens": 8
                }
            }),
        )
        .await?;
    expect_session_frame(&mut provider_bridge, &session_id, "s.req").await?;

    for (idx, chunk) in ["hello", " ", "world"].iter().enumerate() {
        provider_bridge
            .session_send(
                &user_key,
                &session_id,
                json!({
                    "t": "s.delta",
                    "rid": request_id,
                    "i": idx,
                    "d": chunk,
                    "tool": null,
                    "fin": null
                }),
            )
            .await?;
    }
    let artifact_bytes = b"\x89PNG mayhem direct artifact".to_vec();
    let artifact_hash = blake3::hash(&artifact_bytes).to_hex().to_string();
    provider_bridge
        .session_send(
            &user_key,
            &session_id,
            json!({
                "t": "s.delta",
                "rid": request_id,
                "i": 3,
                "d": "",
                "tool": null,
                "fin": null,
                "artifact": {
                    "id": "image-1",
                    "content_type": "image/png",
                    "encoding": "hex",
                    "offset": 0,
                    "len": artifact_bytes.len(),
                    "total_len": artifact_bytes.len(),
                    "blake3": artifact_hash,
                    "data": hex::encode(&artifact_bytes),
                    "final": true
                }
            }),
        )
        .await?;
    provider_bridge
        .session_send(
            &user_key,
            &session_id,
            json!({
                "t": "s.delta",
                "rid": request_id,
                "i": 4,
                "d": "",
                "tool": null,
                "fin": "stop",
                "usage": { "in": 3, "out": 3 },
                "artifacts": [{
                    "id": "image-1",
                    "content_type": "image/png",
                    "bytes": artifact_bytes.len(),
                    "blake3": artifact_hash
                }]
            }),
        )
        .await?;

    let mut completion = String::new();
    let mut artifact_seen = false;
    for _ in 0..5 {
        let frame = expect_session_frame(&mut user_bridge, &session_id, "s.delta").await?;
        completion.push_str(frame["d"].as_str().unwrap_or_default());
        if let Some(artifact) = frame.get("artifact") {
            artifact_seen = true;
            assert_eq!(artifact["id"], "image-1");
            assert_eq!(artifact["content_type"], "image/png");
            assert_eq!(artifact["encoding"], "hex");
            assert_eq!(artifact["blake3"], artifact_hash);
            assert_eq!(
                hex::decode(artifact["data"].as_str().unwrap_or_default())?,
                artifact_bytes
            );
            assert_eq!(artifact["final"], true);
        }
        if frame["fin"].as_str() == Some("stop") {
            assert_eq!(frame["usage"]["out"], 3);
            assert_eq!(frame["artifacts"][0]["id"], "image-1");
            assert_eq!(frame["artifacts"][0]["blake3"], artifact_hash);
        }
    }
    assert_eq!(completion, "hello world");
    assert!(artifact_seen);

    provider_bridge
        .session_send(
            &user_key,
            &session_id,
            json!({
                "t": "s.receipt",
                "v": 1,
                "session_id": session_id,
                "seq": 1,
                "in_tokens": 3,
                "out_tokens": 3,
                "au_owed_cum": "1",
                "enclave_sig": "aa".repeat(64)
            }),
        )
        .await?;
    expect_session_frame(&mut user_bridge, &session_id, "s.receipt").await?;

    user_bridge
        .session_send(
            &provider_key,
            &session_id,
            json!({
                "t": "s.receipt_ack",
                "v": 1,
                "session_id": session_id,
                "seq": 1,
                "user_sig": "bb".repeat(64)
            }),
        )
        .await?;
    expect_session_frame(&mut provider_bridge, &session_id, "s.receipt_ack").await?;

    user_bridge
        .session_send(
            &provider_key,
            &session_id,
            json!({ "t": "s.close", "v": 1, "session_id": session_id, "reason": "done" }),
        )
        .await?;
    expect_session_frame(&mut provider_bridge, &session_id, "s.close").await?;
    user_bridge
        .session_close(&provider_key, &session_id)
        .await?;
    provider_bridge
        .session_close(&user_key, &session_id)
        .await?;

    provider.shutdown().await;
    user.shutdown().await;
    dht.shutdown().await;
    Ok(())
}

async fn expect_session_frame(
    client: &mut ScBridgeClient,
    session_id: &str,
    expected_t: &str,
) -> anyhow::Result<Value> {
    let mut observed = Vec::new();
    for _ in 0..30 {
        match client.next_session_frame(Duration::from_secs(2)).await {
            Ok(event)
                if event["session_id"].as_str() == Some(session_id)
                    && event["frame"]["t"].as_str() == Some(expected_t) =>
            {
                assert_eq!(event["direct"], true);
                assert_eq!(event["relayed"], false);
                assert_eq!(event["channel"], format!("mx/s/{session_id}"));
                return Ok(event["frame"].clone());
            }
            Ok(event) => {
                observed.push(json!({
                    "session_id": event["session_id"].clone(),
                    "t": event["frame"]["t"].clone()
                }));
            }
            Err(BridgeError::Timeout) => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Err(anyhow::anyhow!(
        "did not observe {expected_t} for session {session_id}; observed {observed:?}"
    ))
}

fn peer_pubkey(info: Value) -> anyhow::Result<String> {
    info.get("info")
        .and_then(|info| info.get("peerPubkey"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("SC-Bridge info missing peerPubkey"))
}

async fn connect_bridge(port: u16, token: &str) -> anyhow::Result<ScBridgeClient> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let url = format!("ws://127.0.0.1:{port}");
    let mut last_error = None;
    while Instant::now() < deadline {
        match ScBridgeClient::connect(ScBridgeConfig::new(&url, token)?).await {
            Ok(client) => return Ok(client),
            Err(err) => {
                last_error = Some(err);
                sleep(Duration::from_millis(300)).await;
            }
        }
    }
    Err(anyhow::anyhow!(
        "timed out connecting to SC-Bridge on {url}: {:?}",
        last_error
    ))
}

async fn wait_for_connections(client: &mut ScBridgeClient, wait: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        let stats = client.stats().await?;
        if stats
            .get("connectionCount")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            > 0
        {
            return Ok(());
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(anyhow::anyhow!(
        "timed out waiting for sidechannel peer connection"
    ))
}

struct PeerSpec {
    pear_runtime: PathBuf,
    intercom_dir: PathBuf,
    store: String,
    msb_store: String,
    subnet_channel: String,
    subnet_bootstrap: Option<String>,
    dht_bootstrap: Option<String>,
    sidechannel: String,
    bridge_port: u16,
    bridge_token: String,
}

struct PeerProcess {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl PeerProcess {
    fn spawn(spec: PeerSpec) -> anyhow::Result<Self> {
        let mut command = Command::new(spec.pear_runtime);
        command
            .arg("run")
            .arg(".")
            .arg("--peer-store-name")
            .arg(spec.store)
            .arg("--msb-store-name")
            .arg(spec.msb_store)
            .arg("--subnet-channel")
            .arg(spec.subnet_channel)
            .arg("--sidechannels")
            .arg(spec.sidechannel)
            .arg("--sidechannel-quiet")
            .arg("1")
            .arg("--sc-bridge")
            .arg("1")
            .arg("--sc-bridge-host")
            .arg("127.0.0.1")
            .arg("--sc-bridge-port")
            .arg(spec.bridge_port.to_string())
            .arg("--sc-bridge-token")
            .arg(spec.bridge_token)
            .current_dir(spec.intercom_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(bootstrap) = spec.subnet_bootstrap {
            command.arg("--subnet-bootstrap").arg(bootstrap);
        }
        if let Some(bootstrap) = spec.dht_bootstrap {
            command.arg("--dht-bootstrap").arg(bootstrap);
        }

        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (tx, rx) = mpsc::channel(256);
        spawn_reader(stdout, tx.clone());
        spawn_reader(stderr, tx);
        Ok(Self { child, lines: rx })
    }

    async fn wait_for_contains(&mut self, needle: &str, wait: Duration) -> anyhow::Result<()> {
        self.wait_for_line(wait, |line| line.contains(needle).then_some(()))
            .await
    }

    async fn wait_for_value(&mut self, prefix: &str, wait: Duration) -> anyhow::Result<String> {
        self.wait_for_line(wait, |line| extract_prefixed_value(line, prefix))
            .await
    }

    async fn wait_for_line<T>(
        &mut self,
        wait: Duration,
        mut matcher: impl FnMut(&str) -> Option<T>,
    ) -> anyhow::Result<T> {
        timeout(wait, async {
            loop {
                if let Some(status) = self.child.try_wait()? {
                    return Err(anyhow::anyhow!("peer exited before readiness: {status}"));
                }
                match timeout(Duration::from_millis(250), self.lines.recv()).await {
                    Ok(Some(line)) => {
                        if let Some(value) = matcher(&line) {
                            return Ok(value);
                        }
                    }
                    Ok(None) => return Err(anyhow::anyhow!("peer output stream closed")),
                    Err(_) => {}
                }
            }
        })
        .await?
    }

    async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
        let _ = timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

struct DhtBootstrapProcess {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl DhtBootstrapProcess {
    fn spawn(intercom_dir: &std::path::Path, port: u16) -> anyhow::Result<Self> {
        let mut child = Command::new("node")
            .arg("node_modules/hyperdht/bin.js")
            .arg("--bootstrap")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .current_dir(intercom_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (tx, rx) = mpsc::channel(64);
        spawn_reader(stdout, tx.clone());
        spawn_reader(stderr, tx);
        Ok(Self { child, lines: rx })
    }

    async fn wait_for_contains(&mut self, needle: &str, wait: Duration) -> anyhow::Result<()> {
        timeout(wait, async {
            loop {
                if let Some(status) = self.child.try_wait()? {
                    return Err(anyhow::anyhow!("DHT exited before readiness: {status}"));
                }
                match timeout(Duration::from_millis(250), self.lines.recv()).await {
                    Ok(Some(line)) => {
                        if line.contains(needle) {
                            return Ok(());
                        }
                    }
                    Ok(None) => return Err(anyhow::anyhow!("DHT output stream closed")),
                    Err(_) => {}
                }
            }
        })
        .await?
    }

    async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
        let _ = timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

impl Drop for DhtBootstrapProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Drop for PeerProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn spawn_reader<R>(reader: R, tx: mpsc::Sender<String>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(line).await;
        }
    });
}

fn extract_prefixed_value(line: &str, prefix: &str) -> Option<String> {
    let (_, rest) = line.split_once(prefix)?;
    rest.split_whitespace().next().map(str::to_owned)
}

fn free_port() -> anyhow::Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn unique_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis()
        .to_string()
}

fn now_millis() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis();
    u64::try_from(millis).expect("Unix epoch milliseconds overflowed u64")
}

fn intercom_app_dir() -> anyhow::Result<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| anyhow::anyhow!("could not locate repository root"))?;
    let intercom_dir = repo_root.join("intercom");
    if !intercom_dir.exists() {
        return Err(anyhow::anyhow!(
            "missing Mayhem Intercom app at {intercom_dir:?}"
        ));
    }
    Ok(intercom_dir)
}

fn pear_runtime_path() -> anyhow::Result<PathBuf> {
    if let Ok(path) = env::var("MAYHEM_PEAR_RUNTIME") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var("HOME")?;
    let path = PathBuf::from(home)
        .join("Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime");
    if path.exists() {
        return Ok(path);
    }
    Err(anyhow::anyhow!(
        "set MAYHEM_PEAR_RUNTIME to the pear-runtime binary path"
    ))
}
