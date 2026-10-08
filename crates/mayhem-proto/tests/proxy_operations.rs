use mayhem_proto::proxy::ProxyOperation;
use serde_json::Value;

#[test]
fn proxy_operation_bytes_and_digests_match_javascript_for_every_action_and_family() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/proxy-operations-v1.json")).unwrap();
    for row in fixtures["cases"].as_array().unwrap() {
        let intent: ProxyOperation = serde_json::from_value(row["intent"].clone()).unwrap();
        assert_eq!(intent.digest().unwrap(), row["digest"]);
        assert_eq!(
            String::from_utf8(intent.signing_bytes().unwrap()).unwrap(),
            row["signing_utf8"]
        );
    }
}

#[test]
fn proxy_operations_reject_shared_malformed_and_cross_owner_cases() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/proxy-operations-v1.json")).unwrap();
    for bad in fixtures["invalid"].as_array().unwrap() {
        let row = fixtures["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == bad["base"])
            .unwrap();
        let mut intent = row["intent"].clone();
        let (parent, field) = bad["path"].as_str().unwrap().rsplit_once('/').unwrap();
        intent
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(field.into(), bad["value"].clone());
        let result = serde_json::from_value::<ProxyOperation>(intent)
            .map_err(|e| e.to_string())
            .and_then(|v| v.validate());
        assert!(result.is_err(), "accepted {}", bad["name"]);
    }
}
