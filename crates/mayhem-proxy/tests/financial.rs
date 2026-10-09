use mayhem_proto::proxy::finance::ProxySpendAuthorization;
use mayhem_proxy::{discovery::Identity, financial::Client};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};
struct Fixture {
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
    client: Client,
    auth: ProxySpendAuthorization,
    policy: mayhem_proto::proxy::finance::ProxySettlementPolicy,
    buyer_client: std::sync::Arc<Client>,
}
impl Fixture {
    async fn new(rail: &str, family: &str) -> Self {
        Self::new_with_expiry(rail, family, false).await
    }
    async fn new_with_expiry(rail: &str, family: &str, expiry: bool) -> Self {
        Self::new_with_mode(rail, family, if expiry { "expiry" } else { "no_expiry" }).await
    }
    async fn new_with_mode(rail: &str, family: &str, mode: &str) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut child = tokio::process::Command::new("node")
            .arg("intercom/tests/helpers/proxy-financial-rpc-fixture.mjs")
            .arg(rail)
            .arg(family)
            .arg("null")
            .arg(mode)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let ready = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("fixture ready");
        let v: Value = serde_json::from_str(&ready).unwrap();
        let identity: Identity = serde_json::from_value(v["identity"].clone()).unwrap();
        let client = Client::new(
            v["url"].as_str().unwrap(),
            identity.clone(),
            v["requester"].as_str().unwrap().into(),
            2,
        )
        .unwrap();
        Self {
            stdin: child.stdin.take().unwrap(),
            child,
            lines,
            client,
            buyer_client: std::sync::Arc::new(
                Client::new(
                    v["buyer_url"].as_str().unwrap(),
                    identity,
                    v["buyer"].as_str().unwrap().into(),
                    4,
                )
                .unwrap(),
            ),
            auth: serde_json::from_value(v["authorization"].clone()).unwrap(),
            policy: serde_json::from_value(v["policy"].clone()).unwrap(),
        }
    }
    async fn command(&mut self, c: &str) {
        assert_eq!(self.request(c).await["done"], c);
    }
    async fn request(&mut self, c: &str) -> Value {
        self.stdin
            .write_all(format!("{c}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
        let v: Value = serde_json::from_str(
            &tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        v
    }
    async fn stop(mut self) {
        self.stdin.write_all(b"stop\n").await.unwrap();
        self.stdin.flush().await.unwrap();
        drop(self.stdin);
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
#[tokio::test]
async fn real_signed_canonical_rpc_binds_six_family_rail_reservations_and_stops_final_redispatch() {
    for family in ["llm", "decisions"] {
        for rail in ["fiat", "tnk", "tap"] {
            let mut f = Fixture::new(rail, family).await;
            let state = f.client.observe(&f.auth).await.unwrap();
            let binding = state.initial_binding().unwrap();
            assert_eq!(
                binding.accepted_terms.as_str(),
                f.auth.terms.digest().unwrap()
            );
            assert_eq!(binding.rail, f.auth.terms.rail);
            assert_eq!(
                binding.offer_digest.as_str(),
                f.auth.terms.offer.digest().unwrap()
            );
            assert_eq!(binding.reservation.as_str(), f.auth.terms.reservation_id);
            assert!(!state.has_receipt());
            assert!(!state.is_closed());
            f.command("final").await;
            let recovered = f.client.observe(&f.auth).await.unwrap();
            assert!(recovered.has_receipt());
            assert!(recovered.initial_binding().is_err());
            assert!(recovered.proof().follows(state.proof()));
            f.stop().await;
        }
    }
}
#[tokio::test]
async fn canonical_closure_is_recovery_evidence_not_a_new_dispatch_permission() {
    let mut f = Fixture::new("tnk", "llm").await;
    f.command("close").await;
    let state = f.client.observe(&f.auth).await.unwrap();
    assert!(state.is_closed());
    assert!(state.initial_binding().is_err());
    f.stop().await;
}
#[tokio::test]
async fn altered_challenge_network_authorization_and_reservation_cannot_admit() {
    let mut f = Fixture::new("tap", "llm").await;
    for kind in ["nonce", "network", "signature", "unknown"] {
        f.command(kind).await;
        assert!(f.client.observe(&f.auth).await.is_err(), "{kind}");
        f.command("reset").await;
    }
    f.command("hold").await;
    assert!(f
        .client
        .observe(&f.auth)
        .await
        .unwrap()
        .initial_binding()
        .is_err());
    f.command("reset").await;
    let mut changed = f.auth.clone();
    changed.buyer_sig = "0".repeat(128);
    assert!(f.client.observe(&changed).await.is_err());
    f.command("foreign").await;
    assert!(f.client.observe(&f.auth).await.is_err());
    f.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn initial_financial_acceptance_survives_restart_and_uses_owned_quota_and_pruning() {
    use mayhem_proxy::attempts::{self, Digest, Event, Journal};
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("tnk", "llm").await;
    let observation = f.client.observe(&f.auth).await.unwrap();
    let binding = observation.initial_binding().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("journal");
    let d = |s: &str| Digest::new(s).unwrap();
    let identity = attempts::Identity {
        network_id: f.auth.terms.network_id.clone(),
        msb_bootstrap: d(&f.auth.terms.msb_bootstrap),
        subnet_bootstrap: d(&f.auth.terms.subnet_bootstrap),
        controller_pubkey: d(&f.auth.terms.offer.provider_pubkey),
    };
    let limits = attempts::Limits {
        max_records: 10,
        max_unfinished: 10,
        closed_retention_ms: 1000,
        max_payload_bytes: 128 * 1024,
    };
    let journal = Journal::open(&path, identity.clone(), limits).unwrap();
    let r = journal
        .prepare(d(&"f".repeat(64)), binding.clone(), 100)
        .unwrap();
    journal
        .retain_financial_acceptance(&r.invocation, r.attempt, &observation)
        .unwrap();
    let bytes = journal.allocated_payload_bytes().unwrap();
    assert!(bytes > 0);
    journal
        .retain_financial_acceptance(&r.invocation, r.attempt, &observation)
        .unwrap();
    assert_eq!(journal.allocated_payload_bytes().unwrap(), bytes);
    let mut wrong = binding;
    wrong.reservation = d(&"c".repeat(64));
    let other = journal.prepare(d(&"b".repeat(64)), wrong, 100).unwrap();
    assert!(journal
        .retain_financial_acceptance(&other.invocation, other.attempt, &observation)
        .is_err());
    drop(journal);
    let journal = Journal::open(&path, identity.clone(), limits).unwrap();
    let recovered = journal
        .recover(&r.invocation, r.attempt)
        .unwrap()
        .financial
        .unwrap();
    assert_eq!(recovered.accepted().authorization, f.auth);
    assert_eq!(
        recovered.accepted().settlement_policy,
        observation.accepted().settlement_policy
    );
    assert_eq!(journal.prune_closed(u64::MAX, 64).unwrap(), 0);
    let r = journal
        .advance(&r.invocation, r.generation, Event::CancelRequested, 101)
        .unwrap();
    let r = journal
        .advance(
            &r.invocation,
            r.generation,
            Event::Close(d(&"e".repeat(64))),
            102,
        )
        .unwrap();
    assert_eq!(
        journal.prune_closed(r.expires_at_ms.unwrap(), 64).unwrap(),
        1
    );
    assert_eq!(journal.allocated_payload_bytes().unwrap(), 0);
    drop(journal);
    let path = dir.path().join("small");
    let journal = Journal::open(
        &path,
        identity,
        attempts::Limits {
            max_payload_bytes: 1,
            ..limits
        },
    )
    .unwrap();
    let r = journal
        .prepare(
            d(&"a".repeat(64)),
            observation.initial_binding().unwrap(),
            100,
        )
        .unwrap();
    assert!(matches!(
        journal.retain_financial_acceptance(&r.invocation, r.attempt, &observation),
        Err(attempts::Error::Capacity)
    ));
    assert!(journal
        .financial_acceptance(&r.invocation, r.attempt)
        .unwrap()
        .is_none());
    assert_eq!(journal.allocated_payload_bytes().unwrap(), 0);
    f.stop().await;
}

#[test]
fn financial_authority_never_travels_over_unprotected_remote_http_or_url_credentials() {
    let identity = Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: 30,
    };
    for url in [
        "http://example.invalid/v1",
        "http://192.0.2.1/v1",
        "https://user:password@example.invalid/v1",
        "https://example.invalid/v1?secret=x",
        "file:///tmp/peer",
    ] {
        assert!(Client::new(url, identity.clone(), "c".repeat(64), 2).is_err());
    }
    assert!(Client::new("http://127.0.0.1:1/v1", identity.clone(), "c".repeat(64), 2).is_ok());
    assert!(Client::new("https://example.invalid/v1", identity, "c".repeat(64), 2).is_ok());
}

#[cfg(unix)]
#[path = "support/buyer_recovery.rs"]
mod buyer_recovery;
