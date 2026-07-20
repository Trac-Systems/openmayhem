#![forbid(unsafe_code)]

mod amd;
mod collateral;
mod maa;
mod measurements;
mod tdx;
#[cfg(test)]
mod tests;
mod tpm;

use std::collections::{BTreeMap, BTreeSet};

use mayhem_attestation::{EvidenceBinding, ValidatedAttestationPolicy};
use mayhem_proto::{
    AdminAttestationPolicy, AdminEnclaveAttestationBinding, AttestationMeasurementLayer,
    AttestationVerifierProfile, HardwareQuote, HardwareQuoteKind, HardwareQuoteRoutePolicyBinding,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use collateral::AuthenticatedCollateral;
use measurements::{merge_matches, verify_contract_layer, verify_policy_layer};

pub const MAX_VERIFIER_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const VERIFIER_ID: &str = "mayhem-attestation-verifier";
pub const VERIFIER_VERSION: u32 = 1;
const VERIFIER_INPUT_SCHEMA_VERSION: u32 = 1;
const MANAGED_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const VERIFIER_IDENTITY_SCHEMA_VERSION: u32 = 1;
const PUBLIC_TRUST_SOURCE: &str = "authenticated_admin_policy_input";

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct VerifierIdentity {
    pub schema_version: u32,
    pub verifier_id: &'static str,
    pub version: u32,
    pub profiles: BTreeMap<AttestationVerifierProfile, BTreeSet<u32>>,
    pub max_input_bytes: usize,
    pub public_trust_source: &'static str,
}

pub fn verifier_identity() -> VerifierIdentity {
    VerifierIdentity {
        schema_version: VERIFIER_IDENTITY_SCHEMA_VERSION,
        verifier_id: VERIFIER_ID,
        version: VERIFIER_VERSION,
        profiles: BTreeMap::from([
            (
                AttestationVerifierProfile::AmdSevSnpVcekV1,
                BTreeSet::from([MANAGED_EVIDENCE_SCHEMA_VERSION]),
            ),
            (
                AttestationVerifierProfile::IntelTdxDcapV1,
                BTreeSet::from([MANAGED_EVIDENCE_SCHEMA_VERSION]),
            ),
            (
                AttestationVerifierProfile::NvidiaNrasCompositeV1,
                BTreeSet::from([MANAGED_EVIDENCE_SCHEMA_VERSION]),
            ),
            (
                AttestationVerifierProfile::NvidiaNvtrustOfflineCompositeV1,
                BTreeSet::from([MANAGED_EVIDENCE_SCHEMA_VERSION]),
            ),
        ]),
        max_input_bytes: MAX_VERIFIER_INPUT_BYTES,
        public_trust_source: PUBLIC_TRUST_SOURCE,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyRequest {
    pub schema_version: u32,
    pub kind: HardwareQuoteKind,
    pub expected_binding: String,
    pub declared_platform: Option<String>,
    pub evidence_binding: EvidenceBindingInput,
    pub admin_policy: AdminPolicyInput,
    pub admin_enclave_binding: AdminEnclaveAttestationBinding,
    pub policy_measurement_collateral: BTreeMap<AttestationMeasurementLayer, Value>,
    pub managed_verifier: ManagedVerifierInput,
    pub golden_measurement_layers: Value,
    pub quote: HardwareQuote,
    pub report: mayhem_proto::AttestationReport,
    pub contract: ContractInput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBindingInput {
    pub kind: HardwareQuoteKind,
    pub evidence_schema_version: u32,
    pub policy_sequence: u64,
    pub policy_digest: String,
    pub platform: Option<String>,
    pub nonce: String,
    pub enclave_id: String,
    pub device_id: String,
    pub quote_binding: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminPolicyInput {
    pub digest: String,
    pub record: AdminAttestationPolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedVerifierInput {
    pub id: String,
    pub version: u32,
    pub executable_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractInput {
    pub enclave_id: String,
    pub admin_pubkey: String,
    pub model_id: String,
    pub model_class: String,
    pub artifact_root: String,
    pub artifact_sidecar_roots: BTreeMap<String, String>,
    pub manifest_hash: String,
    pub binary_hash: String,
    pub launch_measurements: Value,
    pub att_tier: u8,
    pub caps: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedEvidenceEnvelope {
    schema_version: u32,
    platform: String,
    cpu: CpuEvidence,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case")]
enum CpuEvidence {
    AmdSevSnpVcekV1 {
        report_b64: String,
        vcek_der_b64: String,
    },
    IntelTdxDcapV1 {
        quote_b64: String,
        collateral: dcap_qvl::QuoteCollateralV3,
    },
    AzureMaaSnpV1 {
        maa_jwt: String,
        workload_quote: mayhem_proto::TpmQuoteEvidence,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct VerifierVerdict {
    pub ok: bool,
    pub kind: String,
    pub binding: String,
    pub att_tier: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub matched_measurements: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snp_chip_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snp_chip_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snp_tcb: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct VerifiedCpu {
    roots: Vec<String>,
    cpu_measurements: BTreeMap<String, String>,
    workload_measurements: BTreeMap<String, String>,
    device_id: String,
    snp_chip_family: Option<String>,
    snp_chip_id: Option<String>,
    snp_tcb: Option<String>,
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("{0}")]
    Rejected(String),
    #[error("cryptographic evidence gap: {0}")]
    EvidenceGap(String),
}

type Result<T> = std::result::Result<T, VerifyError>;

pub fn verify_json_at(input: &[u8], now_unix: u64) -> VerifierVerdict {
    let parsed = serde_json::from_slice::<VerifyRequest>(input);
    let request = match parsed {
        Ok(request) => request,
        Err(error) => {
            return rejected_verdict(
                "invalid",
                "",
                0,
                format!("strict verifier input JSON is invalid: {error}"),
            );
        }
    };
    let kind = request.kind.as_str();
    let binding = request.expected_binding.clone();
    let tier = request.kind.attestation_tier();
    match verify_request_at(&request, now_unix) {
        Ok(verdict) => verdict,
        Err(error) => rejected_verdict(kind, binding, tier, error.to_string()),
    }
}

pub fn rejected_verdict(
    kind: impl Into<String>,
    binding: impl Into<String>,
    att_tier: u8,
    reason: impl Into<String>,
) -> VerifierVerdict {
    VerifierVerdict {
        ok: false,
        kind: kind.into(),
        binding: binding.into(),
        att_tier,
        reason: Some(reason.into()),
        roots: Vec::new(),
        matched_measurements: BTreeMap::new(),
        platform: None,
        platform_id: None,
        snp_chip_family: None,
        snp_chip_id: None,
        snp_tcb: None,
    }
}

pub fn verify_request_at(request: &VerifyRequest, now_unix: u64) -> Result<VerifierVerdict> {
    let validated = validate_request(request)?;
    validate_environment(request)?;
    reject_provider_authority(&request.quote.metadata)?;
    let collateral = AuthenticatedCollateral::from_endorsements(
        &validated,
        request.kind,
        &request.quote.endorsements,
    )?;
    let envelope = managed_evidence(request)?;
    if envelope.schema_version != MANAGED_EVIDENCE_SCHEMA_VERSION {
        return reject(format!(
            "managed evidence schema {} is unsupported; expected {}",
            envelope.schema_version, MANAGED_EVIDENCE_SCHEMA_VERSION
        ));
    }
    let platform = request
        .declared_platform
        .as_deref()
        .ok_or_else(|| VerifyError::Rejected("managed proof has no declared platform".into()))?;
    if envelope.platform != platform {
        return reject("evidence platform does not match the policy-bound platform");
    }

    let verified = match &envelope.cpu {
        CpuEvidence::AmdSevSnpVcekV1 {
            report_b64,
            vcek_der_b64,
        } => amd::verify(
            request,
            &validated,
            &collateral,
            report_b64,
            vcek_der_b64,
            now_unix,
        )?,
        CpuEvidence::IntelTdxDcapV1 {
            quote_b64,
            collateral: dcap,
        } => tdx::verify(request, &validated, &collateral, quote_b64, dcap, now_unix)?,
        CpuEvidence::AzureMaaSnpV1 {
            maa_jwt,
            workload_quote,
        } => maa::verify(
            request,
            &validated,
            &collateral,
            maa_jwt,
            workload_quote,
            now_unix,
        )?,
    };
    if verified.device_id != request.evidence_binding.device_id {
        return reject("verified CPU device identity does not match the selected route");
    }

    let cpu_policy = request
        .policy_measurement_collateral
        .get(&AttestationMeasurementLayer::Cpu)
        .ok_or_else(|| VerifyError::Rejected("admin policy has no CPU measurement layer".into()))?;
    let workload_policy = request
        .policy_measurement_collateral
        .get(&AttestationMeasurementLayer::Workload)
        .ok_or_else(|| {
            VerifyError::Rejected("admin policy has no workload measurement layer".into())
        })?;
    let cpu_matches = verify_policy_layer("cpu", cpu_policy, &verified.cpu_measurements)?;
    let workload_matches =
        verify_policy_layer("workload", workload_policy, &verified.workload_measurements)?;
    if matches!(
        request.kind,
        HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
    ) {
        let gpu_policy = request
            .policy_measurement_collateral
            .get(&AttestationMeasurementLayer::Gpu)
            .ok_or_else(|| {
                VerifyError::Rejected("admin policy has no GPU measurement layer".into())
            })?;
        measurements::expected_measurements(gpu_policy, "gpu")?;
    }

    let mut vendor = cpu_matches;
    let contract_vendor = verify_contract_layer(
        "vendor",
        &request.golden_measurement_layers,
        &verified.cpu_measurements,
    )?;
    merge_matches(&mut vendor, contract_vendor)?;
    let mut workload = workload_matches;
    let contract_workload = verify_contract_layer(
        "workload",
        &request.golden_measurement_layers,
        &verified.workload_measurements,
    )?;
    merge_matches(&mut workload, contract_workload)?;

    let mut matched_measurements = BTreeMap::new();
    matched_measurements.insert("vendor".to_owned(), vendor);
    matched_measurements.insert("workload".to_owned(), workload);
    let mut roots = verified.roots;
    if matches!(
        request.kind,
        HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
    ) {
        roots.extend([
            "nvidia_gpu_cert_chain".to_owned(),
            "nvidia_driver_rim".to_owned(),
            "nvidia_vbios_rim".to_owned(),
        ]);
    }
    roots.sort();
    roots.dedup();

    Ok(VerifierVerdict {
        ok: true,
        kind: request.kind.as_str().to_owned(),
        binding: request.expected_binding.clone(),
        att_tier: request.kind.attestation_tier(),
        reason: None,
        roots,
        matched_measurements,
        platform: request.declared_platform.clone(),
        platform_id: request.declared_platform.clone(),
        snp_chip_family: verified.snp_chip_family,
        snp_chip_id: verified.snp_chip_id,
        snp_tcb: verified.snp_tcb,
    })
}

fn validate_request(request: &VerifyRequest) -> Result<ValidatedAttestationPolicy> {
    if request.schema_version != VERIFIER_INPUT_SCHEMA_VERSION {
        return reject(format!(
            "verifier input schema {} is unsupported",
            request.schema_version
        ));
    }
    if !managed_kind(request.kind) {
        return reject(format!(
            "{} is a native gateway profile, not a managed verifier profile",
            request.kind.as_str()
        ));
    }
    if request.quote.kind != request.kind || request.evidence_binding.kind != request.kind {
        return reject("quote kind, request kind, and evidence kind differ");
    }
    if request.kind.attestation_tier() != 3
        || request.report.att_tier != 3
        || request.contract.att_tier != 3
    {
        return reject("managed confidential-compute profiles require exact Tier 3");
    }
    if request.report.hw_quote.as_ref() != Some(&request.quote) {
        return reject("standalone quote differs from the signed attestation report quote");
    }
    if request.quote.binding != request.evidence_binding.quote_binding {
        return reject("quote binding differs from the immutable evidence binding");
    }
    require_digest("quote binding", &request.quote.binding)?;
    require_digest("expected binding", &request.expected_binding)?;
    if request.report.nonce_u != request.evidence_binding.nonce {
        return reject("report nonce differs from the immutable evidence binding");
    }
    if request.report.enclave_id != request.evidence_binding.enclave_id
        || request.contract.enclave_id != request.evidence_binding.enclave_id
        || request.admin_enclave_binding.enclave_id != request.evidence_binding.enclave_id
    {
        return reject("enclave identity differs across report, contract, policy, and evidence");
    }
    if request.admin_enclave_binding.kind != request.kind {
        return reject("admin enclave binding has the wrong quote kind");
    }
    if request.declared_platform != request.evidence_binding.platform
        || request.declared_platform != request.admin_enclave_binding.platform
    {
        return reject("platform differs across route, evidence, and admin enclave binding");
    }
    if request.managed_verifier.id != VERIFIER_ID
        || request.managed_verifier.version != VERIFIER_VERSION
    {
        return reject("managed verifier identity or version does not match this executable");
    }
    require_digest(
        "managed verifier executable digest",
        &request.managed_verifier.executable_sha256,
    )?;

    let policy = ValidatedAttestationPolicy::validate(request.admin_policy.record.clone())
        .map_err(|error| VerifyError::Rejected(format!("admin policy is invalid: {error}")))?;
    if policy.digest() != request.admin_policy.digest
        || policy.digest() != request.evidence_binding.policy_digest
        || policy.policy().sequence != request.evidence_binding.policy_sequence
    {
        return reject("admin policy digest or sequence differs from the evidence binding");
    }
    let kind_policy = policy
        .quote_kind(request.kind)
        .ok_or_else(|| VerifyError::Rejected("admin policy has no exact quote kind".into()))?;
    if !kind_policy.enabled
        || policy
            .policy()
            .emergency_disabled_quote_kinds
            .contains(&request.kind)
    {
        return reject("quote kind is not enabled by the active admin policy");
    }
    if kind_policy.verifier_profile != expected_profile(request.kind)
        || kind_policy.evidence_schema_version != request.evidence_binding.evidence_schema_version
    {
        return reject("managed verifier profile or evidence schema differs from policy");
    }
    if request.managed_verifier.version < policy.policy().min_verifier_version {
        return reject("managed verifier version is below the active policy minimum");
    }
    let platform = request
        .declared_platform
        .as_ref()
        .ok_or_else(|| VerifyError::Rejected("managed Tier-3 policy has no platform".into()))?;
    if !kind_policy.platforms.contains(platform) {
        return reject("declared platform is not allowed by the quote-kind policy");
    }
    let expected_layers = match request.kind {
        HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
            BTreeSet::from([
                AttestationMeasurementLayer::Cpu,
                AttestationMeasurementLayer::Gpu,
                AttestationMeasurementLayer::Workload,
            ])
        }
        _ => BTreeSet::from([
            AttestationMeasurementLayer::Cpu,
            AttestationMeasurementLayer::Workload,
        ]),
    };
    if kind_policy.required_measurement_layers != expected_layers
        || request
            .admin_enclave_binding
            .measurement_trust_data
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != expected_layers
        || request
            .policy_measurement_collateral
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != expected_layers
    {
        return reject("admin policy does not carry the exact CPU/GPU/workload layer set");
    }
    for (layer, id) in &request.admin_enclave_binding.measurement_trust_data {
        let reference = policy
            .trust_data(id)
            .ok_or_else(|| VerifyError::Rejected(format!("unknown measurement trust data {id}")))?;
        if reference.kind != mayhem_proto::AttestationTrustDataKind::Measurement
            || !kind_policy.measurement_trust_data.contains(id)
            || !kind_policy.required_trust_data.contains(id)
            || !request.policy_measurement_collateral.contains_key(layer)
        {
            return reject(format!(
                "measurement layer {layer:?} is not bound to active admin collateral"
            ));
        }
    }

    let route = HardwareQuoteRoutePolicyBinding {
        enclave_id: request.evidence_binding.enclave_id.clone(),
        device_id: request.evidence_binding.device_id.clone(),
        kind: request.kind,
        evidence_schema_version: request.evidence_binding.evidence_schema_version,
        policy_sequence: request.evidence_binding.policy_sequence,
        policy_digest: request.evidence_binding.policy_digest.clone(),
        platform: request.evidence_binding.platform.clone(),
    };
    let binding = EvidenceBinding::new(
        &route,
        request.evidence_binding.nonce.clone(),
        request.evidence_binding.enclave_id.clone(),
        request.evidence_binding.device_id.clone(),
        request.evidence_binding.quote_binding.clone(),
    )
    .map_err(|error| VerifyError::Rejected(format!("evidence binding is invalid: {error}")))?;
    if hex::encode(
        binding
            .digest()
            .map_err(|error| VerifyError::Rejected(error.to_string()))?,
    ) != request.expected_binding
    {
        return reject("expected binding is not the canonical policy-bound evidence digest");
    }
    Ok(policy)
}

fn managed_evidence(request: &VerifyRequest) -> Result<ManagedEvidenceEnvelope> {
    if matches!(
        request.kind,
        HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
    ) {
        let value = serde_json::from_str::<Value>(&request.quote.evidence).map_err(|error| {
            VerifyError::Rejected(format!(
                "NVIDIA composite evidence is invalid JSON: {error}"
            ))
        })?;
        let cpu = value.get("mayhem_cpu_evidence").ok_or_else(|| {
            VerifyError::EvidenceGap(
                "NVIDIA composite evidence has no cryptographic mayhem_cpu_evidence half".into(),
            )
        })?;
        let cpu = serde_json::from_value::<CpuEvidence>(cpu.clone()).map_err(|error| {
            VerifyError::Rejected(format!("NVIDIA CPU evidence is invalid: {error}"))
        })?;
        let platform = value
            .get("platform_id")
            .and_then(Value::as_str)
            .or(request.declared_platform.as_deref())
            .ok_or_else(|| VerifyError::Rejected("NVIDIA evidence has no platform".into()))?
            .to_owned();
        return Ok(ManagedEvidenceEnvelope {
            schema_version: MANAGED_EVIDENCE_SCHEMA_VERSION,
            platform,
            cpu,
        });
    }
    let envelope = serde_json::from_str::<ManagedEvidenceEnvelope>(&request.quote.evidence)
        .map_err(|error| {
            VerifyError::Rejected(format!("managed evidence envelope is invalid: {error}"))
        })?;
    match (request.kind, &envelope.cpu) {
        (HardwareQuoteKind::AmdSevSnpVcek, CpuEvidence::AmdSevSnpVcekV1 { .. })
        | (HardwareQuoteKind::AmdSevSnpVcek, CpuEvidence::AzureMaaSnpV1 { .. })
        | (HardwareQuoteKind::IntelTdxDcap, CpuEvidence::IntelTdxDcapV1 { .. }) => {}
        _ => return reject("CPU evidence profile does not match the outer quote kind"),
    }
    Ok(envelope)
}

fn validate_environment(request: &VerifyRequest) -> Result<()> {
    let mut values = BTreeMap::new();
    for name in [
        "MAYHEM_HW_VERIFY_KIND",
        "MAYHEM_HW_VERIFY_BINDING",
        "MAYHEM_HW_VERIFY_PLATFORM",
        "MAYHEM_HW_VERIFY_ATTESTATION_TIER",
    ] {
        if let Some(actual) = std::env::var_os(name) {
            values.insert(
                name,
                actual
                    .into_string()
                    .map_err(|_| VerifyError::Rejected(format!("{name} is not UTF-8")))?,
            );
        }
    }
    validate_environment_values(request, |name| values.get(name).cloned())
}

fn validate_environment_values(
    request: &VerifyRequest,
    mut value: impl FnMut(&str) -> Option<String>,
) -> Result<()> {
    for (name, expected) in [
        ("MAYHEM_HW_VERIFY_KIND", request.kind.as_str().to_owned()),
        ("MAYHEM_HW_VERIFY_BINDING", request.expected_binding.clone()),
        (
            "MAYHEM_HW_VERIFY_PLATFORM",
            request.declared_platform.clone().unwrap_or_default(),
        ),
        (
            "MAYHEM_HW_VERIFY_ATTESTATION_TIER",
            request.kind.attestation_tier().to_string(),
        ),
    ] {
        if let Some(actual) = value(name) {
            if actual != expected {
                return reject(format!("{name} disagrees with authenticated stdin"));
            }
        }
    }
    Ok(())
}

fn reject_provider_authority(metadata: &Value) -> Result<()> {
    let Some(object) = metadata.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        let normalized = key.to_ascii_lowercase().replace('-', "_");
        if matches!(
            normalized.as_str(),
            "root"
                | "roots"
                | "trust_root"
                | "trust_roots"
                | "jwks"
                | "jku"
                | "x5u"
                | "endpoint"
                | "verifier"
                | "verifier_command"
                | "policy"
                | "golden"
                | "golden_measurements"
        ) {
            return reject(format!(
                "provider quote metadata attempts to supply authority through {key}"
            ));
        }
    }
    Ok(())
}

fn expected_profile(kind: HardwareQuoteKind) -> AttestationVerifierProfile {
    match kind {
        HardwareQuoteKind::AmdSevSnpVcek => AttestationVerifierProfile::AmdSevSnpVcekV1,
        HardwareQuoteKind::IntelTdxDcap => AttestationVerifierProfile::IntelTdxDcapV1,
        HardwareQuoteKind::NvidiaNrasJwt => AttestationVerifierProfile::NvidiaNrasCompositeV1,
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
            AttestationVerifierProfile::NvidiaNvtrustOfflineCompositeV1
        }
        _ => unreachable!("caller checked managed kind"),
    }
}

fn managed_kind(kind: HardwareQuoteKind) -> bool {
    matches!(
        kind,
        HardwareQuoteKind::AmdSevSnpVcek
            | HardwareQuoteKind::IntelTdxDcap
            | HardwareQuoteKind::NvidiaNrasJwt
            | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
    )
}

fn require_digest(field: &str, value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        reject(format!("{field} must be a lowercase 32-byte digest"))
    }
}

fn device_digest(bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() || bytes.iter().all(|byte| *byte == 0) {
        return reject("hardware device identity is absent or masked");
    }
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn reject<T>(reason: impl Into<String>) -> Result<T> {
    Err(VerifyError::Rejected(reason.into()))
}

fn evidence_gap<T>(reason: impl Into<String>) -> Result<T> {
    Err(VerifyError::EvidenceGap(reason.into()))
}
