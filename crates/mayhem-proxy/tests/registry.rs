use mayhem_proto::{
    endpoint_family_contract_template, proxy::ProxyEndpoint, EndpointAttributeSpec,
};
use mayhem_proxy::registry::*;
use serde_json::json;
use std::collections::BTreeMap;

fn definition(id: &str, value_schema: ValueSchema) -> Definition {
    Definition {
        schema_version: 1,
        field_id: id.into(),
        schema_revision: 1,
        labels: BTreeMap::from([("en".into(), "Example capability".into())]),
        help: BTreeMap::new(),
        units: None,
        group: "advanced".into(),
        order: 0,
        endpoints: vec![ProxyEndpoint::Chat],
        value_schema,
        default: None,
        operators: vec![Operator::Eq],
        minimum_assurance: Assurance::Declared,
        max_evidence_age_ms: None,
        usage: Usage::FilterOnly,
        rules: vec![],
    }
}
fn observation(id: &str, value: TypedValue) -> Observation {
    Observation {
        field_id: id.into(),
        schema_revision: 1,
        endpoint: ProxyEndpoint::Chat,
        status: Support::Supported,
        value: Some(value),
        observed_at_ms: 100,
        expires_at_ms: Some(1000),
        source_id: "provider.probe.v1".into(),
    }
}
fn predicate(id: &str, value: TypedValue) -> Predicate {
    Predicate {
        field_id: id.into(),
        schema_revision: 1,
        operator: Operator::Eq,
        value,
        evidence: Assurance::Probed,
        max_age_ms: Some(100),
    }
}
fn request() -> serde_json::Value {
    json!({"model":"example", "messages":[{"role":"user","content":"hello"}]})
}
fn control(id: &str, value: TypedValue) -> Control {
    Control {
        field_id: id.into(),
        schema_revision: 1,
        value,
    }
}

#[test]
fn absent_unsupported_stale_and_declared_claims_never_become_verified_support() {
    let def = definition("tools.parallel", ValueSchema::Boolean);
    let pred = predicate(&def.field_id, TypedValue::Boolean(true));
    let mut obs = observation(&def.field_id, TypedValue::Boolean(true));
    assert_eq!(
        evaluate(
            &def,
            &pred,
            None,
            Assurance::Verified,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Unknown
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Declared,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::InsufficientEvidence
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Probed,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Satisfied
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Verified,
            ProxyEndpoint::Chat,
            201
        )
        .unwrap(),
        Match::Stale
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Verified,
            ProxyEndpoint::Chat,
            99
        )
        .unwrap(),
        Match::Stale
    );
    obs.status = Support::Unsupported;
    obs.value = None;
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Verified,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Unsupported
    );
    obs.status = Support::Unknown;
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Verified,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Unknown
    );
    obs.status = Support::Supported;
    obs.value = Some(TypedValue::Boolean(false));
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Verified,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::DifferentValue
    );
    obs.schema_revision = 2;
    assert!(evaluate(
        &def,
        &pred,
        Some(&obs),
        Assurance::Verified,
        ProxyEndpoint::Chat,
        110
    )
    .is_err());
}

#[test]
fn profiles_cannot_waive_registry_freshness_or_assurance() {
    let mut def = definition(
        "performance.rate",
        ValueSchema::Integer {
            minimum: 0,
            maximum: 1000,
        },
    );
    def.minimum_assurance = Assurance::Verified;
    def.max_evidence_age_ms = Some(20);
    let mut pred = predicate(&def.field_id, TypedValue::Integer(50));
    pred.max_age_ms = None;
    pred.evidence = Assurance::Declared;
    let obs = observation(&def.field_id, TypedValue::Integer(50));
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Probed,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::InsufficientEvidence
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Verified,
            ProxyEndpoint::Chat,
            121
        )
        .unwrap(),
        Match::Stale
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Verified,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Satisfied
    );
}

