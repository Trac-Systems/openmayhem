use std::collections::{BTreeMap, BTreeSet};

use mayhem_proto::{
    endpoint_contract_fingerprint, generate_endpoint_calibration_cases,
    materialize_endpoint_calibration_request, validate_endpoint_request,
    validate_endpoint_response, EndpointCalibrationCase, EndpointFamilyContract,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const ENDPOINT_CALIBRATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EndpointCalibrationStageStatus {
    Passed,
    RejectedAsExpected,
    BlockedByExpectedRejection,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EndpointCalibrationStageReport {
    pub(crate) status: EndpointCalibrationStageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EndpointCalibrationCaseReport {
    pub(crate) case_id: String,
    pub(crate) endpoint_family: String,
    pub(crate) case_kind: String,
    pub(crate) attributes: Vec<String>,
    pub(crate) expect_accept: bool,
    pub(crate) contract_fingerprint: String,
    pub(crate) request_fingerprint: String,
    pub(crate) contract_validation: EndpointCalibrationStageReport,
    pub(crate) gateway_normalization: EndpointCalibrationStageReport,
    pub(crate) provider_translation: EndpointCalibrationStageReport,
    pub(crate) backend_execution: EndpointCalibrationStageReport,
    pub(crate) response_normalization: EndpointCalibrationStageReport,
    pub(crate) ok: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EndpointFamilyCalibrationReport {
    pub(crate) endpoint_family: String,
    pub(crate) contract_fingerprint: String,
    pub(crate) matrix_fingerprint: String,
    pub(crate) case_count: usize,
    pub(crate) cases: Vec<EndpointCalibrationCaseReport>,
    pub(crate) ok: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EndpointCalibrationReport {
    pub(crate) schema_version: u32,
    pub(crate) matrix_fingerprint: String,
    pub(crate) family_count: usize,
    pub(crate) case_count: usize,
    pub(crate) families: Vec<EndpointFamilyCalibrationReport>,
    pub(crate) ok: bool,
}

pub(crate) struct EndpointCalibrationExecution {
    pub(crate) provider_translation_fingerprint: String,
    pub(crate) handled_request_attributes: BTreeSet<String>,
    pub(crate) backend_execution_fingerprint: String,
    pub(crate) response: Value,
}

pub(crate) fn run_endpoint_calibration_matrix<F>(
    contracts: &[EndpointFamilyContract],
    substitutions: &BTreeMap<String, Value>,
    mut execute: F,
) -> EndpointCalibrationReport
where
    F: FnMut(
        &EndpointFamilyContract,
        &EndpointCalibrationCase,
        &Value,
    ) -> Result<EndpointCalibrationExecution, String>,
{
    let mut families = contracts
        .iter()
        .map(|contract| run_endpoint_family_calibration(contract, substitutions, &mut execute))
        .collect::<Vec<_>>();
    families.sort_by(|left, right| left.endpoint_family.cmp(&right.endpoint_family));
    let case_count = families.iter().map(|family| family.case_count).sum();
    let ok = !families.is_empty() && families.iter().all(|family| family.ok);
    let matrix_fingerprint = aggregate_matrix_fingerprint(&families);
    EndpointCalibrationReport {
        schema_version: ENDPOINT_CALIBRATION_SCHEMA_VERSION,
        matrix_fingerprint,
        family_count: families.len(),
        case_count,
        families,
        ok,
    }
}

fn run_endpoint_family_calibration<F>(
    contract: &EndpointFamilyContract,
    substitutions: &BTreeMap<String, Value>,
    execute: &mut F,
) -> EndpointFamilyCalibrationReport
where
    F: FnMut(
        &EndpointFamilyContract,
        &EndpointCalibrationCase,
        &Value,
    ) -> Result<EndpointCalibrationExecution, String>,
{
    let contract_fingerprint = endpoint_contract_fingerprint(contract);
    let generated = generate_endpoint_calibration_cases(contract);
    let cases = match generated {
        Ok(cases) => cases,
        Err(error) => {
            return EndpointFamilyCalibrationReport {
                endpoint_family: contract.family.clone(),
                contract_fingerprint,
                matrix_fingerprint: blake3_hex(error.as_bytes()),
                case_count: 0,
                cases: Vec::new(),
                ok: false,
            };
        }
    };
    let matrix_fingerprint = calibration_cases_fingerprint(&cases);
    let reports = cases
        .iter()
        .map(|case| run_endpoint_calibration_case(contract, case, substitutions, execute))
        .collect::<Vec<_>>();
    let ok = !reports.is_empty() && reports.iter().all(|report| report.ok);
    EndpointFamilyCalibrationReport {
        endpoint_family: contract.family.clone(),
        contract_fingerprint,
        matrix_fingerprint,
        case_count: reports.len(),
        cases: reports,
        ok,
    }
}

fn run_endpoint_calibration_case<F>(
    contract: &EndpointFamilyContract,
    case: &EndpointCalibrationCase,
    substitutions: &BTreeMap<String, Value>,
    execute: &mut F,
) -> EndpointCalibrationCaseReport
where
    F: FnMut(
        &EndpointFamilyContract,
        &EndpointCalibrationCase,
        &Value,
    ) -> Result<EndpointCalibrationExecution, String>,
{
    let request = match materialize_endpoint_calibration_request(case, substitutions) {
        Ok(request) => request,
        Err(error) => return failed_materialization_report(case, error),
    };
    if let Some(marker) = unresolved_calibration_marker(&request) {
        return failed_materialization_report(
            case,
            format!("calibration fixture {marker} was not resolved"),
        );
    }
    let request_fingerprint = stable_value_fingerprint(&request);
    let contract_result = validate_endpoint_request(contract, &request);
    let gateway_result =
        mayhem_gateway::openai::normalize_endpoint_request_for_provider(contract, &request);

    if !case.expect_accept {
        let contract_validation = match contract_result {
            Err(_) => expected_rejection_stage(),
            Ok(()) => failed_stage("contract accepted an invalid calibration request"),
        };
        let gateway_normalization = match gateway_result {
            Err(_) => expected_rejection_stage(),
            Ok(_) => failed_stage("gateway accepted an invalid calibration request"),
        };
        let ok = contract_validation.status == EndpointCalibrationStageStatus::RejectedAsExpected
            && gateway_normalization.status == EndpointCalibrationStageStatus::RejectedAsExpected;
        return EndpointCalibrationCaseReport {
            case_id: case.case_id.clone(),
            endpoint_family: case.endpoint_family.clone(),
            case_kind: case.case_kind.clone(),
            attributes: case.attributes.clone(),
            expect_accept: false,
            contract_fingerprint: case.contract_fingerprint.clone(),
            request_fingerprint,
            contract_validation,
            gateway_normalization,
            provider_translation: blocked_stage(),
            backend_execution: blocked_stage(),
            response_normalization: blocked_stage(),
            ok,
        };
    }

    let contract_validation = match contract_result {
        Ok(()) => passed_stage(Some(request_fingerprint.clone())),
        Err(violations) => {
            return failed_valid_request_report(
                case,
                request_fingerprint,
                format!(
                    "generated valid request failed contract validation: {}",
                    violations
                        .iter()
                        .map(|violation| format!("{}: {}", violation.path, violation.reason))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            )
        }
    };
    let normalized = match gateway_result {
        Ok(normalized) => normalized,
        Err(error) => {
            return failed_gateway_report(case, request_fingerprint, contract_validation, error)
        }
    };
    let gateway_normalization = passed_stage(Some(normalized.normalized_request_fingerprint));
    let execution = match execute(contract, case, &normalized.normalized_request) {
        Ok(execution) => execution,
        Err(error) => {
            return failed_execution_report(
                case,
                request_fingerprint,
                contract_validation,
                gateway_normalization,
                error,
            )
        }
    };
    let present_attributes = contract
        .request_attributes
        .iter()
        .filter(|path| value_has_path(&normalized.normalized_request, path))
        .cloned()
        .collect::<BTreeSet<_>>();
    let missing_attributes = present_attributes
        .difference(&execution.handled_request_attributes)
        .cloned()
        .collect::<Vec<_>>();
    let unknown_attributes = execution
        .handled_request_attributes
        .difference(
            &contract
                .request_attributes
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
        )
        .cloned()
        .collect::<Vec<_>>();
    if !missing_attributes.is_empty() || !unknown_attributes.is_empty() {
        let mut reasons = Vec::new();
        if !missing_attributes.is_empty() {
            reasons.push(format!(
                "provider translation did not handle signed request attribute(s): {}",
                missing_attributes.join(", ")
            ));
        }
        if !unknown_attributes.is_empty() {
            reasons.push(format!(
                "provider translation claimed unknown request attribute(s): {}",
                unknown_attributes.join(", ")
            ));
        }
        return failed_execution_report(
            case,
            request_fingerprint,
            contract_validation,
            gateway_normalization,
            reasons.join("; "),
        );
    }
    let provider_translation = passed_stage(Some(execution.provider_translation_fingerprint));
    let backend_execution = passed_stage(Some(execution.backend_execution_fingerprint));
    let response_fingerprint = stable_value_fingerprint(&execution.response);
    let response_result =
        validate_endpoint_response(contract, &execution.response).map_err(|violations| {
            violations
                .iter()
                .map(|violation| format!("{}: {}", violation.path, violation.reason))
                .collect::<Vec<_>>()
                .join("; ")
        });
    let missing_response_attributes = case
        .expected_response_attributes
        .iter()
        .filter(|path| !value_has_path(&execution.response, path))
        .cloned()
        .collect::<Vec<_>>();
    let response_normalization = match (response_result, missing_response_attributes.is_empty()) {
        (Ok(()), true) => passed_stage(Some(response_fingerprint)),
        (Err(error), _) => failed_stage(format!(
            "public response violates the signed endpoint contract: {error}"
        )),
        (Ok(()), false) => failed_stage(format!(
            "public response is missing matrix attribute(s): {}",
            missing_response_attributes.join(", ")
        )),
    };
    let ok = response_normalization.status == EndpointCalibrationStageStatus::Passed;
    EndpointCalibrationCaseReport {
        case_id: case.case_id.clone(),
        endpoint_family: case.endpoint_family.clone(),
        case_kind: case.case_kind.clone(),
        attributes: case.attributes.clone(),
        expect_accept: true,
        contract_fingerprint: case.contract_fingerprint.clone(),
        request_fingerprint,
        contract_validation,
        gateway_normalization,
        provider_translation,
        backend_execution,
        response_normalization,
        ok,
    }
}

pub(crate) fn validate_endpoint_calibration_report(
    contracts: &[EndpointFamilyContract],
    report: &EndpointCalibrationReport,
) -> Vec<String> {
    let mut errors = Vec::new();
    if report.schema_version != ENDPOINT_CALIBRATION_SCHEMA_VERSION {
        errors.push(format!(
            "endpoint calibration schema version {} is unsupported",
            report.schema_version
        ));
    }
    let mut expected_families = contracts
        .iter()
        .filter_map(|contract| {
            generate_endpoint_calibration_cases(contract)
                .map(|cases| {
                    (
                        contract.family.clone(),
                        endpoint_contract_fingerprint(contract),
                        calibration_cases_fingerprint(&cases),
                        cases,
                    )
                })
                .map_err(|error| {
                    errors.push(format!(
                        "cannot regenerate endpoint calibration matrix for {}: {error}",
                        contract.family
                    ));
                })
                .ok()
        })
        .collect::<Vec<_>>();
    expected_families.sort_by(|left, right| left.0.cmp(&right.0));
    if report.family_count != report.families.len() {
        errors.push(format!(
            "endpoint calibration family_count {} does not match {} family reports",
            report.family_count,
            report.families.len()
        ));
    }
    if report.family_count != expected_families.len() {
        errors.push(format!(
            "endpoint calibration covers {} families, expected {}",
            report.family_count,
            expected_families.len()
        ));
    }
    let mut report_families = BTreeMap::new();
    for family in &report.families {
        if report_families
            .insert(family.endpoint_family.as_str(), family)
            .is_some()
        {
            errors.push(format!(
                "endpoint calibration duplicates family {}",
                family.endpoint_family
            ));
        }
    }
    for (family_name, contract_fingerprint, matrix_fingerprint, expected_cases) in
        &expected_families
    {
        let Some(family) = report_families.get(family_name.as_str()) else {
            errors.push(format!(
                "endpoint calibration is missing family {family_name}"
            ));
            continue;
        };
        if &family.contract_fingerprint != contract_fingerprint {
            errors.push(format!(
                "endpoint calibration contract fingerprint differs for {family_name}"
            ));
        }
        if &family.matrix_fingerprint != matrix_fingerprint {
            errors.push(format!(
                "endpoint calibration matrix fingerprint differs for {family_name}"
            ));
        }
        if family.case_count != family.cases.len() || family.case_count != expected_cases.len() {
            errors.push(format!(
                "endpoint calibration case count differs for {family_name}: report {} / rows {}, expected {}",
                family.case_count,
                family.cases.len(),
                expected_cases.len()
            ));
        }
        validate_family_cases(family_name, expected_cases, family, &mut errors);
        if !family.ok || family.cases.iter().any(|case| !case.ok) {
            errors.push(format!(
                "endpoint calibration family {family_name} contains failed rows"
            ));
        }
    }
    let expected_case_count = expected_families
        .iter()
        .map(|(_, _, _, cases)| cases.len())
        .sum::<usize>();
    if report.case_count
        != report
            .families
            .iter()
            .map(|family| family.cases.len())
            .sum::<usize>()
    {
        errors.push("endpoint calibration case_count does not match report rows".to_owned());
    }
    if report.case_count != expected_case_count {
        errors.push(format!(
            "endpoint calibration covers {} cases, expected {expected_case_count}",
            report.case_count
        ));
    }
    let expected_matrix_fingerprint = aggregate_expected_matrix_fingerprint(&expected_families);
    if report.matrix_fingerprint != expected_matrix_fingerprint {
        errors.push("endpoint calibration aggregate matrix fingerprint differs".to_owned());
    }
    if !report.ok || !errors.is_empty() {
        errors.push("endpoint calibration report is not wholly successful".to_owned());
    }
    errors
}

fn validate_family_cases(
    family_name: &str,
    expected_cases: &[EndpointCalibrationCase],
    family: &EndpointFamilyCalibrationReport,
    errors: &mut Vec<String>,
) {
    let mut cases = BTreeMap::new();
    for case in &family.cases {
        if cases.insert(case.case_id.as_str(), case).is_some() {
            errors.push(format!(
                "endpoint calibration family {family_name} duplicates case {}",
                case.case_id
            ));
        }
    }
    for expected in expected_cases {
        let Some(case) = cases.get(expected.case_id.as_str()) else {
            errors.push(format!(
                "endpoint calibration family {family_name} is missing case {}",
                expected.case_id
            ));
            continue;
        };
        if case.endpoint_family != expected.endpoint_family
            || case.case_kind != expected.case_kind
            || case.attributes != expected.attributes
            || case.expect_accept != expected.expect_accept
            || case.contract_fingerprint != expected.contract_fingerprint
        {
            errors.push(format!(
                "endpoint calibration case {} metadata differs from generated matrix",
                expected.case_id
            ));
        }
        if !is_blake3_hex(&case.request_fingerprint) {
            errors.push(format!(
                "endpoint calibration case {} has an invalid request fingerprint",
                expected.case_id
            ));
        }
        validate_case_stages(expected, case, errors);
    }
}

fn validate_case_stages(
    expected: &EndpointCalibrationCase,
    case: &EndpointCalibrationCaseReport,
    errors: &mut Vec<String>,
) {
    let expected_statuses = if expected.expect_accept {
        [
            EndpointCalibrationStageStatus::Passed,
            EndpointCalibrationStageStatus::Passed,
            EndpointCalibrationStageStatus::Passed,
            EndpointCalibrationStageStatus::Passed,
            EndpointCalibrationStageStatus::Passed,
        ]
    } else {
        [
            EndpointCalibrationStageStatus::RejectedAsExpected,
            EndpointCalibrationStageStatus::RejectedAsExpected,
            EndpointCalibrationStageStatus::BlockedByExpectedRejection,
            EndpointCalibrationStageStatus::BlockedByExpectedRejection,
            EndpointCalibrationStageStatus::BlockedByExpectedRejection,
        ]
    };
    let stages = [
        ("contract_validation", &case.contract_validation),
        ("gateway_normalization", &case.gateway_normalization),
        ("provider_translation", &case.provider_translation),
        ("backend_execution", &case.backend_execution),
        ("response_normalization", &case.response_normalization),
    ];
    for ((name, stage), expected_status) in stages.into_iter().zip(expected_statuses) {
        if stage.status != expected_status {
            errors.push(format!(
                "endpoint calibration case {} stage {name} is {:?}, expected {:?}",
                expected.case_id, stage.status, expected_status
            ));
        }
        if stage.status == EndpointCalibrationStageStatus::Passed {
            if !stage.fingerprint.as_deref().is_some_and(is_blake3_hex) {
                errors.push(format!(
                    "endpoint calibration case {} stage {name} has no valid fingerprint",
                    expected.case_id
                ));
            }
            if stage.error.is_some() {
                errors.push(format!(
                    "endpoint calibration case {} stage {name} passed with an error",
                    expected.case_id
                ));
            }
        } else if stage.fingerprint.is_some() {
            errors.push(format!(
                "endpoint calibration case {} stage {name} unexpectedly has a fingerprint",
                expected.case_id
            ));
        }
    }
    if !case.ok {
        errors.push(format!(
            "endpoint calibration case {} is not successful",
            expected.case_id
        ));
    }
}

fn failed_materialization_report(
    case: &EndpointCalibrationCase,
    error: String,
) -> EndpointCalibrationCaseReport {
    EndpointCalibrationCaseReport {
        case_id: case.case_id.clone(),
        endpoint_family: case.endpoint_family.clone(),
        case_kind: case.case_kind.clone(),
        attributes: case.attributes.clone(),
        expect_accept: case.expect_accept,
        contract_fingerprint: case.contract_fingerprint.clone(),
        request_fingerprint: blake3_hex(error.as_bytes()),
        contract_validation: failed_stage(format!("materializing request: {error}")),
        gateway_normalization: failed_stage("request was not materialized"),
        provider_translation: failed_stage("request was not materialized"),
        backend_execution: failed_stage("request was not materialized"),
        response_normalization: failed_stage("request was not materialized"),
        ok: false,
    }
}

fn failed_valid_request_report(
    case: &EndpointCalibrationCase,
    request_fingerprint: String,
    error: String,
) -> EndpointCalibrationCaseReport {
    EndpointCalibrationCaseReport {
        case_id: case.case_id.clone(),
        endpoint_family: case.endpoint_family.clone(),
        case_kind: case.case_kind.clone(),
        attributes: case.attributes.clone(),
        expect_accept: true,
        contract_fingerprint: case.contract_fingerprint.clone(),
        request_fingerprint,
        contract_validation: failed_stage(error),
        gateway_normalization: failed_stage("contract validation failed"),
        provider_translation: failed_stage("contract validation failed"),
        backend_execution: failed_stage("contract validation failed"),
        response_normalization: failed_stage("contract validation failed"),
        ok: false,
    }
}

fn failed_gateway_report(
    case: &EndpointCalibrationCase,
    request_fingerprint: String,
    contract_validation: EndpointCalibrationStageReport,
    error: String,
) -> EndpointCalibrationCaseReport {
    EndpointCalibrationCaseReport {
        case_id: case.case_id.clone(),
        endpoint_family: case.endpoint_family.clone(),
        case_kind: case.case_kind.clone(),
        attributes: case.attributes.clone(),
        expect_accept: true,
        contract_fingerprint: case.contract_fingerprint.clone(),
        request_fingerprint,
        contract_validation,
        gateway_normalization: failed_stage(error),
        provider_translation: failed_stage("gateway normalization failed"),
        backend_execution: failed_stage("gateway normalization failed"),
        response_normalization: failed_stage("gateway normalization failed"),
        ok: false,
    }
}

fn failed_execution_report(
    case: &EndpointCalibrationCase,
    request_fingerprint: String,
    contract_validation: EndpointCalibrationStageReport,
    gateway_normalization: EndpointCalibrationStageReport,
    error: String,
) -> EndpointCalibrationCaseReport {
    EndpointCalibrationCaseReport {
        case_id: case.case_id.clone(),
        endpoint_family: case.endpoint_family.clone(),
        case_kind: case.case_kind.clone(),
        attributes: case.attributes.clone(),
        expect_accept: true,
        contract_fingerprint: case.contract_fingerprint.clone(),
        request_fingerprint,
        contract_validation,
        gateway_normalization,
        provider_translation: failed_stage(error),
        backend_execution: failed_stage("provider translation or execution failed"),
        response_normalization: failed_stage("provider translation or execution failed"),
        ok: false,
    }
}

fn passed_stage(fingerprint: Option<String>) -> EndpointCalibrationStageReport {
    EndpointCalibrationStageReport {
        status: EndpointCalibrationStageStatus::Passed,
        fingerprint,
        error: None,
    }
}

fn expected_rejection_stage() -> EndpointCalibrationStageReport {
    EndpointCalibrationStageReport {
        status: EndpointCalibrationStageStatus::RejectedAsExpected,
        fingerprint: None,
        error: None,
    }
}

fn blocked_stage() -> EndpointCalibrationStageReport {
    EndpointCalibrationStageReport {
        status: EndpointCalibrationStageStatus::BlockedByExpectedRejection,
        fingerprint: None,
        error: None,
    }
}

fn failed_stage(error: impl Into<String>) -> EndpointCalibrationStageReport {
    EndpointCalibrationStageReport {
        status: EndpointCalibrationStageStatus::Failed,
        fingerprint: None,
        error: Some(error.into()),
    }
}

fn stable_value_fingerprint(value: &Value) -> String {
    blake3_hex(
        &mayhem_proto::stable_json_bytes(value)
            .expect("endpoint calibration values are JSON serializable"),
    )
}

fn calibration_cases_fingerprint(cases: &[EndpointCalibrationCase]) -> String {
    let bytes = serde_json::to_vec(cases).expect("endpoint calibration cases serialize");
    blake3_hex(&bytes)
}

fn aggregate_matrix_fingerprint(families: &[EndpointFamilyCalibrationReport]) -> String {
    let rows = families
        .iter()
        .map(|family| {
            (
                family.endpoint_family.as_str(),
                family.contract_fingerprint.as_str(),
                family.matrix_fingerprint.as_str(),
                family.case_count,
            )
        })
        .collect::<Vec<_>>();
    blake3_hex(&serde_json::to_vec(&rows).expect("endpoint family rows serialize"))
}

fn aggregate_expected_matrix_fingerprint(
    families: &[(String, String, String, Vec<EndpointCalibrationCase>)],
) -> String {
    let rows = families
        .iter()
        .map(|(family, contract, matrix, cases)| {
            (
                family.as_str(),
                contract.as_str(),
                matrix.as_str(),
                cases.len(),
            )
        })
        .collect::<Vec<_>>();
    blake3_hex(&serde_json::to_vec(&rows).expect("expected endpoint family rows serialize"))
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn is_blake3_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn value_has_path(value: &Value, path: &str) -> bool {
    value_has_segments(value, &path.split('.').collect::<Vec<_>>())
}

fn value_has_segments(value: &Value, segments: &[&str]) -> bool {
    if segments.is_empty() {
        return true;
    }
    match value {
        Value::Object(object) => object
            .get(segments[0])
            .is_some_and(|child| value_has_segments(child, &segments[1..])),
        Value::Array(items) => items.iter().any(|item| value_has_segments(item, segments)),
        _ => false,
    }
}

fn unresolved_calibration_marker(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) if value.starts_with('$') => Some(value),
        Value::Array(items) => items.iter().find_map(unresolved_calibration_marker),
        Value::Object(object) => object.values().find_map(unresolved_calibration_marker),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn report_validation_rejects_missing_and_failed_rows() {
        let contract = mayhem_proto::endpoint_family_contract_template(
            mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
        )
        .unwrap();
        let substitutions = BTreeMap::from([("$MODEL".to_owned(), json!("test/model"))]);
        let mut report = run_endpoint_calibration_matrix(
            std::slice::from_ref(&contract),
            &substitutions,
            |contract, _case, request| {
                Ok(EndpointCalibrationExecution {
                    provider_translation_fingerprint: "11".repeat(32),
                    handled_request_attributes: contract
                        .request_attributes
                        .iter()
                        .filter(|path| value_has_path(request, path))
                        .cloned()
                        .collect(),
                    backend_execution_fingerprint: "22".repeat(32),
                    response: json!({
                        "id": "cmpl-test",
                        "object": "text_completion",
                        "created": 1,
                        "model": "test/model",
                        "choices": [{"index": 0, "text": "ok", "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                        "mayhem": {}
                    }),
                })
            },
        );
        let initial_errors =
            validate_endpoint_calibration_report(std::slice::from_ref(&contract), &report);
        let failed = report.families[0]
            .cases
            .iter()
            .filter(|case| !case.ok)
            .collect::<Vec<_>>();
        assert!(
            initial_errors.is_empty(),
            "{initial_errors:#?}\n{failed:#?}"
        );

        report.families[0].cases.pop();
        report.families[0].case_count -= 1;
        report.case_count -= 1;
        let errors = validate_endpoint_calibration_report(&[contract], &report);
        assert!(errors.iter().any(|error| error.contains("missing case")));
        assert!(errors.iter().any(|error| error.contains("case count")));
    }

    #[test]
    fn invalid_rows_must_be_rejected_by_contract_and_gateway() {
        let contract = mayhem_proto::endpoint_family_contract_template(
            mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
        )
        .unwrap();
        let substitutions = BTreeMap::from([("$MODEL".to_owned(), json!("test/model"))]);
        let report = run_endpoint_calibration_matrix(
            std::slice::from_ref(&contract),
            &substitutions,
            |contract, _case, request| {
                Ok(EndpointCalibrationExecution {
                    provider_translation_fingerprint: "11".repeat(32),
                    handled_request_attributes: contract
                        .request_attributes
                        .iter()
                        .filter(|path| value_has_path(request, path))
                        .cloned()
                        .collect(),
                    backend_execution_fingerprint: "22".repeat(32),
                    response: json!({
                        "id": "cmpl-test",
                        "object": "text_completion",
                        "created": 1,
                        "model": "test/model",
                        "choices": [{"index": 0, "text": "ok", "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                        "mayhem": {}
                    }),
                })
            },
        );
        let invalid = report.families[0]
            .cases
            .iter()
            .filter(|case| !case.expect_accept)
            .collect::<Vec<_>>();
        assert!(!invalid.is_empty());
        assert!(invalid.iter().all(|case| {
            case.contract_validation.status == EndpointCalibrationStageStatus::RejectedAsExpected
                && case.gateway_normalization.status
                    == EndpointCalibrationStageStatus::RejectedAsExpected
                && case.provider_translation.status
                    == EndpointCalibrationStageStatus::BlockedByExpectedRejection
        }));
    }
}
