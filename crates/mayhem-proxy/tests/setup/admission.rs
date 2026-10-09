use super::*;
use mayhem_proxy::setup::AdmissionState;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Peer {
    url: String,
    calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn peer(
    f: &Fixture,
    reply: impl Fn(Value, usize) -> (u16, Value) + Send + Sync + 'static,
    delay: Duration,
) -> Peer {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let context = json!({"network_id":f.input.network.network_id,"msb_bootstrap":f.input.network.msb_bootstrap,
        "subnet_bootstrap":f.input.network.subnet_bootstrap,"contract_version":f.input.network.contract_version,"epoch":100});
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 1024];
            let end = loop {
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break None;
                }
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 4096);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    break Some(end + 4);
                }
            };
            let Some(end) = end else {
                continue;
            };
            let header = String::from_utf8(bytes[..end].to_vec())
                .unwrap()
                .to_ascii_lowercase();
            assert!(header.starts_with("post /v1/proxy/provider-state http/1.1\r\n"));
            assert!(!header.contains("authorization:"));
            let length: usize = header
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            while bytes.len() < end + length {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            let query: Value = serde_json::from_slice(&bytes[end..end + length]).unwrap();
            assert_eq!(query.as_object().unwrap().len(), 3);
            let wire = json!({"ok":true,"schema_version":1,"lane":"proxy","provider_pubkey":query["provider_pubkey"],
                "requester":query["provider_pubkey"],"initial_operation_digest":query["initial_operation_digest"],"request_nonce":query["request_nonce"],
                "context":context,"proof":{"view_key":d(30),"tree_hash":d(31),"signed_length":10,"fork":0},
                "registry_enabled":true,"fee_policy_hash":d(32),"provider":null,"provider_revoked":false,"admission_revoked":false});
            let (status, wire) = reply(wire, count.fetch_add(1, Ordering::SeqCst));
            tokio::time::sleep(delay).await;
            let body = serde_json::to_vec(&wire).unwrap();
            let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(&body).await;
            }
        }
    });
    Peer { url, calls, task }
}
async fn wait_call(peer: &Peer) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while peer.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn checked(f: &Fixture) {
    f.store().create(f.input.clone()).unwrap();
    f.store().check(1).unwrap();
}

#[tokio::test]
async fn four_families_retain_explicit_canonical_absence_without_payment_or_upstream_claims() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let f = Fixture::new(endpoint);
        checked(&f);
        let rpc = peer(&f, |v, _| (200, v), Duration::ZERO);
        let result = f.store().admission_check(2, &rpc.url, 2000).await.unwrap();
        assert_eq!(result.revision, 4);
        assert_eq!(result.admission_status, "observed_not_registered");
        let report = result.admission.as_ref().unwrap();
        assert_eq!(report.state, AdmissionState::Observed);
        assert_eq!(report.next_sequence, Some(1));
        assert_eq!(report.sequence_matches, Some(true));
        assert!(!report.expired);
        assert!(report.for_current_configuration);
        assert_eq!(report.payment_status, "not_checked");
        assert!(!report.authorizes_publication);
        assert_eq!(result.publication_status, "not_submitted");
        assert_eq!(result.serving_status, "not_started");
        assert_eq!(
            serde_json::to_value(f.store().inspect().unwrap()).unwrap(),
            json!(result)
        );
        assert_eq!(rpc.calls.load(Ordering::SeqCst), 1);
        f.no_network_or_secret();
        let public = serde_json::to_string(&result).unwrap();
        for hidden in [
            "http://",
            "never-read-secret",
            "connection_file",
            "invoice",
            "unpaid",
            "private-upstream-model",
        ] {
            assert!(!public.contains(hidden), "{hidden}");
        }
    }
}