#[test]
fn decimals_are_compared_exactly_without_float_ranking_or_vendor_ordinals() {
    let mut def = definition(
        "performance.precision",
        ValueSchema::Decimal {
            minimum: "-999999999999999999".into(),
            maximum: "999999999999999999.999999999999999999".into(),
        },
    );
    def.operators = vec![Operator::Eq, Operator::Gte, Operator::Lte];
    let mut pred = predicate(
        &def.field_id,
        TypedValue::Decimal("999999999999999999.999999999999999998".into()),
    );
    pred.operator = Operator::Gte;
    let obs = observation(
        &def.field_id,
        TypedValue::Decimal("999999999999999999.999999999999999999".into()),
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Probed,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Satisfied
    );
    pred.operator = Operator::Lte;
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Probed,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::DifferentValue
    );
    for invalid in [
        "-0",
        "0.10",
        "01",
        "1e2",
        "0.",
        "1000000000000000000",
        "0.0000000000000000001",
    ] {
        assert!(
            TypedValue::Decimal(invalid.into()).validate().is_err(),
            "{invalid}"
        );
    }
    let modes = definition(
        "vendor_x.effort",
        ValueSchema::Enum {
            values: vec!["High".into(), "Low".into()],
        },
    );
    let mut pred = predicate(&modes.field_id, TypedValue::Enum("High".into()));
    pred.operator = Operator::Gte;
    assert!(evaluate(
        &modes,
        &pred,
        None,
        Assurance::Verified,
        ProxyEndpoint::Chat,
        110
    )
    .is_err());
}

#[test]
fn set_requirements_keep_exact_values_and_do_not_turn_missing_into_false() {
    let mut def = definition(
        "schema.subsets",
        ValueSchema::Set {
            values: vec!["Objects".into(), "uniqueItems".into(), "枚挙".into()],
        },
    );
    def.operators = vec![Operator::Eq, Operator::ContainsAll];
    let mut pred = predicate(
        &def.field_id,
        TypedValue::Set(vec!["Objects".into(), "uniqueItems".into()]),
    );
    pred.operator = Operator::ContainsAll;
    let obs = observation(
        &def.field_id,
        TypedValue::Set(vec!["Objects".into(), "uniqueItems".into(), "枚挙".into()]),
    );
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Probed,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Satisfied
    );
    let obs = observation(&def.field_id, TypedValue::Set(vec!["Objects".into()]));
    assert_eq!(
        evaluate(
            &def,
            &pred,
            Some(&obs),
            Assurance::Probed,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::DifferentValue
    );
}

#[test]
fn semantic_changes_need_new_revisions_but_ui_translation_does_not() {
    let def = definition(
        "vendor_x.effort",
        ValueSchema::Enum {
            values: vec!["High".into(), "Low".into()],
        },
    );
    let mut next = def.clone();
    next.labels.insert("de".into(), "Denkaufwand".into());
    next.order = 8;
    def.check_successor(&next).unwrap();
    assert_ne!(def.digest().unwrap(), next.digest().unwrap());
    next.value_schema = ValueSchema::Enum {
        values: vec!["High".into(), "Low".into(), "Max".into()],
    };
    assert!(def.check_successor(&next).is_err());
    next.schema_revision = 2;
    def.check_successor(&next).unwrap();
    let mut old_pred = predicate(&def.field_id, TypedValue::Enum("High".into()));
    assert!(evaluate(
        &next,
        &old_pred,
        None,
        Assurance::Verified,
        ProxyEndpoint::Chat,
        110
    )
    .is_err());
    old_pred.schema_revision = 2;
    assert_eq!(
        evaluate(
            &next,
            &old_pred,
            None,
            Assurance::Verified,
            ProxyEndpoint::Chat,
            110
        )
        .unwrap(),
        Match::Unknown
    );
}

