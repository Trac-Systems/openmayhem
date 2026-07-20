use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use mayhem_attestation::{
    ActivatedTpmIdentity, AttestationPolicyChain, AttestationReadiness, CollateralInventory,
    EvidenceBinding, ValidatedAttestationPolicy, VerifierCapabilities,
};
use mayhem_proto::{
    AdminEnclaveAttestationBinding, AttestationMeasurementLayer, AttestationQuoteKindPolicy,
    AttestationTrustDataKind, AttestationVerifierProfile, HardwareQuoteKind,
    HardwareQuoteRoutePolicyBinding,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::HardwareQuoteVerifierCommand;

pub const GATEWAY_ATTESTATION_VERIFIER_VERSION: u32 = 3;
pub const MANAGED_VERIFIER_EXECUTABLE_MEDIA_TYPE: &str =
    ValidatedAttestationPolicy::MANAGED_VERIFIER_EXECUTABLE_MEDIA_TYPE;
pub const MANAGED_VERIFIER_MANIFEST_MEDIA_TYPE: &str =
    ValidatedAttestationPolicy::MANAGED_VERIFIER_MANIFEST_MEDIA_TYPE;
pub const MANAGED_VERIFIER_TARGETS: &[&str] = &ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS;
const MANAGED_VERIFIER_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug)]
pub struct AttestationPolicyVerificationContext<'a> {
    pub policy_chain: &'a AttestationPolicyChain,
    pub collateral: &'a CollateralInventory,
    pub route_binding: &'a HardwareQuoteRoutePolicyBinding,
    pub policy_epoch: u64,
    pub device_id: &'a str,
    pub activated_tpm_identity: Option<&'a ActivatedTpmIdentity>,
}

impl<'a> AttestationPolicyVerificationContext<'a> {
    pub fn new(
        policy_chain: &'a AttestationPolicyChain,
        collateral: &'a CollateralInventory,
        route_binding: &'a HardwareQuoteRoutePolicyBinding,
        policy_epoch: u64,
        device_id: &'a str,
    ) -> Self {
        Self {
            policy_chain,
            collateral,
            route_binding,
            policy_epoch,
            device_id,
            activated_tpm_identity: None,
        }
    }

    pub fn with_activated_tpm_identity(mut self, identity: &'a ActivatedTpmIdentity) -> Self {
        self.activated_tpm_identity = Some(identity);
        self
    }
}

pub(crate) struct ResolvedAttestationPolicy<'a> {
    pub policy: &'a ValidatedAttestationPolicy,
    pub kind_policy: &'a AttestationQuoteKindPolicy,
    pub enclave_binding: AdminEnclaveAttestationBinding,
    pub evidence_binding: EvidenceBinding,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedManagedVerifier {
    pub verifier_id: String,
    pub executable_sha256: String,
    pub capabilities: VerifierCapabilities,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedVerifierManifest {
    schema_version: u32,
    target: String,
    verifier_id: String,
    version: u32,
    executable_sha256: String,
    profiles: BTreeMap<AttestationVerifierProfile, BTreeSet<u32>>,
}

pub fn managed_verifier_target() -> Result<&'static str, String> {
    ValidatedAttestationPolicy::compiled_managed_verifier_target()
}

pub fn managed_verifier_executable_trust_data_id(kind: HardwareQuoteKind, target: &str) -> String {
    ValidatedAttestationPolicy::managed_verifier_trust_data_ids(kind, target)
        .expect("managed verifier executable id requires a canonical kind and target")
        .0
}

pub fn managed_verifier_manifest_trust_data_id(kind: HardwareQuoteKind, target: &str) -> String {
    ValidatedAttestationPolicy::managed_verifier_trust_data_ids(kind, target)
        .expect("managed verifier manifest id requires a canonical kind and target")
        .1
}

