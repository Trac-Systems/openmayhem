use super::*;
use mayhem_proto::proxy::{ProxyMarketDescriptor, ProxyMembership, ProxyRail};
use serde_json::json;

fn digest(c: char) -> Digest {
    Digest::new(c.to_string().repeat(64)).unwrap()
}

fn policy() -> Policy {
    Policy::new(
        digest('1'),
        digest('2'),
        Lifetimes {
            acceptance_epochs: 2,
            reservation_epochs: 20,
            receipt_grace_epochs: 4,
        },
        1024 * 1024,
    )
    .unwrap()
}

fn candidate(decisions: bool) -> PublishedOffer {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
    ))
    .unwrap();
    let value = &fixture["cases"][usize::from(decisions)];
    let market: ProxyMarketDescriptor = serde_json::from_value(value["market"].clone()).unwrap();
    let membership: ProxyMembership = serde_json::from_value(value["membership"].clone()).unwrap();
    let offer: ProxyOffer = serde_json::from_value(value["offer"].clone()).unwrap();
    PublishedOffer {
        id: format!(
            "{}/{}/{}",
            offer.market_id,
            offer.provider_pubkey,
            offer.slot_id().unwrap()
        ),
        lane: "proxy",
        market,
        membership,
        digest: offer.digest().unwrap(),
        offer,
        active: true,
        catalog_eligible: true,
        catalog_observed_at_ms: Some(1000),
        operator_verification: "unknown",
        family_label: None,
    }
}

fn raw(candidate: &PublishedOffer, rail: ProxyRail) -> Value {
    let decisions = candidate.offer.endpoint == ProxyEndpoint::Decisions;
    let controls = Controls {
        prices: PriceLimits {
            rates: candidate.offer.rates.clone(),
            per_request_au: candidate.offer.per_request_au,
            min_session_au: candidate.offer.min_session_au,
            max_total_spend_au: 1_000_000_000_000_000_000,
        },
        rail,
        settlement_policy_hash: digest('2'),
        output_units: (!decisions).then_some(128),
        minimum_context: Some(1024),
        minimum_tokens_per_second: None,
        require_verified_operator: false,
    };
    let mut body = json!({"model":format!("proxy/offer/{}", candidate.id),"proxy":controls});
    match candidate.offer.endpoint {
        ProxyEndpoint::Decisions => {
            body["state"] = json!("Evaluate this text");
            body["questions"] =
                json!({"label":{"type":"noul","instructions":"How relevant is this text?"}});
        }
        ProxyEndpoint::Chat => {
            body["messages"] = json!([{"role":"user","content":"Hello"}]);
            body["max_tokens"] = json!(128);
        }
        ProxyEndpoint::Completions => {
            body["prompt"] = json!("Hello");
            body["max_tokens"] = json!(128);
        }
        ProxyEndpoint::Responses => {
            body["input"] = json!("Hello");
            body["max_output_tokens"] = json!(128);
        }
    }
    body
}

fn parse(candidate: &PublishedOffer, raw: Value) -> Request {
    Request::parse(candidate.offer.endpoint, raw, &policy())
        .unwrap()
        .unwrap()
}

#[test]
fn exact_selector_rejects_aliases_and_cross_lane_controls() {
    let c = candidate(false);
    let selector = Selector::parse(&format!("proxy/offer/{}", c.id))
        .unwrap()
        .unwrap();
    assert_eq!(selector.id(), c.id);
    assert_eq!(selector.model(), format!("proxy/offer/{}", c.id));
    assert!(Selector::parse("qwen/native").unwrap().is_none());
    for wrong in [
        "proxy/friendly-name".into(),
        format!("proxy/offer/{}/extra", c.id),
        format!("proxy/offer/{}", c.id.replace('/', "%2F")),
        "proxy/offer/../bad".into(),
        format!("proxy/offer/{}", c.id.to_uppercase()),
    ] {
        assert!(
            matches!(Selector::parse(&wrong), Err(Error::Invalid)),
            "{wrong}"
        );
    }
    let mut body = raw(&c, ProxyRail::Fiat);
    body["model"] = json!("qwen/native");
    assert!(matches!(
        Request::parse(ProxyEndpoint::Chat, body.clone(), &policy()),
        Err(Error::Invalid)
    ));
    body.as_object_mut().unwrap().remove("proxy");
    assert!(Request::parse(ProxyEndpoint::Chat, body, &policy())
        .unwrap()
        .is_none());
}