#[test]
fn unknown_fields_paths_endpoint_controls_and_conflicting_values_fail_atomically() {
    let request = request();
    let original = request.clone();
    let mut def = definition(
        "sampling.temperature",
        ValueSchema::Decimal {
            minimum: "0".into(),
            maximum: "1".into(),
        },
    );
    def.usage = Usage::RequestControl {
        request_path: "temperature".into(),
    };
    let contract = endpoint_family_contract_template("openai_chat_completions").unwrap();
    let wanted = vec![control(&def.field_id, TypedValue::Decimal("0.25".into()))];
    assert!(
        apply_controls(&request, ProxyEndpoint::Chat, &contract, &wanted, |_, _| {
            None
        })
        .is_err()
    );
    let output = apply_controls(&request, ProxyEndpoint::Chat, &contract, &wanted, |_, _| {
        Some(&def)
    })
    .unwrap();
    assert_eq!(output["temperature"], json!(0.25));
    assert_eq!(request, original);
    let mut conflicting = request.clone();
    conflicting["temperature"] = json!(0.5);
    assert!(apply_controls(
        &conflicting,
        ProxyEndpoint::Chat,
        &contract,
        &wanted,
        |_, _| Some(&def)
    )
    .is_err());
    assert_eq!(conflicting["temperature"], json!(0.5));
    for path in [
        "model",
        "proxy.rail",
        "messages",
        "headers.authorization",
        "upstream.temperature",
        "temperature.constructor",
        "__proto__.x",
    ] {
        let mut altered = def.clone();
        altered.usage = Usage::RequestControl {
            request_path: path.into(),
        };
        assert!(
            apply_controls(&request, ProxyEndpoint::Chat, &contract, &wanted, |_, _| {
                Some(&altered)
            })
            .is_err(),
            "{path}"
        );
    }
    let mut narrower = contract.clone();
    narrower
        .request_attribute_specs
        .get_mut("temperature")
        .unwrap()
        .maximum = Some(0.1);
    assert!(
        apply_controls(&request, ProxyEndpoint::Chat, &narrower, &wanted, |_, _| {
            Some(&def)
        })
        .is_err()
    );
    assert!(apply_controls(
        &request,
        ProxyEndpoint::Decisions,
        &contract,
        &wanted,
        |_, _| Some(&def)
    )
    .is_err());
    assert!(apply_controls(
        &request,
        ProxyEndpoint::Chat,
        &contract,
        &[wanted[0].clone(), wanted[0].clone()],
        |_, _| Some(&def)
    )
    .is_err());
}

