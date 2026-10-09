use super::*;
use crate::openai::GatewayState;
use axum::{routing::post, Json, Router};
use futures_util::{SinkExt, StreamExt};
use mayhem_proxy::discovery::{Context, Entry, Mode, Page, Proof, QueryBinding, CATALOG_PREFIX};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, sync::atomic::AtomicUsize};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_tungstenite::{accept_async, tungstenite::Message};

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn config(dir: &Path, rpc: String, bridge: String) -> PathBuf {
    write(&dir.join("bridge-token"), b"fixture-gateway-token");
    let config = Config {
        schema_version: 1,
        network: identity(),
        peer_rpc_url: rpc,
        state_dir: "proxy-state".into(),
        bridge: Bridge {
            url: bridge,
            token_file: "bridge-token".into(),
            operation_timeout_ms: 500,
            frame_bytes: 64 * 1024,
            queue_events: 8,
            queue_bytes: 128 * 1024,
        },
        max_markets: 2,
        max_presence_routes: 8,
        selected_markets: vec![],
        refresh: refresh_policy(),
        rpc_timeout_ms: 500,
    };
    let path = dir.join("gateway.json");
    write(&path, &serde_json::to_vec(&config).unwrap());
    path
}

fn page(network: Identity) -> Page {
    let mut row = serde_json::to_value(&network).unwrap();
    row["enabled"] = json!(true);
    Page {
        ok: true,
        lane: "proxy".into(),
        schema_version: 1,
        request_nonce: "c".repeat(64),
        query: QueryBinding::catalog(),
        context: Context {
            network_id: network.network_id,
            msb_bootstrap: network.msb_bootstrap,
            subnet_bootstrap: network.subnet_bootstrap,
            contract_version: network.contract_version,
            epoch: 1,
        },
        proof: Proof {
            view_key: "d".repeat(64),
            fork: 0,
            signed_length: 1,
            tree_hash: "e".repeat(64),
        },
        base_proof: None,
        mode: Mode::Snapshot,
        entries: vec![Entry {
            key: format!("{CATALOG_PREFIX}network/current"),
            value: row,
        }],
        truncated: false,
        next_cursor: None,
        checkpoint: Some(format!("pdc1.fixture.{}", "f".repeat(128))),
    }
}

struct Server(JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn rpc(network: Identity) -> (String, Server) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/proxy/discovery",
        post(move || {
            let network = network.clone();
            async move { Json(page(network)) }
        }),
    );
    (
        url,
        Server(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        })),
    )
}

async fn bridge() -> (String, Arc<AtomicUsize>, Server) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let subscriptions = Arc::new(AtomicUsize::new(0));
    let counts = subscriptions.clone();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
            let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            let reply = match value["type"].as_str().unwrap() {
                "auth" => {
                    assert_eq!(value["token"], "fixture-gateway-token");
                    "auth_ok"
                }
                "subscribe" => {
                    let channels = value["channels"].as_array().unwrap();
                    assert!(channels.iter().all(|v| v != "*"));
                    counts.fetch_add(channels.len(), Ordering::SeqCst);
                    "subscribed"
                }
                "join" => "joined",
                "clear_filter" => "filter_set",
                kind => panic!("unexpected bridge operation {kind}"),
            };
            if socket
                .send(Message::Text(
                    json!({"type":reply,"id":value["id"]}).to_string().into(),
                ))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    (url, subscriptions, Server(task))
}

