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

#[tokio::test]
async fn declaration_withdrawal_is_explicit_signed_unknown_and_preserves_financial_draft() {
    let (mock, defs) = published().await;
    let s = setup(ProxyEndpoint::Chat);
    let initial=s.store.plan_data_handling(1,0,&defs,choices(false),100,1000).unwrap();
    s.store.confirm_data_handling(1,&initial.plan.plan_digest,&s.authority,101).unwrap();
    let draft=std::fs::read(s.dir.path().join("draft.json")).unwrap();
    let original=std::fs::read(s.dir.path().join("wizard-declaration-signed.json")).unwrap();
    mock.state.lock().unwrap().status=503;
    let p=s.store.plan_declaration_withdrawal(1,1,200,1200).unwrap();
    assert_eq!(original,std::fs::read(s.dir.path().join("wizard-declaration-signed.json")).unwrap());
    assert_eq!(p.plan.body.revision,2);
    assert!(p.plan.body.claims.iter().all(|c|c.status==Support::Unknown && c.value.is_none()));
    let signed=s.store.confirm_data_handling(1,&p.plan.plan_digest,&s.authority,201).unwrap();
    assert_eq!(signed.signed.as_ref().unwrap().body.revision,2);
    assert_eq!(draft,std::fs::read(s.dir.path().join("draft.json")).unwrap());
    assert!(s.store.plan_declaration_withdrawal(1,1,300,1300).is_err());
}

