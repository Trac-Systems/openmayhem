use ed25519_dalek::{Signer, SigningKey};
use mayhem_proto::proxy::ProxyEndpoint;
use mayhem_proxy::{attempts::Digest, declaration::*, registry::*};
use serde_json::json;
fn d(n: u8) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn definition() -> Definition {
    serde_json::from_value(json!({"schema_version":1,"field_id":"privacy.training","schema_revision":1,"labels":{"en":"Provider promises no training"},"help":{},"units":null,"group":"privacy","order":0,"endpoints":["mayhem_decisions","openai_chat_completions","openai_completions","openai_responses"],"value_schema":{"type":"boolean"},"default":null,"operators":["eq"],"minimum_assurance":"declared","max_evidence_age_ms":1000,"usage":{"kind":"filter_only"},"rules":[]})).unwrap()
}
fn body(endpoint: ProxyEndpoint) -> Body {
    let key = SigningKey::from_bytes(&[98; 32]);
    let provider = Digest::new(
        key.verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    Body {
        schema_version: 1,
        subject: Subject {
            network: mayhem_proxy::discovery::Identity {
                network_id: "918".into(),
                msb_bootstrap: d(4).as_str().into(),
                subnet_bootstrap: d(5).as_str().into(),
                contract_version: mayhem_proto::CONTRACT_VERSION,
            },
            provider,
            market: d(6),
            membership_revision: 1,
            membership_digest: d(7),
            endpoint,
            endpoint_contract: d(8),
            recipe_hash: d(9),
            connection_revision: 1,
        },
        revision: 1,
        issued_at_ms: 100,
        expires_at_ms: 1100,
        claims: vec![Claim {
            field_id: "privacy.training".into(),
            schema_revision: 1,
            definition_digest: Digest::new(definition().digest().unwrap()).unwrap(),
            status: Support::Supported,
            value: Some(TypedValue::Boolean(false)),
        }],
    }
}
fn signed(body: Body) -> Signed {
    let signature = SigningKey::from_bytes(&[98; 32])
        .sign(&body.signing_bytes().unwrap())
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Signed { body, signature }
}
fn wanted() -> Predicate {
    Predicate {
        field_id: "privacy.training".into(),
        schema_revision: 1,
        operator: Operator::Eq,
        value: TypedValue::Boolean(false),
        evidence: Assurance::Declared,
        max_age_ms: None,
    }
}
#[test]
fn declarations_never_upgrade_assurance_across_all_endpoints() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let record = signed(body(endpoint));
        let def = definition();
        let mut predicate = wanted();
        record.check(&record.body.subject, 101).unwrap();
        assert_eq!(
            record.evaluate(&def, &predicate, 101).unwrap(),
            Match::Satisfied
        );
        for evidence in [Assurance::Probed, Assurance::Verified] {
            predicate.evidence = evidence;
            assert_eq!(
                record.evaluate(&def, &predicate, 101).unwrap(),
                Match::InsufficientEvidence
            );
        }
        predicate = wanted();
        predicate.value = TypedValue::Boolean(true);
        assert_eq!(
            record.evaluate(&def, &predicate, 101).unwrap(),
            Match::DifferentValue
        );
        predicate = wanted();
        predicate.max_age_ms = Some(1);
        assert_eq!(
            record.evaluate(&def, &predicate, 102).unwrap(),
            Match::Stale
        );
        assert!(record.check(&record.body.subject, 1100).is_err());
        assert!(record.check(&record.body.subject, 99).is_err());
        let mut strict = def.clone();
        strict.minimum_assurance = Assurance::Verified;
        let mut b = record.body.clone();
        b.claims[0].definition_digest = Digest::new(strict.digest().unwrap()).unwrap();
        assert_eq!(
            signed(b).evaluate(&strict, &wanted(), 101).unwrap(),
            Match::InsufficientEvidence
        );
    }
}
#[test]
fn signatures_exact_binding_unknown_and_bounded_wire() {
    let record = signed(body(ProxyEndpoint::Chat));
    for field in [
        "provider",
        "market",
        "membership_digest",
        "endpoint_contract",
        "recipe_hash",
    ] {
        let mut value = serde_json::to_value(&record.body.subject).unwrap();
        value[field] = json!(d(42));
        assert!(record
            .check(&serde_json::from_value(value).unwrap(), 101)
            .is_err());
    }
    for field in ["membership_revision", "connection_revision"] {
        let mut value = serde_json::to_value(&record.body.subject).unwrap();
        value[field] = json!(2);
        assert!(record
            .check(&serde_json::from_value(value).unwrap(), 101)
            .is_err());
    }
    let mut mutated = record.clone();
    mutated.body.claims[0].value = Some(TypedValue::Boolean(true));
    assert!(mutated.verify().is_err());
    mutated = record.clone();
    mutated.signature = "0".repeat(128);
    assert!(mutated.verify().is_err());
    let mut b = record.body.clone();
    b.subject.network.network_id = "919".into();
    assert!(signed(b).check(&record.body.subject, 101).is_err());
    let mut b = record.body.clone();
    b.claims[0].status = Support::Unknown;
    b.claims[0].value = None;
    assert_eq!(
        signed(b).evaluate(&definition(), &wanted(), 101).unwrap(),
        Match::Unknown
    );
    let mut b = record.body.clone();
    b.claims[0].status = Support::Unsupported;
    b.claims[0].value = None;
    assert_eq!(
        signed(b).evaluate(&definition(), &wanted(), 101).unwrap(),
        Match::Unsupported
    );
    let mut b = record.body.clone();
    b.claims.push(b.claims[0].clone());
    assert!(b.signing_bytes().is_err());
    let mut b = record.body.clone();
    b.expires_at_ms = 9_007_199_254_740_992;
    assert!(b.signing_bytes().is_err());
    let mut b = record.body.clone();
    b.revision = 0;
    assert!(b.signing_bytes().is_err());
    let mut wire = serde_json::to_value(&record).unwrap();
    wire["assurance"] = json!("verified");
    assert!(serde_json::from_value::<Signed>(wire).is_err());
}
#[test]
fn declared_conditional_rules_require_exact_signed_dependency_values() {
    let mut def = definition();
    let mut region = definition();
    region.field_id = "privacy.region".into();
    region.value_schema = ValueSchema::Enum {
        values: vec!["EU".into(), "US".into()],
    };
    let condition = Condition {
        field_id: region.field_id.clone(),
        schema_revision: 1,
        operator: Operator::Eq,
        value: TypedValue::Enum("EU".into()),
    };
    def.rules = vec![Rule {
        when: vec![condition.clone()],
        require: vec![condition],
        forbid: vec![],
    }];
    let mut b = body(ProxyEndpoint::Chat);
    b.claims[0].definition_digest = Digest::new(def.digest().unwrap()).unwrap();
    let record = signed(b.clone());
    assert_eq!(
        record
            .evaluate_with_rules(
                &def,
                &wanted(),
                |id, _| if id == region.field_id {
                    Some(&region)
                } else {
                    Some(&def)
                },
                101
            )
            .unwrap(),
        Match::Unknown
    );
    b.claims.insert(
        0,
        Claim {
            field_id: region.field_id.clone(),
            schema_revision: 1,
            definition_digest: Digest::new(region.digest().unwrap()).unwrap(),
            status: Support::Supported,
            value: Some(TypedValue::Enum("EU".into())),
        },
    );
    assert_eq!(
        signed(b)
            .evaluate_with_rules(
                &def,
                &wanted(),
                |id, _| if id == region.field_id {
                    Some(&region)
                } else {
                    Some(&def)
                },
                101
            )
            .unwrap(),
        Match::Satisfied
    );
}

#[test]
fn typed_signer_cannot_change_identity_network_or_sign_malformed_promises() {
    let body = body(ProxyEndpoint::Chat);
    let authority = mayhem_proxy::signing::Authority::from_unlocked_wallet(
        SigningKey::from_bytes(&[98; 32]),
        mayhem_proxy::attempts::Identity {
            network_id: body.subject.network.network_id.clone(),
            msb_bootstrap: Digest::new(&body.subject.network.msb_bootstrap).unwrap(),
            subnet_bootstrap: Digest::new(&body.subject.network.subnet_bootstrap).unwrap(),
            controller_pubkey: body.subject.provider.clone(),
        },
    )
    .unwrap();
    authority
        .declare_data_handling(body.clone())
        .unwrap()
        .verify()
        .unwrap();
    let mut other = body.clone();
    other.subject.provider = d(11);
    assert!(authority.declare_data_handling(other).is_err());
    let mut other = body.clone();
    other.subject.network.network_id = "919".into();
    assert!(authority.declare_data_handling(other).is_err());
    let mut other = body;
    other.claims.clear();
    assert!(authority.declare_data_handling(other).is_err());
}
