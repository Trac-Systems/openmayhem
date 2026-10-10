use mayhem_proto::proxy::{finance::*, ProxyMarketDescriptor, ProxyMembership};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/proxy-finance-v1.json")).unwrap()
}
fn decode<T: DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn approved_platform_policy_keeps_rust_and_javascript_receipt_meaning_identical() {
    let settings: Value = serde_json::from_str(include_str!(
        "../../../config/proxy/platform-commercial-policy-v1.json"
    ))
    .unwrap();
    let policy: ProxySettlementPolicy = decode(&settings["settlement_policy"]);
    assert_eq!(policy.digest().unwrap(), settings["settlement_policy_hash"]);
    for row in fixture()["cases"].as_array().unwrap() {
        let mut terms: ProxySpendTerms = decode(&row["terms"]);
        terms.settlement_policy_hash = policy.digest().unwrap();
        terms.acceptance_expires_after_epoch = terms.billing_epoch
            + settings["buyer_lifetimes"]["acceptance_epochs"]
                .as_u64()
                .unwrap();
        terms.reservation_expires_after_epoch = terms.billing_epoch
            + settings["buyer_lifetimes"]["reservation_epochs"]
                .as_u64()
                .unwrap();
        terms.reservation_receipt_grace_epochs = settings["buyer_lifetimes"]
            ["receipt_grace_epochs"]
            .as_u64()
            .unwrap();
        terms
            .validate_new_acceptance(
                &decode(&row["market"]),
                &decode(&row["membership"]),
                &terms.offer,
                &policy,
                terms.billing_epoch,
            )
            .unwrap();
        let mut receipt: ProxyReceiptBody = decode(&row["receipt"]);
        receipt.accepted_terms = terms.digest().unwrap();
        for outcome in &policy.payable_outcomes {
            receipt.outcome = *outcome;
            receipt.validate_for(&terms, &policy, None).unwrap();
        }
        receipt.final_receipt = false;
        receipt.outcome = ProxyReceiptOutcome::Running;
        assert!(receipt.validate_for(&terms, &policy, None).is_err());
        receipt.final_receipt = true;
        receipt.outcome = ProxyReceiptOutcome::Complete;
        receipt.au_owed_cum += 1;
        assert!(receipt.validate_for(&terms, &policy, None).is_err());
    }
}

#[test]
fn all_endpoint_rail_financial_vectors_match_javascript() {
    for row in fixture()["cases"].as_array().unwrap() {
        let terms: ProxySpendTerms = decode(&row["terms"]);
        let policy: ProxySettlementPolicy = decode(&row["policy"]);
        let receipt: ProxyReceiptBody = decode(&row["receipt"]);
        let checkpoint: ProxyReceiptBody = decode(&row["checkpoint"]);
        let market: ProxyMarketDescriptor = decode(&row["market"]);
        let member: ProxyMembership = decode(&row["membership"]);
        terms
            .validate_new_acceptance(&market, &member, &terms.offer, &policy, 51)
            .unwrap();
        checkpoint.validate_for(&terms, &policy, None).unwrap();
        receipt
            .validate_for(&terms, &policy, Some(&checkpoint))
            .unwrap();
        receipt
            .validate_for(&terms, &policy, Some(&receipt))
            .unwrap();
        assert_eq!(terms.digest().unwrap(), row["digests"]["terms"]);
        assert_eq!(policy.digest().unwrap(), row["digests"]["policy"]);
        assert_eq!(receipt.digest().unwrap(), row["digests"]["receipt"]);
        for (key, bytes) in [
            ("buyer_terms", terms.buyer_signing_bytes().unwrap()),
            ("provider_terms", terms.provider_signing_bytes().unwrap()),
            ("buyer_receipt", receipt.buyer_signing_bytes().unwrap()),
            (
                "provider_receipt",
                receipt.provider_signing_bytes().unwrap(),
            ),
        ] {
            assert_eq!(String::from_utf8(bytes).unwrap(), row["signing_utf8"][key]);
        }
        assert_ne!(
            terms.buyer_signing_bytes().unwrap(),
            terms.provider_signing_bytes().unwrap()
        );
        assert_ne!(
            receipt.buyer_signing_bytes().unwrap(),
            receipt.provider_signing_bytes().unwrap()
        );
    }
}