#[test]
fn new_data_driven_fields_and_conditional_controls_work_without_vendor_code() {
    let mut contract = endpoint_family_contract_template("openai_chat_completions").unwrap();
    for (path, values) in [
        ("reasoning.mode", vec!["disabled", "enabled"]),
        ("reasoning.effort", vec!["Adaptive High", "Low"]),
    ] {
        contract.request_attributes.push(path.into());
        contract.request_attribute_specs.insert(
            path.into(),
            serde_json::from_value::<EndpointAttributeSpec>(
                json!({"value_types":["string"],"enum_values":values}),
            )
            .unwrap(),
        );
    }
    let mut mode = definition(
        "new_family.mode",
        ValueSchema::Enum {
            values: vec!["disabled".into(), "enabled".into()],
        },
    );
    mode.usage = Usage::RequestControl {
        request_path: "reasoning.mode".into(),
    };
    let mut effort = definition(
        "new_family.effort",
        ValueSchema::Enum {
            values: vec!["Adaptive High".into(), "Low".into()],
        },
    );
    effort.usage = Usage::RequestControl {
        request_path: "reasoning.effort".into(),
    };
    effort.rules.push(Rule {
        when: vec![Condition {
            field_id: effort.field_id.clone(),
            schema_revision: 1,
            operator: Operator::Eq,
            value: TypedValue::Enum("Adaptive High".into()),
        }],
        require: vec![Condition {
            field_id: mode.field_id.clone(),
            schema_revision: 1,
            operator: Operator::Eq,
            value: TypedValue::Enum("enabled".into()),
        }],
        forbid: vec![],
    });
    let definitions = BTreeMap::from([
        (mode.field_id.clone(), mode.clone()),
        (effort.field_id.clone(), effort.clone()),
    ]);
    let lookup = |id: &str, revision: u32| {
        definitions
            .get(id)
            .filter(|d| d.schema_revision == revision)
    };
    let wanted = control(&effort.field_id, TypedValue::Enum("Adaptive High".into()));
    assert!(apply_controls(
        &request(),
        ProxyEndpoint::Chat,
        &contract,
        &[wanted.clone()],
        lookup
    )
    .is_err());
    let disabled = control(&mode.field_id, TypedValue::Enum("disabled".into()));
    assert!(apply_controls(
        &request(),
        ProxyEndpoint::Chat,
        &contract,
        &[wanted.clone(), disabled],
        lookup
    )
    .is_err());
    let enabled = control(&mode.field_id, TypedValue::Enum("enabled".into()));
    let mut raw = request();
    raw["reasoning"] = json!({"mode":"enabled"});
    let raw_result = apply_controls(
        &raw,
        ProxyEndpoint::Chat,
        &contract,
        &[wanted.clone()],
        lookup,
    )
    .unwrap();
    assert_eq!(
        raw_result["reasoning"],
        json!({"mode":"enabled","effort":"Adaptive High"})
    );
    assert_eq!(raw["reasoning"], json!({"mode":"enabled"}));
    raw["reasoning"]["mode"] = json!("disabled");
    assert!(apply_controls(
        &raw,
        ProxyEndpoint::Chat,
        &contract,
        &[wanted.clone()],
        lookup
    )
    .is_err());
    raw["reasoning"]["mode"] = json!(true);
    assert!(apply_controls(
        &raw,
        ProxyEndpoint::Chat,
        &contract,
        &[wanted.clone()],
        lookup
    )
    .is_err());
    let result = apply_controls(
        &request(),
        ProxyEndpoint::Chat,
        &contract,
        &[wanted, enabled],
        lookup,
    )
    .unwrap();
    assert_eq!(
        result["reasoning"],
        json!({"mode":"enabled","effort":"Adaptive High"})
    );
}

#[test]
fn shared_site_wire_fixture_has_identical_persisted_intent_validation() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/registry-wire-v1.json")).unwrap();
    let mut count = 0;
    for group in ["values", "predicates", "controls"] {
        for case in cases[group].as_array().unwrap() {
            let input = case["input"].clone();
            let accepted = match group {
                "values" => serde_json::from_value::<TypedValue>(input)
                    .ok()
                    .is_some_and(|v| v.validate().is_ok()),
                "predicates" => serde_json::from_value::<Predicate>(input)
                    .ok()
                    .is_some_and(|v| v.validate().is_ok()),
                "controls" => serde_json::from_value::<Control>(input)
                    .ok()
                    .is_some_and(|v| v.validate().is_ok()),
                _ => unreachable!(),
            };
            assert_eq!(
                accepted,
                case["valid"].as_bool().unwrap(),
                "{group}/{}",
                case["name"]
            );
            count += 1;
        }
    }
    assert_eq!(count, 45);
}

