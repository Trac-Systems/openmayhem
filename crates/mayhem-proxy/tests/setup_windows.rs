//! Native Windows acceptance: explicit isolated private parent, no paid/network work.
#![cfg(windows)]
use mayhem_proto::proxy::{
    finance::{ProxyReceiptOutcome, ProxySettlementPolicy},
    *,
};
use mayhem_proxy::{
    attempts::Digest,
    discovery::Identity,
    metering::Policy,
    setup::{
        bootstrap::{self, Choices, Credential, Host},
        Error, FlowConfig, OfferInput, ProfileMarket, Store,
    },
};
use mayhem_windows_sandbox::{DirectoryEntry, LeafName, NtfsDirectory};
use std::path::PathBuf;

fn d(n: u8) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn leaf(v: &str) -> LeafName {
    LeafName::new(v).unwrap()
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = PathBuf::from(
            std::env::var_os("MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT")
                .expect("provide a dedicated existing current-owner private NTFS directory"),
        );
        let mut guard = NtfsDirectory::open_existing(&parent)
            .unwrap()
            .into_lock()
            .unwrap();
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let name = format!("fixture-{}", blake3::hash(&nonce).to_hex());
        let bridge = [leaf("bridge-token")];
        let entries = [DirectoryEntry::File {
            path: &bridge,
            bytes: b"synthetic-bridge-token",
        }];
        let mut staged = guard
            .stage_directory(leaf(&format!("{name}.next")), &entries, 1024)
            .unwrap();
        staged.publish(leaf(&name)).unwrap();
        Self(parent.join(name))
    }
    fn host(&self) -> Host {
        Host {
            network: Identity {
                network_id: "windows-local-fixture".into(),
                msb_bootstrap: d(3).as_str().into(),
                subnet_bootstrap: d(4).as_str().into(),
                contract_version: mayhem_proto::CONTRACT_VERSION,
            },
            provider_pubkey: d(1),
            peer_rpc: "http://127.0.0.1:1/".into(),
            bridge_url: "ws://127.0.0.1:1/".into(),
            bridge_token_file: self.0.join("bridge-token"),
            worker_program: std::env::current_exe().unwrap(),
            wallet_password_file: None,
            admission_origin: None,
            declaration_registry: None,
        }
    }
    fn choices(&self) -> Choices {
        Choices {
            base_url: "http://127.0.0.1:1/v1/".into(),
            network_policy: serde_json::from_value(
                serde_json::json!({"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true}),
            )
            .unwrap(),
            credential: Credential::BearerValue(zeroize::Zeroizing::new(
                b"synthetic-upstream-key".to_vec(),
            )),
            endpoint: ProxyEndpoint::Decisions,
            upstream_model: "synthetic-upstream".into(),
            market: ProfileMarket::CreateMarket {
                slug: "native-storage-fixture".into(),
                model: ProxyModelClaim {
                    family_id: "fixture".into(),
                    model_id: "Synthetic public model".into(),
                    revision: "".into(),
                    quantization: "".into(),
                },
            },
            served_context: 4096,
            concurrency: 2,
            offers: vec![OfferInput {
                revision: 1,
                ctx_bracket: "ctx4k".into(),
                outcome_class: "".into(),
                rates: Policy::for_endpoint(ProxyEndpoint::Decisions)
                    .contract()
                    .units
                    .into_iter()
                    .map(|unit| ProxyRate {
                        unit,
                        per_unit_au: 123,
                        granularity: 1000,
                    })
                    .collect(),
                per_request_au: 2,
                min_session_au: 3,
                accepted_rails: vec![ProxyRail::Fiat],
            }],
            accepted_rails: vec![ProxyRail::Fiat],
            sequence: 1,
            settlement_policy: ProxySettlementPolicy {
                schema_version: 1,
                lane: ProxyLane::Proxy,
                payable_outcomes: vec![ProxyReceiptOutcome::Complete],
                allow_checkpoints: false,
                hold_expiry: None,
            },
            probe_budget: mayhem_proxy::capacity::probes::Budget {
                max_attempts: 2,
                max_cost_microusd: 20,
                per_attempt_cost_microusd: 10,
            },
            probe_output_limit: 32,
            probe_timeout_ms: 3000,
            allow_recovery_probes: false,
            tokenizer: None,
            closed_retention_ms: 86_400_000,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires an isolated native Windows private NTFS fixture parent"]
fn windows_setup_actual_factory_store_restart_cas_and_unpublished_slot_recovery() {
    let f = Fixture::new();
    let target = f.0.join("bundle");
    let bundle = bootstrap::create(&target, f.host(), f.choices()).unwrap();
    assert!(
        !bundle.capacity_created
            && !bundle.authorizes_run
            && !bundle.authorizes_probe
            && !bundle.authorizes_publication
    );
    assert_eq!(bundle.network_requests, 0);
    let config = FlowConfig::load(&bundle.config_file).unwrap();
    let input = config.profile.prepare().unwrap();
    let store = Store::open(&config.directory).unwrap();
    let created = store.create(input.clone()).unwrap();
    let checked = store.check(1).unwrap();
    assert_eq!(checked.draft_id, created.draft_id);
    assert_eq!(checked.revision, 2);
    let public = serde_json::to_string(&checked).unwrap();
    assert!(!public.contains("synthetic-upstream-key"));
    assert!(!public.contains("synthetic-upstream"));
    assert!(!target.join("runtime").join("capacity.redb").exists());
    let mut guard = NtfsDirectory::open_existing(&config.directory)
        .unwrap()
        .into_lock()
        .unwrap();
    drop(
        guard
            .prepare(leaf("draft.next"), b"{interrupted", 64)
            .unwrap(),
    );
    assert!(matches!(Store::open(&config.directory), Err(Error::Busy)));
    drop(guard);
    let restored = Store::open(&config.directory).unwrap();
    assert_eq!(restored.inspect().unwrap().revision, 2);
    let changed = restored.update(2, input.clone()).unwrap();
    assert_eq!(changed.draft_id, created.draft_id);
    assert_eq!(changed.revision, 3);
    assert!(!config.directory.join("draft.next").exists());
    assert!(matches!(restored.update(2, input), Err(Error::Conflict)));
    assert_eq!(
        Store::open(&config.directory)
            .unwrap()
            .inspect()
            .unwrap()
            .revision,
        3
    );
}

#[test]
#[ignore = "requires an isolated native Windows private NTFS fixture parent"]
fn windows_setup_actual_factory_never_overwrites_and_invalid_input_never_publishes() {
    let f = Fixture::new();
    let target = f.0.join("bundle");
    let bundle = bootstrap::create(&target, f.host(), f.choices()).unwrap();
    let original = std::fs::read(&bundle.config_file).unwrap();
    assert!(matches!(
        bootstrap::create(&target, f.host(), f.choices()),
        Err(Error::Conflict)
    ));
    assert_eq!(std::fs::read(&bundle.config_file).unwrap(), original);
    let mut invalid = f.choices();
    invalid.concurrency = 0;
    assert!(bootstrap::create(&f.0.join("invalid"), f.host(), invalid).is_err());
    assert!(!f.0.join("invalid").exists());
    // A valid plan with an absent protected reference must fail full validation.
    let mut host = f.host();
    host.bridge_token_file = f.0.join("missing-bridge");
    assert!(bootstrap::create(&f.0.join("unvalidated"), host, f.choices()).is_err());
    assert!(!f.0.join("unvalidated").exists());
    assert!(!f.0.join("missing-bridge").exists());
}