#[test]
fn envelope_strips_controls_only_and_requires_explicit_limits_and_policy() {
    let c = candidate(false);
    let body = raw(&c, ProxyRail::Fiat);
    let request = parse(&c, body.clone());
    let mut expected = body.clone();
    expected.as_object_mut().unwrap().remove("proxy");
    assert_eq!(
        serde_json::from_slice::<Value>(request.provider_request()).unwrap(),
        expected
    );
    for field in ["prices", "rail", "settlement_policy_hash", "output_units"] {
        let mut broken = body.clone();
        broken["proxy"].as_object_mut().unwrap().remove(field);
        assert!(
            Request::parse(ProxyEndpoint::Chat, broken, &policy()).is_err(),
            "{field}"
        );
    }
    let mut broken = body.clone();
    broken["proxy"]["settlement_policy_hash"] = json!(digest('3'));
    let mismatched = parse(&c, broken);
    assert!(matches!(
        mismatched.check_settlement_policy(),
        Err(Error::SettlementPolicyMismatch)
    ));
    assert!(matches!(
        mismatched.check_candidate(&c, Eligibility::Available),
        Err(Error::SettlementPolicyMismatch)
    ));
    let mut broken = body.clone();
    broken["proxy"]["fallback_to_native"] = json!(true);
    assert!(Request::parse(ProxyEndpoint::Chat, broken, &policy()).is_err());
    let mut broken = body.clone();
    broken["proxy"]["prices"]["max_total_spend_au"] = json!("0");
    assert!(Request::parse(ProxyEndpoint::Chat, broken, &policy()).is_err());
    let mut small = policy();
    small.request_bytes = 16;
    assert!(Request::parse(ProxyEndpoint::Chat, body, &small).is_err());
}

#[test]
fn valid_offers_preserve_all_prices_rails_and_endpoint_types() {
    for decisions in [false, true] {
        for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
            let mut c = candidate(decisions);
            let endpoints = if decisions {
                vec![ProxyEndpoint::Decisions]
            } else {
                vec![
                    ProxyEndpoint::Chat,
                    ProxyEndpoint::Completions,
                    ProxyEndpoint::Responses,
                ]
            };
            for endpoint in endpoints {
                c.offer.endpoint = endpoint;
                c.id = format!(
                    "{}/{}/{}",
                    c.offer.market_id,
                    c.offer.provider_pubkey,
                    c.offer.slot_id().unwrap()
                );
                let request = parse(&c, raw(&c, rail));
                let selected = request.check_candidate(&c, Eligibility::Available).unwrap();
                assert_eq!(selected.offer, c.offer);
                assert_eq!(request.controls().rail, rail);
                assert_eq!(selected.recipe_hash.as_str(), c.membership.recipe_hash);
            }
        }
    }
}

#[test]
fn decisions_do_not_invent_token_allowances_or_throughput_guarantees() {
    let c = candidate(true);
    for field in ["output_units", "minimum_tokens_per_second"] {
        let mut body = raw(&c, ProxyRail::Fiat);
        body["proxy"][field] = json!(5);
        assert!(Request::parse(ProxyEndpoint::Decisions, body, &policy()).is_err());
    }
}