#[test]
fn explicit_raw_dependency_rules_cannot_be_bypassed_and_cycles_are_bounded() {
    let mut contract = endpoint_family_contract_template("openai_chat_completions").unwrap();
    let mut definitions = BTreeMap::new();
    for (id, next) in [
        ("fixture.a", "fixture.b"),
        ("fixture.b", "fixture.c"),
        ("fixture.c", "fixture.a"),
    ] {
        let path = format!("options.{}", id.strip_prefix("fixture.").unwrap());
        contract.request_attributes.push(path.clone());
        contract.request_attribute_specs.insert(
            path.clone(),
            serde_json::from_value(json!({"value_types":["boolean"]})).unwrap(),
        );
        let mut def = definition(id, ValueSchema::Boolean);
        def.usage = Usage::RequestControl { request_path: path };
        def.rules.push(Rule {
            when: vec![Condition {
                field_id: id.into(),
                schema_revision: 1,
                operator: Operator::Eq,
                value: TypedValue::Boolean(true),
            }],
            require: vec![Condition {
                field_id: next.into(),
                schema_revision: 1,
                operator: Operator::Eq,
                value: TypedValue::Boolean(true),
            }],
            forbid: vec![],
        });
        definitions.insert(id.to_owned(), def);
    }
    let lookups = std::cell::Cell::new(0);
    let lookup = |id: &str, _| {
        lookups.set(lookups.get() + 1);
        definitions.get(id)
    };
    let mut raw = request();
    raw["options"] = json!({"b":true,"c":false});
    let controls = [control("fixture.a", TypedValue::Boolean(true))];
    assert!(apply_controls(&raw, ProxyEndpoint::Chat, &contract, &controls, lookup).is_err());
    assert_eq!(lookups.get(), 3);
    raw["options"]["c"] = json!(true);
    lookups.set(0);
    let result = apply_controls(&raw, ProxyEndpoint::Chat, &contract, &controls, lookup).unwrap();
    assert_eq!(result["options"], json!({"a":true,"b":true,"c":true}));
    assert_eq!(lookups.get(), 3);
}

#[test]
fn numeric_transport_never_silently_rounds_the_selected_value_or_injects_defaults() {
    let mut def = definition(
        "sampling.temperature",
        ValueSchema::Decimal {
            minimum: "0".into(),
            maximum: "999999999999999999".into(),
        },
    );
    def.usage = Usage::RequestControl {
        request_path: "temperature".into(),
    };
    def.default = Some(TypedValue::Decimal("0.5".into()));
    let mut contract = endpoint_family_contract_template("openai_chat_completions").unwrap();
    contract
        .request_attribute_specs
        .get_mut("temperature")
        .unwrap()
        .maximum = Some(1e18);
    let unchanged = apply_controls(&request(), ProxyEndpoint::Chat, &contract, &[], |_, _| {
        Some(&def)
    })
    .unwrap();
    assert!(unchanged.get("temperature").is_none());
    for value in ["0.000001", "0.0000001", "0.1", "9007199254740991"] {
        let wanted = control(&def.field_id, TypedValue::Decimal(value.into()));
        assert!(
            apply_controls(
                &request(),
                ProxyEndpoint::Chat,
                &contract,
                &[wanted],
                |_, _| Some(&def)
            )
            .is_ok(),
            "{value}"
        );
    }
    let wanted = control(
        &def.field_id,
        TypedValue::Decimal("99999999999999999.1".into()),
    );
    assert!(apply_controls(
        &request(),
        ProxyEndpoint::Chat,
        &contract,
        &[wanted],
        |_, _| Some(&def)
    )
    .is_err());
    let wanted = control(
        &def.field_id,
        TypedValue::Decimal("9999999999999999".into()),
    );
    assert!(apply_controls(
        &request(),
        ProxyEndpoint::Chat,
        &contract,
        &[wanted],
        |_, _| Some(&def)
    )
    .is_err());
}

#[test]
fn registry_work_is_bounded_and_structural_json_never_executes() {
    let mut def = definition("custom.flag", ValueSchema::Boolean);
    let mut raw = serde_json::to_value(&def).unwrap();
    raw["callback"] = json!("https://example.invalid");
    assert!(serde_json::from_value::<Definition>(raw).is_err());
    def.rules = vec![
        Rule {
            when: vec![],
            require: vec![],
            forbid: vec![]
        };
        9
    ];
    assert!(def.validate().is_err());
    def.rules.clear();
    def.labels.insert("en".into(), "x".repeat(17000));
    assert!(def.validate().is_err());
    let contract = endpoint_family_contract_template("openai_chat_completions").unwrap();
    let controls = vec![control("custom.flag", TypedValue::Boolean(true)); 33];
    let called = std::cell::Cell::new(0);
    assert!(apply_controls(
        &request(),
        ProxyEndpoint::Chat,
        &contract,
        &controls,
        |_, _| {
            called.set(called.get() + 1);
            None
        }
    )
    .is_err());
    assert_eq!(called.get(), 0);
}
