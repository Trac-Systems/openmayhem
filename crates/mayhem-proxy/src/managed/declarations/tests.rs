//! Actual protected source + signed checkpoint + metadata runner. No provider,
//! inference client, payment client or financial journal is constructed.
#![cfg(unix)]
use super::*;
use crate::{declaration, discovery, registry};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

fn d(n: u8) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
struct Fixture {
    _dir: tempfile::TempDir,
    entry: Entry,
    key: SigningKey,
    issued: u64,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = SigningKey::from_bytes(&[221; 32]);
        let source = crate::setup::DeclarationSource {
            directory: dir.path().to_owned(),
            draft_id: d(1),
            subject: declaration::Subject {
                network: discovery::Identity {
                    network_id: "managed-declaration-fixture".into(),
                    msb_bootstrap: d(2).as_str().into(),
                    subnet_bootstrap: d(3).as_str().into(),
                    contract_version: mayhem_proto::CONTRACT_VERSION,
                },
                provider: Digest::new(hex(&key.verifying_key().to_bytes())).unwrap(),
                market: d(4),
                membership_revision: 1,
                membership_digest: d(5),
                endpoint: mayhem_proto::proxy::ProxyEndpoint::Chat,
                endpoint_contract: d(6),
                recipe_hash: d(7),
                connection_revision: 1,
            },
        };
        let live = Arc::new(Live::new(source.subject.clone()));
        let fixture = Self {
            _dir: dir,
            entry: Entry {
                route: d(8),
                source,
                live,
                source_status: Arc::new(Mutex::new("not_read")),
            },
            key,
            issued: now().unwrap().saturating_sub(1_000),
        };
        // Invalid as real financial databases on purpose: the metadata loop has
        // no reason to open them. Compare bytes after every acceptance scenario.
        fixture.write("capacity.redb", b"original capacity identity");
        fixture.write("route-financial.redb", b"original accepted terms");
        fixture.write("managed-config.json", b"original immutable launch config");
        fixture
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.entry.source.directory.join(name)
    }
    fn lock(&self) -> File {
        use rustix::fs::{flock, FlockOperation};
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.path("draft.lock"))
            .unwrap();
        let started = Instant::now();
        loop {
            match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => return file,
                Err(rustix::io::Errno::WOULDBLOCK)
                    if started.elapsed() < Duration::from_secs(2) =>
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("fixture lock failed: {error}"),
            }
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let _lock = self.lock();
        let temporary = self.path("fixture-update.next");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        std::fs::rename(temporary, self.path(name)).unwrap();
        File::open(&self.entry.source.directory)
            .unwrap()
            .sync_all()
            .unwrap();
    }
    fn publish(&self, revision: u64, support: registry::Support) -> declaration::Signed {
        let body = declaration::Body {
            schema_version: 1,
            subject: self.entry.source.subject.clone(),
            revision,
            issued_at_ms: self.issued,
            expires_at_ms: self.issued + 120_000,
            claims: vec![declaration::Claim {
                field_id: "privacy.training".into(),
                schema_revision: 1,
                definition_digest: d(9),
                status: support,
                value: (support == registry::Support::Supported)
                    .then_some(registry::TypedValue::Boolean(false)),
            }],
        };
        let signed = declaration::Signed {
            signature: hex(&self.key.sign(&body.signing_bytes().unwrap()).to_bytes()),
            body: body.clone(),
        };
        // Reproduce the public retained wire without widening setup internals.
        let mut plan = json!({
            "schema_version":1,"draft_id":self.entry.source.draft_id,
            "draft_revision":1,"registry_release_id":"managed-fixture",
            "registry_release_hash":d(10),"body":body,
        });
        let plan_digest = Digest::hash(
            "mayhem/proxy/setup-data-handling-plan/v1",
            &[&mayhem_proto::stable_json_bytes(&plan).unwrap()],
        );
        plan["plan_digest"] = json!(plan_digest);
        self.write(
            "wizard-declaration-signed.json",
            &serde_json::to_vec(&json!({"schema_version":1,"plan":plan,"signed":signed})).unwrap(),
        );
        signed
    }
    fn checkpoint(&self) -> Vec<u8> {
        std::fs::read(self.path("wizard-declaration-observed.json")).unwrap()
    }
    fn status(&self) -> Status {
        Runner {
            entries: vec![self.entry.clone()],
        }
        .snapshots()
        .remove(&self.entry.route)
        .unwrap()
    }
    fn assert_current(&self, signed: &declaration::Signed) {
        let current = self.entry.live.read(now().unwrap()).unwrap();
        assert_eq!(current.digest().unwrap(), signed.digest().unwrap());
        let status = self.status();
        assert_eq!(status.source_status, "read");
        assert_eq!(status.declaration.state, "available");
        assert_eq!(
            status.declaration.observed_revision,
            Some(signed.body.revision)
        );
    }
    fn assert_unchanged(&self) {
        for (name, bytes) in [
            ("capacity.redb", b"original capacity identity".as_slice()),
            (
                "route-financial.redb",
                b"original accepted terms".as_slice(),
            ),
            (
                "managed-config.json",
                b"original immutable launch config".as_slice(),
            ),
        ] {
            assert_eq!(std::fs::read(self.path(name)).unwrap(), bytes);
        }
    }
}