pub(crate) fn authenticate_managed_verifier(
    context: &AttestationPolicyVerificationContext<'_>,
    kind: HardwareQuoteKind,
    command: &HardwareQuoteVerifierCommand,
) -> Result<AuthenticatedManagedVerifier, String> {
    let policy = context
        .policy_chain
        .active_at(context.policy_epoch)
        .ok_or_else(|| "no active admin attestation policy".to_owned())?;
    let kind_policy = policy
        .quote_kind(kind)
        .ok_or_else(|| format!("active admin policy has no entry for {}", kind.as_str()))?;
    let target = managed_verifier_target()?;
    let executable_id = managed_verifier_executable_trust_data_id(kind, target);
    let manifest_id = managed_verifier_manifest_trust_data_id(kind, target);
    let executable_ref = policy.trust_data(&executable_id).ok_or_else(|| {
        format!("managed verifier executable for target {target} is not bound by active policy")
    })?;
    let manifest_ref = policy.trust_data(&manifest_id).ok_or_else(|| {
        format!("managed verifier manifest for target {target} is not bound by active policy")
    })?;
    if ValidatedAttestationPolicy::parse_managed_verifier_trust_data(executable_ref)
        .map_err(|err| err.to_string())?
        != Some((kind, target, "executable"))
    {
        return Err(format!(
            "managed verifier executable policy binding for target {target} is invalid"
        ));
    }
    if ValidatedAttestationPolicy::parse_managed_verifier_trust_data(manifest_ref)
        .map_err(|err| err.to_string())?
        != Some((kind, target, "manifest"))
    {
        return Err(format!(
            "managed verifier manifest policy binding for target {target} is invalid"
        ));
    }
    for (reference, collateral) in [(executable_ref, "executable"), (manifest_ref, "manifest")] {
        if !context
            .collateral
            .contains_reference_at(reference, context.policy_epoch)
        {
            return Err(format!(
                "managed verifier {collateral} collateral for target {target} is missing"
            ));
        }
        if reference
            .valid_from_epoch
            .is_some_and(|valid_from| context.policy_epoch < valid_from)
        {
            return Err(format!(
                "managed verifier {collateral} collateral for target {target} is not yet valid"
            ));
        }
        if reference
            .valid_until_epoch
            .is_some_and(|valid_until| context.policy_epoch >= valid_until)
        {
            return Err(format!(
                "managed verifier {collateral} collateral for target {target} is expired"
            ));
        }
    }
    let executable_collateral = context
        .collateral
        .get(&executable_ref.sha256)
        .expect("selected managed verifier executable collateral was checked above");
    let manifest_bytes = context
        .collateral
        .get(&manifest_ref.sha256)
        .expect("selected managed verifier manifest collateral was checked above")
        .bytes();
    let manifest = serde_json::from_slice::<ManagedVerifierManifest>(manifest_bytes)
        .map_err(|err| format!("managed verifier manifest is invalid: {err}"))?;
    if manifest.schema_version != MANAGED_VERIFIER_MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "managed verifier manifest schema {} is unsupported",
            manifest.schema_version
        ));
    }
    if manifest.target != target {
        return Err(format!(
            "managed verifier manifest target {} does not match runtime target {target}",
            manifest.target
        ));
    }
    if manifest.version < policy.policy().min_verifier_version {
        return Err(format!(
            "managed verifier version {} is older than active policy minimum {}",
            manifest.version,
            policy.policy().min_verifier_version
        ));
    }
    if manifest.verifier_id.is_empty()
        || manifest.verifier_id.len() > 128
        || !manifest
            .verifier_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("managed verifier manifest identity is invalid".to_owned());
    }
    if manifest.executable_sha256 != executable_ref.sha256 {
        return Err(
            "managed verifier manifest does not bind the policy executable digest".to_owned(),
        );
    }
    if !manifest
        .profiles
        .get(&kind_policy.verifier_profile)
        .is_some_and(|schemas| schemas.contains(&kind_policy.evidence_schema_version))
    {
        return Err(format!(
            "managed verifier {} does not advertise {:?} evidence schema {}",
            manifest.verifier_id, kind_policy.verifier_profile, kind_policy.evidence_schema_version
        ));
    }

    let metadata = fs::symlink_metadata(&command.command)
        .map_err(|err| format!("managed verifier executable metadata failed: {err}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("managed verifier command must be a regular non-symlink file".to_owned());
    }
    if metadata.len() > executable_ref.max_bytes {
        return Err("managed verifier executable exceeds its policy bound".to_owned());
    }
    let executable = fs::read(&command.command)
        .map_err(|err| format!("managed verifier executable read failed: {err}"))?;
    let executable_sha256 = hex::encode(Sha256::digest(&executable));
    if executable_sha256 != executable_ref.sha256 || executable != executable_collateral.bytes() {
        return Err(
            "managed verifier command is not the executable authenticated by active policy"
                .to_owned(),
        );
    }

    Ok(AuthenticatedManagedVerifier {
        verifier_id: manifest.verifier_id,
        executable_sha256,
        capabilities: VerifierCapabilities::with_evidence_schemas(
            manifest.version,
            manifest.profiles,
        ),
    })
}