#[test]
fn terms_reject_unpriced_limits_overflow_native_lane_and_extra_fields() {
    let row = &fixture()["cases"][0];
    for (field, value) in [
        ("lane", json!("native")),
        ("schema_version", json!(2)),
        ("contract_version", json!(0)),
        ("network_id", json!("")),
        ("buyer_pubkey", json!("ABC")),
        ("billing_attempt", json!(0)),
        ("billing_attempt", json!(9_007_199_254_740_992_u64)),
        ("billing_epoch", json!(50.1)),
        ("max_spend_au", json!("99999999")),
        ("max_spend_au", json!("0001")),
        ("max_spend_au", json!(1)),
        ("max_total_spend_au", json!("1")),
        ("prior_spend_au", json!(u128::MAX.to_string())),
        ("acceptance_expires_after_epoch", json!(49)),
        ("reservation_expires_after_epoch", json!(51)),
        (
            "reservation_receipt_grace_epochs",
            json!(9_007_199_254_740_991_u64),
        ),
        ("served_context", json!(4_294_967_296_u64)),
        ("rail", json!("btc")),
        ("max_usage", json!({"input_token":1})),
        ("max_usage", json!({"input_token":0,"output_token":0})),
        ("max_usage", json!({"input_token":1.1,"output_token":1})),
        ("max_usage", json!({"input_token":-1,"output_token":1})),
        (
            "max_usage",
            json!({"input_token":1,"output_token":1,"hidden":1}),
        ),
        ("private_url", json!("must-not-be-in-wire")),
    ] {
        let mut v = row["terms"].clone();
        v[field] = value;
        assert!(
            serde_json::from_value::<ProxySpendTerms>(v)
                .map_err(|e| e.to_string())
                .and_then(|t| t.validate())
                .is_err(),
            "{field}"
        );
    }
    let mut terms: ProxySpendTerms = decode(&row["terms"]);
    terms.offer.rates[0].per_unit_au = u128::MAX;
    assert!(terms.validate().is_err());
    let mut omitted = row["terms"].clone();
    omitted
        .as_object_mut()
        .unwrap()
        .remove("payment_terms_hash");
    assert!(serde_json::from_value::<ProxySpendTerms>(omitted).is_err());
    // Valid alternative JSON spellings normalize identically to JS, including quantities.
    let mut floats = row["terms"].clone();
    floats["billing_attempt"] = json!(2.0);
    floats["max_usage"]["input_token"] = json!(1024.0);
    assert_eq!(
        decode::<ProxySpendTerms>(&floats).digest().unwrap(),
        row["digests"]["terms"]
    );
}

#[test]
fn new_quotes_use_current_membership_but_old_receipts_keep_locked_terms() {
    let row = &fixture()["cases"][0];
    let terms: ProxySpendTerms = decode(&row["terms"]);
    let policy = decode(&row["policy"]);
    let market = decode(&row["market"]);
    let member: ProxyMembership = decode(&row["membership"]);
    let mut repriced = terms.offer.clone();
    repriced.revision += 1;
    repriced.rates[0].per_unit_au *= 10;
    assert!(terms
        .validate_new_acceptance(&market, &member, &repriced, &policy, 51)
        .is_err());
    assert!(terms
        .validate_new_acceptance(&market, &member, &terms.offer, &policy, 52)
        .is_err());
    for field in [
        "recipe_hash",
        "connection_revision",
        "served_context",
        "endpoint_contract",
    ] {
        let mut v = row["terms"].clone();
        v[field] = if field.ends_with("hash") || field == "endpoint_contract" {
            json!("f".repeat(64))
        } else {
            json!(9)
        };
        assert!(decode::<ProxySpendTerms>(&v)
            .validate_new_acceptance(&market, &member, &terms.offer, &policy, 51)
            .is_err());
    }
    // Receipt validation deliberately has no current-version/offer/epoch argument.
    decode::<ProxyReceiptBody>(&row["receipt"])
        .validate_for(&terms, &policy, None)
        .unwrap();
}

