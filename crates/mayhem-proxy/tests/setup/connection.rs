use super::*;
use mayhem_proxy::setup::DiscoveryState;
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Backend {
    calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn backend(f: &mut Fixture, status: u16, body: Vec<u8>, delay: Duration) -> Backend {
    f.connection
        .as_object_mut()
        .unwrap()
        .remove("authentication");
    f.connection["paths"]["models"] = json!("models");
    f.connection["error_profile"] = json!("open_ai");
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    let listener = f.async_listener();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            let mut buf = [0; 1024];
            while !header.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                header.extend_from_slice(&buf[..n]);
                assert!(header.len() < 8192);
            }
            if header.is_empty() {
                continue;
            }
            let header = String::from_utf8(header).unwrap();
            assert!(header.starts_with("GET /v1/models HTTP/1.1\r\n"));
            assert!(!header.to_ascii_lowercase().contains("authorization:"));
            count.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nx-private-fixture: never-retain-header\r\nConnection: close\r\n\r\n", body.len());
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(&body).await;
            }
        }
    });
    Backend { calls, task }
}
async fn called(backend: &Backend) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while backend.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn list() -> Vec<u8> {
    serde_json::to_vec(&json!({"object":"list", "data":[
        {"id":"private-zeta", "object":"model", "owned_by":"discard-vendor", "capabilities":{"context":999999}, "secret":"never-retain-body"},
        {"id":"private-α", "object":"model"}
    ], "url":"http://discard-origin", "concurrency":100000})).unwrap()
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "requires isolated native Windows private NTFS fixture parent"
)]
async fn discovery_is_explicit_private_bounded_and_never_changes_or_certifies_a_draft() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let backend = backend(&mut f, 200, list(), Duration::ZERO);
    let store = f.store();
    store.create(f.input.clone()).unwrap();
    store.check(1).unwrap();
    let draft = std::fs::read(f.store.join("draft.json")).unwrap();
    let found = store
        .discover(&f.input.connection_file, 0, 2000)
        .await
        .unwrap();
    assert_eq!(found.state, DiscoveryState::Listed);
    assert_eq!(found.revision, 2);
    assert_eq!(found.model_count, 2);
    assert!(!found.truncated);
    assert!(found.for_current_configuration);
    assert_eq!(found.configured_endpoints.len(), 4);
    assert!(found.model_ids.is_none());
    for fact in [
        found.model_identity,
        found.capabilities,
        found.readiness,
        found.concurrency,
    ] {
        assert_eq!(fact, "not_verified");
    }
    assert_eq!(found.admission_status, "not_checked");
    let output = serde_json::to_string(&found).unwrap();
    for hidden in [
        "private-zeta",
        "private-α",
        "discard-vendor",
        "never-retain",
        "127.0.0.1",
        "connection_file",
    ] {
        assert!(!output.contains(hidden));
    }
    let retained = std::fs::read_to_string(f.store.join("discovery.json")).unwrap();
    for hidden in [
        "discard-vendor",
        "never-retain",
        "discard-origin",
        "999999",
        "100000",
        "authentication",
    ] {
        assert!(!retained.contains(hidden));
    }
    let resumed = f.store().inspect_connection(true).unwrap();
    assert_eq!(resumed.discovery_id, found.discovery_id);
    assert_eq!(
        resumed.model_ids.unwrap(),
        vec!["private-zeta", "private-α"]
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        store.discover(&f.input.connection_file, 0, 2000).await,
        Err(Error::Conflict)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    let refresh = store
        .discover(&f.input.connection_file, 2, 2000)
        .await
        .unwrap();
    assert_eq!(refresh.discovery_id, found.discovery_id);
    assert_eq!(refresh.revision, 4);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    assert!(
        std::fs::read(f.store.join("draft.json")).unwrap() == draft,
        "discovery never mutates declarations"
    );
    assert_private_file(&f.store.join("discovery.json"));
    let names: BTreeSet<_> = std::fs::read_dir(&f.store)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["draft.json", "draft.lock", "discovery.json"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert!(!f.dir.path().join("never-read-secret").exists());
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "requires isolated native Windows private NTFS fixture parent"
)]
async fn absent_models_is_supported_offline_and_bad_or_oversize_lists_are_not_observations() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let unsupported = f
        .store()
        .discover(&f.input.connection_file, 0, 2000)
        .await
        .unwrap();
    assert_eq!(unsupported.state, DiscoveryState::Unsupported);
    f.no_network_or_secret();
    f.store().create(f.input.clone()).unwrap(); // A custom explicit profile remains possible.
    for (status, body, expected) in [
        (404, b"{}".to_vec(), DiscoveryState::Unsupported),
        (
            401,
            br#"{"error":{"message":"never-retain-body"}}"#.to_vec(),
            DiscoveryState::Unavailable,
        ),
        (
            200,
            br#"{"data":[{"id":"bad\nline"}]}"#.to_vec(),
            DiscoveryState::InvalidResponse,
        ),
        (
            200,
            br#"{"data":[{"id":"duplicate"},{"id":"duplicate"}]}"#.to_vec(),
            DiscoveryState::InvalidResponse,
        ),
        (
            200,
            br#"{"data":[{"id":12}]}"#.to_vec(),
            DiscoveryState::InvalidResponse,
        ),
        (
            200,
            br#"{"data":{},"has_more":true}"#.to_vec(),
            DiscoveryState::InvalidResponse,
        ),
        (200, vec![b' '; 65537], DiscoveryState::LimitExceeded),
    ] {
        let mut f = Fixture::new(ProxyEndpoint::Chat);
        let backend = backend(&mut f, status, body, Duration::ZERO);
        let result = f
            .store()
            .discover(&f.input.connection_file, 0, 2000)
            .await
            .unwrap();
        assert_eq!(result.state, expected);
        assert_eq!(result.model_count, 0);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert!(!std::fs::read_to_string(f.store.join("discovery.json"))
            .unwrap()
            .contains("never-retain-body"));
    }
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "requires isolated native Windows private NTFS fixture parent"
)]
async fn truncation_is_explicit_and_single_response_never_claims_global_completeness() {
    for (count, has_more) in [(140, false), (1, true)] {
        let mut f = Fixture::new(ProxyEndpoint::Chat);
        let body = serde_json::to_vec(&json!({"data":(0..count).map(|i| json!({"id":format!("model-{i:04}")})).collect::<Vec<_>>(), "has_more":has_more})).unwrap();
        let _backend = backend(&mut f, 200, body, Duration::ZERO);
        let found = f
            .store()
            .discover(&f.input.connection_file, 0, 2000)
            .await
            .unwrap();
        assert_eq!(found.model_count, count.min(128));
        assert!(found.truncated);
        assert_eq!(found.scope, "single_model_list_response");
        assert!(
            f.store()
                .inspect_connection(true)
                .unwrap()
                .model_ids
                .unwrap()
                .len()
                <= 128
        );
    }
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "requires isolated native Windows private NTFS fixture parent"
)]
async fn interrupted_discovery_remains_pending_and_resume_does_not_dispatch_or_unlock_a_second_owner(
) {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let backend = backend(&mut f, 200, list(), Duration::from_secs(5));
    let store = f.store();
    let connection = f.input.connection_file.clone();
    let task = tokio::spawn(async move { store.discover(&connection, 0, 9000).await });
    called(&backend).await;
    assert!(matches!(Store::open(&f.store), Err(Error::Busy)));
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    let resumed = f.store().inspect_connection(true).unwrap();
    assert_eq!(resumed.state, DiscoveryState::Pending);
    assert_eq!(resumed.revision, 1);
    assert!(resumed.observed_at_ms.is_none());
    assert!(resumed.model_ids.unwrap().is_empty());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    let before = std::fs::read(f.store.join("discovery.json")).unwrap();
    let _ = f.store().inspect_connection(false).unwrap();
    assert!(std::fs::read(f.store.join("discovery.json")).unwrap() == before);
    assert!(matches!(
        f.store().discover(&f.input.connection_file, 0, 2000).await,
        Err(Error::Conflict)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "requires isolated native Windows private NTFS fixture parent"
)]
async fn timeout_configuration_drift_and_revision_overflow_never_retain_successful_model_evidence()
{
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let first_backend = backend(&mut f, 200, list(), Duration::from_millis(100));
    let result = f
        .store()
        .discover(&f.input.connection_file, 0, 10)
        .await
        .unwrap();
    assert_eq!(result.state, DiscoveryState::TimedOut);
    assert_eq!(result.model_count, 0);
    drop(first_backend);
    let backend = backend(&mut f, 200, list(), Duration::from_millis(100));
    let store = f.store();
    let connection = f.input.connection_file.clone();
    let task = tokio::spawn(async move { store.discover(&connection, 2, 2000).await });
    called(&backend).await;
    f.connection["revision"] = json!(2);
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    let result = task.await.unwrap().unwrap();
    assert_eq!(result.state, DiscoveryState::ConfigurationChanged);
    assert!(!result.for_current_configuration);
    assert_eq!(result.model_count, 0);
    let path = f.store.join("discovery.json");
    let mut retained: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    retained["revision"] = json!(PROXY_MAX_SAFE_INTEGER);
    private(&path, &serde_json::to_vec(&retained).unwrap());
    assert!(matches!(
        f.store()
            .discover(&f.input.connection_file, PROXY_MAX_SAFE_INTEGER, 2000)
            .await,
        Err(Error::Invalid)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    #[cfg(unix)]
    {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            f.store().inspect_connection(false),
            Err(Error::Protection)
        ));
    }
}
