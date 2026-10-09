#![cfg(unix)]
#[path = "setup/admission.rs"]
mod admission;
#[path = "setup/connection.rs"]
mod connection;
#[path = "setup/probe.rs"]
mod probes;
#[path = "setup/profile.rs"]
mod profile;
#[path = "setup/publication.rs"]
mod publication;
use mayhem_proto::proxy::{
    finance::{ProxyReceiptOutcome, ProxySettlementPolicy},
    *,
};
use mayhem_proxy::{
    attempts::Digest,
    discovery::Identity,
    endpoint::{Adapter, Limits},
    metering::Policy,
    setup::{Error, Input, Selection, State, Store},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
};

fn d(n: u8) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn private(path: &Path, bytes: &[u8]) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}
struct Fixture {
    dir: tempfile::TempDir,
    store: PathBuf,
    input: Input,
    connection: Value,
    listener: std::net::TcpListener,
}
impl Fixture {
    fn new(endpoint: ProxyEndpoint) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = dir.path().join("setup");
        std::fs::create_dir(&store).unwrap();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let connection = json!({"schema_version":1,"id":"fixture-private-connection","revision":1,
            "base_url":format!("http://{}/v1/",listener.local_addr().unwrap()),
            "authentication":{"type":"bearer","secret":{"source":"file","path":"never-read-secret"}},
            "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},
            "paths":{"chat_completions":"chat/completions","completions":"completions","responses":"responses","decisions":"decisions"}});
        let connection_file = dir.path().join("private-connection.json");
        private(&connection_file, &serde_json::to_vec(&connection).unwrap());
        let family = match endpoint {
            ProxyEndpoint::Chat => mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            ProxyEndpoint::Completions => mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
            ProxyEndpoint::Responses => mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
            ProxyEndpoint::Decisions => mayhem_proto::ENDPOINT_MAYHEM_DECISIONS,
        };
        let mut contract = mayhem_proto::endpoint_family_contract_template(family).unwrap();
        if endpoint == ProxyEndpoint::Chat {
            contract
                .request_attribute_specs
                .get_mut("temperature")
                .unwrap()
                .maximum = Some(0.5);
        }
        let adapter = Adapter::new(
            endpoint,
            contract,
            "private-upstream-model".into(),
            Limits {
                request_bytes: 65536,
                response_bytes: 65536,
                choices: 4,
                tools: 4,
                questions: 4,
                decision_options: 4,
            },
        )
        .unwrap();
        let market = ProxyMarketDescriptor {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            creator_pubkey: d(1).as_str().into(),
            slug: "test-setup".into(),
            model: ProxyModelClaim {
                family_id: "fixture".into(),
                model_id: "Declared public model".into(),
                revision: "".into(),
                quantization: "".into(),
            },
            family: endpoint.family(),
            endpoints: vec![ProxyEndpointContract {
                endpoint,
                contract_hash: adapter.contract_hash().as_str().into(),
            }],
            metering: Policy::for_endpoint(endpoint).contract(),
            pricing: ProxyPricing::ProviderOffers,
        };
        let membership = ProxyMembership {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            market_id: market.id().unwrap(),
            provider_pubkey: d(1).as_str().into(),
            revision: 1,
            endpoints: market.endpoints.clone(),
            served_context: 4096,
            max_concurrency: 2,
            recipe_hash: adapter.recipe_hash().as_str().into(),
            connection_revision: 1,
            capacity_group: d(2).as_str().into(),
            accepted_rails: vec![ProxyRail::Fiat, ProxyRail::Tap, ProxyRail::Tnk],
        };
        let offer = ProxyOffer {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            market_id: membership.market_id.clone(),
            provider_pubkey: membership.provider_pubkey.clone(),
            membership_revision: 1,
            revision: 1,
            endpoint,
            ctx_bracket: "ctx4k".into(),
            outcome_class: "".into(),
            metering_policy_hash: market.metering.policy_hash.clone(),
            rates: market
                .metering
                .units
                .iter()
                .map(|unit| ProxyRate {
                    unit: unit.clone(),
                    per_unit_au: 10,
                    granularity: 1,
                })
                .collect(),
            per_request_au: 1,
            min_session_au: 2,
            accepted_rails: membership.accepted_rails.clone(),
        };
        let input = Input {
            schema_version: 1,
            network: Identity {
                network_id: "local-test".into(),
                msb_bootstrap: d(3).as_str().into(),
                subnet_bootstrap: d(4).as_str().into(),
                contract_version: mayhem_proto::CONTRACT_VERSION,
            },
            provider_pubkey: d(1),
            connection_file,
            adapter: adapter.snapshot(),
            market,
            membership,
            offers: vec![offer],
            selection: Selection::CreateMarket,
            sequence: 1,
            settlement_policy: ProxySettlementPolicy {
                schema_version: 1,
                lane: ProxyLane::Proxy,
                payable_outcomes: vec![ProxyReceiptOutcome::Complete],
                allow_checkpoints: false,
                hold_expiry: None,
            },
        };
        Self {
            dir,
            store,
            input,
            connection,
            listener,
        }
    }
    fn store(&self) -> Store {
        Store::open(&self.store).unwrap()
    }
    fn no_network_or_secret(&self) {
        assert_eq!(
            self.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!self.dir.path().join("never-read-secret").exists());
    }
}