#[test]
fn receipts_cannot_change_owner_rail_payout_offer_network_or_limits() {
    let row = &fixture()["cases"][0];
    let receipt: ProxyReceiptBody = decode(&row["receipt"]);
    let policy = decode(&row["policy"]);
    for field in [
        "buyer_pubkey",
        "billing_id",
        "session_id",
        "reservation_id",
        "payout_revision",
        "request_hash",
        "endpoint_contract",
        "recipe_hash",
        "connection_digest",
        "capacity_lease",
        "payment_terms_hash",
        "settlement_policy_hash",
        "msb_bootstrap",
        "subnet_bootstrap",
    ] {
        let mut v = row["terms"].clone();
        v[field] = json!("0".repeat(64));
        assert!(
            receipt.validate_for(&decode(&v), &policy, None).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("rail", json!("tap")),
        ("network_id", json!("another-network")),
        ("contract_version", json!(29)),
        ("billing_attempt", json!(3)),
        ("rules_ver", json!(2)),
        ("prior_spend_au", json!("28")),
        ("max_total_spend_au", json!("99999")),
    ] {
        let mut v = row["terms"].clone();
        v[field] = value;
        assert!(
            receipt.validate_for(&decode(&v), &policy, None).is_err(),
            "{field}"
        );
    }
}

#[test]
fn receipt_progress_is_monotonic_final_replays_only_and_bounded() {
    let row = &fixture()["cases"][0];
    let terms = decode(&row["terms"]);
    let policy = decode(&row["policy"]);
    let prev: ProxyReceiptBody = decode(&row["checkpoint"]);
    let done: ProxyReceiptBody = decode(&row["receipt"]);
    for (field, value) in [
        ("seq", json!(0)),
        ("seq", json!(1)),
        ("at_ms", json!(0)),
        ("final", json!(false)),
        ("outcome", json!("unknown")),
        ("outcome", json!("partial")),
        ("au_owed_cum", json!("1")),
        ("billing_au_owed_cum", json!("1")),
        ("usage", json!({"input_token":99999,"output_token":7})),
        ("usage", json!({"input_token":7})),
        ("extra", json!(true)),
    ] {
        let mut v = row["receipt"].clone();
        v[field] = value;
        assert!(
            serde_json::from_value::<ProxyReceiptBody>(v)
                .map_err(|e| e.to_string())
                .and_then(|r| r.validate_for(&terms, &policy, Some(&prev)))
                .is_err(),
            "{field}"
        );
    }
    let mut changed = done.clone();
    changed.seq += 1;
    changed.at_ms += 1;
    assert!(changed.validate_for(&terms, &policy, Some(&done)).is_err());
    let mut decreased = done.clone();
    decreased.usage.insert("input_token".into(), 0);
    decreased.au_owed_cum = terms.offer.cost(&decreased.usage).unwrap();
    decreased.billing_au_owed_cum = terms.prior_spend_au + decreased.au_owed_cum;
    assert!(decreased
        .validate_for(&terms, &policy, Some(&prev))
        .is_err());
    let mut conflict = prev.clone();
    conflict.result_hash = "0".repeat(64);
    assert!(conflict.validate_for(&terms, &policy, Some(&prev)).is_err());
}

#[test]
fn partial_and_cancelled_rules_require_explicit_preaccepted_policy() {
    let row = &fixture()["cases"][0];
    let terms: ProxySpendTerms = decode(&row["terms"]);
    let policy: ProxySettlementPolicy = decode(&row["policy"]);
    for outcome in [
        ProxyReceiptOutcome::Partial,
        ProxyReceiptOutcome::Cancelled,
        ProxyReceiptOutcome::Refused,
    ] {
        let mut r: ProxyReceiptBody = decode(&row["receipt"]);
        r.outcome = outcome;
        assert!(r.validate_for(&terms, &policy, None).is_err());
        let mut explicit = policy.clone();
        explicit.payable_outcomes.push(outcome);
        explicit.payable_outcomes.sort();
        assert!(r.validate_for(&terms, &explicit, None).is_err());
        let mut accepted = terms.clone();
        accepted.settlement_policy_hash = explicit.digest().unwrap();
        r.accepted_terms = accepted.digest().unwrap();
        r.validate_for(&accepted, &explicit, None).unwrap();
    }
    let mut invalid = policy.clone();
    invalid.payable_outcomes.push(ProxyReceiptOutcome::Running);
    assert!(invalid.validate().is_err());
    let mut no_checkpoints = policy.clone();
    no_checkpoints.allow_checkpoints = false;
    let mut t = terms.clone();
    t.settlement_policy_hash = no_checkpoints.digest().unwrap();
    let mut r: ProxyReceiptBody = decode(&row["checkpoint"]);
    r.accepted_terms = t.digest().unwrap();
    assert!(r.validate_for(&t, &no_checkpoints, None).is_err());
}

#[test]
fn signatures_use_separate_roles_and_exact_buyer_provider_keys() {
    let row = &fixture()["cases"][0];
    let terms: ProxySpendTerms = decode(&row["terms"]);
    let authorization = ProxySpendAuthorization {
        terms: terms.clone(),
        buyer_sig: "a".repeat(128),
        provider_sig: "b".repeat(128),
    };
    authorization
        .verify(|sig, bytes, key| {
            if sig.starts_with('a') {
                bytes == terms.buyer_signing_bytes().unwrap() && key == terms.buyer_pubkey
            } else {
                bytes == terms.provider_signing_bytes().unwrap()
                    && key == terms.offer.provider_pubkey
            }
        })
        .unwrap();
    assert!(authorization.verify(|_, _, _| false).is_err());
    let signed = ProxyUsageReceipt {
        body: decode(&row["receipt"]),
        buyer_sig: "a".repeat(128),
        provider_sig: "b".repeat(128),
    };
    signed
        .verify(&terms, &decode(&row["policy"]), None, |sig, bytes, key| {
            if sig.starts_with('a') {
                bytes == signed.body.buyer_signing_bytes().unwrap() && key == terms.buyer_pubkey
            } else {
                bytes == signed.body.provider_signing_bytes().unwrap()
                    && key == terms.offer.provider_pubkey
            }
        })
        .unwrap();
    assert!(signed
        .verify(&terms, &decode(&row["policy"]), None, |_, _, _| false)
        .is_err());
}

#[test]
fn unresolved_exposure_is_not_reset_on_retry_or_included_as_delivered_work() {
    let row = &fixture()["cases"][0];
    let terms: ProxySpendTerms = decode(&row["terms"]);
    let policy: ProxySettlementPolicy = decode(&row["policy"]);
    let receipt: ProxyReceiptBody = decode(&row["receipt"]);
    assert!(terms.prior_reserved_au > 0);
    assert_eq!(
        receipt.billing_au_owed_cum,
        terms.prior_spend_au + receipt.au_owed_cum
    );
    let mut understated = terms.clone();
    understated.max_total_spend_au -= terms.prior_reserved_au;
    assert!(understated.validate().is_err());
    let mut changed = terms.clone();
    changed.prior_reserved_au = 0;
    assert!(receipt.validate_for(&changed, &policy, None).is_err());
    let mut over = terms.clone();
    over.prior_reserved_au = u128::MAX;
    assert!(over.validate().is_err());
}

#[test]
fn closure_expiry_vectors_and_original_policy_opt_in_match_javascript() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/proxy-closure-v1.json")).unwrap();
    for row in fixtures["cases"].as_array().unwrap() {
        let terms: ProxySpendTerms = decode(&row["terms"]);
        let policy: ProxySettlementPolicy = decode(&row["policy"]);
        let closure: ProxyClosureBody = decode(&row["closure"]);
        let expiry: ProxyExpiryBody = decode(&row["expiry"]);
        assert_eq!(policy.digest().unwrap(), row["digests"]["policy"]);
        assert_eq!(terms.digest().unwrap(), row["digests"]["terms"]);
        assert_eq!(closure.digest().unwrap(), row["digests"]["closure"]);
        assert_eq!(expiry.digest().unwrap(), row["digests"]["expiry"]);
        for (name, bytes) in [
            ("buyer_closure", closure.buyer_signing_bytes().unwrap()),
            (
                "provider_closure",
                closure.provider_signing_bytes().unwrap(),
            ),
            ("buyer_expiry", expiry.buyer_signing_bytes().unwrap()),
        ] {
            assert_eq!(String::from_utf8(bytes).unwrap(), row["signing_utf8"][name]);
        }
        let signed = ProxyReservationClosure {
            body: closure.clone(),
            buyer_sig: "a".repeat(128),
            provider_sig: "b".repeat(128),
        };
        signed
            .verify(&terms, |sig, bytes, key| {
                (sig == "a".repeat(128)
                    && bytes == closure.buyer_signing_bytes().unwrap()
                    && key == terms.buyer_pubkey)
                    || (sig == "b".repeat(128)
                        && bytes == closure.provider_signing_bytes().unwrap()
                        && key == terms.offer.provider_pubkey)
            })
            .unwrap();
        assert!(signed.verify(&terms, |_, _, _| false).is_err());
        let signed = ProxyReservationExpiry {
            body: expiry.clone(),
            buyer_sig: "a".repeat(128),
        };
        signed
            .verify(&terms, &policy, 75, |_, bytes, key| {
                bytes == expiry.buyer_signing_bytes().unwrap() && key == terms.buyer_pubkey
            })
            .unwrap();
        assert!(signed.verify(&terms, &policy, 74, |_, _, _| true).is_err());
        assert!(signed.verify(&terms, &policy, 75, |_, _, _| false).is_err());
        let mut disabled = policy.clone();
        disabled.hold_expiry = None;
        assert!(signed
            .verify(&terms, &disabled, 75, |_, _, _| true)
            .is_err());
        for bad in [Value::Null, json!(false), json!("release_and_retry")] {
            let mut value = row["policy"].clone();
            value["hold_expiry"] = bad;
            assert!(serde_json::from_value::<ProxySettlementPolicy>(value).is_err());
        }
        let mut unsafe_time = row["closure"].clone();
        unsafe_time["at_ms"] = json!(9007199254740992_u64);
        assert!(serde_json::from_value::<ProxyClosureBody>(unsafe_time).is_err());
        let mut wrong_terms = terms.clone();
        wrong_terms.billing_id = "e".repeat(64);
        assert!(signed
            .verify(&wrong_terms, &policy, 75, |_, _, _| true)
            .is_err());
    }
}
