use std::env;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signer, SigningKey};
use mayhem_bridge::{BridgeError, ScBridgeClient, ScBridgeConfig};
use mayhem_gateway::{heartbeat_signing_payload, HeartbeatReceiver, HEARTBEAT_SCHEMA_VERSION};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_receives_valid_heartbeat_and_logs_bad_signature_drop() -> anyhow::Result<()> {
    if env::var("MAYHEM_RUN_INTERCOM_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping live Intercom heartbeat test; set MAYHEM_RUN_INTERCOM_TESTS=1 to run");
        return Ok(());
    }

    let pear_runtime = pear_runtime_path()?;
    let intercom_dir = intercom_app_dir()?;
    let suffix = unique_suffix();
    let room_id = "a1".repeat(16);
    let room_channel = format!("mx/room/{room_id}");
    let bootstrap_channel = "0000intercom".to_owned();
    let peer_sidechannels = format!("{bootstrap_channel},{room_channel}");
    let subnet_channel = format!("p3-1-subnet-{suffix}");
    let token = format!("p3-1-token-{suffix}");
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
        store: format!("p31-provider-{suffix}"),
        msb_store: format!("p31-provider-msb-{suffix}"),
        subnet_channel: subnet_channel.clone(),
        subnet_bootstrap: None,
        dht_bootstrap: Some(dht_bootstrap.clone()),
        sidechannel: peer_sidechannels.clone(),
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
        store: format!("p31-user-{suffix}"),
        msb_store: format!("p31-user-msb-{suffix}"),
        subnet_channel,
        subnet_bootstrap: Some(bootstrap),
        dht_bootstrap: Some(dht_bootstrap),
        sidechannel: peer_sidechannels,
        bridge_port: user_port,
        bridge_token: token.clone(),
    })?;
    user.wait_for_contains("Sidechannel: ready", Duration::from_secs(60))
        .await?;

    let mut sender = connect_bridge(provider_port, &token).await?;
    let mut receiver_bridge = connect_bridge(user_port, &token).await?;
    receiver_bridge.subscribe([room_channel.as_str()]).await?;

    wait_for_connections(&mut sender, Duration::from_secs(120)).await?;
    wait_for_connections(&mut receiver_bridge, Duration::from_secs(120)).await?;

    let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
    let provider_key = hex::encode(signing_key.verifying_key().to_bytes());
    let mut receiver = HeartbeatReceiver::new();

    let accepted = send_until_observed(
        &mut sender,
        &mut receiver_bridge,
        &mut receiver,
        &room_channel,
        |seq| signed_heartbeat(&signing_key, &provider_key, &room_id, seq, false),
        true,
    )
    .await?;
    assert_eq!(accepted["provider"], provider_key);
    assert_eq!(accepted["room_id"], room_id);

    let drops_before = receiver.drops().len();
    send_until_observed(
        &mut sender,
        &mut receiver_bridge,
        &mut receiver,
        &room_channel,
        |seq| signed_heartbeat(&signing_key, &provider_key, &room_id, seq + 100, true),
        false,
    )
    .await?;
    assert_eq!(receiver.drops().len(), drops_before + 1);
    assert!(receiver.drops()[drops_before].reason.contains("signature"));

    provider.shutdown().await;
    user.shutdown().await;
    dht.shutdown().await;
    Ok(())
}

async fn send_until_observed(
    sender: &mut ScBridgeClient,
    receiver_bridge: &mut ScBridgeClient,
    receiver: &mut HeartbeatReceiver,
    room_channel: &str,
    mut make_heartbeat: impl FnMut(u32) -> Value,
    expect_accept: bool,
) -> anyhow::Result<Value> {
    let mut observed_channels = Vec::new();
    for seq in 0..20 {
        let heartbeat = make_heartbeat(seq);
        sender.send(room_channel, &heartbeat).await?;
        match receiver_bridge
            .next_sidechannel_message(Duration::from_secs(2))
            .await
        {
            Ok(event) if event["channel"].as_str() == Some(room_channel) => {
                if observed_channels.len() < 10 {
                    observed_channels.push(room_channel.to_owned());
                }
                let message = event
                    .get("message")
                    .ok_or_else(|| anyhow::anyhow!("sidechannel event missing message"))?;
                let accepted = receiver.receive(message, now_millis());
                if expect_accept {
                    if let Some(accepted) = accepted {
                        return Ok(serde_json::to_value(accepted)?);
                    }
                } else if accepted.is_none() {
                    return Ok(message.clone());
                }
            }
            Ok(event) => {
                if observed_channels.len() < 10 {
                    observed_channels.push(
                        event["channel"]
                            .as_str()
                            .unwrap_or("<missing-channel>")
                            .to_owned(),
                    );
                }
                continue;
            }
            Err(BridgeError::Timeout) => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Err(anyhow::anyhow!(
        "heartbeat was not {} by receiver; drops: {:?}; observed channels: {:?}",
        if expect_accept { "accepted" } else { "dropped" },
        receiver.drops(),
        observed_channels
    ))
}

fn signed_heartbeat(
    signing_key: &SigningKey,
    provider: &str,
    room_id: &str,
    seq: u32,
    tamper_after_signing: bool,
) -> Value {
    let now = now_millis();
    let mut heartbeat = json!({
        "t": "hb",
        "v": HEARTBEAT_SCHEMA_VERSION,
        "provider": provider,
        "enclave_id": "11".repeat(32),
        "model_id": "model/test@4bit",
        "room_id": room_id,
        "sat": 0.12,
        "slots": { "active": 1, "active_requests": 0, "max": 4 },
        "q": { "free_slots": 1, "engine_backlog": 0, "est_wait_ms": 0 },
        "perf": { "tok_s": 42.0, "ttft_ms": 120 },
        "price_ver": 3,
        "caps": { "tools": true, "json": true, "ctx": 8192, "vision": false },
        "att": { "epoch": 1, "head": "44".repeat(32) },
        "ts": now,
        "nonce": blake3::hash(format!("{room_id}:{provider}:{now}:{seq}").as_bytes()).to_hex().to_string(),
    });
    let payload = heartbeat_signing_payload(&heartbeat).expect("heartbeat signing payload");
    let signature = signing_key.sign(&payload);
    heartbeat["sig"] = json!(hex::encode(signature.to_bytes()));
    if tamper_after_signing {
        heartbeat["sat"] = json!(0.34);
    }
    heartbeat
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