#[test]
fn four_endpoint_drafts_resume_public_review_without_claiming_payment_or_probes() {
    let mut public_reviews = Vec::new();
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let f = Fixture::new(endpoint);
        let store = f.store();
        let created = store.create(f.input.clone()).unwrap();
        assert_eq!(created.state, State::Unchecked);
        assert!(created.admission_handoff.is_none());
        let checked = store.check(1).unwrap();
        assert_eq!(checked.revision, 2);
        assert_eq!(checked.state, State::StructurallyValid);
        assert_eq!(checked.draft_id, created.draft_id);
        let handoff = checked.admission_handoff.as_ref().unwrap();
        assert_eq!(
            handoff.initial_operation_digest,
            handoff.initial_operation.digest().unwrap()
        );
        let value = serde_json::to_value(&checked).unwrap();
        let output = serde_json::to_string(&value).unwrap();
        assert_eq!(value["claim_status"], "operator_declared");
        assert_eq!(value["probe_status"], "not_run");
        assert_eq!(value["admission_status"], "not_checked");
        assert_eq!(value["publication_status"], "not_submitted");
        assert_eq!(value["serving_status"], "not_started");
        assert_eq!(value["contract"], json!(f.input.adapter.contract));
        for private in [
            "private-upstream-model",
            "private-connection.json",
            "never-read-secret",
            "127.0.0.1",
            "connection_file",
            "connection_fingerprint",
            "authentication",
        ] {
            assert!(!output.contains(private), "{private}");
        }
        assert_eq!(
            serde_json::to_value(f.store().inspect().unwrap()).unwrap(),
            value
        );
        public_reviews.push(value);
        assert_eq!(
            std::fs::metadata(f.store.join("draft.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(matches!(
            store.create(f.input.clone()),
            Err(Error::Conflict)
        ));
        f.no_network_or_secret();
    }
    if let Some(path) = std::env::var_os("MAYHEM_TEST_PROXY_SETUP_FIXTURE") {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(
            &serde_json::to_vec_pretty(
                &json!({"schema_version":1,"test_only":true,"reviews":public_reviews}),
            )
            .unwrap(),
        )
        .unwrap();
        file.sync_all().unwrap();
    }
}

#[test]
fn changed_connection_and_revision_require_explicit_update_and_recheck() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let store = f.store();
    store.create(f.input.clone()).unwrap();
    let first = store.check(1).unwrap();
    let original = first.admission_handoff.unwrap().initial_operation_digest;
    f.connection["revision"] = json!(2);
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    let stale = store.inspect().unwrap();
    assert_eq!(stale.state, State::RecheckRequired);
    assert!(stale.admission_handoff.is_none());
    assert!(store.check(2).is_err());
    assert_eq!(store.inspect().unwrap().revision, 2);
    f.input.membership.connection_revision = 2;
    let updated = store.update(2, f.input.clone()).unwrap();
    assert_eq!(updated.state, State::Unchecked);
    assert!(updated.admission_handoff.is_none());
    let checked = store.check(3).unwrap();
    assert_eq!(checked.draft_id, first.draft_id);
    assert_eq!(checked.revision, 4);
    assert_ne!(
        checked.admission_handoff.unwrap().initial_operation_digest,
        original
    );
    assert!(matches!(
        store.update(2, f.input.clone()),
        Err(Error::Conflict)
    ));
    let mut other = f.input.clone();
    other.network.network_id = "another-network".into();
    assert!(store.update(4, other).is_err());
    f.no_network_or_secret();
}

