use std::collections::BTreeMap;

use mayhem_proto::proxy::{
    ProxyAdmissionPermit, ProxyMarketDescriptor, ProxyMembership, ProxyOffer,
};
use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/proxy-wire-v1.json")).unwrap()
}

fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap()
}

fn validate(kind: &str, value: &Value) -> Result<(), String> {
    match kind {
        "permit" => serde_json::from_value::<ProxyAdmissionPermit>(value.clone())
            .map_err(|e| e.to_string())?
            .validate(),
        "market" => serde_json::from_value::<ProxyMarketDescriptor>(value.clone())
            .map_err(|e| e.to_string())?
            .validate(),
        "membership" => serde_json::from_value::<ProxyMembership>(value.clone())
            .map_err(|e| e.to_string())?
            .validate(),
        "offer" => serde_json::from_value::<ProxyOffer>(value.clone())
            .map_err(|e| e.to_string())?
            .validate(),
        _ => panic!("unknown fixture record"),
    }
}

fn apply_change(value: &mut Value, change: &Value) {
    let (parent, key) = change["path"].as_str().unwrap().rsplit_once('/').unwrap();
    let target = value.pointer_mut(parent).unwrap().as_object_mut().unwrap();
    if change["op"] == "remove" {
        target.remove(key);
    } else {
        target.insert(key.to_owned(), change["value"].clone());
    }
}

#[test]
fn proxy_rust_and_javascript_sign_identical_bytes_and_hashes() {
    for row in fixture()["cases"].as_array().unwrap() {
        let market: ProxyMarketDescriptor = decode(&row["market"]);
        let member: ProxyMembership = decode(&row["membership"]);
        let offer: ProxyOffer = decode(&row["offer"]);
        let permit: ProxyAdmissionPermit = decode(&row["permit"]);
        offer.validate_for_membership(&market, &member).unwrap();
        assert_eq!(market.id().unwrap(), row["digests"]["market"]);
        assert_eq!(member.digest().unwrap(), row["digests"]["membership"]);
        assert_eq!(offer.digest().unwrap(), row["digests"]["offer"]);
        assert_eq!(offer.slot_id().unwrap(), row["digests"]["offer_slot"]);
        assert_eq!(permit.digest().unwrap(), row["digests"]["permit"]);
        assert_eq!(
            String::from_utf8(permit.signing_bytes().unwrap()).unwrap(),
            row["signing_utf8"]["permit"]
        );
        for (kind, bytes) in [
            ("market", market.signing_bytes().unwrap()),
            ("membership", member.signing_bytes().unwrap()),
            ("offer", offer.signing_bytes().unwrap()),
        ] {
            assert_eq!(String::from_utf8(bytes).unwrap(), row["signing_utf8"][kind]);
        }
    }
}

#[test]
fn proxy_shared_adversarial_corpus_rejects_invalid_records_and_bindings() {
    let fixture = fixture();
    for change in fixture["invalid"].as_array().unwrap() {
        let row = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == change["base"])
            .unwrap();
        let mut mutated = row.clone();
        let kind = change["record"].as_str().unwrap();
        apply_change(&mut mutated[kind], change);
        let result = if change["binding"] == true {
            let market: ProxyMarketDescriptor = decode(&mutated["market"]);
            let member: ProxyMembership = decode(&mutated["membership"]);
            match kind {
                "membership" => member.validate_for_market(&market),
                "offer" => decode::<ProxyOffer>(&mutated["offer"])
                    .validate_for_membership(&market, &member),
                _ => panic!("unsupported binding mutation"),
            }
        } else {
            validate(kind, &mutated[kind])
        };
        assert!(
            result.is_err(),
            "accepted invalid fixture: {}",
            change["name"]
        );
    }
}

