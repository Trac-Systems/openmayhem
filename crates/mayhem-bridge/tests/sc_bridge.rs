use std::env;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mayhem_bridge::{ScBridgeClient, ScBridgeConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sends_and_receives_sidechannel_message_through_running_peers() -> anyhow::Result<()> {
    if env::var("MAYHEM_RUN_INTERCOM_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping live Intercom test; set MAYHEM_RUN_INTERCOM_TESTS=1 to run it");
        return Ok(());
    }

    let pear_runtime = pear_runtime_path()?;
    let contract_dir = contract_test_dir()?;
    let suffix = unique_suffix();
    let sidechannel = "0000intercom".to_owned();
    let subnet_channel = format!("p0-3-subnet-{suffix}");
    let token = format!("p0-3-token-{suffix}");
    let admin_port = free_port()?;
    let joiner_port = free_port()?;

    let mut admin = PeerProcess::spawn(PeerSpec {
        pear_runtime: pear_runtime.clone(),
        contract_dir: contract_dir.clone(),
        store: format!("p03-admin-{suffix}"),
        msb_store: format!("p03-admin-msb-{suffix}"),
        subnet_channel: subnet_channel.clone(),
        subnet_bootstrap: None,
        sidechannel: sidechannel.clone(),
        bridge_port: admin_port,
        bridge_token: token.clone(),
    })?;
    let bootstrap = admin
        .wait_for_value("Peer subnet bootstrap:", Duration::from_secs(60))
        .await?;
    admin
        .wait_for_contains("Sidechannel: ready", Duration::from_secs(60))
        .await?;

    let mut joiner = PeerProcess::spawn(PeerSpec {
        pear_runtime,
        contract_dir,
        store: format!("p03-joiner-{suffix}"),
        msb_store: format!("p03-joiner-msb-{suffix}"),
        subnet_channel,
        subnet_bootstrap: Some(bootstrap),
        sidechannel: sidechannel.clone(),
        bridge_port: joiner_port,
        bridge_token: token.clone(),
    })?;
    joiner
        .wait_for_contains("Sidechannel: ready", Duration::from_secs(60))
        .await?;

    let mut sender = connect_bridge(admin_port, &token).await?;
    let mut receiver = connect_bridge(joiner_port, &token).await?;
    receiver.subscribe([sidechannel.as_str()]).await?;

    wait_for_connections(&mut sender, Duration::from_secs(45)).await?;
    wait_for_connections(&mut receiver, Duration::from_secs(45)).await?;

    let text = format!("hello-from-rust-{suffix}");
    let payload = json!({ "kind": "p0.3", "text": text });
    let mut observed = None;
    for _ in 0..20 {
        sender.send(&sidechannel, payload.clone()).await?;
        match receiver
            .next_sidechannel_message(Duration::from_secs(2))
            .await
        {
            Ok(event) if event_contains_text(&event, &text) => {
                observed = Some(event);
                break;
            }
            Ok(_) | Err(mayhem_bridge::BridgeError::Timeout) => continue,
            Err(err) => return Err(err.into()),
        }
    }

    let event = observed
        .ok_or_else(|| anyhow::anyhow!("receiver did not observe test sidechannel message"))?;
    assert_eq!(event["channel"], sidechannel);
    assert_eq!(event["message"]["text"], text);

    admin.shutdown().await;
    joiner.shutdown().await;
    Ok(())
}

fn event_contains_text(event: &Value, text: &str) -> bool {
    event
        .get("message")
        .and_then(|message| message.get("text"))
        .and_then(Value::as_str)
        == Some(text)
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
    contract_dir: PathBuf,
    store: String,
    msb_store: String,
    subnet_channel: String,
    subnet_bootstrap: Option<String>,
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
            .current_dir(spec.contract_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(bootstrap) = spec.subnet_bootstrap {
            command.arg("--subnet-bootstrap").arg(bootstrap);
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

fn contract_test_dir() -> anyhow::Result<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| anyhow::anyhow!("could not locate repository root"))?;
    let contract_dir = repo_root.join("intercom/trac/contract-test-latest");
    if !contract_dir.exists() {
        return Err(anyhow::anyhow!(
            "missing contract-test-latest at {contract_dir:?}"
        ));
    }
    Ok(contract_dir)
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