#[test]
fn concurrent_edits_have_one_cas_winner_and_never_reset_identity() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let original = f.store().create(f.input.clone()).unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|n| {
            let path = f.store.clone();
            let mut input = f.input.clone();
            input.offers[0].per_request_au += n;
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Store::open(path).and_then(|s| s.update(1, input))
            })
        })
        .collect();
    let mut success = 0;
    for handle in handles {
        match handle.join().unwrap() {
            Ok(v) => {
                success += 1;
                assert_eq!(v.revision, 2)
            }
            Err(Error::Conflict | Error::Busy) => (),
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!(success, 1);
    let latest = f.store().inspect().unwrap();
    assert_eq!(latest.draft_id, original.draft_id);
    assert_eq!(latest.state, State::Unchecked);
}

#[test]
fn partial_temp_never_becomes_a_draft_and_corrupt_authority_is_not_recreated() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let store = f.store();
    let original = store.create(f.input.clone()).unwrap();
    private(&f.store.join("draft.next"), b"{incomplete");
    assert_eq!(store.inspect().unwrap().revision, original.revision);
    store.update(1, f.input.clone()).unwrap();
    assert!(!f.store.join("draft.next").exists());
    private(&f.store.join("draft.json"), b"{corrupt");
    assert!(matches!(store.inspect(), Err(Error::Invalid)));
    assert!(store.create(f.input.clone()).is_err());
    assert_eq!(
        std::fs::read(f.store.join("draft.json")).unwrap(),
        b"{corrupt"
    );
}

#[test]
fn unsafe_permissions_symlinks_hardlinks_and_oversized_files_fail_closed() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    std::fs::set_permissions(&f.store, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(Store::open(&f.store), Err(Error::Protection)));
    std::fs::set_permissions(&f.store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let alias = f.dir.path().join("alias");
    symlink(&f.store, &alias).unwrap();
    assert!(Store::open(alias).is_err());
    let store = f.store();
    store.create(f.input.clone()).unwrap();
    std::fs::hard_link(f.store.join("draft.json"), f.dir.path().join("hardlink")).unwrap();
    assert!(store.inspect().is_err());
    std::fs::remove_file(f.dir.path().join("hardlink")).unwrap();
    std::fs::set_permissions(
        f.store.join("draft.json"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(store.inspect().is_err());
    std::fs::remove_file(f.store.join("draft.json")).unwrap();
    symlink(&f.input.connection_file, f.store.join("draft.json")).unwrap();
    assert!(store.inspect().is_err());
    let input = f.dir.path().join("input.json");
    private(&input, &vec![b' '; mayhem_proxy::setup::MAX_BYTES + 1]);
    assert!(Input::load(&input).is_err());
    std::fs::remove_file(f.store.join("draft.lock")).unwrap();
    symlink(&f.input.connection_file, f.store.join("draft.lock")).unwrap();
    assert!(Store::open(&f.store).is_err());
    f.no_network_or_secret();
}

#[test]
fn substituted_identity_contract_recipe_and_unsupported_families_are_rejected() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let mut input = f.input.clone();
    input.membership.recipe_hash = d(99).as_str().into();
    assert!(input.validate().is_err());
    input = f.input.clone();
    input
        .adapter
        .contract
        .request_attribute_specs
        .get_mut("temperature")
        .unwrap()
        .maximum = Some(0.25);
    assert!(input.validate().is_err());
    input = f.input.clone();
    input.provider_pubkey = d(99);
    assert!(input.validate().is_err());
    input = f.input.clone();
    input.offers[0].rates.clear();
    assert!(input.validate().is_err());
    input = f.input.clone();
    input.network.contract_version = 0;
    assert!(input.validate().is_err());
    input = f.input.clone();
    input.offers.push(input.offers[0].clone());
    assert!(input.validate().is_err());
    let mut value = json!(f.input);
    value["adapter"]["endpoint"] = json!("openai_embeddings");
    assert!(serde_json::from_value::<Input>(value).is_err());
    let input_file = f.dir.path().join("relative-input.json");
    let mut value = json!(f.input);
    value["connection_file"] = json!("private-connection.json");
    private(&input_file, &serde_json::to_vec(&value).unwrap());
    let restored = Input::load(&input_file).unwrap();
    assert_eq!(
        restored.connection_file,
        std::fs::canonicalize(&f.input.connection_file).unwrap()
    );
    f.no_network_or_secret();
}