#[test]
fn completion_and_response_controls_preserve_endpoint_schema_and_bind_body_exactly() {
    for endpoint in [ProxyEndpoint::Completions, ProxyEndpoint::Responses] {
        let mut c = candidate(false);
        c.offer.endpoint = endpoint;
        c.id = format!(
            "{}/{}/{}",
            c.offer.market_id,
            c.offer.provider_pubkey,
            c.offer.slot_id().unwrap()
        );
        let mut body = raw(&c, ProxyRail::Tap);
        if endpoint == ProxyEndpoint::Completions {
            body["temperature"] = json!(0.2);
            body["seed"] = json!(2);
        } else {
            body["temperature"] = json!(0.2);
            body["text"] = json!({"format":{"type":"json_schema","name":"answer","strict":true,
                "schema":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}}});
        }
        let request = parse(&c, body.clone());
        let fingerprint = request.fingerprint(&digest('a')).unwrap();
        let mut expected = body.clone();
        expected.as_object_mut().unwrap().remove("proxy");
        assert_eq!(
            serde_json::from_slice::<Value>(request.provider_request()).unwrap(),
            expected
        );
        for field in if endpoint == ProxyEndpoint::Completions {
            ["prompt", "temperature", "max_tokens"]
        } else {
            ["input", "temperature", "max_output_tokens"]
        } {
            let mut changed = body.clone();
            changed[field] = if field.starts_with("max_") {
                json!(127)
            } else if field == "temperature" {
                json!(0.5)
            } else {
                json!("changed")
            };
            assert_ne!(
                parse(&c, changed).fingerprint(&digest('a')).unwrap(),
                fingerprint,
                "{endpoint:?}/{field}"
            );
        }
        for output in [Value::Null, json!(0), json!(PROXY_MAX_SAFE_INTEGER + 1)] {
            let mut changed = body.clone();
            changed["proxy"]["output_units"] = output;
            assert!(Request::parse(endpoint, changed, &policy()).is_err());
        }
    }
}

#[test]
fn every_price_component_and_canonical_capability_is_checked() {
    let c = candidate(false);
    for field in ["per_request_au", "min_session_au"] {
        let mut changed = c.clone();
        if field == "per_request_au" {
            changed.offer.per_request_au += 1;
        } else {
            changed.offer.min_session_au += 1;
        }
        assert!(matches!(
            parse(&c, raw(&c, ProxyRail::Fiat)).check_candidate(&changed, Eligibility::Available),
            Err(Error::Price)
        ));
    }
    for index in 0..c.offer.rates.len() {
        let mut changed = c.clone();
        changed.offer.rates[index].per_unit_au += 1;
        assert!(matches!(
            parse(&c, raw(&c, ProxyRail::Fiat)).check_candidate(&changed, Eligibility::Available),
            Err(Error::Price)
        ));
    }
    let mut body = raw(&c, ProxyRail::Fiat);
    body["proxy"]["minimum_context"] = json!(c.membership.served_context + 1);
    assert!(matches!(
        parse(&c, body).check_candidate(&c, Eligibility::Available),
        Err(Error::Constraints)
    ));
    let mut changed = c.clone();
    changed
        .offer
        .accepted_rails
        .retain(|v| *v != ProxyRail::Tap);
    assert!(matches!(
        parse(&c, raw(&c, ProxyRail::Tap)).check_candidate(&changed, Eligibility::Available),
        Err(Error::Constraints)
    ));
    let mut request = parse(&c, raw(&c, ProxyRail::Fiat));
    request.endpoint = ProxyEndpoint::Decisions;
    assert!(matches!(
        request.check_candidate(&c, Eligibility::Available),
        Err(Error::Constraints)
    ));
}

#[test]
fn all_noneligible_statuses_fail_closed_without_native_fallback() {
    let c = candidate(false);
    let request = parse(&c, raw(&c, ProxyRail::Fiat));
    for state in [
        Eligibility::Busy,
        Eligibility::Unavailable,
        Eligibility::Checking,
        Eligibility::Draining,
        Eligibility::HeartbeatMissing,
        Eligibility::StaleEvidence,
        Eligibility::ThroughputUnverified,
        Eligibility::ThroughputFloor,
        Eligibility::ControllerConflict,
        Eligibility::CatalogUnavailable,
    ] {
        assert!(
            matches!(request.check_candidate(&c,state),Err(Error::Availability(actual)) if actual==state)
        );
    }
    let mut changed = c.clone();
    changed.catalog_eligible = false;
    assert!(matches!(
        request.check_candidate(&changed, Eligibility::Available),
        Err(Error::Catalog)
    ));
    changed = c.clone();
    changed.active = false;
    assert!(matches!(
        request.check_candidate(&changed, Eligibility::Available),
        Err(Error::Catalog)
    ));
    changed = c.clone();
    changed.id.push('x');
    assert!(matches!(
        request.check_candidate(&changed, Eligibility::Available),
        Err(Error::Catalog)
    ));
    let mut body = raw(&c, ProxyRail::Fiat);
    body["proxy"]["require_verified_operator"] = json!(true);
    changed = c.clone();
    changed.operator_verification = "verified";
    assert!(matches!(
        parse(&c, body).check_candidate(&changed, Eligibility::Available),
        Err(Error::Verification)
    ));
}

