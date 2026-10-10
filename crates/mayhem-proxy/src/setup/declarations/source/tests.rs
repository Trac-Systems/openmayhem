use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::os::unix::fs::{symlink, PermissionsExt};

fn d(n: u8) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
struct Fixture {
    _dir: tempfile::TempDir,
    source: DeclarationSource,
    key: SigningKey,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = SigningKey::from_bytes(&[219; 32]);
        let provider = Digest::new(
            key.verifying_key()
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
        .unwrap();
        let source = DeclarationSource {
            directory: dir.path().to_owned(),
            draft_id: d(1),
            subject: declaration::Subject {
                network: Identity {
                    network_id: "source-fixture".into(),
                    msb_bootstrap: d(2).as_str().into(),
                    subnet_bootstrap: d(3).as_str().into(),
                    contract_version: mayhem_proto::CONTRACT_VERSION,
                },
                provider,
                market: d(4),
                membership_revision: 1,
                membership_digest: d(5),
                endpoint: mayhem_proto::proxy::ProxyEndpoint::Chat,
                endpoint_contract: d(6),
                recipe_hash: d(7),
                connection_revision: 1,
            },
        };
        // Source must never parse, overwrite or walk unrelated financial state.
        std::fs::write(dir.path().join("financial-sentinel"), b"unchanged").unwrap();
        Self {
            _dir: dir,
            source,
            key,
        }
    }
    fn publish(
        &self,
        revision: u64,
        expires_at_ms: u64,
        status: registry::Support,
    ) -> declaration::Signed {
        let body = declaration::Body {
            schema_version: 1,
            subject: self.source.subject.clone(),
            revision,
            issued_at_ms: 100,
            expires_at_ms,
            claims: vec![declaration::Claim {
                field_id: "privacy.training".into(),
                schema_revision: 1,
                definition_digest: d(8),
                status,
                value: (status == registry::Support::Supported)
                    .then_some(registry::TypedValue::Boolean(false)),
            }],
        };
        let signed = declaration::Signed {
            signature: self
                .key
                .sign(&body.signing_bytes().unwrap())
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            body: body.clone(),
        };
        let mut plan = DeclarationPlan {
            schema_version: 1,
            draft_id: self.source.draft_id.clone(),
            draft_revision: 1,
            registry_release_id: "fixture".into(),
            registry_release_hash: d(9),
            body,
            plan_digest: d(0),
        };
        plan.plan_digest = digest(&plan).unwrap();
        let retained = Retained {
            schema_version: 1,
            plan,
            signed: Some(signed.clone()),
        };
        let guard = store::Guard::open(&self.source.directory).unwrap();
        guard
            .write_json(
                "wizard-declaration-signed.json",
                "wizard-declaration-signed.next",
                &retained,
            )
            .unwrap();
        signed
    }
    fn checkpoint(&self) -> Vec<u8> {
        std::fs::read(
            self.source
                .directory
                .join("wizard-declaration-observed.json"),
        )
        .unwrap()
    }
}

#[test]
fn metadata_source_renews_withdraws_and_rejects_rollback_after_restart_without_money_changes() {
    let f = Fixture::new();
    let first = f.publish(1, 1000, registry::Support::Supported);
    assert_eq!(
        f.source.refresh(101, None).unwrap().unwrap().signature,
        first.signature
    );
    let original =
        std::fs::read(f.source.directory.join("wizard-declaration-signed.json")).unwrap();
    let checkpoint = f.checkpoint();
    f.source.refresh(102, None).unwrap();
    assert_eq!(checkpoint, f.checkpoint());
    let second = f.publish(2, 1500, registry::Support::Supported);
    assert_eq!(
        f.source.refresh(110, None).unwrap().unwrap().signature,
        second.signature
    );
    let checkpoint = f.checkpoint();
    let guard = store::Guard::open(&f.source.directory).unwrap();
    let old: Retained = serde_json::from_slice(&original).unwrap();
    guard
        .write_json(
            "wizard-declaration-signed.json",
            "wizard-declaration-signed.next",
            &old,
        )
        .unwrap();
    drop(guard);
    let restarted: DeclarationSource =
        serde_json::from_slice(&serde_json::to_vec(&f.source).unwrap()).unwrap();
    assert!(matches!(restarted.refresh(120, None), Err(Error::Conflict)));
    assert_eq!(checkpoint, f.checkpoint());
    let revoked = f.publish(3, 2000, registry::Support::Unknown);
    assert_eq!(
        restarted.refresh(130, None).unwrap().unwrap().signature,
        revoked.signature
    );
    assert_eq!(
        std::fs::read(f.source.directory.join("financial-sentinel")).unwrap(),
        b"unchanged"
    );
}

#[test]
fn metadata_source_expiry_is_durable_even_on_clock_rollback_and_higher_expired_revisions_advance_floor(
) {
    let f = Fixture::new();
    let first = f.publish(1, 1000, registry::Support::Supported);
    f.source.refresh(101, None).unwrap();
    std::fs::remove_file(f.source.directory.join("wizard-declaration-signed.json")).unwrap();
    assert!(f
        .source
        .refresh(110, Some((1, first.digest().unwrap())))
        .is_err());
    f.publish(1, 1000, registry::Support::Supported);
    assert!(f.source.clone().refresh(101, None).unwrap().is_none());
    let higher = f.publish(2, 200, registry::Support::Unknown);
    assert!(f.source.refresh(201, None).unwrap().is_none());
    assert!(f.source.refresh(150, None).unwrap().is_none());
    let cp: Checkpoint = serde_json::from_slice(&f.checkpoint()).unwrap();
    assert_eq!(cp.signed.signature, higher.signature);
    assert!(cp.expired);
}

#[test]
fn metadata_source_missing_invalid_conflict_and_checkpoint_failure_never_fall_back() {
    let f = Fixture::new();
    f.publish(1, 1000, registry::Support::Supported);
    f.source.refresh(101, None).unwrap();
    std::fs::remove_file(f.source.directory.join("wizard-declaration-signed.json")).unwrap();
    assert!(f.source.refresh(102, None).is_err());
    f.publish(1, 1200, registry::Support::Supported);
    assert!(f.source.refresh(103, None).is_err()); // same revision, different signed body
    f.publish(2, 1500, registry::Support::Supported);
    let held = store::Guard::open(&f.source.directory).unwrap();
    assert!(matches!(f.source.refresh(104, None), Err(Error::Busy)));
    drop(held);
    let checkpoint = f.source.directory.join("wizard-declaration-observed.json");
    std::fs::remove_file(&checkpoint).unwrap();
    symlink(f.source.directory.join("financial-sentinel"), &checkpoint).unwrap();
    assert!(f.source.refresh(105, None).is_err());
    assert_eq!(
        std::fs::read(f.source.directory.join("financial-sentinel")).unwrap(),
        b"unchanged"
    );
}