#[test]
fn managed_declaration_entry_hides_missing_corrupt_busy_and_old_input_then_recovers_exact_latest() {
    let fixture = Fixture::new();
    let original = fixture.publish(1, registry::Support::Supported);
    fixture.entry.refresh();
    fixture.assert_current(&original);
    let old_wire = std::fs::read(fixture.path("wizard-declaration-signed.json")).unwrap();
    let renewed = fixture.publish(2, registry::Support::Supported);
    fixture.entry.refresh();
    fixture.assert_current(&renewed);
    let latest_wire = std::fs::read(fixture.path("wizard-declaration-signed.json")).unwrap();
    let checkpoint = fixture.checkpoint();

    std::fs::remove_file(fixture.path("wizard-declaration-signed.json")).unwrap();
    fixture.entry.refresh();
    assert!(fixture.entry.live.read(now().unwrap()).is_none());
    assert_eq!(fixture.status().source_status, "missing");
    assert_eq!(fixture.checkpoint(), checkpoint);

    fixture.write("wizard-declaration-signed.json", b"{");
    fixture.entry.refresh();
    assert!(fixture.entry.live.read(now().unwrap()).is_none());
    assert_eq!(fixture.status().source_status, "invalid_record");
    assert_eq!(fixture.checkpoint(), checkpoint);

    fixture.write("wizard-declaration-signed.json", &old_wire);
    fixture.entry.refresh();
    assert!(fixture.entry.live.read(now().unwrap()).is_none());
    assert_eq!(fixture.status().source_status, "revision_conflict");
    fixture.write("wizard-declaration-signed.json", &latest_wire);
    fixture.entry.refresh();
    fixture.assert_current(&renewed);
    assert_eq!(fixture.checkpoint(), checkpoint);

    let held = fixture.lock();
    fixture.entry.refresh();
    assert!(fixture.entry.live.read(now().unwrap()).is_none());
    assert_eq!(fixture.status().source_status, "busy");
    drop(held);
    fixture.entry.refresh();
    fixture.assert_current(&renewed);

    std::fs::set_permissions(
        fixture.path("wizard-declaration-signed.json"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    fixture.entry.refresh();
    assert!(fixture.entry.live.read(now().unwrap()).is_none());
    assert_eq!(fixture.status().source_status, "protection_rejected");
    std::fs::set_permissions(
        fixture.path("wizard-declaration-signed.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fixture.entry.refresh();
    fixture.assert_current(&renewed);
    fixture.assert_unchanged();
}

async fn observed(fixture: &Fixture, signed: &declaration::Signed) {
    let expected = signed.digest().unwrap();
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            if fixture
                .entry
                .live
                .read(now().unwrap())
                .is_some_and(|v| v.digest().unwrap() == expected)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual metadata runner did not observe signed update");
    fixture.assert_current(signed);
}

#[tokio::test]
async fn managed_declaration_runner_observes_renewal_and_unknown_withdrawal_without_replacing_live()
{
    tokio::time::timeout(Duration::from_secs(25), async {
        let fixture = Fixture::new();
        let first = fixture.publish(1, registry::Support::Supported);
        let original_live = fixture.entry.live.clone();
        let runner = Arc::new(Runner { entries: vec![fixture.entry.clone()] });
        let (stop, stopped) = watch::channel(false);
        let running = runner.clone();
        let task = tokio::spawn(async move { running.run(stopped).await });
        // Independent executor activity is not governed by the metadata stop.
        // This is a liveness sentinel, not simulated paid inference.
        let progress = Arc::new(AtomicUsize::new(0));
        let progress_task = progress.clone();
        let (other_stop, mut other_stopped) = watch::channel(false);
        let other = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = other_stopped.changed() => break,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => { progress_task.fetch_add(1, Ordering::SeqCst); }
                }
            }
        });
        observed(&fixture, &first).await;
        let first_checkpoint = fixture.checkpoint();
        let second = fixture.publish(2, registry::Support::Supported);
        observed(&fixture, &second).await;
        assert_ne!(fixture.checkpoint(), first_checkpoint);
        assert!(!task.is_finished());
        let withdrawn = fixture.publish(3, registry::Support::Unknown);
        observed(&fixture, &withdrawn).await;
        let current = original_live.read(now().unwrap()).unwrap();
        assert_eq!(current.body.revision, 3);
        assert!(matches!(current.body.claims[0].status, registry::Support::Unknown));
        assert!(current.body.claims[0].value.is_none());
        assert!(Arc::ptr_eq(&original_live, &runner.entries[0].live));
        let final_checkpoint = fixture.checkpoint();
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap().unwrap();
        assert!(original_live.read(now().unwrap()).is_none());
        assert_ne!(runner.snapshots()[&fixture.entry.route].declaration.state, "available");
        let progress_at_shutdown = progress.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(progress.load(Ordering::SeqCst) > progress_at_shutdown);
        assert!(!other.is_finished());
        other_stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), other).await.unwrap().unwrap();
        assert_eq!(fixture.checkpoint(), final_checkpoint);
        fixture.assert_unchanged();
    }).await.expect("bounded managed metadata acceptance timed out");
}

#[tokio::test]
async fn managed_declaration_runner_stopped_before_start_clears_without_reading_input() {
    let fixture = Fixture::new();
    let first = fixture.publish(1, registry::Support::Supported);
    fixture.entry.refresh();
    fixture.assert_current(&first);
    let checkpoint = fixture.checkpoint();
    fixture.write("wizard-declaration-signed.json", b"not a signed record");
    let runner = Runner {
        entries: vec![fixture.entry.clone()],
    };
    let (_stop, stopped) = watch::channel(true);
    tokio::time::timeout(Duration::from_secs(1), runner.run(stopped))
        .await
        .unwrap()
        .unwrap();
    assert!(fixture.entry.live.read(now().unwrap()).is_none());
    // The last read status remains unchanged: biased shutdown did not read the
    // invalid input or mutate the durable checkpoint before clearing availability.
    assert_eq!(fixture.status().source_status, "read");
    assert_eq!(fixture.checkpoint(), checkpoint);
    fixture.assert_unchanged();
}
