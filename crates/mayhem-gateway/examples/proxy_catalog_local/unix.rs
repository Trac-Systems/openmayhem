//! Opt-in persistent LOCAL TEST directory. Never installed as a production binary.
//! Run only after reviewing docs/proxy-local-catalog.md. Unix containment required.
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json,
};
use mayhem_gateway::openai::{
    gateway_token_hash, openai_router,
    proxy_control::{Prepared, ProxyControl},
    GatewayAccessControl, GatewayState, GatewayTokenRecord, GatewayTokenStore,
};
use mayhem_proxy::{
    connector::config::private_file,
    discovery::{DiscoveryClient, Identity},
    presence::CATALOG_AGE_MS,
    supervisor::{self, RefreshPolicy},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::Write,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::watch,
    task::JoinHandle,
};

const KIND: &str = "local_test_proxy_catalog";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    schema_version: u32,
    test_only: bool,
    network: Identity,
    rpc_url: String,
    canonical_length: u64,
    provider_sequence: u64,
    policy_revision: u64,
    admission: String,
    paid_execution: bool,
}
struct Args {
    directory: PathBuf,
    bind: SocketAddr,
    duration: u64,
}
fn args(values: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut values = values.into_iter();
    let mut local = false;
    let mut directory = None;
    let mut bind = "127.0.0.1:0".parse()?;
    let mut duration = 1800;
    while let Some(flag) = values.next() {
        match flag.as_str() {
            "--local-test" if !local => local = true,
            "--directory" if directory.is_none() => {
                directory = Some(PathBuf::from(values.next().context("directory required")?))
            }
            "--bind" => bind = values.next().context("bind required")?.parse()?,
            "--duration-seconds" => {
                duration = values.next().context("duration required")?.parse()?
            }
            _ => bail!("unsupported or duplicate argument"),
        }
    }
    ensure!(
        local && (1..=86400).contains(&duration),
        "explicit bounded local test required"
    );
    ensure!(
        matches!(bind, SocketAddr::V4(address) if address.ip().is_loopback()),
        "literal IPv4 loopback required"
    );
    Ok(Args {
        directory: directory.context("private directory required")?,
        bind,
        duration,
    })
}
fn protected_dir(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        path.is_absolute()
            && meta.is_dir()
            && !meta.file_type().is_symlink()
            && meta.uid() == rustix::process::geteuid().as_raw()
            && meta.mode() & 0o077 == 0
            && path.canonicalize()? == path,
        "canonical owner-only directory required"
    );
    Ok(())
}
fn make_dir(path: &Path) -> Result<()> {
    if !path.try_exists()? {
        fs::create_dir(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    protected_dir(path)
}
fn write_private(path: &Path, value: &Value) -> Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        private_file(path, 65536)?;
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("parent required")?)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|_| anyhow::anyhow!("private state replacement failed"))?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
fn access(token: &str) -> GatewayAccessControl {
    GatewayAccessControl::new(
        true,
        GatewayTokenStore {
            version: 1,
            tokens: vec![GatewayTokenRecord {
                name: "local-test-directory-only".into(),
                token_hash: gateway_token_hash(token),
                token_id: "local-test-directory".into(),
                created_at: 1,
                expires_at: None,
                budget_au: None,
                budget_period: None,
                spent_total_au: 0,
                spent_period_au: 0,
                period_started_at: Some(1),
                max_rate_per_minute: Some(240),
                models: vec![],
                last_used_at: None,
                revoked_at: None,
            }],
        },
        None,
    )
}
#[derive(Clone)]
struct ReadGate {
    control: Arc<ProxyControl>,
    source_alive: Arc<AtomicBool>,
}
fn permitted(method: &Method, path: &str) -> bool {
    if method != Method::GET {
        return false;
    }
    if matches!(path, "/v1/proxy/offers" | "/v1/proxy/offers/batch") {
        return true;
    }
    let Some(tail) = path.strip_prefix("/v1/proxy/offers/") else {
        return false;
    };
    let parts: Vec<_> = tail.split('/').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            part.len() == 64
                && part
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
async fn directory_only(State(gate): State<ReadGate>, request: Request, next: Next) -> Response {
    let mut response = if !permitted(request.method(), request.uri().path()) {
        StatusCode::NOT_FOUND.into_response()
    } else if request.headers().contains_key(header::TRANSFER_ENCODING)
        || request
            .headers()
            .get(header::CONTENT_LENGTH)
            .is_some_and(|v| v != "0")
    {
        StatusCode::BAD_REQUEST.into_response()
    } else if !gate.source_alive.load(Ordering::Acquire)
        || !gate.control.catalog().read().is_ok_and(|read| {
            read.status()
                .discovery_is_fresh(supervisor::unix_ms(), CATALOG_AGE_MS)
        })
    {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":{"code":"proxy_directory_unavailable"}})),
        )
            .into_response()
    } else {
        next.run(request).await
    };
    response
        .headers_mut()
        .insert("x-proxy-catalog-environment", "local-test".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
struct Running {
    _owner: File,
    stop: watch::Sender<bool>,
    child_stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<Result<()>>>,
    manifest_path: PathBuf,
    manifest: Value,
    source_alive: Arc<AtomicBool>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        self.child_stop.send_replace(true);
    }
}
impl Running {
    async fn close(mut self) -> Result<()> {
        self.stop.send_replace(true);
        self.child_stop.send_replace(true);
        let mut failed = false;
        for task in self.tasks.drain(..) {
            failed |= !matches!(task.await, Ok(Ok(())));
        }
        self.manifest["ready"] = json!(false);
        self.manifest["stopped_at_ms"] = json!(supervisor::unix_ms());
        write_private(&self.manifest_path, &self.manifest)?;
        ensure!(
            !failed,
            "local catalog shutdown failed; retain original state"
        );
        Ok(())
    }
}
async fn stopped(mut stop: watch::Receiver<bool>) {
    while !*stop.borrow() && stop.changed().await.is_ok() {}
}
async fn start(args: &Args) -> Result<Running> {
    protected_dir(&args.directory)?;
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(args.directory.join("owner.lock"))?;
    let meta = owner.metadata()?;
    ensure!(
        meta.is_file()
            && meta.nlink() == 1
            && meta.uid() == rustix::process::geteuid().as_raw()
            && meta.mode() & 0o077 == 0,
        "unsafe owner lock"
    );
    rustix::fs::flock(&owner, rustix::fs::FlockOperation::NonBlockingLockExclusive)?;
    let token_path = args.directory.join("gateway-token.json");
    if !token_path.try_exists()? {
        ensure!(
            fs::read_dir(&args.directory)?
                .all(|entry| entry.is_ok_and(|entry| entry.file_name() == "owner.lock")),
            "partial local state must be retained"
        );
        let mut random = [0; 32];
        getrandom::fill(&mut random)
            .map_err(|_| anyhow::anyhow!("token randomness unavailable"))?;
        write_private(
            &token_path,
            &json!({"schema_version":1,"test_only":true,"token":format!("local-catalog-{}",hex::encode(random))}),
        )?;
    }
    let saved: Value = serde_json::from_slice(&private_file(&token_path, 4096)?)?;
    let token = saved["token"]
        .as_str()
        .context("local token missing")?
        .to_owned();
    ensure!(
        saved["schema_version"] == 1
            && saved["test_only"] == true
            && token.len() == 78
            && token.starts_with("local-catalog-")
            && token[14..].bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid local token"
    );
    let backend = args.directory.join("backend");
    make_dir(&backend)?;
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let mut child = tokio::process::Command::new("node")
        .arg(checkout.join("intercom/tests/helpers/proxy-catalog-local.mjs"))
        .arg("--directory")
        .arg(&backend)
        .current_dir(&checkout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let child_pid = child.id().context("local helper PID unavailable")?;
    let mut output = BufReader::new(child.stdout.take().context("helper stdout missing")?);
    let mut line = String::new();
    // Source emits one bounded public line. Never consume or print wallet material.
    let read = tokio::time::timeout(Duration::from_secs(20), async {
        while line.len() <= 8192 {
            let bytes = output.fill_buf().await?;
            ensure!(
                !bytes.is_empty(),
                "local catalog helper stopped before readiness"
            );
            let end = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(bytes.len());
            ensure!(line.len() + end <= 8192, "source metadata exceeds bound");
            line.push_str(std::str::from_utf8(&bytes[..end])?);
            output.consume(end);
            if line.ends_with('\n') {
                return Ok::<_, anyhow::Error>(());
            }
        }
        bail!("source metadata exceeds bound")
    })
    .await;
    if !matches!(read, Ok(Ok(()))) {
        let _ = child.kill().await;
        bail!("local catalog helper startup failed; retain original state");
    }
    let source: Source = serde_json::from_str(&line).context("local source metadata")?;
    ensure!(
        source.schema_version == 1
            && source.test_only
            && !source.paid_execution
            && source.canonical_length > 0
            && source.provider_sequence == 2
            && (4..=8).contains(&source.policy_revision)
            && source.admission == "synthetic_local_test_permit"
            && source.network.network_id == "918",
        "invalid local source"
    );
    let url = url::Url::parse(&source.rpc_url)?;
    ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port().is_some()
            && url.path() == "/v1"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "source must use literal loopback"
    );
    let config_path = args.directory.join("gateway-control.json");
    // Prepared owns the existing protected catalog/presence pair. This directory
    // runner deliberately drops its presence lifecycle and runs ONLY the shared
    // catalog supervisor below. This inert bridge address/token is never opened.
    let inert = args.directory.join("unused-presence-token");
    if !inert.try_exists()? {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&inert)?;
        file.write_all(b"unused-local-test-presence")?;
        file.sync_all()?;
    }
    write_private(
        &config_path,
        &json!({"schema_version":1,"network":source.network,"peer_rpc_url":source.rpc_url,
        "state_dir":"cache","bridge":{"url":"ws://127.0.0.1:1","token_file":"unused-presence-token",
        "operation_timeout_ms":1000,"frame_bytes":65536,"queue_events":8,"queue_bytes":131072},
        "max_markets":1,"max_presence_routes":1,"selected_markets":[]}),
    )?;
    let prepared =
        Prepared::load(&config_path, &source.network).context("local gateway configuration")?;
    let (control, unused_lifecycle) = prepared.open().context("local gateway stores")?;
    drop(unused_lifecycle);
    let (stop, stopping) = watch::channel(false);
    let (child_stop, child_stopping) = watch::channel(false);
    let source_alive = Arc::new(AtomicBool::new(true));
    let child_alive = source_alive.clone();
    // Child::wait closes a stdin retained on Child. Keep this owner channel
    // outside it so merely monitoring exit cannot signal EOF to the helper.
    let mut child_input = child.stdin.take();
    let child_task = tokio::spawn(async move {
        let status = tokio::select! {
            status = child.wait() => status?,
            _ = stopped(child_stopping) => {
                if let Some(mut stdin) = child_input.take() { let _ = stdin.write_all(b"stop\n").await; drop(stdin); }
                match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
                    Ok(status) => status?, Err(_) => { child.kill().await?; child.wait().await? }
                }
            }
        };
        child_alive.store(false, Ordering::Release);
        ensure!(status.success(), "local source exited unsuccessfully");
        Ok(())
    });
    let (updates, mut health) = watch::channel(supervisor::Health::default());
    let client = DiscoveryClient::with_timeout(
        &source.rpc_url,
        source.network.clone(),
        Duration::from_secs(2),
    )?;
    let catalog = control.catalog().clone();
    let refresh_stop = stopping.clone();
    let refresh = tokio::spawn(async move {
        let result = supervisor::run(
            catalog.clone(),
            client,
            RefreshPolicy {
                interval_ms: 2000,
                page_pause_ms: 10,
                retry_initial_ms: 100,
                retry_max_ms: 1000,
                jitter_percent: 0,
            },
            1,
            refresh_stop,
            updates,
        )
        .await;
        catalog.wait_for_refresh_idle().await;
        result.map_err(Into::into)
    });
    let manifest_path = args.directory.join("manifest.json");
    let mut running = Running {
        _owner: owner,
        stop,
        child_stop,
        tasks: vec![child_task, refresh],
        manifest_path,
        manifest: json!({"schema_version":1,"kind":KIND,"test_only":true,"ready":false}),
        source_alive: source_alive.clone(),
    };
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(health.borrow().phase, supervisor::Phase::Ready) {
                return Ok::<_, anyhow::Error>(());
            }
            ensure!(
                running.source_alive.load(Ordering::Acquire),
                "local catalog source stopped"
            );
            health.changed().await.context("catalog refresh stopped")?;
        }
    })
    .await;
    if !matches!(ready, Ok(Ok(()))) {
        running.close().await?;
        bail!("local signed catalog did not hydrate");
    }
    let state = GatewayState::from_models(vec![])
        .with_access_control(access(&token))
        .with_proxy_control(control.clone());
    let router = openai_router(state).layer(middleware::from_fn_with_state(
        ReadGate {
            control,
            source_alive,
        },
        directory_only,
    ));
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    let address = listener.local_addr()?;
    running.tasks.push(tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped(stopping))
            .await?;
        Ok(())
    }));
    let started = supervisor::unix_ms();
    running.manifest = json!({"schema_version":1,"kind":KIND,"test_only":true,"ready":true,
        "gateway_url":format!("http://{address}"),"token":token,"gateway_pid":std::process::id(),"catalog_pid":child_pid,
        "state_path":args.directory,"network":source.network,"started_at_ms":started,"expires_at_ms":started + args.duration * 1000,
        "routes":["GET /v1/proxy/offers","GET /v1/proxy/offers/batch","GET /v1/proxy/offers/{market}/{provider}/{slot}"],
        "paid_execution":false,"admission":"synthetic_local_test_permit"});
    write_private(&running.manifest_path, &running.manifest)?;
    Ok(running)
}
#[tokio::main]
pub(super) async fn main() {
    let result = async {
        let args = args(std::env::args().skip(1))?;
        let running = start(&args).await?;
        eprintln!("LOCAL TEST catalog ready. Private manifest: {}", running.manifest_path.display());
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; },
            _ = term.recv() => {},
            _ = tokio::time::sleep(Duration::from_secs(args.duration)) => {},
            _ = async { while running.source_alive.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(100)).await; } } => {},
        }
        running.close().await
    }.await;
    if let Err(error) = result {
        eprintln!("Local test catalog stopped: configuration, startup or state validation failed. Original state retained.");
        // Static stage names only: never print inner errors, config or credentials.
        for stage in [
            "local source metadata",
            "local gateway configuration",
            "local gateway stores",
        ] {
            if error.chain().any(|cause| cause.to_string() == stage) {
                eprintln!("Rejected stage: {stage}");
                break;
            }
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_local_args_and_exact_read_routes_only() {
        for values in [
            vec!["--directory", "/tmp/test"],
            vec![
                "--local-test",
                "--directory",
                "/tmp/test",
                "--bind",
                "0.0.0.0:1234",
            ],
            vec![
                "--local-test",
                "--directory",
                "/tmp/test",
                "--duration-seconds",
                "86401",
            ],
        ] {
            assert!(args(values.into_iter().map(str::to_owned)).is_err());
        }
        assert!(permitted(&Method::GET, "/v1/proxy/offers"));
        assert!(permitted(&Method::GET, "/v1/proxy/offers/batch"));
        assert!(!permitted(&Method::POST, "/v1/proxy/offers/batch"));
        assert!(permitted(
            &Method::GET,
            &format!("/v1/proxy/offers/{0}/{0}/{0}", "a".repeat(64))
        ));
        for path in [
            "/v1/models",
            "/v1/proxy/estimate",
            "/v1/proxy/buyer-policy",
            "/v1/jobs",
            "/v1/chat/completions",
            "/v1/proxy/offers/x/y/z/contract",
        ] {
            assert!(!permitted(&Method::GET, path));
            assert!(!permitted(&Method::POST, path));
        }
        assert!(!permitted(&Method::POST, "/v1/proxy/offers"));
    }
    #[tokio::test]
    async fn signed_catalog_http_auth_restart_and_source_loss() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let args = Args {
            directory: temp.path().canonicalize().unwrap(),
            bind: "127.0.0.1:0".parse().unwrap(),
            duration: 30,
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let mut original = None;
        for _ in 0..2 {
            let running = start(&args).await.unwrap();
            let manifest = running.manifest.clone();
            assert_eq!(
                fs::metadata(&running.manifest_path).unwrap().mode() & 0o777,
                0o600
            );
            let url = manifest["gateway_url"].as_str().unwrap();
            let token = manifest["token"].as_str().unwrap();
            assert_eq!(
                client
                    .get(format!("{url}/v1/proxy/offers"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            let response = client
                .get(format!("{url}/v1/proxy/offers"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()["x-proxy-catalog-environment"],
                "local-test"
            );
            let page: Value = response.json().await.unwrap();
            assert_eq!(page["entries"].as_array().unwrap().len(), 1);
            let offer = &page["entries"][0];
            assert_eq!(
                offer["market"]["model"]["model_id"],
                "LOCAL TEST ONLY / catalog acceptance"
            );
            let detail = client
                .get(format!(
                    "{url}/v1/proxy/offers/{}",
                    offer["id"].as_str().unwrap()
                ))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(detail.status(), StatusCode::OK);
            let batch: Value = client
                .get(format!(
                    "{url}/v1/proxy/offers/batch?{}",
                    url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("ids", offer["id"].as_str().unwrap())
                        .finish()
                ))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(batch["entries"][0]["offer"]["digest"], offer["digest"]);
            assert_eq!(
                batch["entries"][0]["offer"]["availability"]["status"],
                "heartbeat_missing"
            );
            for path in [
                "/v1/chat/completions",
                "/v1/proxy/estimate",
                "/v1/jobs",
                "/v1/proxy/buyer-policy",
            ] {
                assert_eq!(
                    client
                        .post(format!("{url}{path}"))
                        .bearer_auth(token)
                        .json(&json!({}))
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    StatusCode::NOT_FOUND
                );
            }
            let retained = json!({"network":manifest["network"],"token":token,"id":offer["id"],"digest":offer["digest"]});
            if let Some(original) = &original {
                assert!(
                    original == &retained,
                    "restart must preserve original private identity"
                );
            } else {
                original = Some(retained);
            }
            running.child_stop.send_replace(true);
            tokio::time::timeout(Duration::from_secs(5), async {
                while running.source_alive.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                client
                    .get(format!("{url}/v1/proxy/offers"))
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
            running.close().await.unwrap();
            let stopped: Value = serde_json::from_slice(
                &private_file(&args.directory.join("manifest.json"), 65536).unwrap(),
            )
            .unwrap();
            assert_eq!(stopped["ready"], false);
        }
    }
}