#[tokio::test]
async fn admitted_revoked_disabled_and_exact_applied_operation_are_observations_not_permissions() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    checked(&f);
    let rpc = peer(
        &f,
        |mut v, n| {
            v["provider"] = json!({"sequence":1,"operation_digest":v["initial_operation_digest"],"entitlement_id":d(44)});
            if n == 1 {
                v["admission_revoked"] = json!(true);
            }
            if n == 2 {
                v["registry_enabled"] = json!(false);
            }
            if n == 3 {
                v["provider"]["sequence"] = json!(mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER);
            }
            (200, v)
        },
        Duration::ZERO,
    );
    let first = f.store().admission_check(2, &rpc.url, 2000).await.unwrap();
    assert_eq!(first.admission_status, "observed_admitted");
    let a = first.admission.unwrap();
    assert_eq!(a.next_sequence, Some(2));
    assert_eq!(a.sequence_matches, Some(false));
    assert_eq!(a.operation_already_applied, Some(true));
    assert!(!a.authorizes_publication);
    assert_eq!(
        f.store()
            .admission_check(4, &rpc.url, 2000)
            .await
            .unwrap()
            .admission_status,
        "observed_revoked"
    );
    let disabled = f
        .store()
        .admission_check(6, &rpc.url, 2000)
        .await
        .unwrap()
        .admission
        .unwrap();
    assert!(!disabled.evidence.unwrap().registry_enabled);
    assert!(!disabled.authorizes_publication);
    let exhausted = f
        .store()
        .admission_check(8, &rpc.url, 2000)
        .await
        .unwrap()
        .admission
        .unwrap();
    assert_eq!(exhausted.next_sequence, None);
    assert_eq!(exhausted.sequence_matches, None);
    f.no_network_or_secret();
}

#[tokio::test]
async fn rejects_substitution_oversize_old_peer_and_regression_without_erasing_the_proof_floor() {
    for fault in [
        "provider",
        "nonce",
        "operation",
        "network",
        "extra",
        "oversize",
        "old-peer",
        "bad-proof",
        "missing-admission",
    ] {
        let f = Fixture::new(ProxyEndpoint::Chat);
        checked(&f);
        let rpc = peer(
            &f,
            move |mut v, _| {
                match fault {
                    "provider" => v["requester"] = json!(d(88)),
                    "nonce" => v["request_nonce"] = json!(d(88)),
                    "operation" => v["initial_operation_digest"] = json!(d(88)),
                    "network" => v["context"]["network_id"] = json!("other"),
                    "extra" => v["paid"] = json!(true),
                    "oversize" => v["extra"] = json!("x".repeat(8192)),
                    "old-peer" => return (404, json!({"error":"unknown route"})),
                    "bad-proof" => v["proof"]["signed_length"] = json!(0),
                    "missing-admission" => v["admission_revoked"] = json!(true),
                    _ => unreachable!(),
                }
                (200, v)
            },
            Duration::ZERO,
        );
        let review = f.store().admission_check(2, &rpc.url, 2000).await.unwrap();
        assert_eq!(
            review.admission_status, "observation_unavailable",
            "{fault}"
        );
        assert!(review.admission.unwrap().evidence.is_none());
        f.no_network_or_secret();
    }
    let f = Fixture::new(ProxyEndpoint::Chat);
    checked(&f);
    let rpc = peer(
        &f,
        |mut v, n| {
            if n == 1 {
                v["proof"]["signed_length"] = json!(9);
            }
            if n == 2 {
                v["context"]["epoch"] = json!(99);
            }
            (200, v)
        },
        Duration::ZERO,
    );
    for (revision, state) in [
        (2, AdmissionState::Observed),
        (4, AdmissionState::InvalidResponse),
        (6, AdmissionState::InvalidResponse),
        (8, AdmissionState::Observed),
    ] {
        assert_eq!(
            f.store()
                .admission_check(revision, &rpc.url, 2000)
                .await
                .unwrap()
                .admission
                .unwrap()
                .state,
            state
        );
    }
}

