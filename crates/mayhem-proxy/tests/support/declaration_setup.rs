use super::*;
use ed25519_dalek::SigningKey;
use mayhem_proxy::{
    attempts::{Digest, Identity},
    setup::{DeclarationChoice, ProfileInput, Store},
    signing::Authority,
};
use std::os::unix::fs::PermissionsExt;

async fn published() -> (Mock, Definitions) {
    let mock = Mock::new().await;
    let mut doc = definition("privacy.training");
    doc.definition = serde_json::from_value(json!({"schema_version":1,"field_id":"privacy.training","schema_revision":1,
        "labels":{"en":"Training use"},"help":{},"units":null,"group":"privacy","order":0,
        "endpoints":["mayhem_decisions","openai_chat_completions","openai_completions","openai_responses"],
        "value_schema":{"type":"boolean"},"default":null,"operators":["eq"],
        "minimum_assurance":"declared","max_evidence_age_ms":1000,"usage":{"kind":"filter_only"},"rules":[]})).unwrap();
    doc.definition_hash = doc.definition.digest().unwrap();
    let release = release(None, 1, &[doc.clone()]);
    {
        let mut state = mock.state.lock().unwrap();
        state.head = json!(release);
        state.add(&release, &[doc.clone()]);
    }
    let reader = Reader::new(
        TrustedOrigin::local_loopback_http(&mock.origin).unwrap(),
        Limits::default(),
    )
    .unwrap();
    let pin = reader.current().await.unwrap();
    let definitions = reader.lookup_exact(&pin, &[doc.reference()]).await.unwrap();
    (mock, definitions)
}
fn choices(value: bool) -> Vec<DeclarationChoice> {
    vec![DeclarationChoice {
        field_id: "privacy.training".into(),
        schema_revision: 1,
        status: Support::Supported,
        value: Some(TypedValue::Boolean(value)),
    }]
}
struct Setup {
    dir: tempfile::TempDir,
    store: Store,
    authority: Authority,
    profile: ProfileInput,
}
fn setup(endpoint: ProxyEndpoint) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.path().join("connection.json");
    std::fs::write(&file,serde_json::to_vec(&json!({"schema_version":1,"id":"decl-fixture","revision":1,
        "base_url":"http://127.0.0.1:1/v1/","authentication":{"type":"none"},
        "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},
        "paths":{"chat_completions":"chat/completions","completions":"completions","responses":"responses","decisions":"decisions"}})).unwrap()).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let key = SigningKey::from_bytes(&[77; 32]);
    let provider = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let profile: ProfileInput = serde_json::from_value(json!({"schema_version":1,
        "network":{"network_id":"fixture","msb_bootstrap":"03".repeat(32),"subnet_bootstrap":"04".repeat(32),"contract_version":mayhem_proto::CONTRACT_VERSION},
        "provider_pubkey":provider,"connection_file":file,"profile":{"kind":"standard","endpoint":endpoint},
        "upstream_model":"fixture","limits":{"request_bytes":65536,"response_bytes":65536,"choices":4,"tools":4,"questions":4,"decision_options":4},
        "market":{"action":"create_market","slug":"declarations","model":{"family_id":"fixture","model_id":"fixture","revision":"","quantization":""}},
        "membership":{"revision":1,"served_context":4096,"max_concurrency":1,"capacity_group":"05".repeat(32),"accepted_rails":["fiat"]},
        "offers":[{"revision":1,"ctx_bracket":"ctx4096","outcome_class":"",
        "rates":mayhem_proxy::metering::Policy::for_endpoint(endpoint).contract().units.into_iter().map(|unit|json!({"unit":unit,"per_unit_au":"1","granularity":1})).collect::<Vec<_>>(),
        "per_request_au":"0","min_session_au":"0","accepted_rails":["fiat"]}],"sequence":1,
        "settlement_policy":{"schema_version":1,"lane":"proxy","payable_outcomes":["complete"],"allow_checkpoints":false}})).unwrap();
    let authority = Authority::from_unlocked_wallet(
        key,
        Identity {
            network_id: "fixture".into(),
            msb_bootstrap: Digest::new("03".repeat(32)).unwrap(),
            subnet_bootstrap: Digest::new("04".repeat(32)).unwrap(),
            controller_pubkey: Digest::new(provider).unwrap(),
        },
    )
    .unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(profile.clone(), None).unwrap();
    Setup {
        dir,
        store,
        authority,
        profile,
    }
}

