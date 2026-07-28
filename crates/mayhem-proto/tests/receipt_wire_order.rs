use mayhem_proto::{RateMapEntry, USAGE_INPUT_TOKEN};

#[test]
fn receipt_rate_map_value_keeps_rust_wire_order() {
    let wire_value = serde_json::json!({
        "locked_rate_map": [RateMapEntry {
            unit: USAGE_INPUT_TOKEN.to_owned(),
            per_unit_au: 10_000_000,
            granularity: 1,
        }],
    });

    assert_eq!(
        serde_json::to_string(&wire_value).unwrap(),
        concat!(
            "{\"locked_rate_map\":[",
            "{\"unit\":\"input_token\",\"per_unit_au\":\"10000000\",\"granularity\":1}",
            "]}"
        )
    );
}
