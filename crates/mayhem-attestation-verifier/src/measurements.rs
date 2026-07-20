use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{reject, Result};

pub(crate) type LayerMatches = BTreeMap<String, String>;

pub(crate) fn verify_policy_layer(
    layer: &str,
    policy_document: &Value,
    actual: &BTreeMap<String, String>,
) -> Result<LayerMatches> {
    let expected = expected_measurements(policy_document, layer)?;
    match_expected(layer, &expected, actual)
}

pub(crate) fn expected_measurements(
    document: &Value,
    layer: &str,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    expected_layer(document, layer)
}

pub(crate) fn verify_contract_layer(
    layer: &str,
    contract_layers: &Value,
    actual: &BTreeMap<String, String>,
) -> Result<LayerMatches> {
    let Some(value) = contract_layers.get(layer) else {
        return Ok(BTreeMap::new());
    };
    let expected = expected_object(value)?;
    if expected.is_empty() {
        return Ok(BTreeMap::new());
    }
    match_expected(layer, &expected, actual)
}

pub(crate) fn merge_matches(target: &mut LayerMatches, source: LayerMatches) -> Result<()> {
    for (name, measurement) in source {
        if target
            .insert(name.clone(), measurement.clone())
            .is_some_and(|existing| existing != measurement)
        {
            return reject(format!(
                "measurement {name} resolved inconsistently across policy and contract"
            ));
        }
    }
    Ok(())
}

fn expected_layer(document: &Value, layer: &str) -> Result<BTreeMap<String, BTreeSet<String>>> {
    if let Some(value) = document
        .get("layers")
        .and_then(|layers| layers.get(layer))
        .or_else(|| document.get(layer))
    {
        return expected_object(value);
    }
    expected_object(document)
}

fn expected_object(value: &Value) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let object = value.as_object().ok_or_else(|| {
        crate::VerifyError::Rejected("measurement collateral layer is not an object".into())
    })?;
    let mut expected = BTreeMap::new();
    for (name, value) in object {
        if matches!(
            name.as_str(),
            "schema_version" | "effective_epoch" | "platform" | "layer"
        ) {
            continue;
        }
        let mut values = BTreeSet::new();
        collect_hex_values(value, &mut values)?;
        if !values.is_empty() {
            expected.insert(name.clone(), values);
        }
    }
    if expected.is_empty() {
        return reject("measurement collateral contains no cryptographic measurements");
    }
    Ok(expected)
}

fn collect_hex_values(value: &Value, output: &mut BTreeSet<String>) -> Result<()> {
    match value {
        Value::String(value) => {
            let normalized = normalize_measurement(value)?;
            output.insert(normalized);
        }
        Value::Array(values) => {
            if values.is_empty() {
                return reject("measurement allow-list is empty");
            }
            for value in values {
                collect_hex_values(value, output)?;
            }
        }
        Value::Object(object) => {
            if let Some(values) = object.get("values") {
                collect_hex_values(values, output)?;
            } else if let Some(value) = object.get("measurement") {
                collect_hex_values(value, output)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn normalize_measurement(value: &str) -> Result<String> {
    let normalized = value
        .trim()
        .strip_prefix("0x")
        .unwrap_or(value.trim())
        .to_ascii_lowercase();
    if !(64..=128).contains(&normalized.len())
        || normalized.len() % 2 != 0
        || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return reject("golden measurement is not a bounded hexadecimal digest");
    }
    Ok(normalized)
}

fn match_expected(
    layer: &str,
    expected: &BTreeMap<String, BTreeSet<String>>,
    actual: &BTreeMap<String, String>,
) -> Result<LayerMatches> {
    let mut matched = BTreeMap::new();
    for (expected_name, allowed) in expected {
        let actual_name = actual
            .keys()
            .find(|actual_name| measurement_names_match(expected_name, actual_name))
            .ok_or_else(|| {
                crate::VerifyError::EvidenceGap(format!(
                    "{layer} evidence has no cryptographically verified {expected_name} measurement"
                ))
            })?;
        let value = actual
            .get(actual_name)
            .expect("alias was selected from the actual map");
        if !allowed.contains(value) {
            return reject(format!(
                "{layer} measurement {expected_name} is not in the admin golden set"
            ));
        }
        matched.insert(expected_name.clone(), value.clone());
    }
    Ok(matched)
}

fn measurement_names_match(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    if matches!(
        expected,
        "measurement" | "launch_measurement" | "snp_launch_digest"
    ) && actual == "snp_launch_measurement"
    {
        return true;
    }
    match (pcr_index(expected), pcr_index(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => false,
    }
}

pub(crate) fn pcr_index(name: &str) -> Option<u8> {
    let normalized = name.trim().to_ascii_lowercase().replace('-', "_");
    let suffix = normalized
        .strip_prefix("vtpm_pcr_")
        .or_else(|| normalized.strip_prefix("pcr_"))
        .or_else(|| normalized.strip_prefix("pcr"))
        .or_else(|| normalized.strip_prefix("sha256:"))?;
    let index = suffix.parse::<u8>().ok()?;
    (index <= 23).then_some(index)
}