#[test]
fn proxy_identity_excludes_provider_prices_and_hardware() {
    let row = &fixture()["cases"][0];
    let market: ProxyMarketDescriptor = decode(&row["market"]);
    let mut member: ProxyMembership = decode(&row["membership"]);
    let first: ProxyOffer = decode(&row["offer"]);
    let mut second = first.clone();
    member.provider_pubkey = "c".repeat(64);
    member.recipe_hash = "d".repeat(64);
    member.max_concurrency = 1;
    second.provider_pubkey = member.provider_pubkey.clone();
    second.rates[0].per_unit_au *= 100;
    second.validate_for_membership(&market, &member).unwrap();
    assert_eq!(first.market_id, second.market_id);
    assert_ne!(first.digest().unwrap(), second.digest().unwrap());
    let mut changed = market.clone();
    changed.model.revision = "new-weights".into();
    assert_ne!(market.id().unwrap(), changed.id().unwrap());
    changed = market.clone();
    changed.creator_pubkey = "f".repeat(64);
    assert_ne!(market.id().unwrap(), changed.id().unwrap());
    assert!(market.public_handle().unwrap().starts_with("proxy/"));
}

#[test]
fn proxy_rate_revision_and_accepted_snapshot_remain_distinct() {
    let old: ProxyOffer = decode(&fixture()["cases"][0]["offer"]);
    let accepted = old.clone();
    let mut next = old.clone();
    assert!(next.validate_revision_after(1).is_err());
    next.revision = 2;
    next.rates[1].per_unit_au *= 3;
    next.validate_revision_after(1).unwrap();
    assert!(next.validate_revision_after(2).is_err());
    assert_eq!(old.digest().unwrap(), accepted.digest().unwrap());
    assert_ne!(accepted.digest().unwrap(), next.digest().unwrap());
    let usage = BTreeMap::from([("output_token".into(), 10)]);
    assert_eq!(
        accepted.cost(&usage).unwrap() * 3,
        next.cost(&usage).unwrap()
    );
}

#[test]
fn proxy_integer_costs_round_each_unit_and_enforce_full_map_and_overflow() {
    let mut offer: ProxyOffer = decode(&fixture()["cases"][0]["offer"]);
    offer.rates[0].per_unit_au = 1;
    offer.rates[0].granularity = 3;
    offer.rates[1].per_unit_au = 2;
    offer.rates[1].granularity = 3;
    offer.per_request_au = 7;
    let usage = BTreeMap::from([("input_token".into(), 1), ("output_token".into(), 1)]);
    assert_eq!(offer.cost(&usage).unwrap(), 9);
    offer.min_session_au = 10;
    assert_eq!(offer.cost(&usage).unwrap(), 10);
    assert!(offer
        .cost(&BTreeMap::from([("hidden_compute".into(), 1)]))
        .is_err());
    offer.rates[0].per_unit_au = u128::MAX;
    assert!(offer
        .cost(&BTreeMap::from([("input_token".into(), 2)]))
        .is_err());
    offer.rates[0].granularity = 1;
    assert!(offer
        .cost(&BTreeMap::from([("input_token".into(), 1)]))
        .is_err());
    offer.per_request_au = 0;
    assert_eq!(
        offer
            .cost(&BTreeMap::from([("input_token".into(), 1)]))
            .unwrap(),
        u128::MAX
    );
}

#[test]
fn proxy_json_integer_spelling_matches_javascript_and_rejects_strings() {
    let row = &fixture()["cases"][0];
    let member: ProxyMembership = decode(&row["membership"]);
    let serialized = serde_json::to_string(&member).unwrap();
    for text in ["1.0", "1e0", "1.0000000000000000"] {
        let changed = serialized.replace("\"revision\":1", &format!("\"revision\":{text}"));
        let decoded: ProxyMembership = serde_json::from_str(&changed).unwrap();
        assert_eq!(decoded.digest().unwrap(), member.digest().unwrap());
    }
    let mut changed = row["membership"].clone();
    changed["revision"] = json!("1");
    assert!(serde_json::from_value::<ProxyMembership>(changed).is_err());
}

#[test]
fn proxy_rail_subset_and_private_configuration_are_not_trusted_implicitly() {
    let row = &fixture()["cases"][0];
    let market: ProxyMarketDescriptor = decode(&row["market"]);
    let mut member: ProxyMembership = decode(&row["membership"]);
    let offer: ProxyOffer = decode(&row["offer"]);
    member.accepted_rails.truncate(1);
    assert!(offer.validate_for_membership(&market, &member).is_err());
    // A disclosed URL cannot be smuggled into signed membership records.
    let mut json = row["membership"].clone();
    json["upstream_url"] = json!("http://127.0.0.1:1/admin");
    assert!(serde_json::from_value::<ProxyMembership>(json).is_err());
}