async fn wait_health(control: &ProxyControl, predicate: impl Fn(&Health) -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate(&control.health().unwrap()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn default_gateway_has_no_proxy_control_and_keeps_native_catalog() {
    let state = GatewayState::fixture();
    assert!(state.proxy_control().is_none());
    assert!(!state.models_snapshot().is_empty());
}

#[test]
fn protected_config_rejects_identity_permissions_remote_sources_and_stale_cadence() {
    let dir = tempfile::tempdir().unwrap();
    let path = config(
        dir.path(),
        "http://127.0.0.1:1".into(),
        "ws://127.0.0.1:2".into(),
    );
    let bytes = std::fs::read(&path).unwrap();
    let mut other = identity();
    other.network_id = "other".into();
    assert!(matches!(
        Prepared::load(&path, &other),
        Err(Error::Identity)
    ));
    assert!(!dir.path().join("proxy-state").exists());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Prepared::load(&path, &identity()),
        Err(Error::Protection)
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    for change in ["rpc", "bridge", "cadence"] {
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        match change {
            "rpc" => value["peer_rpc_url"] = json!("http://example.invalid"),
            "bridge" => value["bridge"]["url"] = json!("ws://localhost:2"),
            _ => value["refresh"]["interval_ms"] = json!(15_000),
        }
        write(&path, &serde_json::to_vec(&value).unwrap());
        assert!(matches!(
            Prepared::load(&path, &identity()),
            Err(Error::Configuration)
        ));
    }
    assert!(!dir.path().join("proxy-state").exists());
    write(&path, &bytes);
    std::fs::remove_file(dir.path().join("bridge-token")).unwrap();
    write(&dir.path().join("other-token"), b"fixture-gateway-token");
    std::os::unix::fs::symlink("other-token", dir.path().join("bridge-token")).unwrap();
    assert!(matches!(
        Prepared::load(&path, &identity()),
        Err(Error::Protection)
    ));
}

#[test]
fn existing_empty_or_redirected_proxy_store_is_never_reset() {
    for mode in ["empty", "symlink", "hardlink"] {
        let dir = tempfile::tempdir().unwrap();
        let path = config(
            dir.path(),
            "http://127.0.0.1:1".into(),
            "ws://127.0.0.1:2".into(),
        );
        let state = dir.path().join("proxy-state");
        private_dir(&state).unwrap();
        let catalog = state.join("proxy-catalog.redb");
        match mode {
            "symlink" => {
                write(&dir.path().join("other"), b"do not modify");
                std::os::unix::fs::symlink("../other", &catalog).unwrap();
            }
            "hardlink" => {
                write(&dir.path().join("other"), b"do not modify");
                std::fs::hard_link(dir.path().join("other"), &catalog).unwrap();
            }
            _ => write(&catalog, b""),
        }
        write(&state.join("proxy-presence.redb"), b"preserve presence");
        assert!(matches!(
            Prepared::load(&path, &identity()).unwrap().open(),
            Err(Error::Protection)
        ));
        assert_eq!(
            std::fs::read(state.join("proxy-presence.redb")).unwrap(),
            b"preserve presence"
        );
        if mode != "empty" {
            assert_eq!(
                std::fs::read(dir.path().join("other")).unwrap(),
                b"do not modify"
            );
        }
    }
}

#[tokio::test]
async fn explicit_lifecycle_hydrates_catalog_selects_presence_and_joins_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (rpc_url, _rpc) = rpc(identity()).await;
    let (bridge_url, subscriptions, _bridge) = bridge().await;
    let path = config(dir.path(), rpc_url, bridge_url);
    let (control, lifecycle) = Prepared::load(&path, &identity()).unwrap().open().unwrap();
    let state = GatewayState::fixture().with_proxy_control(control.clone());
    let native = state.models_snapshot();
    assert!(!control.health().unwrap().running);
    assert!(!control.health().unwrap().catalog_fresh);
    let (stop, stopping) = watch::channel(false);
    let task = tokio::spawn(lifecycle.run(stopping));
    wait_health(&control, |h| {
        h.running && h.catalog_fresh && h.presence.running
    })
    .await;
    assert!(!control.health().unwrap().presence.connected);
    assert_eq!(subscriptions.load(Ordering::SeqCst), 0);
    control
        .select_markets(vec![Digest::new("1".repeat(64)).unwrap()])
        .unwrap();
    wait_health(&control, |h| h.presence.connected).await;
    assert_eq!(subscriptions.load(Ordering::SeqCst), 1);
    assert!(Arc::ptr_eq(&native, &state.models_snapshot()));
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let health = control.health().unwrap();
    assert!(!health.running && !health.catalog_fresh && !health.presence.running);
    assert_eq!(health.catalog.phase, supervisor::Phase::Stopped);
    assert!(health.failure.is_none());
    drop(state);
    drop(control);
    // Successful reopen demonstrates the joined lifecycle released store owners.
    let (_, reopened) = Prepared::load(&path, &identity()).unwrap().open().unwrap();
    drop(reopened);
    std::fs::remove_file(dir.path().join("proxy-state/proxy-presence.redb")).unwrap();
    assert!(matches!(
        Prepared::load(&path, &identity()).unwrap().open(),
        Err(Error::Protection)
    ));
    assert!(!dir.path().join("proxy-state/proxy-presence.redb").exists());
}

#[tokio::test]
async fn stop_sender_loss_cancels_inflight_catalog_read_and_joins_both_controls() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let entered = Arc::new(tokio::sync::Notify::new());
    let notice = entered.clone();
    let app = Router::new().route(
        "/proxy/discovery",
        post(move || {
            let notice = notice.clone();
            async move {
                notice.notify_one();
                std::future::pending::<Json<Value>>().await
            }
        }),
    );
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let path = config(dir.path(), url, "ws://127.0.0.1:2".into());
    let (control, lifecycle) = Prepared::load(&path, &identity()).unwrap().open().unwrap();
    let (stop, stopping) = watch::channel(false);
    let task = tokio::spawn(lifecycle.run(stopping));
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    drop(stop);
    tokio::time::timeout(Duration::from_millis(300), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let health = control.health().unwrap();
    assert!(!health.running && !health.catalog_fresh && !health.presence.running);
    assert_eq!(health.catalog.phase, supervisor::Phase::Stopped);
    assert!(health.failure.is_none());
}

#[tokio::test]
async fn wrong_canonical_network_stops_only_proxy_control_and_preserves_native_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let mut other = identity();
    other.network_id = "other".into();
    let (rpc_url, _rpc) = rpc(other).await;
    let path = config(dir.path(), rpc_url, "ws://127.0.0.1:2".into());
    let (control, lifecycle) = Prepared::load(&path, &identity()).unwrap().open().unwrap();
    let state = GatewayState::fixture().with_proxy_control(control.clone());
    let native = state.models_snapshot();
    let (_stop, stopping) = watch::channel(false);
    let result = tokio::time::timeout(Duration::from_secs(2), lifecycle.run(stopping))
        .await
        .unwrap();
    assert!(matches!(result, Err(Error::Supervisor)));
    let health = control.health().unwrap();
    assert_eq!(health.failure, Some("catalog_identity_mismatch"));
    assert!(!health.running && !health.catalog_fresh && !health.presence.running);
    assert_eq!(health.catalog.phase, supervisor::Phase::Degraded);
    assert!(Arc::ptr_eq(&native, &state.models_snapshot()));
}