#[tokio::test]
async fn drift_expiry_and_future_clock_make_retained_evidence_require_refresh() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    checked(&f);
    let rpc = peer(&f, |v, _| (200, v), Duration::ZERO);
    f.store().admission_check(2, &rpc.url, 2000).await.unwrap();
    let path = f.store.join("draft.json");
    let original: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for offset in [-60_000i64, 60_000] {
        let mut changed = original.clone();
        for key in ["started_at_ms", "expires_at_ms"] {
            changed["admission"][key] =
                json!((changed["admission"][key].as_u64().unwrap() as i64 + offset) as u64);
        }
        private(&path, &serde_json::to_vec(&changed).unwrap());
        let review = f.store().inspect().unwrap();
        assert_eq!(review.admission_status, "refresh_required");
        assert!(review.admission.unwrap().expired);
    }
    private(&path, &serde_json::to_vec(&original).unwrap());
    f.input.offers[0].per_request_au += 1;
    let updated = f.store().update(4, f.input.clone()).unwrap();
    assert_eq!(updated.admission_status, "refresh_required");
    assert!(!updated.admission.unwrap().for_current_configuration);
    assert_eq!(rpc.calls.load(Ordering::SeqCst), 1);
    f.no_network_or_secret();
}

#[tokio::test]
async fn cancelled_read_retains_pending_original_revision_and_never_retries_on_resume() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    checked(&f);
    let rpc = peer(&f, |v, _| (200, v), Duration::from_secs(3));
    let store = f.store();
    let url = rpc.url.clone();
    let run = tokio::spawn(async move { store.admission_check(2, &url, 5000).await });
    wait_call(&rpc).await;
    assert!(matches!(Store::open(&f.store), Err(Error::Busy)));
    run.abort();
    assert!(run.await.is_err());
    let resumed = f.store().inspect().unwrap();
    assert_eq!(resumed.revision, 3);
    assert_eq!(resumed.admission.unwrap().state, AdmissionState::Pending);
    assert!(matches!(
        f.store().admission_check(2, &rpc.url, 5000).await,
        Err(Error::Conflict)
    ));
    assert_eq!(rpc.calls.load(Ordering::SeqCst), 1);
    f.no_network_or_secret();
}

#[tokio::test]
async fn in_flight_connection_drift_and_deadline_failure_never_install_an_observation() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    checked(&f);
    let rpc = peer(&f, |v, _| (200, v), Duration::from_millis(100));
    let store = f.store();
    let url = rpc.url.clone();
    let run = tokio::spawn(async move { store.admission_check(2, &url, 2000).await });
    wait_call(&rpc).await;
    f.connection["id"] = json!("changed-private-connection");
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.admission_status, "refresh_required");
    let observation = result.admission.unwrap();
    assert_eq!(observation.state, AdmissionState::ConfigurationChanged);
    assert!(observation.evidence.is_none());
    let f = Fixture::new(ProxyEndpoint::Chat);
    checked(&f);
    let rpc = peer(&f, |v, _| (200, v), Duration::from_millis(100));
    let observation = f
        .store()
        .admission_check(2, &rpc.url, 10)
        .await
        .unwrap()
        .admission
        .unwrap();
    assert!(matches!(
        observation.state,
        AdmissionState::TimedOut | AdmissionState::Unavailable
    ));
    assert!(observation.evidence.is_none());
    assert!(!observation.authorizes_publication);
    f.no_network_or_secret();
}

#[tokio::test]
async fn invalid_configuration_and_unchecked_drafts_cannot_contact_peer_or_consume_revision() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let rpc = peer(&f, |v, _| (200, v), Duration::ZERO);
    f.store().create(f.input.clone()).unwrap();
    assert!(f.store().admission_check(1, &rpc.url, 2000).await.is_err());
    f.store().check(1).unwrap();
    for url in [
        "http://example.invalid/v1",
        "http://user@127.0.0.1/v1",
        "http://127.0.0.1/v1?secret=value",
        "file:///tmp/peer",
    ] {
        assert!(f.store().admission_check(2, url, 2000).await.is_err());
    }
    for timeout in [0, 15_001] {
        assert!(f
            .store()
            .admission_check(2, &rpc.url, timeout)
            .await
            .is_err());
    }
    assert_eq!(f.store().inspect().unwrap().revision, 2);
    assert_eq!(rpc.calls.load(Ordering::SeqCst), 0);
    f.no_network_or_secret();
}
