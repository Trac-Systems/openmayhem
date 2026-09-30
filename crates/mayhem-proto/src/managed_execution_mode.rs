//! Signed managed runtime alternatives share their canonical artifact/market.
//! Runtime dependencies belong to the mode policy, never the baseline enclave.

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedManagedExecutionMode {
    schema_version: u32,
    profile: SerializedManagedExecutionProfile,
    #[serde(default)]
    generation_execution_profile: Option<SerializedGenerationExecutionProfile>,
    requests: ExecutionModeRequestPolicy,
    canary: SerializedVllmExecutionModeCanary,
    canary_set_sha256: String,
    #[serde(rename = "modality_fingerprints")]
    _modality_fingerprints: BTreeMap<String, String>,
    #[serde(rename = "resource_profiles")]
    _resource_profiles: BTreeMap<String, Value>,
    #[serde(rename = "speciality_calibrations")]
    _speciality_calibrations: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedManagedExecutionProfile {
    schema_version: u32,
    engine: String,
    architecture: String,
    runtime: SerializedManagedRuntimeBinding,
    sidecars: BTreeMap<String, SerializedModeSidecar>,
    #[serde(rename = "proof_sha256", skip_serializing)]
    _proof_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedModeSidecar {
    source: SerializedModeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    upstream_source: Option<SerializedModeSource>,
    path: String,
    artifact_root: String,
    artifact_root_kind: String,
    weights_bytes: u64,
    source_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedModeSource {
    kind: String,
    repo: String,
    revision: String,
    #[serde(default)]
    publisher_key: Option<String>,
}

/// Derive a managed runtime's binding without changing the canonical artifact.
/// Existing vLLM bindings retain their original domain and byte projection.
pub fn managed_execution_mode_binding(
    artifact_root: &str,
    mode_id: &str,
    serialized_mode: &Value,
) -> Result<ExecutionModeBinding, String> {
    validate_execution_mode_id(mode_id)?;
    validate_execution_mode_policy_hash(artifact_root)
        .map_err(|_| "execution mode requires an exact lowercase artifact root".to_owned())?;
    let mode: SerializedManagedExecutionMode = serde_json::from_value(serialized_mode.clone())
        .map_err(|error| format!("invalid serialized managed execution mode: {error}"))?;
    if mode.schema_version != 1
        || mode.profile.schema_version != 1
        || mode.profile.engine != "openai-compatible"
        || !matches!(mode.profile.architecture.as_str(), "aarch64" | "x86_64")
        || mode.profile.runtime.schema_version != 1
    {
        return Err("unsupported managed execution mode profile".to_owned());
    }
    let generation = mode
        .generation_execution_profile
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let mut policy = serde_json::json!({
        "domain": "mayhem-managed-execution-mode-v1",
        "artifact_root": artifact_root,
        "mode_id": mode_id,
        "schema_version": mode.schema_version,
        "runtime": mode.profile,
        "requests": mode.requests,
        "canary_set": mode.canary.set_id,
        "canary_set_sha256": mode.canary_set_sha256,
        "verification_method": mode.canary.verification_method,
        "match_min": mode.canary.match_min,
        "verification_tolerance_bps": mode.canary.verification_tolerance_bps,
    });
    if let Some(generation) = generation {
        policy["generation_execution_profile"] = generation;
    }
    let bytes = stable_json_bytes(&policy).map_err(|error| error.to_string())?;
    Ok(ExecutionModeBinding {
        mode_id: mode_id.to_owned(),
        policy_hash: blake3::hash(&bytes).to_hex().to_string(),
    })
}

// Keep wire-shape parity with mayhem-engine; catalog tests verify this projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedManagedRuntimeBinding {
    schema_version: u32,
    lifecycle: SerializedManagedLifecycle,
    runtime_id: String,
    runtime_version: String,
    runtime_revision: String,
    implementation: String,
    implementation_version: String,
    container_image_digest: String,
    served_model: String,
    native_context: u32,
    served_context: u32,
    max_concurrent: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    calibrated_peak_gpu_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    calibrated_peak_host_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_measurement_source: Option<String>,
    model_snapshot_file_count: u32,
    snapshot_manifest_sidecar: String,
    snapshot_manifest_sha256: String,
    runtime_recipe_sidecar: String,
    runtime_recipe_sha256: String,
    capabilities: BTreeSet<String>,
    server_info_checks: BTreeMap<String, Value>,
    preflight: SerializedManagedPreflightProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix_cache_metric: Option<String>,
}

/// Signed request controls and budgets used to prove live capabilities.  These
/// are catalog data because a model's default reasoning mode can consume a
/// short probe before it emits text, JSON, or a tool call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedManagedPreflightProfile {
    non_reasoning_chat_template_kwargs: BTreeMap<String, Value>,
    reasoning_chat_template_kwargs: BTreeMap<String, Value>,
    streaming_max_tokens: u32,
    tools_max_tokens: u32,
    json_max_tokens: u32,
    reasoning_max_tokens: u32,
    cache_max_tokens: u32,
    cancellation_max_tokens: u32,
    concurrency_max_tokens: u32,
    cancellation_idle_metrics: BTreeSet<String>,
    concurrency_active_metric: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SerializedManagedLifecycle {
    ManagedOrVerifiedAttach,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../test-data/managed-execution-mode.json")).unwrap()
    }

    #[test]
    fn managed_execution_mode_binds_runtime_dependencies_and_not_measured_evidence() {
        let root = "ab".repeat(32);
        let mode = fixture();
        let expected = managed_execution_mode_binding(&root, "arm", &mode).unwrap();
        for (pointer, value) in [
            ("/profile/architecture", json!("x86_64")),
            ("/profile/runtime/runtime_revision", json!("de".repeat(20))),
            (
                "/profile/runtime/container_image_digest",
                json!(format!("sha256:{}", "ef".repeat(32))),
            ),
            (
                "/profile/sidecars/gb10_recipe/source_sha256",
                json!("ff".repeat(32)),
            ),
            ("/profile/runtime/max_concurrent", json!(2)),
            ("/canary_set_sha256", json!("cc".repeat(32))),
        ] {
            let mut changed = mode.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert_ne!(
                managed_execution_mode_binding(&root, "arm", &changed).unwrap(),
                expected,
                "{pointer}"
            );
        }
        let mut evidence = mode.clone();
        evidence["profile"]["proof_sha256"] = json!("bb".repeat(32));
        evidence["canary"]["fingerprints"]["weights"] = json!("cc".repeat(32));
        assert_eq!(
            managed_execution_mode_binding(&root, "arm", &evidence).unwrap(),
            expected
        );
        assert!(vllm_execution_mode_binding(&root, "arm", &mode).is_err());
    }

    #[test]
    fn managed_execution_mode_rejects_unknown_nested_fields_and_wrong_profile() {
        for pointer in [
            "",
            "/profile",
            "/profile/runtime",
            "/profile/runtime/preflight",
            "/profile/sidecars/gb10_recipe",
            "/profile/sidecars/gb10_recipe/source",
        ] {
            let mut mode = fixture();
            mode.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unbound_override".into(), json!(true));
            assert!(
                managed_execution_mode_binding(&"ab".repeat(32), "arm", &mode).is_err(),
                "{pointer}"
            );
        }
        for (pointer, value) in [
            ("/schema_version", json!(2)),
            ("/profile/engine", json!("vllm")),
            ("/profile/architecture", json!("unknown")),
        ] {
            let mut mode = fixture();
            *mode.pointer_mut(pointer).unwrap() = value;
            assert!(managed_execution_mode_binding(&"ab".repeat(32), "arm", &mode).is_err());
        }
    }
}