#[tokio::test]
async fn declaration_setup_reviews_signs_and_recovers_exact_original_for_every_endpoint() {
    let (_mock, defs) = published().await;
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let s = setup(endpoint);
        let draft_before = std::fs::read(s.dir.path().join("draft.json")).unwrap();
        let p = s
            .store
            .plan_data_handling(1, 0, &defs, choices(false), 100, 1100)
            .unwrap();
        assert_eq!(p.state, "needs_confirmation");
        assert!(p.signed.is_none());
        assert!(s.store.inspect_data_handling(100).unwrap().is_none());
        let signed = s
            .store
            .confirm_data_handling(1, &p.plan.plan_digest, &s.authority, 101)
            .unwrap();
        assert_eq!(signed.state, "signed_not_installed");
        assert!(!signed.installed_in_runtime);
        assert_eq!(signed.assurance, "declared_not_verified");
        let original = serde_json::to_vec(&signed.signed).unwrap();
        let restarted = Store::open(s.dir.path()).unwrap();
        let replay = restarted
            .confirm_data_handling(1, &p.plan.plan_digest, &s.authority, 1200)
            .unwrap();
        assert_eq!(replay.state, "expired");
        assert_eq!(serde_json::to_vec(&replay.signed).unwrap(), original);
        assert_eq!(
            std::fs::read(s.dir.path().join("draft.json")).unwrap(),
            draft_before
        );
        let signed = signed.signed.unwrap();
        let predicate = Predicate {
            field_id: "privacy.training".into(),
            schema_revision: 1,
            operator: Operator::Eq,
            value: TypedValue::Boolean(false),
            evidence: Assurance::Declared,
            max_age_ms: None,
        };
        assert_eq!(
            signed
                .evaluate(defs.get("privacy.training", 1).unwrap(), &predicate, 101)
                .unwrap(),
            Match::Satisfied
        );
    }
}
#[tokio::test]
async fn declaration_setup_renewal_never_overwrites_last_signed_until_confirmed() {
    let (_mock, defs) = published().await;
    let s = setup(ProxyEndpoint::Chat);
    let p = s
        .store
        .plan_data_handling(1, 0, &defs, choices(false), 100, 1100)
        .unwrap();
    s.store
        .confirm_data_handling(1, &p.plan.plan_digest, &s.authority, 101)
        .unwrap();
    let original = std::fs::read(s.dir.path().join("wizard-declaration-signed.json")).unwrap();
    let next = s
        .store
        .plan_data_handling(1, 1, &defs, choices(true), 500, 1500)
        .unwrap();
    assert_eq!(next.plan.body.revision, 2);
    assert_eq!(
        std::fs::read(s.dir.path().join("wizard-declaration-signed.json")).unwrap(),
        original
    );
    assert!(s
        .store
        .plan_data_handling(1, 0, &defs, choices(true), 500, 1500)
        .is_err());
    assert!(s
        .store
        .confirm_data_handling(1, &Digest::new("09".repeat(32)).unwrap(), &s.authority, 501)
        .is_err());
    let replaced = s
        .store
        .confirm_data_handling(1, &next.plan.plan_digest, &s.authority, 501)
        .unwrap();
    assert_eq!(
        replaced.signed.unwrap().body.claims[0].value,
        Some(TypedValue::Boolean(true))
    );
    // A renewed declaration cannot be overwritten by replaying an old confirmation.
    assert!(s
        .store
        .confirm_data_handling(1, &p.plan.plan_digest, &s.authority, 502)
        .is_err());
    assert_eq!(
        s.store
            .inspect_data_handling(502)
            .unwrap()
            .unwrap()
            .plan
            .body
            .revision,
        2
    );
}
#[tokio::test]
async fn declaration_setup_rejects_bad_fields_and_changed_route_without_signing_or_money_changes() {
    let (_mock, defs) = published().await;
    let mut s = setup(ProxyEndpoint::Chat);
    let before = std::fs::read(s.dir.path().join("draft.json")).unwrap();
    let mut unknown = choices(false);
    unknown[0].field_id = "privacy.not_published".into();
    let mut wrong_type = choices(false);
    wrong_type[0].value = Some(TypedValue::Text("false".into()));
    let mut unsupported = choices(false);
    unsupported[0].status = Support::Unsupported;
    for bad in [
        unknown,
        wrong_type,
        unsupported,
        vec![choices(false)[0].clone(), choices(false)[0].clone()],
    ] {
        assert!(s
            .store
            .plan_data_handling(1, 0, &defs, bad, 100, 1100)
            .is_err());
    }
    assert_eq!(
        std::fs::read(s.dir.path().join("draft.json")).unwrap(),
        before
    );
    let p = s
        .store
        .plan_data_handling(1, 0, &defs, choices(false), 100, 1100)
        .unwrap();
    assert!(s
        .store
        .confirm_data_handling(1, &p.plan.plan_digest, &s.authority, 1100)
        .is_err());
    let wrong = Authority::from_unlocked_wallet(
        SigningKey::from_bytes(&[78; 32]),
        Identity {
            controller_pubkey: Digest::new(
                SigningKey::from_bytes(&[78; 32])
                    .verifying_key()
                    .to_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
            )
            .unwrap(),
            ..s.authority.identity().clone()
        },
    )
    .unwrap();
    assert!(s
        .store
        .confirm_data_handling(1, &p.plan.plan_digest, &wrong, 101)
        .is_err());
    assert!(!s.dir.path().join("wizard-declaration-signed.json").exists());
    s.profile.membership.revision = 2;
    s.profile.offers[0].revision = 2;
    let updated = s.store.prepare(s.profile.clone(), Some(1)).unwrap();
    assert_eq!(updated.revision, 2);
    assert!(s
        .store
        .confirm_data_handling(2, &p.plan.plan_digest, &s.authority, 101)
        .is_err());
    assert!(!s.dir.path().join("wizard-declaration-signed.json").exists());
}
