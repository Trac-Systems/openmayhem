use super::*;
use clap::Parser;
use serde_json::json;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[test]
fn use_proxy_config_is_explicit_and_rejects_embedded_catalog_mode() {
    assert!(crate::UseArgs::try_parse_from(["use"])
        .unwrap()
        .proxy_config
        .is_none());
    let parsed =
        crate::UseArgs::try_parse_from(["use", "--proxy-config", "/private/proxy.json"]).unwrap();
    assert_eq!(
        parsed.proxy_config,
        Some(PathBuf::from("/private/proxy.json"))
    );
    assert!(crate::UseArgs::try_parse_from([
        "use",
        "--dev-embedded-catalog",
        "--proxy-config",
        "/private/proxy.json"
    ])
    .is_err());
}

fn fixture() -> (Value, Value, String, toml::Value) {
    let admin = "c".repeat(64);
    let status = json!({"peer":{"admin":admin,"subnetBootstrapHex":"B".repeat(64)},
        "msb":{"networkId":918,"bootstrapHex":"A".repeat(64)}});
    let health = json!({"contract_version":mayhem_proto::CONTRACT_VERSION});
    (
        status,
        health,
        admin,
        toml::Value::Table(Default::default()),
    )
}

#[test]
fn expected_network_uses_canonical_peer_and_existing_pins_before_proxy_config() {
    let (status, health, admin, config) = fixture();
    let expected = expected_identity(&status, &health, &admin, &config).unwrap();
    assert_eq!(expected.network_id, "918");
    assert_eq!(expected.msb_bootstrap, "a".repeat(64));
    assert_eq!(expected.subnet_bootstrap, "b".repeat(64));
    let pinned: toml::Value = toml::from_str(&format!(
        "[network]\nmsb_bootstrap = '{}'\nsubnet_bootstrap = '{}'\nadmin_peer_pubkey = '{}'\n",
        "a".repeat(64),
        "b".repeat(64),
        admin
    ))
    .unwrap();
    assert_eq!(
        expected_identity(&status, &health, &admin, &pinned).unwrap(),
        expected
    );
    for change in 0..7 {
        let (mut status, mut health, admin, mut config) = fixture();
        match change {
            0 => status["peer"]["admin"] = json!("d".repeat(64)),
            1 => status["msb"]["bootstrapHex"] = json!("not-a-bootstrap"),
            2 => status["msb"]["networkId"] = json!("918"),
            3 => health["contract_version"] = json!(mayhem_proto::CONTRACT_VERSION + 1),
            4 => {
                config =
                    toml::from_str(&format!("[network]\nsubnet_bootstrap='{}'", "d".repeat(64)))
                        .unwrap()
            }
            5 => {
                config = toml::from_str(&format!(
                    "[network]\nadmin_peer_pubkey='{}'",
                    "d".repeat(64)
                ))
                .unwrap()
            }
            _ => config = toml::from_str("[network]\nname='mainnet'").unwrap(),
        }
        assert!(expected_identity(&status, &health, &admin, &config).is_err());
    }
}

#[tokio::test]
async fn proxy_failure_keeps_native_service_until_owner_stop() {
    let (shutdown, stop) = watch::channel(false);
    let (signal, signalled) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(supervise(
        std::future::pending(),
        async { Err(anyhow!("fixture control failure")) },
        async {
            signalled.await?;
            Ok(())
        },
        shutdown,
    ));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!task.is_finished());
    assert!(!*stop.borrow());
    signal.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(*stop.borrow());
}

#[tokio::test]
async fn signal_joins_proxy_cleanup_without_waiting_for_open_native_streams() {
    let (shutdown, stop) = watch::channel(false);
    let joined = Arc::new(AtomicBool::new(false));
    let completed = joined.clone();
    let proxy = async move {
        wait_for_stop(stop).await;
        tokio::task::yield_now().await;
        completed.store(true, Ordering::SeqCst);
        Ok(())
    };
    tokio::time::timeout(
        Duration::from_secs(1),
        supervise(std::future::pending(), proxy, async { Ok(()) }, shutdown),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(joined.load(Ordering::SeqCst));
}

#[tokio::test]
async fn gateway_bind_failure_stops_and_joins_proxy_control() {
    let (shutdown, stop) = watch::channel(false);
    let joined = Arc::new(AtomicBool::new(false));
    let completed = joined.clone();
    let result = supervise(
        async { Err(io::Error::from(io::ErrorKind::AddrInUse)) },
        async move {
            wait_for_stop(stop).await;
            completed.store(true, Ordering::SeqCst);
            Ok(())
        },
        std::future::pending(),
        shutdown,
    )
    .await;
    assert!(result.is_err());
    assert!(joined.load(Ordering::SeqCst));
}

#[tokio::test]
async fn buyer_failure_leaves_discovery_and_native_serving_until_owner_stop() {
    let (shutdown, stopped) = watch::channel(false);
    let (signal, signalled) = tokio::sync::oneshot::channel();
    let joined = Arc::new(AtomicBool::new(false));
    let completed = joined.clone();
    let proxy = async move {
        join_controls(
            async move {
                wait_for_stop(stopped).await;
                completed.store(true, Ordering::SeqCst);
                Ok(())
            },
            async { Err(anyhow!("fixture buyer failure")) },
        )
        .await;
        Ok(())
    };
    let task = tokio::spawn(supervise(
        std::future::pending(),
        proxy,
        async {
            signalled.await?;
            Ok(())
        },
        shutdown,
    ));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!task.is_finished());
    assert!(!joined.load(Ordering::SeqCst));
    signal.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(joined.load(Ordering::SeqCst));
}

#[tokio::test]
async fn stop_joins_buyer_commit_even_after_discovery_failure() {
    let (shutdown, stopped) = watch::channel(false);
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let proxy = async move {
        join_controls(
            async { Err(anyhow!("fixture discovery failure")) },
            async move {
                wait_for_stop(stopped).await;
                entered.send(()).unwrap();
                released.await?;
                Ok(())
            },
        )
        .await;
        Ok(())
    };
    let task = tokio::spawn(supervise(
        std::future::pending(),
        proxy,
        async { Ok(()) },
        shutdown,
    ));
    tokio::time::timeout(Duration::from_secs(1), entry)
        .await
        .unwrap()
        .unwrap();
    assert!(!task.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