pub(crate) fn resolve_attestation_policy<'a>(
    context: &'a AttestationPolicyVerificationContext<'a>,
    kind: HardwareQuoteKind,
    nonce: &str,
    enclave_id: &str,
    quote_binding: &str,
    managed_verifier: Option<&AuthenticatedManagedVerifier>,
) -> Result<ResolvedAttestationPolicy<'a>, String> {
    let capabilities = gateway_verifier_capabilities(managed_verifier);
    let readiness = AttestationReadiness::evaluate(
        context.policy_chain,
        context.collateral,
        &capabilities,
        context.policy_epoch,
    );
    readiness
        .verify_binding(context.route_binding)
        .map_err(|err| err.to_string())?;
    let enclave_binding = readiness
        .enclave_binding(context.route_binding)
        .map_err(|err| err.to_string())?
        .clone();
    if context.route_binding.kind != kind {
        return Err(format!(
            "route policy binding kind {} does not match quote kind {}",
            context.route_binding.kind.as_str(),
            kind.as_str()
        ));
    }

    let policy = context
        .policy_chain
        .active_at(context.policy_epoch)
        .ok_or_else(|| "no active admin attestation policy".to_owned())?;
    if readiness.policy_sequence != Some(policy.policy().sequence)
        || readiness.policy_digest.as_deref() != Some(policy.digest())
    {
        return Err("readiness did not resolve to the exact active admin policy".to_owned());
    }
    let kind_policy = policy
        .quote_kind(kind)
        .ok_or_else(|| format!("active admin policy has no entry for {}", kind.as_str()))?;
    if kind_policy.verifier_profile.quote_kind() != kind {
        return Err(format!(
            "active admin verifier profile {:?} does not match {}",
            kind_policy.verifier_profile,
            kind.as_str()
        ));
    }

    let evidence_binding = EvidenceBinding::new(
        context.route_binding,
        nonce,
        enclave_id,
        context.device_id,
        quote_binding,
    )
    .map_err(|err| err.to_string())?;
    Ok(ResolvedAttestationPolicy {
        policy,
        kind_policy,
        enclave_binding,
        evidence_binding,
    })
}

pub(crate) fn policy_measurement_json(
    resolved: &ResolvedAttestationPolicy<'_>,
    collateral: &CollateralInventory,
) -> Result<BTreeMap<AttestationMeasurementLayer, Value>, String> {
    let mut measurements = BTreeMap::new();
    for (layer, id) in &resolved.enclave_binding.measurement_trust_data {
        let reference = resolved
            .policy
            .trust_data(id)
            .ok_or_else(|| format!("active enclave binding references unknown trust data {id}"))?;
        if reference.kind != AttestationTrustDataKind::Measurement {
            return Err(format!(
                "active enclave binding trust data {id} is not measurement collateral"
            ));
        }
        if reference.media_type != "application/json" && !reference.media_type.ends_with("+json") {
            return Err(format!(
                "active enclave binding measurement {id} is not immutable JSON collateral"
            ));
        }
        let bytes = collateral
            .get(&reference.sha256)
            .ok_or_else(|| format!("policy measurement collateral {id} is missing"))?
            .bytes();
        let value = serde_json::from_slice(bytes)
            .map_err(|err| format!("policy measurement collateral {id} is invalid: {err}"))?;
        measurements.insert(*layer, value);
    }
    Ok(measurements)
}

pub(crate) fn policy_jwks(
    resolved: &ResolvedAttestationPolicy<'_>,
    collateral: &CollateralInventory,
) -> Result<Value, String> {
    let mut keys = Vec::new();
    let mut saw_key_material = false;
    for id in &resolved.kind_policy.required_trust_data {
        let reference = resolved
            .policy
            .trust_data(id)
            .ok_or_else(|| format!("active policy references unknown trust data {id}"))?;
        if !matches!(
            reference.kind,
            AttestationTrustDataKind::TrustAnchor | AttestationTrustDataKind::VerificationKey
        ) || !matches!(
            reference.media_type.as_str(),
            "application/jwk-set+json" | "application/json"
        ) {
            continue;
        }
        saw_key_material = true;
        let bytes = collateral
            .get(&reference.sha256)
            .ok_or_else(|| format!("policy trust data {id} is not in the collateral inventory"))?
            .bytes();
        let value = serde_json::from_slice::<Value>(bytes)
            .map_err(|err| format!("policy trust data {id} is not JWKS JSON: {err}"))?;
        let jwks = value.get("jwks").unwrap_or(&value);
        let policy_keys = jwks
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("policy trust data {id} is not a JWK set"))?;
        keys.extend(policy_keys.iter().cloned());
    }
    if !saw_key_material || keys.is_empty() {
        return Err("active policy contains no usable immutable JWK set".to_owned());
    }
    Ok(json!({ "keys": keys }))
}

fn gateway_verifier_capabilities(
    managed_verifier: Option<&AuthenticatedManagedVerifier>,
) -> VerifierCapabilities {
    let mut profiles = BTreeMap::from([
        (
            AttestationVerifierProfile::AppleAppAttestNativeV1,
            BTreeSet::from([1]),
        ),
        (
            AttestationVerifierProfile::NvidiaGb10DeviceV1,
            BTreeSet::from([1]),
        ),
        (
            AttestationVerifierProfile::Tpm2EkActivateCredentialV1,
            BTreeSet::from([1]),
        ),
    ]);
    let mut version = GATEWAY_ATTESTATION_VERIFIER_VERSION;
    if let Some(managed) = managed_verifier {
        version = version.max(managed.capabilities.version);
        profiles.extend(managed.capabilities.profiles.clone());
    }
    VerifierCapabilities::with_evidence_schemas(version, profiles)
}