#[tokio::test]
async fn declaration_authoring_uses_observed_revision_after_old_source_restore() {
    let (_mock, defs)=published().await;
    let s=setup(ProxyEndpoint::Chat);
    let p=s.store.plan_data_handling(1,0,&defs,choices(false),100,1000).unwrap();
    s.store.confirm_data_handling(1,&p.plan.plan_digest,&s.authority,101).unwrap();
    let original=std::fs::read(s.dir.path().join("wizard-declaration-signed.json")).unwrap();
    let p2=s.store.plan_data_handling(1,1,&defs,choices(true),200,1200).unwrap();
    let second=s.store.confirm_data_handling(1,&p2.plan.plan_digest,&s.authority,201).unwrap();
    let checkpoint=s.dir.path().join("wizard-declaration-observed.json");
    std::fs::write(&checkpoint,serde_json::to_vec(&json!({"schema_version":1,"draft_id":second.plan.draft_id,"signed":second.signed,"expired":false})).unwrap()).unwrap();
    std::fs::set_permissions(&checkpoint,std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(s.store.inspect_data_handling(300).unwrap().unwrap().observed_by_controller);
    assert!(!s.store.inspect_data_handling(1200).unwrap().unwrap().observed_by_controller);
    std::fs::write(s.dir.path().join("wizard-declaration-signed.json"),original).unwrap();
    assert_eq!(s.store.inspect_data_handling(300).unwrap().unwrap().latest_revision,2);
    assert!(!s.store.inspect_data_handling(300).unwrap().unwrap().observed_by_controller);
    assert!(s.store.confirm_data_handling(1,&p.plan.plan_digest,&s.authority,300).is_err());
    assert!(s.store.plan_data_handling(1,1,&defs,choices(false),300,1300).is_err());
    let next=s.store.plan_data_handling(1,2,&defs,choices(false),300,1300).unwrap();
    assert_eq!(next.plan.body.revision,3);
    s.store.confirm_data_handling(1,&next.plan.plan_digest,&s.authority,301).unwrap();
}

#[tokio::test]
async fn declaration_flow_browses_reviews_signs_recovers_without_endpoint_or_wallet_injection() {
    use mayhem_proxy::setup::{Flow, FlowAction, FlowConfig};
    let (mock, defs) = published().await;
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let s = setup(endpoint);
        let config: FlowConfig = serde_json::from_value(
            json!({"schema_version":1,"directory":s.dir.path(),"profile":s.profile,
            "probe_plan":null,"peer_rpc":null,"admission_origin":null,"timeout_ms":1000,"run":null,
            "declaration_registry":{"origin":mock.origin,"local_loopback_http":true}}),
        )
        .unwrap();
        let flow = Flow::open(config.clone()).unwrap();
        let before = std::fs::read(s.dir.path().join("draft.json")).unwrap();
        let fields = flow
            .execute(
                FlowAction::DeclarationFields {
                    release_id: None,
                    cursor: None,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            fields.action_result["data"][0]["field_id"],
            "privacy.training"
        );
        let meta = defs.release().metadata();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let plan = flow
            .execute(
                FlowAction::DeclarationPlan {
                    expected_revision: 1,
                    expected_declaration_revision: 0,
                    release_id: meta.release_id.clone(),
                    release_hash: Digest::new(meta.release_hash.clone()).unwrap(),
                    choices: choices(false),
                    expires_at_ms: now + 60000,
                },
                None,
            )
            .await
            .unwrap();
        let pending = plan.view.pending_declaration.unwrap();
        assert_eq!(pending.state, "needs_confirmation");
        assert!(plan.view.declaration.is_none());
        let action = || FlowAction::ConfirmDeclaration {
            expected_revision: 1,
            plan_digest: pending.plan.plan_digest.clone(),
        };
        assert!(flow.execute(action(), None).await.is_err());
        assert!(flow
            .execute(action(), Some(&SigningKey::from_bytes(&[76; 32])))
            .await
            .is_err());
        let signed = flow
            .execute(action(), Some(&SigningKey::from_bytes(&[77; 32])))
            .await
            .unwrap();
        assert!(signed.view.pending_declaration.is_none());
        assert_eq!(
            signed.view.declaration.as_ref().unwrap().state,
            "signed_not_installed"
        );
        mock.state.lock().unwrap().status = 503;
        let recovered = Flow::open(config)
            .unwrap()
            .execute(action(), Some(&SigningKey::from_bytes(&[77; 32])))
            .await
            .unwrap();
        assert_eq!(
            signed.action_result["signed"],
            recovered.action_result["signed"]
        );
        assert_eq!(
            before,
            std::fs::read(s.dir.path().join("draft.json")).unwrap()
        );
        assert!(serde_json::from_value::<FlowAction>(json!({"action":"declaration_fields","release_id":null,"cursor":null,"origin":"http://untrusted.invalid"})).is_err());
        assert!(serde_json::from_value::<FlowAction>(json!({"action":"confirm_declaration","expected_revision":1,"plan_digest":pending.plan.plan_digest,"body":{"claims":[]}})).is_err());
        mock.state.lock().unwrap().status = 200;
    }
}

#[tokio::test]
async fn declaration_fields_page_is_pinned_bounded_and_checks_immutable_identity() {
    let (mock, _) = published().await;
    let reader = Reader::new(
        TrustedOrigin::local_loopback_http(&mock.origin).unwrap(),
        Limits::default(),
    )
    .unwrap();
    let pin = reader.current().await.unwrap();
    assert_eq!(reader.fields_page(&pin, None).await.unwrap().data.len(), 1);
    assert!(reader
        .fields_page(&pin, Some(&"x".repeat(513)))
        .await
        .is_err());
    assert!(reader.fields_page(&pin, Some("\n")).await.is_err());
    mock.state.lock().unwrap().mutate = Some(|body| body["release_hash"] = json!("00".repeat(32)));
    assert!(reader.fields_page(&pin, None).await.is_err());
    mock.state.lock().unwrap().mutate =
        Some(|body| body["data"] = json!(vec![body["data"][0].clone(); 33]));
    assert!(reader.fields_page(&pin, None).await.is_err());
    mock.state.lock().unwrap().mutate = Some(|body| {
        body["data"][0]["definition"]["labels"]["en"] = json!("changed without digest")
    });
    assert!(reader.fields_page(&pin, None).await.is_err());
    // A valid new self-hash still cannot change a definition under the same
    // already observed immutable release/reference.
    mock.state.lock().unwrap().mutate = Some(|body| {
        let mut doc: Document = serde_json::from_value(body["data"][0].clone()).unwrap();
        doc.definition
            .labels
            .insert("en".into(), "Changed immutable meaning".into());
        doc.definition_hash = doc.definition.digest().unwrap();
        body["data"][0] = json!(doc);
    });
    assert!(matches!(
        reader.fields_page(&pin, None).await,
        Err(mayhem_proxy::registry::publication::Error::Equivocation)
    ));
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