#[test]
fn idempotency_binds_every_authorization_dimension_and_authenticated_owner() {
    let c = candidate(false);
    let body = raw(&c, ProxyRail::Fiat);
    let original = parse(&c, body.clone()).fingerprint(&digest('a')).unwrap();
    for (field, value) in [
        ("rail", json!("tnk")),
        ("output_units", json!(127)),
        ("minimum_context", json!(2048)),
        ("minimum_tokens_per_second", json!(6)),
        ("require_verified_operator", json!(true)),
    ] {
        let mut changed = body.clone();
        changed["proxy"][field] = value;
        assert_ne!(
            parse(&c, changed).fingerprint(&digest('a')).unwrap(),
            original,
            "{field}"
        );
    }
    for field in ["per_request_au", "min_session_au", "max_total_spend_au"] {
        let mut changed = body.clone();
        changed["proxy"]["prices"][field] = json!("1234567");
        assert_ne!(
            parse(&c, changed).fingerprint(&digest('a')).unwrap(),
            original,
            "{field}"
        );
    }
    let mut changed = body.clone();
    changed["proxy"]["prices"]["rates"][0]["per_unit_au"] = json!("123");
    assert_ne!(
        parse(&c, changed).fingerprint(&digest('a')).unwrap(),
        original
    );
    let mut changed = body.clone();
    changed["messages"][0]["content"] = json!("Different");
    assert_ne!(
        parse(&c, changed).fingerprint(&digest('a')).unwrap(),
        original
    );
    assert_ne!(
        parse(&c, body.clone()).fingerprint(&digest('b')).unwrap(),
        original
    );
    let mut changed = parse(&c, body.clone());
    changed.policy.revision = digest('4');
    assert_ne!(changed.fingerprint(&digest('a')).unwrap(), original);
    changed = parse(&c, body.clone());
    changed.policy.lifetimes.reservation_epochs += 1;
    assert_ne!(changed.fingerprint(&digest('a')).unwrap(), original);
    changed = parse(&c, body.clone());
    changed.selector.provider = digest('c');
    assert_ne!(changed.fingerprint(&digest('a')).unwrap(), original);
    changed = parse(&c, body.clone());
    changed.endpoint = ProxyEndpoint::Responses;
    assert_ne!(changed.fingerprint(&digest('a')).unwrap(), original);
    let mut ordered = serde_json::Map::new();
    for (key, value) in body.as_object().unwrap().iter().rev() {
        ordered.insert(key.clone(), value.clone());
    }
    assert_eq!(
        parse(&c, Value::Object(ordered))
            .fingerprint(&digest('a'))
            .unwrap(),
        original
    );
}

#[test]
fn operator_policy_lifetimes_and_resource_bounds_cannot_expand_silently() {
    for bytes in [0, 256 * 1024 * 1024 + 1] {
        assert!(Policy::new(digest('1'), digest('2'), policy().lifetimes, bytes).is_err());
    }
    for lifetimes in [
        Lifetimes {
            acceptance_epochs: 20,
            reservation_epochs: 20,
            receipt_grace_epochs: 1,
        },
        Lifetimes {
            acceptance_epochs: 1,
            reservation_epochs: PROXY_MAX_SAFE_INTEGER + 1,
            receipt_grace_epochs: 1,
        },
        Lifetimes {
            acceptance_epochs: 1,
            reservation_epochs: 2,
            receipt_grace_epochs: PROXY_MAX_SAFE_INTEGER + 1,
        },
    ] {
        assert!(Policy::new(digest('1'), digest('2'), lifetimes, 1024).is_err());
    }
}
