use mayhem_proxy::routing::Policy;
use serde_json::{json, Value};

#[test]
fn actual_site_preparations_retain_identical_policy_semantics_and_hashes() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/routing-profiles-v1.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 4);
    for case in fixture["cases"].as_array().unwrap() {
        let raw = case["request"]["proxy"]["profile"].clone();
        let profile: Policy = serde_json::from_value(raw.clone()).unwrap();
        profile.validate().unwrap();
        assert_eq!(
            profile.digest().unwrap(),
            case["policy_hash"].as_str().unwrap()
        );
        assert_eq!(serde_json::to_value(profile).unwrap(), raw);
    }
}

#[test]
fn invalid_profile_fields_or_weakening_never_become_implicit_defaults() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/routing-profiles-v1.json")).unwrap();
    let original = &fixture["cases"][0]["request"]["proxy"]["profile"];
    for (path, bad) in [
        ("lane", json!("native")),
        ("schema_version", json!(2)),
        ("max_retail_cost_micro", json!("0")),
        ("max_retail_cost_micro", json!("9223372036854775808")),
        ("ranking", json!("highest_trust")),
        ("allowed_rails", json!(["tnk", "fiat"])),
    ] {
        let mut changed = original.clone();
        changed[path] = bad;
        assert!(serde_json::from_value::<Policy>(changed)
            .map(|v| v.validate().is_err())
            .unwrap_or(true));
    }
    for path in [
        "minimum_context",
        "minimum_tokens_per_second",
        "output_units",
    ] {
        let mut changed = original.clone();
        changed["constraints"].as_object_mut().unwrap().remove(path);
        assert!(serde_json::from_value::<Policy>(changed).is_err());
    }
    let mut changed = original.clone();
    changed["providers"]["allow"] = json!(["1".repeat(64)]);
    changed["providers"]["deny"] = json!(["1".repeat(64)]);
    assert!(serde_json::from_value::<Policy>(changed)
        .unwrap()
        .validate()
        .is_err());
}
