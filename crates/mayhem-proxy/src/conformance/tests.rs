use super::*;
use ed25519_dalek::SigningKey;
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
fn d(n: u64) -> Digest {
    hash_value(format!("{n:064x}")).unwrap()
}
fn signer() -> Arc<crate::signing::Authority> {
    let key = SigningKey::from_bytes(&[91; 32]);
    let public = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Arc::new(
        crate::signing::Authority::from_unlocked_wallet(
            key,
            crate::attempts::Identity {
                network_id: "918".into(),
                msb_bootstrap: d(1),
                subnet_bootstrap: d(2),
                controller_pubkey: hash_value(public).unwrap(),
            },
        )
        .unwrap(),
    )
}
fn network() -> discovery::Identity {
    discovery::Identity {
        network_id: "918".into(),
        msb_bootstrap: d(1).as_str().into(),
        subnet_bootstrap: d(2).as_str().into(),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}
fn config() -> Config {
    Config {
        schema_version: 1,
        tester: signer().identity().controller_pubkey.clone(),
        ttl_ms: 60_000,
        maximum_records: 16,
        maximum_bytes: MAX_RECORD_BYTES as u64 * 16,
        minimum_interval_tokens: 2,
        minimum_interval_us: 1,
        mappings: vec![],
        tokenizer: None,
    }
}
#[cfg(unix)]
fn directory() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}
#[cfg(windows)]
struct PrivateDirectory(std::path::PathBuf);
#[cfg(windows)]
impl PrivateDirectory {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
#[cfg(windows)]
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[cfg(windows)]
fn directory() -> PrivateDirectory {
    use mayhem_windows_sandbox::{LeafName, NtfsDirectory};
    let parent = std::path::PathBuf::from(
        std::env::var_os("MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT")
            .expect("provide an isolated existing private NTFS test directory"),
    );
    let mut guard = NtfsDirectory::open_existing(&parent)
        .unwrap()
        .into_lock()
        .unwrap();
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let name = format!("conformance-test-{}", blake3::hash(&random).to_hex());
    let mut pending = guard
        .stage_directory(LeafName::new(&format!("{name}.next")).unwrap(), &[], 0)
        .unwrap();
    pending.publish(LeafName::new(&name).unwrap()).unwrap();
    PrivateDirectory(parent.join(name))
}
fn body(store: &Store) -> Body {
    let now = store.now_ms();
    Body {
        schema_version: 1,
        network: network(),
        tester: store.config.tester.clone(),
        provenance: Provenance::GatewayObservation,
        boot: store.boot.clone(),
        configuration: store.configuration.clone(),
        suite: SUITE.into(),
        subject: Subject {
            offer_digest: d(10),
            provider: d(11),
            membership_revision: 1,
            endpoint: ProxyEndpoint::Chat,
            endpoint_contract: d(12),
            recipe_hash: d(13),
            connection_revision: 1,
        },
        class: Class::request(
            &json!({"model":"public","messages":[{"role":"user","content":"hello"}],"stream":true}),
        )
        .unwrap(),
        session: d(20),
        request_hash: d(21),
        connection_digest: d(22),
        result_digest: d(23),
        observed_at_ms: now,
        expires_at_ms: now + store.config.ttl_ms,
        assertions: vec![Assertion::ValidEndpointOutput, Assertion::ValidatedStream],
        speed: None,
    }
}
fn definition() -> registry::Definition {
    serde_json::from_value(json!({"schema_version":1,"field_id":"fixture.stream","schema_revision":1,
    "labels":{"en":"Observed stream"},"help":{},"units":null,"group":"protocol","order":0,"endpoints":["openai_chat_completions"],
    "value_schema":{"type":"boolean"},"default":null,"operators":["eq"],"minimum_assurance":"probed","max_evidence_age_ms":60000,
    "usage":{"kind":"filter_only"},"rules":[]})).unwrap()
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires an isolated native Windows private NTFS fixture parent"
)]
fn signed_exact_index_binding_rejects_mutations_unknown_fields_and_request_class_changes() {
    let dir = directory();
    let store = Store::open(&dir.path().join("records"), network(), config()).unwrap();
    let b = body(&store);
    let signed = signer().conformance(b.clone()).unwrap();
    store.retain(signed.clone()).unwrap();
    assert!(matches!(
        store.lookup(&b.subject, &b.class).unwrap(),
        Lookup::Present(_)
    ));
    for field in [
        "boot",
        "configuration",
        "session",
        "request_hash",
        "connection_digest",
        "result_digest",
    ] {
        let mut wire = serde_json::to_value(&signed).unwrap();
        wire["body"][field] = json!(d(999));
        let changed: Signed = serde_json::from_value(wire).unwrap();
        assert!(changed.verify().is_err(), "{field}");
    }
    let mut extra = serde_json::to_value(&signed).unwrap();
    extra["body"]["trust"] = json!("verified");
    assert!(serde_json::from_value::<Signed>(extra).is_err());
    let mut c = b.class.clone();
    c.shape_digest = d(9);
    assert!(matches!(
        store.lookup(&b.subject, &c).unwrap(),
        Lookup::Missing
    ));
    for field in [
        "offer_digest",
        "provider",
        "endpoint_contract",
        "recipe_hash",
    ] {
        let mut subject = serde_json::to_value(&b.subject).unwrap();
        subject[field] = json!(d(88));
        assert!(matches!(
            store
                .lookup(&serde_json::from_value(subject).unwrap(), &b.class)
                .unwrap(),
            Lookup::Missing
        ));
    }
    let mut missing = serde_json::to_value(&signed).unwrap();
    missing["body"].as_object_mut().unwrap().remove("speed");
    assert!(serde_json::from_value::<Signed>(missing).is_err());
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires an isolated native Windows private NTFS fixture parent"
)]
fn restart_and_changed_configuration_never_restore_live_samples_and_replay_does_not_renew() {
    let dir = directory();
    let path = dir.path().join("records");
    let store = Store::open(&path, network(), config()).unwrap();
    let b = body(&store);
    let original = signer().conformance(b.clone()).unwrap();
    store.retain(original.clone()).unwrap();
    let mut changed = b.clone();
    changed.observed_at_ms += 10;
    changed.expires_at_ms += 10;
    store
        .retain(signer().conformance(changed).unwrap())
        .unwrap();
    let Lookup::Present(retained) = store.lookup(&b.subject, &b.class).unwrap() else {
        panic!()
    };
    assert_eq!(retained.digest().unwrap(), original.digest().unwrap());
    drop(store);
    let store = Store::open(&path, network(), config()).unwrap();
    assert!(matches!(
        store.lookup(&b.subject, &b.class).unwrap(),
        Lookup::Restarted
    ));
    drop(store);
    let mut changed = config();
    changed.minimum_interval_tokens = 3;
    let store = Store::open(&path, network(), changed).unwrap();
    assert!(matches!(
        store.lookup(&b.subject, &b.class).unwrap(),
        Lookup::ConfigurationChanged
    ));
    drop(store);
    let mut wrong = network();
    wrong.network_id = "other".into();
    assert!(Store::open(&path, wrong, config()).is_err());
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires an isolated native Windows private NTFS fixture parent"
)]
fn expiry_quotas_and_indexed_pruning_are_bounded() {
    let dir = directory();
    let mut c = config();
    c.maximum_records = 1;
    c.ttl_ms = 1000;
    let store = Store::open(&dir.path().join("records"), network(), c).unwrap();
    let b = body(&store);
    store
        .retain(signer().conformance(b.clone()).unwrap())
        .unwrap();
    let mut second = b.clone();
    second.subject.offer_digest = d(77);
    second.session = d(44);
    assert!(
        store
            .retain(signer().conformance(second.clone()).unwrap())
            .is_err()
    );
    std::thread::sleep(Duration::from_millis(1100));
    assert!(matches!(
        store.lookup(&b.subject, &b.class).unwrap(),
        Lookup::Expired
    ));
    second.observed_at_ms = store.now_ms();
    second.expires_at_ms = second.observed_at_ms + 1000;
    store.retain(signer().conformance(second).unwrap()).unwrap();
    assert!(matches!(
        store.lookup(&b.subject, &b.class).unwrap(),
        Lookup::Missing
    ));
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires an isolated native Windows private NTFS fixture parent"
)]
fn exact_mapping_does_not_grant_t4_self_test_assurance_or_unknown_assertions() {
    let dir = directory();
    let def = definition();
    let mut c = config();
    c.mappings.push(Mapping {
        field_id: def.field_id.clone(),
        schema_revision: 1,
        definition_digest: hash_value(def.digest().unwrap()).unwrap(),
        assertion: Assertion::ValidatedStream,
    });
    let store = Store::open(&dir.path().join("records"), network(), c).unwrap();
    let mut b = body(&store);
    let mut predicate:registry::Predicate=serde_json::from_value(json!({"field_id":def.field_id,"schema_revision":1,"operator":"eq","value":{"type":"boolean","value":true},"evidence":"probed","max_age_ms":60000})).unwrap();
    assert_eq!(
        store
            .evaluate(&def, &predicate, &signer().conformance(b.clone()).unwrap())
            .unwrap(),
        registry::Match::Satisfied
    );
    predicate.evidence = registry::Assurance::Verified;
    assert_eq!(
        store
            .evaluate(&def, &predicate, &signer().conformance(b.clone()).unwrap())
            .unwrap(),
        registry::Match::InsufficientEvidence
    );
    predicate.evidence = registry::Assurance::Probed;
    b.provenance = Provenance::ProviderSelfTest;
    assert_eq!(
        store
            .evaluate(&def, &predicate, &signer().conformance(b.clone()).unwrap())
            .unwrap(),
        registry::Match::InsufficientEvidence
    );
    b.provenance = Provenance::GatewayObservation;
    b.assertions = vec![Assertion::ValidEndpointOutput];
    assert_eq!(
        store
            .evaluate(&def, &predicate, &signer().conformance(b.clone()).unwrap())
            .unwrap(),
        registry::Match::Unknown
    );
    let mut other = def.clone();
    other
        .help
        .insert("en".into(), "different pinned meaning".into());
    assert!(
        store
            .evaluate(
                &other,
                &predicate,
                &signer().conformance(b.clone()).unwrap()
            )
            .is_err()
    );
    b.tester = d(77);
    let mut false_signed = signer().conformance(body(&store)).unwrap();
    false_signed.body = b;
    assert!(store.evaluate(&def, &predicate, &false_signed).is_err());
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires an isolated native Windows private NTFS fixture parent"
)]
fn speed_ranking_is_exact_rational_comparable_and_never_billing_usage() {
    let mut a = Speed {
        tokenizer: d(1),
        interval_tokens: 9_007_199_254_740_990,
        interval_us: 9_007_199_254_740_989,
        samples: 1,
        first_output_ms: 20,
        total_ms: 300,
        local_concurrency: 1,
        remote_cache: "unknown".into(),
        remote_concurrency: "unknown".into(),
    };
    let mut b = a.clone();
    b.interval_tokens -= 1;
    assert!(a.faster_than(&b));
    assert!(!b.faster_than(&a));
    assert!(a.comparable(&b));
    b.tokenizer = d(2);
    assert!(!a.comparable(&b));
    b.tokenizer = a.tokenizer.clone();
    b.local_concurrency = 2;
    assert!(!a.comparable(&b));
    a.remote_cache = "warm".into();
    assert!(!a.comparable(&b));
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires an isolated native Windows private NTFS fixture parent"
)]
fn request_classes_bind_custom_controls_input_structure_and_full_schema() {
    let plain = json!({"model":"first","messages":[{"role":"user","content":"hello"}],"stream":true,"vendor_mode":"fast"});
    let class = Class::request(&plain).unwrap();
    let mut changed = plain.clone();
    changed["model"] = json!("another-exact-selector");
    assert_eq!(Class::request(&changed).unwrap(), class);
    changed["vendor_mode"] = json!("slow");
    assert_ne!(
        Class::request(&changed).unwrap().shape_digest,
        class.shape_digest
    );
    changed = plain.clone();
    changed["messages"][0]["content"] =
        json!([{"type":"image_url","image_url":{"url":"https://test.invalid/picture"}}]);
    assert_ne!(
        Class::request(&changed).unwrap().shape_digest,
        class.shape_digest
    );
    changed = plain.clone();
    changed["messages"][0]["role"] = json!("assistant");
    assert_ne!(
        Class::request(&changed).unwrap().shape_digest,
        class.shape_digest
    );
    let too_many = json!({"messages":vec![json!({"role":"user","content":"x"});2000]});
    assert!(Class::request(&too_many).is_err());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires an isolated native Windows private NTFS fixture parent"
)]
fn forward_then_backward_wall_time_cannot_resurrect_or_freeze_a_lease() {
    let dir = directory();
    let store = Store::open(&dir.path().join("records"), network(), config()).unwrap();
    let original = body(&store);
    store
        .retain(signer().conformance(original.clone()).unwrap())
        .unwrap();
    let future = original.expires_at_ms + 1000;
    assert!(store.effective_time(future) >= future);
    assert!(matches!(
        store.lookup(&original.subject, &original.class).unwrap(),
        Lookup::Expired
    ));
    let before = store.effective_time(original.observed_at_ms);
    std::thread::sleep(Duration::from_millis(5));
    assert!(
        store.effective_time(original.observed_at_ms) >= before + 1,
        "advanced clock must continue to age monotonically after rollback"
    );
}
