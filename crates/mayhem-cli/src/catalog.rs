use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signature, VerifyingKey};
use mayhem_attestation::AttestationPolicyChain;
use mayhem_gateway::MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS;
use mayhem_proto::{
    default_model_class, AdminAttestationPolicy, AdminEnclaveAttestationBinding,
    EndpointFamilyContract, EndpointSpecialityTarget, EndpointValueType, ModelSpecialityDescriptor,
    MoneyAu, DEFAULT_MODEL_CLASS, ENDPOINT_OPENAI_CHAT_COMPLETIONS, USAGE_AUDIO_SECOND,
    USAGE_FRAME, USAGE_IMAGE, USAGE_INPUT_CHARACTER, USAGE_INPUT_TOKEN, USAGE_OUTPUT_TOKEN,
    USAGE_STEP, USAGE_VIDEO_SECOND,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const VERIFICATION_TOKEN_FINGERPRINT: &str = "token_fingerprint";
const VERIFICATION_SEED_PERCEPTUAL_HASH: &str = "seed_perceptual_hash";
const VERIFICATION_EMBEDDING_COSINE: &str = "embedding_cosine";
const VERIFICATION_TRANSCRIPT_MATCH: &str = "transcript_match";
const VERIFICATION_AUDIO_FINGERPRINT: &str = "audio_fingerprint";
const VERIFICATION_ATTESTATION_OF_COMPUTE: &str = "attestation_of_compute";
const MODEL_CLASS_EMBEDDING: &str = "embedding";
const MODEL_CLASS_IMAGE_GENERATION: &str = "image-generation";
const MODEL_CLASS_VIDEO_GENERATION: &str = "video-generation";
const MODEL_CLASS_TTS: &str = "tts";
const MODEL_CLASS_STT: &str = "stt";
const MODEL_CLASS_AUDIO_GENERATION: &str = "audio-generation";
const MODEL_CLASS_MUSIC_GENERATION: &str = "music-generation";

#[derive(Debug, Clone)]
pub struct VerifyOptions {
    pub catalog_path: PathBuf,
    pub signature_path: PathBuf,
    pub keys_dir: PathBuf,
    pub canaries_dir: PathBuf,
    pub check_dev_downloads: bool,
    pub check_launch_sources: bool,
    pub hf_token_file: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct CatalogVerifyReport {
    pub ok: bool,
    pub catalog_path: PathBuf,
    pub signature_path: PathBuf,
    pub catalog_hash: String,
    pub key_id: String,
    pub model_count: usize,
    pub dev_model_count: usize,
    pub launch_model_count: usize,
    pub artifact_count: usize,
    pub canary_sets: Vec<String>,
    pub download_checks: Vec<DownloadCheckReport>,
    pub source_checks: Vec<SourceCheckReport>,
    pub errors: Vec<String>,
    pub attestation_policy_errors: Vec<String>,
    #[serde(skip_serializing)]
    attestation_authority: CatalogAttestationAuthority,
}

impl CatalogVerifyReport {
    pub(crate) fn verified_attestation_authority(&self) -> Option<&CatalogAttestationAuthority> {
        (self.ok && self.attestation_policy_errors.is_empty())
            .then_some(&self.attestation_authority)
    }
}

#[derive(Debug, Serialize)]
pub struct DownloadCheckReport {
    pub model_id: String,
    pub artifact: String,
    pub repo: String,
    pub revision: String,
    pub path: String,
    pub ok: bool,
}

#[derive(Debug, Serialize)]
pub struct SourceCheckReport {
    pub model_id: String,
    pub artifact: String,
    pub repo: String,
    pub revision: String,
    pub path: String,
    pub url: String,
    pub artifact_root_kind: String,
    pub source_sha256: Option<String>,
    pub status: Option<u16>,
    pub ok: bool,
    pub error: Option<String>,
    pub metadata_errors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogDocument {
    pub(crate) schema_version: u32,
    pub(crate) catalog_id: String,
    pub(crate) generated_at: String,
    pub(crate) models: Vec<CatalogModel>,
}

/// The optional attestation authority carried inside the signed catalog bytes.
///
/// This is decoded separately from `CatalogDocument` so existing catalog
/// construction tools cannot accidentally synthesize or preserve authority
/// fields without an explicit signed-catalog workflow.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CatalogAttestationAuthority {
    #[serde(default)]
    pub(crate) attestation_policy_chain: Option<Vec<AdminAttestationPolicy>>,
    #[serde(default)]
    pub(crate) enclave_attestation_bindings: Vec<AdminEnclaveAttestationBinding>,
}

impl CatalogAttestationAuthority {
    #[cfg(test)]
    pub(crate) fn is_tier1_only(&self) -> bool {
        self.attestation_policy_chain.is_none()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogModel {
    pub(crate) model_id: String,
    #[serde(default = "default_model_class")]
    pub(crate) model_class: String,
    pub(crate) family: String,
    pub(crate) params_b: f64,
    pub(crate) tier: String,
    #[serde(default)]
    pub(crate) min_app_version: Option<String>,
    pub(crate) provenance: Provenance,
    pub(crate) artifacts: BTreeMap<String, CatalogArtifact>,
    pub(crate) caps: CatalogCaps,
    pub(crate) requirements: CatalogRequirements,
    pub(crate) adapter: CatalogAdapter,
    pub(crate) modality_assessment: CatalogModalityAssessment,
    #[serde(default)]
    pub(crate) speciality_assessment: CatalogSpecialityAssessment,
    #[serde(default)]
    pub(crate) sampling: CatalogSamplingProfile,
    pub(crate) canary: CanaryRef,
    pub(crate) price_ref_au: PriceRef,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub(crate) struct CatalogSamplingProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) top_k: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) min_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repeat_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) frequency_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) presence_penalty: Option<f64>,
}

impl CatalogSamplingProfile {
    pub(crate) fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.top_k.is_none()
            && self.min_p.is_none()
            && self.repeat_penalty.is_none()
            && self.frequency_penalty.is_none()
            && self.presence_penalty.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct CatalogModalityAssessment {
    pub(crate) detected: Vec<String>,
    pub(crate) evidence: Vec<String>,
    #[serde(default)]
    pub(crate) calibrated_fingerprints: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub(crate) resource_profiles:
        BTreeMap<String, BTreeMap<String, CatalogModalityResourceProfile>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct CatalogModalityResourceProfile {
    pub(crate) unit: String,
    pub(crate) measurement_source: String,
    pub(crate) max_item_bytes: u64,
    pub(crate) max_item_units: u64,
    pub(crate) measured_item_bytes: u64,
    pub(crate) measured_item_units: u64,
    pub(crate) measured_working_set_bytes: u64,
    pub(crate) calibration_baseline_memory_bytes: u64,
    pub(crate) calibration_peak_memory_bytes: u64,
    pub(crate) calibration_f13_budget_bytes: u64,
    pub(crate) default_max_inflight_items: u32,
    pub(crate) default_max_items_per_request: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct CatalogSpecialityAssessment {
    #[serde(default)]
    pub(crate) detected: Vec<String>,
    #[serde(default)]
    pub(crate) evidence: Vec<String>,
    #[serde(default)]
    pub(crate) unsupported: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) calibrated:
        BTreeMap<String, BTreeMap<String, BTreeMap<String, CatalogSpecialityCalibration>>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct CatalogSpecialityCalibration {
    pub(crate) fingerprint: String,
    pub(crate) token_prefixes: BTreeMap<String, Vec<i32>>,
    pub(crate) output_tokens_min: u64,
    pub(crate) output_tokens_max: u64,
    pub(crate) reasoning_tokens_min: u64,
    pub(crate) reasoning_tokens_max: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Provenance {
    pub(crate) source: SourceRef,
    #[serde(default)]
    pub(crate) conversion: Vec<ConversionRef>,
    pub(crate) license: String,
    pub(crate) license_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SourceRef {
    pub(crate) kind: String,
    pub(crate) repo: String,
    pub(crate) revision: String,
    #[serde(default)]
    pub(crate) publisher_key: Option<String>,
}

pub(crate) fn huggingface_resolve_url(source: &SourceRef, path: &str) -> Result<String> {
    validate_huggingface_source("artifact", "source", source)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    validate_huggingface_path("artifact", "path", path)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    Ok(format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        source.repo, source.revision, path
    ))
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ConversionRef {
    pub(crate) tool: String,
    pub(crate) method: String,
    pub(crate) input_sha256: String,
    pub(crate) output_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogArtifact {
    pub(crate) engine: String,
    #[serde(default)]
    pub(crate) stable_diffusion_cpp: Option<mayhem_engine::StableDiffusionCppConfig>,
    pub(crate) source: SourceRef,
    #[serde(default)]
    pub(crate) upstream_source: Option<SourceRef>,
    pub(crate) path: String,
    pub(crate) artifact_root: String,
    pub(crate) artifact_root_kind: String,
    pub(crate) weights_bytes: u64,
    #[serde(default)]
    pub(crate) source_sha256: Option<String>,
    #[serde(default)]
    pub(crate) tokenizer_sha256: Option<String>,
    #[serde(default)]
    pub(crate) chat_template_sha256: Option<String>,
    #[serde(default)]
    pub(crate) min_compute_cap: Option<String>,
    #[serde(default)]
    pub(crate) download_check: bool,
    #[serde(default)]
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) sidecars: BTreeMap<String, CatalogArtifactSidecar>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct CatalogArtifactSidecar {
    pub(crate) source: SourceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) upstream_source: Option<SourceRef>,
    pub(crate) path: String,
    pub(crate) artifact_root: String,
    pub(crate) artifact_root_kind: String,
    pub(crate) weights_bytes: u64,
    pub(crate) source_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogCaps {
    pub(crate) tools: bool,
    pub(crate) json: bool,
    pub(crate) ctx_max: u64,
    pub(crate) vision: bool,
    #[serde(default)]
    pub(crate) image: bool,
    #[serde(default)]
    pub(crate) video: bool,
    #[serde(default)]
    pub(crate) audio: bool,
    #[serde(default)]
    pub(crate) output_modality: Option<String>,
    #[serde(default)]
    pub(crate) output_modalities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogRequirements {
    pub(crate) min_ram_gb: u64,
    pub(crate) min_vram_gb_full_offload: u64,
    #[serde(default)]
    pub(crate) cpu_flags: Vec<String>,
    #[serde(default)]
    pub(crate) backends: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub(crate) struct CatalogAdapter {
    pub(crate) endpoint_families: Vec<EndpointFamilyContract>,
    #[serde(default = "default_chat_template_id")]
    pub(crate) chat_template_id: String,
    #[serde(default = "default_tool_call_strategy")]
    pub(crate) tool_call_strategy: String,
    #[serde(default = "default_reasoning_passthrough")]
    pub(crate) reasoning_passthrough: String,
    pub(crate) modality_set: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) specialities: Vec<ModelSpecialityDescriptor>,
}

impl Default for CatalogAdapter {
    fn default() -> Self {
        Self {
            endpoint_families: vec![default_chat_endpoint_contract()],
            chat_template_id: default_chat_template_id(),
            tool_call_strategy: default_tool_call_strategy(),
            reasoning_passthrough: default_reasoning_passthrough(),
            modality_set: default_modality_set(),
            specialities: Vec::new(),
        }
    }
}

fn default_chat_endpoint_contract() -> EndpointFamilyContract {
    endpoint_contract_template(ENDPOINT_OPENAI_CHAT_COMPLETIONS)
        .expect("the built-in chat endpoint contract exists")
}

pub(crate) fn endpoint_contract_template(family: &str) -> Option<EndpointFamilyContract> {
    mayhem_proto::endpoint_family_contract_template(family)
}
fn default_chat_template_id() -> String {
    "generic_chatml".to_owned()
}

fn default_tool_call_strategy() -> String {
    "mayhem_json".to_owned()
}

fn default_reasoning_passthrough() -> String {
    "strip".to_owned()
}

fn default_modality_set() -> Vec<String> {
    vec!["text".to_owned()]
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CanaryRef {
    pub(crate) set_id: String,
    pub(crate) match_min: f64,
    #[serde(default = "default_canary_verification_method")]
    pub(crate) verification_method: String,
    #[serde(default)]
    pub(crate) verification_tolerance_bps: Option<u32>,
    #[serde(default)]
    pub(crate) fingerprints: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) token_prefixes: BTreeMap<String, BTreeMap<String, Vec<i32>>>,
    #[serde(default)]
    pub(crate) perceptual_hashes: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub(crate) embedding_vectors: BTreeMap<String, BTreeMap<String, Vec<f32>>>,
    #[serde(default)]
    pub(crate) transcripts: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub(crate) audio_fingerprints: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PriceRef {
    pub(crate) denom: String,
    #[serde(default, with = "mayhem_proto::decimal_u128")]
    pub(crate) in_per_1k: MoneyAu,
    #[serde(default, with = "mayhem_proto::decimal_u128")]
    pub(crate) out_per_1k: MoneyAu,
    #[serde(default)]
    pub(crate) rate_map: Vec<CatalogRateMapEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogRateMapEntry {
    pub(crate) unit: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub(crate) per_unit_au: MoneyAu,
    pub(crate) granularity: u64,
}

#[derive(Debug, Deserialize)]
struct CatalogSignature {
    schema_version: u32,
    alg: String,
    signed_path: String,
    key_id: String,
    public_key: String,
    blake3: String,
    sig: String,
}

#[derive(Debug, Deserialize)]
struct CatalogKey {
    key_id: String,
    alg: String,
    public_key: String,
    status: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct CanarySet {
    set_id: String,
    #[serde(default)]
    prompts: Vec<CanarySetPrompt>,
}

#[derive(Debug, Deserialize)]
struct CanarySetPrompt {
    id: String,
    #[serde(default)]
    messages: Vec<Value>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    audio_b64: Option<String>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    top_p: Option<f64>,
    #[serde(default)]
    top_k: Option<i32>,
    #[serde(default)]
    min_p: Option<f64>,
    #[serde(default)]
    repeat_penalty: Option<f64>,
    #[serde(default)]
    frequency_penalty: Option<f64>,
    #[serde(default)]
    presence_penalty: Option<f64>,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u32>,
}

fn default_canary_verification_method() -> String {
    VERIFICATION_TOKEN_FINGERPRINT.to_owned()
}

pub fn verify(options: VerifyOptions) -> Result<CatalogVerifyReport> {
    let mut errors = Vec::new();
    let catalog_bytes = fs::read(&options.catalog_path)
        .with_context(|| format!("reading {}", options.catalog_path.display()))?;
    let catalog_hash = blake3::hash(&catalog_bytes).to_hex().to_string();
    let signature_text = fs::read_to_string(&options.signature_path)
        .with_context(|| format!("reading {}", options.signature_path.display()))?;
    let signature: CatalogSignature = serde_json::from_str(&signature_text)
        .with_context(|| format!("parsing {}", options.signature_path.display()))?;

    validate_signature_metadata(&signature, &catalog_hash, &options, &mut errors);
    if errors.is_empty() {
        if let Err(err) = verify_signature_bytes(&catalog_bytes, &signature) {
            errors.push(err.to_string());
        }
    }

    let catalog: CatalogDocument = match serde_json::from_slice(&catalog_bytes)
        .with_context(|| format!("parsing {}", options.catalog_path.display()))
    {
        Ok(catalog) => catalog,
        Err(err) => {
            errors.push(err.to_string());
            return Ok(failed_report(options, signature, catalog_hash, errors));
        }
    };
    let (attestation_authority, attestation_policy_errors) =
        validated_catalog_attestation_authority(&catalog_bytes);

    let mut model_ids = BTreeSet::new();
    let mut canary_sets = BTreeSet::new();
    let mut artifact_count = 0usize;
    let mut dev_model_count = 0usize;
    let mut launch_model_count = 0usize;
    validate_catalog(&catalog, &mut errors);

    for model in &catalog.models {
        if !model_ids.insert(model.model_id.clone()) {
            errors.push(format!("duplicate model_id {}", model.model_id));
        }
        match model.tier.as_str() {
            "dev" => dev_model_count += 1,
            "launch" => launch_model_count += 1,
            other => errors.push(format!("{} has invalid tier {}", model.model_id, other)),
        }
        artifact_count += model.artifacts.len();
        canary_sets.insert(model.canary.set_id.clone());
        validate_model(model, &mut errors);
    }
    if launch_model_count < 1 {
        errors.push("catalog must contain at least one launch entry".to_owned());
    }

    for set_id in &canary_sets {
        validate_canary_set(&options.canaries_dir, set_id, &mut errors);
    }
    for model in &catalog.models {
        validate_model_canary_modality_coverage(&options.canaries_dir, model, &mut errors);
    }

    let download_checks = if options.check_dev_downloads && errors.is_empty() {
        run_download_checks(&catalog, options.hf_token_file.as_deref())?
    } else {
        Vec::new()
    };
    let source_checks = if options.check_launch_sources && errors.is_empty() {
        run_source_checks(&catalog, options.hf_token_file.as_deref(), true)?
    } else {
        Vec::new()
    };

    Ok(CatalogVerifyReport {
        ok: errors.is_empty()
            && attestation_policy_errors.is_empty()
            && download_checks.iter().all(|check| check.ok)
            && source_checks.iter().all(|check| check.ok),
        catalog_path: options.catalog_path,
        signature_path: options.signature_path,
        catalog_hash,
        key_id: signature.key_id,
        model_count: catalog.models.len(),
        dev_model_count,
        launch_model_count,
        artifact_count,
        canary_sets: canary_sets.into_iter().collect(),
        download_checks,
        source_checks,
        errors,
        attestation_policy_errors,
        attestation_authority,
    })
}

pub(crate) fn load_document(path: &Path) -> Result<CatalogDocument> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn failed_report(
    options: VerifyOptions,
    signature: CatalogSignature,
    catalog_hash: String,
    errors: Vec<String>,
) -> CatalogVerifyReport {
    CatalogVerifyReport {
        ok: false,
        catalog_path: options.catalog_path,
        signature_path: options.signature_path,
        catalog_hash,
        key_id: signature.key_id,
        model_count: 0,
        dev_model_count: 0,
        launch_model_count: 0,
        artifact_count: 0,
        canary_sets: Vec::new(),
        download_checks: Vec::new(),
        source_checks: Vec::new(),
        errors,
        attestation_policy_errors: Vec::new(),
        attestation_authority: CatalogAttestationAuthority::default(),
    }
}

fn validate_signature_metadata(
    signature: &CatalogSignature,
    catalog_hash: &str,
    options: &VerifyOptions,
    errors: &mut Vec<String>,
) {
    if signature.schema_version != 1 {
        errors.push("signature schema_version must be 1".to_owned());
    }
    if signature.alg != "ed25519" {
        errors.push("signature alg must be ed25519".to_owned());
    }
    if signature.blake3 != catalog_hash {
        errors.push(format!(
            "catalog hash mismatch: signature has {}, file is {}",
            signature.blake3, catalog_hash
        ));
    }
    if !is_hex_len(&signature.public_key, 64) {
        errors.push("signature public_key must be 32-byte hex".to_owned());
    }
    if !is_hex_len(&signature.sig, 128) {
        errors.push("signature sig must be 64-byte hex".to_owned());
    }
    if let Some(file_name) = options
        .catalog_path
        .file_name()
        .and_then(|value| value.to_str())
    {
        if signature.signed_path != format!("catalog/{file_name}") {
            errors.push(format!(
                "signature signed_path {} does not match catalog/{file_name}",
                signature.signed_path
            ));
        }
    }

    let key_path = options.keys_dir.join(format!("{}.json", signature.key_id));
    match fs::read_to_string(&key_path)
        .with_context(|| format!("reading catalog key {}", key_path.display()))
        .and_then(|text| {
            serde_json::from_str::<CatalogKey>(&text)
                .with_context(|| format!("parsing catalog key {}", key_path.display()))
        }) {
        Ok(key) => {
            if key.key_id != signature.key_id {
                errors.push(format!(
                    "key file id {} does not match signature",
                    key.key_id
                ));
            }
            if key.alg != signature.alg {
                errors.push(format!("key alg {} does not match signature", key.alg));
            }
            if key.public_key != signature.public_key {
                errors.push("key public_key does not match signature".to_owned());
            }
            if key.status != "active" {
                errors.push(format!("catalog key {} is not active", key.key_id));
            }
            if key.created_at.trim().is_empty() {
                errors.push(format!("catalog key {} has empty created_at", key.key_id));
            }
        }
        Err(err) => errors.push(err.to_string()),
    }
}

fn verify_signature_bytes(catalog_bytes: &[u8], signature: &CatalogSignature) -> Result<()> {
    let public_key_bytes = hex_to_array::<32>(&signature.public_key)?;
    let sig_bytes = hex_to_vec(&signature.sig)?;
    let key = VerifyingKey::from_bytes(&public_key_bytes).context("invalid catalog public key")?;
    let sig = Signature::from_slice(&sig_bytes).context("invalid catalog signature bytes")?;
    key.verify_strict(catalog_bytes, &sig)
        .context("catalog signature verification failed")
}

fn validate_catalog(catalog: &CatalogDocument, errors: &mut Vec<String>) {
    if catalog.schema_version != 1 {
        errors.push("catalog schema_version must be 1".to_owned());
    }
    if catalog.catalog_id.trim().is_empty() {
        errors.push("catalog_id is required".to_owned());
    }
    if catalog.generated_at.trim().is_empty() {
        errors.push("generated_at is required".to_owned());
    }
}

fn validate_catalog_attestation_authority(
    authority: &CatalogAttestationAuthority,
    errors: &mut Vec<String>,
) {
    if let Err(err) = validate_catalog_attestation_authority_canonical(authority) {
        errors.push(format!("{err:#}"));
    }
}

pub(crate) fn validate_catalog_attestation_authority_canonical(
    authority: &CatalogAttestationAuthority,
) -> Result<()> {
    AttestationPolicyChain::from_catalog_records(
        authority.attestation_policy_chain.clone(),
        authority.enclave_attestation_bindings.clone(),
    )
    .map_err(anyhow::Error::msg)
    .context("invalid catalog attestation authority")?;

    if let Some(policies) = authority.attestation_policy_chain.as_ref() {
        for (policy_index, policy) in policies.iter().enumerate() {
            for reference in &policy.trust_data {
                if reference.source.is_none() {
                    bail!(
                        "catalog attestation_policy_chain[{policy_index}] trust data {} has no canonical HTTPS source",
                        reference.id
                    );
                }
            }
        }
    }
    Ok(())
}

fn validated_catalog_attestation_authority(
    catalog_bytes: &[u8],
) -> (CatalogAttestationAuthority, Vec<String>) {
    let mut errors = Vec::new();
    let authority = match serde_json::from_slice::<CatalogAttestationAuthority>(catalog_bytes) {
        Ok(authority) => authority,
        Err(err) => {
            errors.push(format!("invalid catalog attestation authority: {err}"));
            return (CatalogAttestationAuthority::default(), errors);
        }
    };
    validate_catalog_attestation_authority(&authority, &mut errors);
    if errors.is_empty() {
        (authority, errors)
    } else {
        (CatalogAttestationAuthority::default(), errors)
    }
}

pub(crate) fn validate_catalog_attestation_authority_bytes(catalog_bytes: &[u8]) -> Result<()> {
    let authority = serde_json::from_slice::<CatalogAttestationAuthority>(catalog_bytes)
        .context("invalid catalog attestation authority")?;
    validate_catalog_attestation_authority_canonical(&authority)
}

fn validate_model(model: &CatalogModel, errors: &mut Vec<String>) {
    if model.model_id.trim().is_empty() {
        errors.push("model_id is required".to_owned());
    }
    if !valid_model_class(&model.model_class) {
        errors.push(format!(
            "{} has unsupported model_class {}",
            model.model_id, model.model_class
        ));
    }
    if model.family.trim().is_empty() {
        errors.push(format!("{} has empty family", model.model_id));
    }
    if model.params_b <= 0.0 {
        errors.push(format!("{} params_b must be positive", model.model_id));
    }
    if let Some(min_app_version) = &model.min_app_version {
        if semver::Version::parse(min_app_version.trim()).is_err() {
            errors.push(format!(
                "{} min_app_version must be a semantic version like 0.1.0",
                model.model_id
            ));
        }
    }
    validate_source(
        &model.model_id,
        "provenance.source",
        &model.provenance.source,
        errors,
    );
    if model.provenance.conversion.is_empty() {
        errors.push(format!(
            "{} must include at least one provenance conversion",
            model.model_id
        ));
    }
    for conversion in &model.provenance.conversion {
        if conversion.tool.trim().is_empty() || conversion.method.trim().is_empty() {
            errors.push(format!(
                "{} has incomplete conversion provenance",
                model.model_id
            ));
        }
        if !is_hex_len(&conversion.input_sha256, 64) || !is_hex_len(&conversion.output_sha256, 64) {
            errors.push(format!(
                "{} conversion hashes must be 32-byte hex",
                model.model_id
            ));
        }
    }
    if model.provenance.license.trim().is_empty() {
        errors.push(format!("{} license is required", model.model_id));
    }
    if !is_hex_len(&model.provenance.license_sha256, 64) {
        errors.push(format!(
            "{} license_sha256 must be 32-byte hex",
            model.model_id
        ));
    }
    if model.artifacts.is_empty() {
        errors.push(format!(
            "{} must include at least one artifact",
            model.model_id
        ));
    }
    for (name, artifact) in &model.artifacts {
        validate_artifact(&model.model_id, &model.tier, name, artifact, errors);
    }
    if model.caps.ctx_max == 0 {
        errors.push(format!("{} caps.ctx_max must be positive", model.model_id));
    }
    if model.caps.vision && model.family.trim().is_empty() {
        errors.push(format!(
            "{} vision model must declare a family",
            model.model_id
        ));
    }
    validate_model_caps_modalities(model, errors);
    validate_model_modality_assessment(model, errors);
    validate_model_adapter(model, errors);
    validate_stable_diffusion_endpoint_ranges(model, errors);
    validate_model_specialities(model, errors);
    validate_model_sampling(model, errors);
    let _ = (model.caps.tools, model.caps.json);
    if model.requirements.min_ram_gb == 0 {
        errors.push(format!(
            "{} requirements.min_ram_gb must be positive",
            model.model_id
        ));
    }
    if model.requirements.backends.is_empty() {
        errors.push(format!(
            "{} requirements.backends must not be empty",
            model.model_id
        ));
    }
    let artifact_engines = model
        .artifacts
        .values()
        .map(|artifact| artifact.engine.as_str())
        .collect::<BTreeSet<_>>();
    let requirement_backends = model
        .requirements
        .backends
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for engine in &artifact_engines {
        if !requirement_backends.contains(engine) {
            errors.push(format!(
                "{} artifact engine {} is missing from requirements.backends",
                model.model_id, engine
            ));
        }
    }
    for backend in &requirement_backends {
        if !artifact_engines.contains(backend) {
            errors.push(format!(
                "{} requirements.backends references unused backend {}",
                model.model_id, backend
            ));
        }
    }
    let _ = (
        model.requirements.min_vram_gb_full_offload,
        &model.requirements.cpu_flags,
    );
    if model.canary.set_id.trim().is_empty() {
        errors.push(format!("{} canary.set_id is required", model.model_id));
    }
    if !(0.0..=1.0).contains(&model.canary.match_min) || model.canary.match_min == 0.0 {
        errors.push(format!(
            "{} canary.match_min must be in (0, 1]",
            model.model_id
        ));
    }
    validate_canary_verification(model, errors);
    if model.price_ref_au.denom != "au_usd" {
        errors.push(format!(
            "{} price_ref_au.denom must be au_usd",
            model.model_id
        ));
    }
    validate_price_ref(model, errors);
}

fn validate_stable_diffusion_endpoint_ranges(model: &CatalogModel, errors: &mut Vec<String>) {
    for (artifact_name, artifact) in &model.artifacts {
        let Some(config) = artifact.stable_diffusion_cpp else {
            continue;
        };
        for (family, steps_path, guidance_path) in [
            (
                mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS,
                "steps",
                "cfg_scale",
            ),
            (
                mayhem_proto::ENDPOINT_HF_TEXT_TO_IMAGE,
                "parameters.num_inference_steps",
                "parameters.guidance_scale",
            ),
        ] {
            let Some(contract) = model
                .adapter
                .endpoint_families
                .iter()
                .find(|contract| contract.family == family)
            else {
                continue;
            };
            validate_stable_diffusion_mapped_range(
                &model.model_id,
                artifact_name,
                family,
                steps_path,
                contract.request_attribute_specs.get(steps_path),
                f64::from(config.steps_offset),
                1.0,
                150.0,
                errors,
            );
            validate_stable_diffusion_mapped_range(
                &model.model_id,
                artifact_name,
                family,
                guidance_path,
                contract.request_attribute_specs.get(guidance_path),
                f64::from(config.guidance_scale_offset),
                0.0,
                50.0,
                errors,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_stable_diffusion_mapped_range(
    model_id: &str,
    artifact_name: &str,
    family: &str,
    path: &str,
    spec: Option<&mayhem_proto::EndpointAttributeSpec>,
    offset: f64,
    engine_minimum: f64,
    engine_maximum: f64,
    errors: &mut Vec<String>,
) {
    let Some(spec) = spec else {
        return;
    };
    let Some(minimum) = spec.minimum else {
        errors.push(format!(
            "{model_id}/{artifact_name} endpoint family {family} {path} needs a signed minimum"
        ));
        return;
    };
    let Some(maximum) = spec.maximum else {
        errors.push(format!(
            "{model_id}/{artifact_name} endpoint family {family} {path} needs a signed maximum"
        ));
        return;
    };
    if minimum + offset < engine_minimum || maximum + offset > engine_maximum {
        errors.push(format!(
            "{model_id}/{artifact_name} endpoint family {family} {path} range {minimum}..={maximum} with backend offset {offset} exceeds stable-diffusion.cpp range {engine_minimum}..={engine_maximum}"
        ));
    }
}

fn validate_model_sampling(model: &CatalogModel, errors: &mut Vec<String>) {
    let sampling = &model.sampling;
    if sampling
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        errors.push(format!(
            "{} sampling.temperature must be finite and between 0 and 2",
            model.model_id
        ));
    }
    if sampling
        .top_p
        .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1.0)
    {
        errors.push(format!(
            "{} sampling.top_p must be finite and in (0, 1]",
            model.model_id
        ));
    }
    if sampling
        .top_k
        .is_some_and(|value| !(0..=1_000_000).contains(&value))
    {
        errors.push(format!(
            "{} sampling.top_k must be between 0 and 1000000",
            model.model_id
        ));
    }
    if sampling
        .min_p
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        errors.push(format!(
            "{} sampling.min_p must be finite and between 0 and 1",
            model.model_id
        ));
    }
    if sampling
        .repeat_penalty
        .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 10.0)
    {
        errors.push(format!(
            "{} sampling.repeat_penalty must be finite and in (0, 10]",
            model.model_id
        ));
    }
    if sampling
        .frequency_penalty
        .is_some_and(|value| !value.is_finite() || !(-2.0..=2.0).contains(&value))
    {
        errors.push(format!(
            "{} sampling.frequency_penalty must be finite and between -2 and 2",
            model.model_id
        ));
    }
    if sampling
        .presence_penalty
        .is_some_and(|value| !value.is_finite() || !(-2.0..=2.0).contains(&value))
    {
        errors.push(format!(
            "{} sampling.presence_penalty must be finite and between -2 and 2",
            model.model_id
        ));
    }
}

fn validate_model_modality_assessment(model: &CatalogModel, errors: &mut Vec<String>) {
    let assessment = &model.modality_assessment;
    if assessment.detected.is_empty() || assessment.detected.len() > 8 {
        errors.push(format!(
            "{} modality_assessment.detected must have 1..=8 entries",
            model.model_id
        ));
    }
    if assessment.evidence.is_empty()
        || assessment
            .evidence
            .iter()
            .any(|entry| entry.trim().is_empty())
    {
        errors.push(format!(
            "{} modality_assessment.evidence must contain non-empty detection evidence",
            model.model_id
        ));
    }

    let mut detected = BTreeSet::new();
    for modality in &assessment.detected {
        if !valid_adapter_modality(modality) {
            errors.push(format!(
                "{} modality_assessment.detected entry is unsupported: {}",
                model.model_id, modality
            ));
        }
        if !detected.insert(modality.as_str()) {
            errors.push(format!(
                "{} modality_assessment.detected duplicates {}",
                model.model_id, modality
            ));
        }
    }

    let enabled = model
        .adapter
        .modality_set
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for modality in &enabled {
        if !detected.contains(modality) {
            errors.push(format!(
                "{} adapter.modality_set enables {} without modality assessment evidence",
                model.model_id, modality
            ));
        }
    }
    for modality in detected.difference(&enabled) {
        errors.push(format!(
            "{} detected modality {} must be enabled; modality exclusion is forbidden",
            model.model_id, modality
        ));
    }
}

fn validate_price_ref(model: &CatalogModel, errors: &mut Vec<String>) {
    if !model.price_ref_au.rate_map.is_empty() {
        validate_price_rate_map(model, errors);
        validate_required_modality_price_units(model, errors);
        return;
    }
    if model.price_ref_au.in_per_1k == 0 {
        errors.push(format!(
            "{} price_ref_au.in_per_1k must be positive",
            model.model_id
        ));
    }
    if model.model_class != "embedding" && model.price_ref_au.out_per_1k == 0 {
        errors.push(format!(
            "{} price_ref_au.out_per_1k must be positive for non-embedding models",
            model.model_id
        ));
    }
}

fn validate_required_modality_price_units(model: &CatalogModel, errors: &mut Vec<String>) {
    let mut required = BTreeSet::new();
    match model.model_class.as_str() {
        DEFAULT_MODEL_CLASS => {
            required.insert("input_token");
            required.insert("output_token");
        }
        "embedding" => {
            required.insert("input_token");
        }
        "image-generation" => {
            required.insert("image");
            required.insert("step");
        }
        "video-generation" => {
            required.insert("video_second");
            required.insert("frame");
        }
        "tts" | "audio-generation" | "music-generation" => {
            required.insert("input_character");
            required.insert("audio_second");
        }
        "stt" => {
            required.insert("audio_second");
        }
        _ => {}
    }
    let units = model
        .price_ref_au
        .rate_map
        .iter()
        .map(|entry| entry.unit.as_str())
        .collect::<BTreeSet<_>>();
    for unit in required.difference(&units) {
        errors.push(format!(
            "{} price_ref_au.rate_map is missing required modality unit {}",
            model.model_id, unit
        ));
    }
    if model.model_class == DEFAULT_MODEL_CLASS {
        for forbidden in ["image", "step", "audio_second", "video_second", "frame"] {
            if units.contains(forbidden) {
                errors.push(format!(
                    "{} multimodal LLM media input is billed through input_token; rate_map must not include separate {} charges",
                    model.model_id, forbidden
                ));
            }
        }
    }
}

fn validate_price_rate_map(model: &CatalogModel, errors: &mut Vec<String>) {
    let mut units = BTreeSet::new();
    for entry in &model.price_ref_au.rate_map {
        if entry.unit.trim().is_empty() {
            errors.push(format!(
                "{} price_ref_au.rate_map unit is required",
                model.model_id
            ));
            continue;
        }
        if !units.insert(entry.unit.as_str()) {
            errors.push(format!(
                "{} price_ref_au.rate_map duplicates unit {}",
                model.model_id, entry.unit
            ));
        }
        if entry.per_unit_au == 0 {
            errors.push(format!(
                "{} price_ref_au.rate_map {} per_unit_au must be positive",
                model.model_id, entry.unit
            ));
        }
        if entry.granularity == 0 {
            errors.push(format!(
                "{} price_ref_au.rate_map {} granularity must be positive",
                model.model_id, entry.unit
            ));
        }
    }

    let required_units: &[&str] = match model.model_class.as_str() {
        MODEL_CLASS_IMAGE_GENERATION => &[USAGE_IMAGE, USAGE_STEP],
        MODEL_CLASS_VIDEO_GENERATION => &[USAGE_VIDEO_SECOND, USAGE_FRAME],
        MODEL_CLASS_EMBEDDING => &[USAGE_INPUT_TOKEN],
        MODEL_CLASS_STT => &[USAGE_AUDIO_SECOND],
        MODEL_CLASS_TTS | MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION => {
            &[USAGE_INPUT_CHARACTER, USAGE_AUDIO_SECOND]
        }
        _ => &[USAGE_INPUT_TOKEN, USAGE_OUTPUT_TOKEN],
    };
    for unit in required_units {
        if !units.contains(unit) {
            errors.push(format!(
                "{} price_ref_au.rate_map missing required unit {}",
                model.model_id, unit
            ));
        }
    }
}

fn validate_canary_verification(model: &CatalogModel, errors: &mut Vec<String>) {
    let method = model.canary.verification_method.as_str();
    if !valid_canary_verification_method(method) {
        errors.push(format!(
            "{} canary.verification_method is unsupported: {}",
            model.model_id, method
        ));
        return;
    }
    if !canary_verification_method_allowed_for_class(&model.model_class, method) {
        errors.push(format!(
            "{} canary.verification_method {} is not allowed for model_class {}",
            model.model_id, method, model.model_class
        ));
    }
    if model.tier == "launch" {
        if let Some(required_method) = required_launch_output_canary_method(&model.model_class) {
            if method != required_method {
                errors.push(format!(
                    "{} launch model_class {} requires output canary method {}, not {}",
                    model.model_id, model.model_class, required_method, method
                ));
            }
        }
    }
    if let Some(tolerance_bps) = model.canary.verification_tolerance_bps {
        if tolerance_bps > 10_000 {
            errors.push(format!(
                "{} canary.verification_tolerance_bps must be between 0 and 10000",
                model.model_id
            ));
        }
    }
    match method {
        VERIFICATION_TOKEN_FINGERPRINT => validate_token_fingerprint_canary(model, errors),
        VERIFICATION_SEED_PERCEPTUAL_HASH => validate_seed_perceptual_hash_canary(model, errors),
        VERIFICATION_EMBEDDING_COSINE => validate_embedding_cosine_canary(model, errors),
        VERIFICATION_TRANSCRIPT_MATCH => validate_transcript_match_canary(model, errors),
        VERIFICATION_AUDIO_FINGERPRINT => validate_audio_fingerprint_canary(model, errors),
        VERIFICATION_ATTESTATION_OF_COMPUTE => {
            validate_attestation_of_compute_canary(model, errors)
        }
        _ => {}
    }
    validate_modality_calibration_fingerprints(model, errors);
    validate_modality_resource_profiles(model, errors);
}

fn validate_modality_resource_profiles(model: &CatalogModel, errors: &mut Vec<String>) {
    for (artifact, modalities) in &model.modality_assessment.resource_profiles {
        if !model.artifacts.contains_key(artifact) {
            errors.push(format!(
                "{} modality resource profiles reference unknown artifact {}",
                model.model_id, artifact
            ));
        }
        for (modality, profile) in modalities {
            if modality == "text" || !model.adapter.modality_set.contains(modality) {
                errors.push(format!(
                    "{} modality resource profile for {} references unsupported modality {}",
                    model.model_id, artifact, modality
                ));
            }
            let expected_unit = match modality.as_str() {
                "image" => "pixel",
                "audio" => "second",
                "video" => "frame",
                "embedding" => "input_token",
                _ => "",
            };
            if profile.unit != expected_unit {
                errors.push(format!(
                    "{} modality resource profile for {}/{} unit must be {}, got {}",
                    model.model_id, artifact, modality, expected_unit, profile.unit
                ));
            }
            if profile.measurement_source.trim().is_empty() {
                errors.push(format!(
                    "{} modality resource profile for {}/{} requires a memory measurement source",
                    model.model_id, artifact, modality
                ));
            }
            if profile.max_item_bytes == 0
                || profile.max_item_units == 0
                || profile.measured_item_bytes == 0
                || profile.measured_item_units == 0
                || profile.measured_working_set_bytes == 0
                || profile.calibration_baseline_memory_bytes == 0
                || profile.calibration_peak_memory_bytes == 0
                || profile.calibration_f13_budget_bytes == 0
            {
                errors.push(format!(
                    "{} modality resource profile for {}/{} must contain positive measured limits",
                    model.model_id, artifact, modality
                ));
            }
            if profile.measured_item_bytes != profile.max_item_bytes
                || profile.measured_item_units != profile.max_item_units
            {
                errors.push(format!(
                    "{} modality resource profile for {}/{} must be measured at its published maximum item shape",
                    model.model_id, artifact, modality
                ));
            }
            if profile.calibration_peak_memory_bytes < profile.calibration_baseline_memory_bytes
                || profile.calibration_peak_memory_bytes > profile.calibration_f13_budget_bytes
                || profile
                    .calibration_baseline_memory_bytes
                    .saturating_add(profile.measured_working_set_bytes)
                    > profile.calibration_f13_budget_bytes
                || profile.measured_working_set_bytes
                    != modality_profile_working_set_bytes(modality, profile)
            {
                errors.push(format!(
                    "{} modality resource profile for {}/{} must bind its measured working set to peak-minus-baseline or the decoded-item floor within its F13 budget",
                    model.model_id, artifact, modality
                ));
            }
            if profile.default_max_inflight_items != 1 || profile.default_max_items_per_request != 1
            {
                errors.push(format!(
                    "{} modality resource profile for {}/{} must default to one in-flight item and one item per request",
                    model.model_id, artifact, modality
                ));
            }
        }
    }
    if model.tier != "launch" {
        return;
    }
    for artifact in model.artifacts.keys() {
        for modality in model
            .adapter
            .modality_set
            .iter()
            .filter(|modality| modality.as_str() != "text")
        {
            if !model
                .modality_assessment
                .resource_profiles
                .get(artifact)
                .is_some_and(|profiles| profiles.contains_key(modality))
            {
                errors.push(format!(
                    "{} modality resource profiles for {} missing served modality {}",
                    model.model_id, artifact, modality
                ));
            }
        }
    }
}

fn modality_profile_working_set_bytes(
    modality: &str,
    profile: &CatalogModalityResourceProfile,
) -> u64 {
    let decoded_floor = match modality {
        "image" => profile.max_item_units.saturating_mul(3).saturating_mul(4),
        "audio" => profile
            .max_item_units
            .saturating_mul(48_000)
            .saturating_mul(4),
        "video" => profile
            .max_item_units
            .saturating_mul(224)
            .saturating_mul(224)
            .saturating_mul(3)
            .saturating_mul(4),
        _ => profile.max_item_bytes,
    }
    .max(profile.max_item_bytes);
    profile
        .calibration_peak_memory_bytes
        .saturating_sub(profile.calibration_baseline_memory_bytes)
        .max(decoded_floor)
        .max(1)
}

fn validate_modality_calibration_fingerprints(model: &CatalogModel, errors: &mut Vec<String>) {
    for (artifact, modalities) in &model.modality_assessment.calibrated_fingerprints {
        if !model.artifacts.contains_key(artifact) {
            errors.push(format!(
                "{} modality calibration references unknown artifact {}",
                model.model_id, artifact
            ));
        }
        for (modality, fingerprint) in modalities {
            if !model.adapter.modality_set.contains(modality) {
                errors.push(format!(
                    "{} modality calibration for {} references disabled modality {}",
                    model.model_id, artifact, modality
                ));
            }
            if !is_hex_len(fingerprint, 64) {
                errors.push(format!(
                    "{} modality calibration fingerprint for {}/{} must be 32-byte hex",
                    model.model_id, artifact, modality
                ));
            }
        }
    }
    if model.tier != "launch" {
        return;
    }
    for artifact in model.artifacts.keys() {
        let Some(calibrated) = model
            .modality_assessment
            .calibrated_fingerprints
            .get(artifact)
        else {
            errors.push(format!(
                "{} modality calibration fingerprints missing artifact {}",
                model.model_id, artifact
            ));
            continue;
        };
        for modality in &model.adapter.modality_set {
            if !calibrated.contains_key(modality) {
                errors.push(format!(
                    "{} modality calibration fingerprints for {} missing served modality {}",
                    model.model_id, artifact, modality
                ));
            }
        }
    }
}

fn valid_canary_verification_method(method: &str) -> bool {
    matches!(
        method,
        VERIFICATION_TOKEN_FINGERPRINT
            | VERIFICATION_SEED_PERCEPTUAL_HASH
            | VERIFICATION_EMBEDDING_COSINE
            | VERIFICATION_TRANSCRIPT_MATCH
            | VERIFICATION_AUDIO_FINGERPRINT
            | VERIFICATION_ATTESTATION_OF_COMPUTE
    )
}

fn canary_verification_method_allowed_for_class(model_class: &str, method: &str) -> bool {
    matches!(
        (model_class, method),
        (DEFAULT_MODEL_CLASS, VERIFICATION_TOKEN_FINGERPRINT)
            | (MODEL_CLASS_EMBEDDING, VERIFICATION_EMBEDDING_COSINE)
            | (
                MODEL_CLASS_IMAGE_GENERATION | MODEL_CLASS_VIDEO_GENERATION,
                VERIFICATION_SEED_PERCEPTUAL_HASH
            )
            | (MODEL_CLASS_STT, VERIFICATION_TRANSCRIPT_MATCH)
            | (
                MODEL_CLASS_TTS | MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION,
                VERIFICATION_AUDIO_FINGERPRINT
            )
            | (_, VERIFICATION_ATTESTATION_OF_COMPUTE)
    )
}

fn required_launch_output_canary_method(model_class: &str) -> Option<&'static str> {
    match model_class {
        DEFAULT_MODEL_CLASS => Some(VERIFICATION_TOKEN_FINGERPRINT),
        MODEL_CLASS_EMBEDDING => Some(VERIFICATION_EMBEDDING_COSINE),
        MODEL_CLASS_IMAGE_GENERATION | MODEL_CLASS_VIDEO_GENERATION => {
            Some(VERIFICATION_SEED_PERCEPTUAL_HASH)
        }
        MODEL_CLASS_STT => Some(VERIFICATION_TRANSCRIPT_MATCH),
        MODEL_CLASS_TTS | MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION => {
            Some(VERIFICATION_AUDIO_FINGERPRINT)
        }
        _ => None,
    }
}

fn validate_token_fingerprint_canary(model: &CatalogModel, errors: &mut Vec<String>) {
    if model.canary.verification_tolerance_bps.unwrap_or(0) != 0 {
        errors.push(format!(
            "{} token_fingerprint canary must not set verification_tolerance_bps",
            model.model_id
        ));
    }
    if !model.canary.perceptual_hashes.is_empty() {
        errors.push(format!(
            "{} token_fingerprint canary must not set perceptual_hashes",
            model.model_id
        ));
    }
    validate_no_non_text_canary_blobs(model, "token_fingerprint", errors);
    for artifact_name in model.artifacts.keys() {
        if !model.canary.fingerprints.contains_key(artifact_name) {
            errors.push(format!(
                "{} canary fingerprints missing artifact {}",
                model.model_id, artifact_name
            ));
        }
    }
    for (artifact, fingerprint) in &model.canary.fingerprints {
        if !model.artifacts.contains_key(artifact) {
            errors.push(format!(
                "{} canary fingerprint references unknown artifact {}",
                model.model_id, artifact
            ));
        }
        if !is_hex_len(fingerprint, 64) {
            errors.push(format!(
                "{} canary fingerprint for {} must be 32-byte hex",
                model.model_id, artifact
            ));
        }
    }
    validate_token_prefixes(model, errors);
}

fn validate_token_prefixes(model: &CatalogModel, errors: &mut Vec<String>) {
    for (artifact, prefixes) in &model.canary.token_prefixes {
        if !model.artifacts.contains_key(artifact) {
            errors.push(format!(
                "{} canary token_prefixes references unknown artifact {}",
                model.model_id, artifact
            ));
        }
        if prefixes.is_empty() {
            errors.push(format!(
                "{} canary token_prefixes for {} must not be empty",
                model.model_id, artifact
            ));
        }
        for (prompt_id, tokens) in prefixes {
            if prompt_id.trim().is_empty() {
                errors.push(format!(
                    "{} canary token_prefixes for {} has empty prompt id",
                    model.model_id, artifact
                ));
            }
            if tokens.is_empty() {
                errors.push(format!(
                    "{} canary token_prefixes for {} prompt {} must not be empty",
                    model.model_id, artifact, prompt_id
                ));
            }
        }
        let stable_tokens = prefixes.values().map(Vec::len).sum::<usize>();
        if model.tier == "launch" && stable_tokens < MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS {
            errors.push(format!(
                "{} canary token_prefixes for {} contain only {} stable tokens; launch requires at least {}",
                model.model_id,
                artifact,
                stable_tokens,
                MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS
            ));
        }
    }
}

fn validate_seed_perceptual_hash_canary(model: &CatalogModel, errors: &mut Vec<String>) {
    if model.canary.verification_tolerance_bps.is_none() {
        errors.push(format!(
            "{} seed_perceptual_hash canary requires verification_tolerance_bps",
            model.model_id
        ));
    }
    if !model.canary.fingerprints.is_empty() {
        errors.push(format!(
            "{} seed_perceptual_hash canary must use perceptual_hashes, not fingerprints",
            model.model_id
        ));
    }
    if !model.canary.token_prefixes.is_empty() {
        errors.push(format!(
            "{} seed_perceptual_hash canary must not set token_prefixes",
            model.model_id
        ));
    }
    validate_no_non_text_canary_blobs(model, "seed_perceptual_hash", errors);
    for artifact_name in model.artifacts.keys() {
        if !model.canary.perceptual_hashes.contains_key(artifact_name) {
            errors.push(format!(
                "{} canary perceptual_hashes missing artifact {}",
                model.model_id, artifact_name
            ));
        }
    }
    for (artifact, hashes) in &model.canary.perceptual_hashes {
        if !model.artifacts.contains_key(artifact) {
            errors.push(format!(
                "{} canary perceptual_hashes references unknown artifact {}",
                model.model_id, artifact
            ));
        }
        if hashes.is_empty() {
            errors.push(format!(
                "{} canary perceptual_hashes for {} must not be empty",
                model.model_id, artifact
            ));
        }
        for (prompt_id, hash) in hashes {
            if prompt_id.trim().is_empty() {
                errors.push(format!(
                    "{} canary perceptual_hashes for {} has empty prompt id",
                    model.model_id, artifact
                ));
            }
            if !valid_perceptual_hash(hash) {
                errors.push(format!(
                    "{} canary perceptual_hashes for {} prompt {} must be hex between 64 and 256 bits",
                    model.model_id, artifact, prompt_id
                ));
            }
        }
    }
}

fn validate_embedding_cosine_canary(model: &CatalogModel, errors: &mut Vec<String>) {
    validate_text_and_image_blobs_absent(model, "embedding_cosine", errors);
    if !model.canary.transcripts.is_empty() || !model.canary.audio_fingerprints.is_empty() {
        errors.push(format!(
            "{} embedding_cosine canary must use embedding_vectors only",
            model.model_id
        ));
    }
    validate_prompt_map_complete(
        model,
        "embedding_vectors",
        &model.canary.embedding_vectors,
        errors,
        |model_id, artifact, prompt_id, vector, errors| {
            if prompt_id.trim().is_empty() {
                errors.push(format!(
                    "{model_id} canary embedding_vectors for {artifact} has empty prompt id"
                ));
            }
            if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
                errors.push(format!(
                    "{model_id} canary embedding_vectors for {artifact} prompt {prompt_id} must contain finite values"
                ));
            }
        },
    );
}

fn validate_transcript_match_canary(model: &CatalogModel, errors: &mut Vec<String>) {
    if model.canary.verification_tolerance_bps.is_some() {
        errors.push(format!(
            "{} transcript_match canary must not set verification_tolerance_bps",
            model.model_id
        ));
    }
    validate_text_and_image_blobs_absent(model, "transcript_match", errors);
    if !model.canary.embedding_vectors.is_empty() || !model.canary.audio_fingerprints.is_empty() {
        errors.push(format!(
            "{} transcript_match canary must use transcripts only",
            model.model_id
        ));
    }
    validate_prompt_map_complete(
        model,
        "transcripts",
        &model.canary.transcripts,
        errors,
        |model_id, artifact, prompt_id, transcript, errors| {
            if prompt_id.trim().is_empty() {
                errors.push(format!(
                    "{model_id} canary transcripts for {artifact} has empty prompt id"
                ));
            }
            if transcript.trim().is_empty() {
                errors.push(format!(
                    "{model_id} canary transcripts for {artifact} prompt {prompt_id} must not be empty"
                ));
            }
        },
    );
}

fn validate_audio_fingerprint_canary(model: &CatalogModel, errors: &mut Vec<String>) {
    match model.canary.verification_tolerance_bps {
        Some(0..=2_500) => {}
        Some(_) => errors.push(format!(
            "{} audio_fingerprint canary verification_tolerance_bps must be between 0 and 2500",
            model.model_id
        )),
        None => errors.push(format!(
            "{} audio_fingerprint canary requires verification_tolerance_bps",
            model.model_id
        )),
    }
    validate_text_and_image_blobs_absent(model, "audio_fingerprint", errors);
    if !model.canary.embedding_vectors.is_empty() || !model.canary.transcripts.is_empty() {
        errors.push(format!(
            "{} audio_fingerprint canary must use audio_fingerprints only",
            model.model_id
        ));
    }
    validate_prompt_map_complete(
        model,
        "audio_fingerprints",
        &model.canary.audio_fingerprints,
        errors,
        |model_id, artifact, prompt_id, fingerprint, errors| {
            if prompt_id.trim().is_empty() {
                errors.push(format!(
                    "{model_id} canary audio_fingerprints for {artifact} has empty prompt id"
                ));
            }
            if !mayhem_gateway::valid_audio_fingerprint(fingerprint) {
                errors.push(format!(
                    "{model_id} canary audio_fingerprints for {artifact} prompt {prompt_id} must be a valid audiospec-v1 spectral fingerprint"
                ));
            }
        },
    );
}

fn validate_attestation_of_compute_canary(model: &CatalogModel, errors: &mut Vec<String>) {
    if model.canary.verification_tolerance_bps.is_some() {
        errors.push(format!(
            "{} attestation_of_compute canary must not set verification_tolerance_bps",
            model.model_id
        ));
    }
    if !model.canary.fingerprints.is_empty()
        || !model.canary.token_prefixes.is_empty()
        || !model.canary.perceptual_hashes.is_empty()
        || !model.canary.embedding_vectors.is_empty()
        || !model.canary.transcripts.is_empty()
        || !model.canary.audio_fingerprints.is_empty()
    {
        errors.push(format!(
            "{} attestation_of_compute canary must not carry output calibration blobs",
            model.model_id
        ));
    }
}

fn validate_text_and_image_blobs_absent(
    model: &CatalogModel,
    method: &str,
    errors: &mut Vec<String>,
) {
    if !model.canary.fingerprints.is_empty()
        || !model.canary.token_prefixes.is_empty()
        || !model.canary.perceptual_hashes.is_empty()
    {
        errors.push(format!(
            "{} {method} canary must not set fingerprints, token_prefixes, or perceptual_hashes",
            model.model_id
        ));
    }
}

fn validate_no_non_text_canary_blobs(model: &CatalogModel, method: &str, errors: &mut Vec<String>) {
    if !model.canary.embedding_vectors.is_empty()
        || !model.canary.transcripts.is_empty()
        || !model.canary.audio_fingerprints.is_empty()
    {
        errors.push(format!(
            "{} {method} canary must not set embedding_vectors, transcripts, or audio_fingerprints",
            model.model_id
        ));
    }
}

fn validate_prompt_map_complete<T>(
    model: &CatalogModel,
    field: &str,
    map: &BTreeMap<String, BTreeMap<String, T>>,
    errors: &mut Vec<String>,
    validate_value: impl Fn(&str, &str, &str, &T, &mut Vec<String>),
) {
    for artifact_name in model.artifacts.keys() {
        if !map.contains_key(artifact_name) {
            errors.push(format!(
                "{} canary {field} missing artifact {}",
                model.model_id, artifact_name
            ));
        }
    }
    for (artifact, prompts) in map {
        if !model.artifacts.contains_key(artifact) {
            errors.push(format!(
                "{} canary {field} references unknown artifact {}",
                model.model_id, artifact
            ));
        }
        if prompts.is_empty() {
            errors.push(format!(
                "{} canary {field} for {} must not be empty",
                model.model_id, artifact
            ));
        }
        for (prompt_id, value) in prompts {
            validate_value(&model.model_id, artifact, prompt_id, value, errors);
        }
    }
}

fn valid_perceptual_hash(value: &str) -> bool {
    let bits = value.len().saturating_mul(4);
    (64..=256).contains(&bits) && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn valid_model_class(model_class: &str) -> bool {
    matches!(
        model_class,
        DEFAULT_MODEL_CLASS
            | MODEL_CLASS_EMBEDDING
            | MODEL_CLASS_IMAGE_GENERATION
            | MODEL_CLASS_VIDEO_GENERATION
            | MODEL_CLASS_TTS
            | MODEL_CLASS_STT
            | MODEL_CLASS_AUDIO_GENERATION
            | MODEL_CLASS_MUSIC_GENERATION
    )
}

fn valid_output_modality(modality: &str) -> bool {
    matches!(modality, "text" | "embedding" | "image" | "video" | "audio")
}

fn output_modality_allowed_for_class(model_class: &str, modality: &str) -> bool {
    matches!(
        (model_class, modality),
        (DEFAULT_MODEL_CLASS, "text")
            | (MODEL_CLASS_EMBEDDING, "embedding")
            | (MODEL_CLASS_IMAGE_GENERATION, "image")
            | (MODEL_CLASS_VIDEO_GENERATION, "video")
            | (
                MODEL_CLASS_TTS | MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION,
                "audio"
            )
            | (MODEL_CLASS_STT, "text")
    )
}

fn validate_model_caps_modalities(model: &CatalogModel, errors: &mut Vec<String>) {
    if let Some(modality) = model.caps.output_modality.as_deref() {
        if !valid_output_modality(modality) {
            errors.push(format!(
                "{} caps.output_modality is unsupported: {}",
                model.model_id, modality
            ));
        } else if !output_modality_allowed_for_class(&model.model_class, modality) {
            errors.push(format!(
                "{} caps.output_modality {} is not allowed for model_class {}",
                model.model_id, modality, model.model_class
            ));
        }
    }
    let mut seen = BTreeSet::new();
    for modality in &model.caps.output_modalities {
        if !valid_output_modality(modality) {
            errors.push(format!(
                "{} caps.output_modalities entry is unsupported: {}",
                model.model_id, modality
            ));
        } else if !output_modality_allowed_for_class(&model.model_class, modality) {
            errors.push(format!(
                "{} caps.output_modalities entry {} is not allowed for model_class {}",
                model.model_id, modality, model.model_class
            ));
        }
        if !seen.insert(modality.clone()) {
            errors.push(format!(
                "{} caps.output_modalities duplicates {}",
                model.model_id, modality
            ));
        }
    }
    if let Some(modality) = model.caps.output_modality.as_deref() {
        if !model.caps.output_modalities.is_empty()
            && !model
                .caps
                .output_modalities
                .iter()
                .any(|entry| entry == modality)
        {
            errors.push(format!(
                "{} caps.output_modalities must include output_modality {}",
                model.model_id, modality
            ));
        }
    }
    for (flag, modality) in [("image", "image"), ("video", "video")] {
        let enabled = match flag {
            "image" => model.caps.image,
            "video" => model.caps.video,
            _ => false,
        };
        if enabled && !output_modality_allowed_for_class(&model.model_class, modality) {
            errors.push(format!(
                "{} caps.{} output is not allowed for model_class {}",
                model.model_id, flag, model.model_class
            ));
        }
    }
}

fn validate_model_adapter(model: &CatalogModel, errors: &mut Vec<String>) {
    let adapter = &model.adapter;
    validate_endpoint_families(model, errors);
    if !matches!(
        adapter.chat_template_id.as_str(),
        "generic_chatml"
            | "llama3-instruct"
            | "qwen2.5-instruct"
            | "qwen3.5-instruct"
            | "smolvlm2-instruct"
            | "gemma4-instruct"
    ) {
        errors.push(format!(
            "{} adapter.chat_template_id is unsupported: {}",
            model.model_id, adapter.chat_template_id
        ));
    }
    if !matches!(
        adapter.tool_call_strategy.as_str(),
        "none" | "mayhem_json" | "openai_tool_calls" | "qwen_function_xml" | "gemma_function_call"
    ) {
        errors.push(format!(
            "{} adapter.tool_call_strategy is unsupported: {}",
            model.model_id, adapter.tool_call_strategy
        ));
    }
    if !matches!(adapter.reasoning_passthrough.as_str(), "strip" | "preserve") {
        errors.push(format!(
            "{} adapter.reasoning_passthrough is unsupported: {}",
            model.model_id, adapter.reasoning_passthrough
        ));
    }
    if adapter.modality_set.is_empty() || adapter.modality_set.len() > 8 {
        errors.push(format!(
            "{} adapter.modality_set must have 1..=8 entries",
            model.model_id
        ));
    }
    let mut seen = BTreeSet::new();
    for modality in &adapter.modality_set {
        if !valid_adapter_modality(modality) {
            errors.push(format!(
                "{} adapter.modality_set entry is unsupported: {}",
                model.model_id, modality
            ));
        } else if !adapter_modality_allowed(model, modality) {
            errors.push(format!(
                "{} adapter.modality_set entry {} is not allowed by model caps",
                model.model_id, modality
            ));
        }
        if !seen.insert(modality.clone()) {
            errors.push(format!(
                "{} adapter.modality_set duplicates {}",
                model.model_id, modality
            ));
        }
    }
}

fn validate_model_specialities(model: &CatalogModel, errors: &mut Vec<String>) {
    let assessment = &model.speciality_assessment;
    if assessment.evidence.is_empty()
        || assessment
            .evidence
            .iter()
            .any(|entry| entry.trim().is_empty())
    {
        errors.push(format!(
            "{} speciality_assessment.evidence must record the pinned card/config research even when no speciality is detected",
            model.model_id
        ));
    }

    let mut detected = BTreeSet::new();
    for name in &assessment.detected {
        if !valid_endpoint_attribute_name(name) || !detected.insert(name.as_str()) {
            errors.push(format!(
                "{} speciality_assessment.detected contains an invalid or duplicate speciality {}",
                model.model_id, name
            ));
        }
    }

    let mut descriptors = BTreeMap::new();
    for descriptor in &model.adapter.specialities {
        validate_speciality_descriptor(model, descriptor, errors);
        if descriptors
            .insert(descriptor.name.as_str(), descriptor)
            .is_some()
        {
            errors.push(format!(
                "{} adapter.specialities duplicates {}",
                model.model_id, descriptor.name
            ));
        }
        if !detected.contains(descriptor.name.as_str()) {
            errors.push(format!(
                "{} adapter speciality {} has no detection evidence",
                model.model_id, descriptor.name
            ));
        }
        if assessment.unsupported.contains_key(&descriptor.name) {
            errors.push(format!(
                "{} speciality {} cannot be both adapterized and unsupported",
                model.model_id, descriptor.name
            ));
        }
    }

    for (name, reason) in &assessment.unsupported {
        if !detected.contains(name.as_str()) {
            errors.push(format!(
                "{} unsupported speciality {} was not detected",
                model.model_id, name
            ));
        }
        if reason.trim().is_empty() {
            errors.push(format!(
                "{} unsupported speciality {} requires an explicit artifact/backend reason",
                model.model_id, name
            ));
        }
    }
    for name in &detected {
        if !descriptors.contains_key(name) && !assessment.unsupported.contains_key(*name) {
            errors.push(format!(
                "{} detected speciality {} is neither adapterized nor explicitly unsupported",
                model.model_id, name
            ));
        }
    }

    for contract in &model.adapter.endpoint_families {
        for (name, mapping) in &contract.speciality_mappings {
            let Some(descriptor) = descriptors.get(name.as_str()) else {
                errors.push(format!(
                    "{} endpoint family {} maps unknown speciality {}",
                    model.model_id, contract.family, name
                ));
                continue;
            };
            if !contract.request_attributes.contains(&mapping.request_path) {
                errors.push(format!(
                    "{} endpoint family {} speciality {} maps undeclared request path {}",
                    model.model_id, contract.family, name, mapping.request_path
                ));
                continue;
            }
            if !valid_endpoint_attribute_name(&mapping.native_path) {
                errors.push(format!(
                    "{} endpoint family {} speciality {} has invalid native path {}",
                    model.model_id, contract.family, name, mapping.native_path
                ));
            }
            let Some(spec) = contract.request_attribute_specs.get(&mapping.request_path) else {
                continue;
            };
            let expected_levels = descriptor
                .levels
                .iter()
                .map(|level| Value::String(level.name.clone()))
                .collect::<Vec<_>>();
            let expected_native_values = descriptor
                .levels
                .iter()
                .map(|level| level.native_value.clone())
                .collect::<Vec<_>>();
            let default_native_value = descriptor
                .levels
                .iter()
                .find(|level| level.name == descriptor.default_level)
                .map(|level| &level.native_value);
            let level_name_contract = spec.value_types == [EndpointValueType::String]
                && spec.enum_values == expected_levels
                && spec.default.as_ref() == Some(&Value::String(descriptor.default_level.clone()))
                && expected_levels
                    .iter()
                    .all(|level| spec.calibration_values.contains(level));
            let native_value_contract = spec.enum_values == expected_native_values
                && spec.default.as_ref() == default_native_value
                && expected_native_values
                    .iter()
                    .all(|value| spec.calibration_values.contains(value));
            if !level_name_contract && !native_value_contract {
                errors.push(format!(
                    "{} endpoint family {} speciality {} request spec must expose exactly its signed level names or native values, default, and calibration values",
                    model.model_id, contract.family, name
                ));
            }
            if mapping.target == EndpointSpecialityTarget::PromptSuffix
                && descriptor
                    .levels
                    .iter()
                    .any(|level| !level.native_value.is_string())
            {
                errors.push(format!(
                    "{} endpoint family {} prompt-suffix speciality {} requires string native values",
                    model.model_id, contract.family, name
                ));
            }
        }
        for descriptor in descriptors.values() {
            if !contract.speciality_mappings.contains_key(&descriptor.name) {
                errors.push(format!(
                    "{} endpoint family {} is missing native mapping for speciality {}",
                    model.model_id, contract.family, descriptor.name
                ));
            }
        }
    }

    validate_speciality_calibrations(model, &descriptors, errors);
}

fn validate_speciality_descriptor(
    model: &CatalogModel,
    descriptor: &ModelSpecialityDescriptor,
    errors: &mut Vec<String>,
) {
    let label = format!("{} speciality {}", model.model_id, descriptor.name);
    if !valid_endpoint_attribute_name(&descriptor.name) {
        errors.push(format!("{label} has an invalid normalized name"));
    }
    if !matches!(
        descriptor.mechanism.as_str(),
        "enum" | "token_budget" | "boolean" | "string_enum"
    ) {
        errors.push(format!(
            "{label} mechanism must be enum, token_budget, boolean, or string_enum"
        ));
    }
    if descriptor.levels.len() < 2 || descriptor.levels.len() > 16 {
        errors.push(format!("{label} must declare 2..=16 researched levels"));
    }
    if descriptor.research_evidence.is_empty()
        || descriptor
            .research_evidence
            .iter()
            .any(|entry| entry.trim().is_empty())
    {
        errors.push(format!("{label} requires non-empty live research evidence"));
    }
    let mut calibration_modalities = BTreeSet::new();
    for modality in &descriptor.calibration_modalities {
        if modality == "text" {
            errors.push(format!(
                "{label} calibration_modalities uses an empty list for text-only calibration"
            ));
        } else if !valid_adapter_modality(modality)
            || !model.adapter.modality_set.contains(modality)
        {
            errors.push(format!(
                "{label} calibration modality {modality} is not served by the model adapter"
            ));
        }
        if !calibration_modalities.insert(modality.as_str()) {
            errors.push(format!(
                "{label} duplicates calibration modality {modality}"
            ));
        }
    }
    let mut names = BTreeSet::new();
    let mut ranks = BTreeSet::new();
    for level in &descriptor.levels {
        if !valid_endpoint_attribute_name(&level.name) || !names.insert(level.name.as_str()) {
            errors.push(format!(
                "{label} contains invalid or duplicate level {}",
                level.name
            ));
        }
        if !ranks.insert(level.rank) {
            errors.push(format!("{label} duplicates level rank {}", level.rank));
        }
        let native_type_ok = match descriptor.mechanism.as_str() {
            "boolean" => level.native_value.is_boolean(),
            "token_budget" => level.native_value.as_u64().is_some(),
            "enum" | "string_enum" => level.native_value.is_string(),
            _ => false,
        };
        if !native_type_ok {
            errors.push(format!(
                "{label} level {} native value does not match mechanism {}",
                level.name, descriptor.mechanism
            ));
        }
        if descriptor.name == "reasoning_effort"
            && level.default_max_output_tokens.unwrap_or_default() == 0
        {
            errors.push(format!(
                "{label} level {} requires a positive default output-token cap",
                level.name
            ));
        }
        if level.max_reasoning_tokens.is_some_and(|reasoning| {
            level
                .default_max_output_tokens
                .is_none_or(|output| reasoning > output)
        }) {
            errors.push(format!(
                "{label} level {} max_reasoning_tokens must fit its default output cap",
                level.name
            ));
        }
    }
    if !names.contains(descriptor.default_level.as_str()) {
        errors.push(format!(
            "{label} default level {} is not declared",
            descriptor.default_level
        ));
    }
}

fn validate_speciality_calibrations(
    model: &CatalogModel,
    descriptors: &BTreeMap<&str, &ModelSpecialityDescriptor>,
    errors: &mut Vec<String>,
) {
    for (artifact, specialities) in &model.speciality_assessment.calibrated {
        if !model.artifacts.contains_key(artifact) {
            errors.push(format!(
                "{} speciality calibration references unknown artifact {}",
                model.model_id, artifact
            ));
        }
        for (name, levels) in specialities {
            let Some(descriptor) = descriptors.get(name.as_str()) else {
                errors.push(format!(
                    "{} speciality calibration for {} references unknown speciality {}",
                    model.model_id, artifact, name
                ));
                continue;
            };
            for (level_name, calibration) in levels {
                if !descriptor
                    .levels
                    .iter()
                    .any(|level| level.name == *level_name)
                {
                    errors.push(format!(
                        "{} speciality calibration for {}/{}/{} references an unknown level",
                        model.model_id, artifact, name, level_name
                    ));
                }
                let stable_tokens = calibration
                    .token_prefixes
                    .values()
                    .map(Vec::len)
                    .sum::<usize>();
                if !is_hex_len(&calibration.fingerprint, 64)
                    || calibration.token_prefixes.is_empty()
                    || calibration.token_prefixes.values().any(Vec::is_empty)
                    || calibration.output_tokens_min == 0
                    || calibration.output_tokens_max < calibration.output_tokens_min
                    || calibration.reasoning_tokens_max < calibration.reasoning_tokens_min
                    || calibration.reasoning_tokens_max > calibration.output_tokens_max
                    || (model.tier == "launch"
                        && stable_tokens < MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS)
                {
                    errors.push(format!(
                        "{} speciality calibration for {}/{}/{} requires exact stable prefixes, valid output/reasoning ranges, and at least {} launch token positions",
                        model.model_id, artifact, name, level_name
                        , MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS
                    ));
                }
            }
        }
    }
    for artifact in model.artifacts.keys() {
        for descriptor in descriptors.values() {
            let calibrated = model
                .speciality_assessment
                .calibrated
                .get(artifact)
                .and_then(|specialities| specialities.get(&descriptor.name));
            for level in &descriptor.levels {
                if !calibrated.is_some_and(|levels| levels.contains_key(&level.name)) {
                    errors.push(format!(
                        "{} speciality calibration for {}/{} missing level {}",
                        model.model_id, artifact, descriptor.name, level.name
                    ));
                }
            }
        }
    }
}

fn validate_endpoint_families(model: &CatalogModel, errors: &mut Vec<String>) {
    let contracts = &model.adapter.endpoint_families;
    if contracts.is_empty() || contracts.len() > 12 {
        errors.push(format!(
            "{} adapter.endpoint_families must contain 1..=12 task contracts",
            model.model_id
        ));
    }
    let mut families = BTreeSet::new();
    for contract in contracts {
        if !valid_endpoint_family(&contract.family) {
            errors.push(format!(
                "{} adapter.endpoint_families contains unsupported family {}",
                model.model_id, contract.family
            ));
        } else if !endpoint_family_allowed_for_model(model, &contract.family) {
            errors.push(format!(
                "{} endpoint family {} is not compatible with model_class {} and modalities {:?}",
                model.model_id, contract.family, model.model_class, model.adapter.modality_set
            ));
        } else if let Some(template) = endpoint_contract_template(&contract.family) {
            let speciality_paths = contract
                .speciality_mappings
                .values()
                .map(|mapping| mapping.request_path.as_str())
                .collect::<BTreeSet<_>>();
            for attribute in &contract.request_attributes {
                if !template.request_attributes.contains(attribute)
                    && !speciality_paths.contains(attribute.as_str())
                {
                    errors.push(format!(
                        "{} endpoint family {} declares unknown request attribute {}",
                        model.model_id, contract.family, attribute
                    ));
                }
            }
            for required in &template.required_request_attributes {
                if !contract.required_request_attributes.contains(required) {
                    errors.push(format!(
                        "{} endpoint family {} omits standard required request attribute {}",
                        model.model_id, contract.family, required
                    ));
                }
            }
            for attribute in &contract.response_attributes {
                if !template.response_attributes.contains(attribute) {
                    errors.push(format!(
                        "{} endpoint family {} declares unknown response attribute {}",
                        model.model_id, contract.family, attribute
                    ));
                }
            }
            for required in &template.required_response_attributes {
                if !contract.required_response_attributes.contains(required) {
                    errors.push(format!(
                        "{} endpoint family {} omits standard required response attribute {}",
                        model.model_id, contract.family, required
                    ));
                }
            }
        }
        if !families.insert(contract.family.as_str()) {
            errors.push(format!(
                "{} adapter.endpoint_families duplicates {}",
                model.model_id, contract.family
            ));
        }
        validate_endpoint_attribute_names(
            &model.model_id,
            &contract.family,
            "request_attributes",
            &contract.request_attributes,
            errors,
        );
        validate_endpoint_attribute_names(
            &model.model_id,
            &contract.family,
            "required_response_attributes",
            &contract.required_response_attributes,
            errors,
        );
        validate_endpoint_attribute_names(
            &model.model_id,
            &contract.family,
            "required_request_attributes",
            &contract.required_request_attributes,
            errors,
        );
        validate_endpoint_attribute_names(
            &model.model_id,
            &contract.family,
            "response_attributes",
            &contract.response_attributes,
            errors,
        );
        validate_endpoint_attribute_specs(model, contract, errors);
        validate_image_endpoint_defaults(model, contract, errors);
        for required in &contract.required_request_attributes {
            if !contract.request_attributes.contains(required) {
                errors.push(format!(
                    "{} endpoint family {} requires undeclared request attribute {}",
                    model.model_id, contract.family, required
                ));
            }
        }
        for required in &contract.required_response_attributes {
            if !contract.response_attributes.contains(required) {
                errors.push(format!(
                    "{} endpoint family {} requires undeclared response attribute {}",
                    model.model_id, contract.family, required
                ));
            }
        }
    }
    for required in required_endpoint_families(model) {
        if !families.contains(required) {
            errors.push(format!(
                "{} adapter.endpoint_families missing required compatible family {}",
                model.model_id, required
            ));
        }
    }
}

fn validate_image_endpoint_defaults(
    model: &CatalogModel,
    contract: &EndpointFamilyContract,
    errors: &mut Vec<String>,
) {
    let has_default = |path: &str| {
        contract
            .request_attribute_specs
            .get(path)
            .and_then(|spec| spec.default.as_ref())
            .is_some()
    };
    let is_required = |path: &str| {
        contract
            .required_request_attributes
            .iter()
            .any(|required| required == path)
    };
    match contract.family.as_str() {
        mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS => {
            let has_size = contract
                .request_attributes
                .iter()
                .any(|path| path == "size");
            let has_width = contract
                .request_attributes
                .iter()
                .any(|path| path == "width");
            let has_height = contract
                .request_attributes
                .iter()
                .any(|path| path == "height");
            if has_width != has_height {
                errors.push(format!(
                    "{} endpoint family {} must declare width and height together",
                    model.model_id, contract.family
                ));
            }
            let width_default = has_default("width");
            let height_default = has_default("height");
            if width_default != height_default {
                errors.push(format!(
                    "{} endpoint family {} must default width and height together",
                    model.model_id, contract.family
                ));
            }
            let size_resolution = has_size && (has_default("size") || is_required("size"));
            let dimension_resolution = has_width
                && has_height
                && ((width_default && height_default)
                    || (is_required("width") && is_required("height")));
            if size_resolution == dimension_resolution {
                errors.push(format!(
                    "{} endpoint family {} must resolve exactly one signed default dimension representation: size or width/height",
                    model.model_id, contract.family
                ));
            }
            for path in ["n", "steps", "cfg_scale", "response_format"] {
                if !contract
                    .request_attributes
                    .iter()
                    .any(|declared| declared == path)
                {
                    errors.push(format!(
                        "{} endpoint family {} must declare {}",
                        model.model_id, contract.family, path
                    ));
                } else if !has_default(path) && !is_required(path) {
                    errors.push(format!(
                        "{} endpoint family {} requires a signed default for {}",
                        model.model_id, contract.family, path
                    ));
                }
            }
        }
        mayhem_proto::ENDPOINT_HF_TEXT_TO_IMAGE => {
            for path in [
                "parameters.width",
                "parameters.height",
                "parameters.num_inference_steps",
                "parameters.guidance_scale",
            ] {
                if !contract
                    .request_attributes
                    .iter()
                    .any(|declared| declared == path)
                {
                    errors.push(format!(
                        "{} endpoint family {} must declare {}",
                        model.model_id, contract.family, path
                    ));
                } else if !has_default(path) && !is_required(path) {
                    errors.push(format!(
                        "{} endpoint family {} requires a signed default for {}",
                        model.model_id, contract.family, path
                    ));
                }
            }
        }
        _ => {}
    }
}

fn validate_endpoint_attribute_specs(
    model: &CatalogModel,
    contract: &EndpointFamilyContract,
    errors: &mut Vec<String>,
) {
    let Some(template) = endpoint_contract_template(&contract.family) else {
        return;
    };
    let request_names = contract
        .request_attributes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let request_spec_names = contract
        .request_attribute_specs
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if request_names != request_spec_names {
        errors.push(format!(
            "{} endpoint family {} request_attribute_specs must exactly cover request_attributes",
            model.model_id, contract.family
        ));
    }
    let response_names = contract
        .response_attributes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let response_spec_names = contract
        .response_attribute_specs
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if response_names != response_spec_names {
        errors.push(format!(
            "{} endpoint family {} response_attribute_specs must exactly cover response_attributes",
            model.model_id, contract.family
        ));
    }
    for (path, spec) in &contract.request_attribute_specs {
        let standard = template.request_attribute_specs.get(path).unwrap_or(spec);
        validate_endpoint_attribute_spec(
            &model.model_id,
            &contract.family,
            "request",
            path,
            spec,
            standard,
            errors,
        );
    }
    for (path, spec) in &contract.response_attribute_specs {
        let Some(standard) = template.response_attribute_specs.get(path) else {
            continue;
        };
        validate_endpoint_attribute_spec(
            &model.model_id,
            &contract.family,
            "response",
            path,
            spec,
            standard,
            errors,
        );
    }
    let mut seen_groups = BTreeSet::new();
    for group in &contract.interaction_groups {
        if group.len() < 2 || group.len() > 12 {
            errors.push(format!(
                "{} endpoint family {} interaction group must contain 2..=12 attributes",
                model.model_id, contract.family
            ));
            continue;
        }
        let mut unique = group.clone();
        unique.sort();
        unique.dedup();
        if unique.len() != group.len() {
            errors.push(format!(
                "{} endpoint family {} interaction group duplicates an attribute",
                model.model_id, contract.family
            ));
        }
        for path in group {
            if !request_names.contains(path.as_str()) {
                errors.push(format!(
                    "{} endpoint family {} interaction group references undeclared request attribute {}",
                    model.model_id, contract.family, path
                ));
            }
        }
        if !seen_groups.insert(unique) {
            errors.push(format!(
                "{} endpoint family {} duplicates an interaction group",
                model.model_id, contract.family
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_endpoint_attribute_spec(
    model_id: &str,
    family: &str,
    direction: &str,
    path: &str,
    spec: &mayhem_proto::EndpointAttributeSpec,
    standard: &mayhem_proto::EndpointAttributeSpec,
    errors: &mut Vec<String>,
) {
    let label = format!("{model_id} endpoint family {family} {direction} attribute {path}");
    if spec.value_types.is_empty() {
        errors.push(format!("{label} must declare at least one value type"));
    }
    let unique_types = spec.value_types.iter().collect::<BTreeSet<_>>();
    if unique_types.len() != spec.value_types.len() {
        errors.push(format!("{label} duplicates a value type"));
    }
    for value_type in &spec.value_types {
        if !standard.value_types.contains(value_type) {
            errors.push(format!(
                "{label} widens the task standard with unsupported type {value_type:?}"
            ));
        }
    }
    if spec.minimum.is_some_and(|value| !value.is_finite())
        || spec.maximum.is_some_and(|value| !value.is_finite())
        || matches!((spec.minimum, spec.maximum), (Some(minimum), Some(maximum)) if minimum > maximum)
    {
        errors.push(format!("{label} has invalid numeric bounds"));
    }
    if spec
        .multiple_of
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        errors.push(format!(
            "{label} multiple_of must be finite and greater than zero"
        ));
    }
    if matches!((spec.min_length, spec.max_length), (Some(minimum), Some(maximum)) if minimum > maximum)
    {
        errors.push(format!("{label} has invalid string-length bounds"));
    }
    if matches!((spec.min_items, spec.max_items), (Some(minimum), Some(maximum)) if minimum > maximum)
    {
        errors.push(format!("{label} has invalid array-length bounds"));
    }
    if standard.minimum.is_some() && spec.minimum.is_none() {
        errors.push(format!("{label} omits the task-standard minimum"));
    } else if let (Some(standard_minimum), Some(minimum)) = (standard.minimum, spec.minimum) {
        if minimum < standard_minimum {
            errors.push(format!("{label} widens the task-standard minimum"));
        }
    }
    if standard.maximum.is_some() && spec.maximum.is_none() {
        errors.push(format!("{label} omits the task-standard maximum"));
    } else if let (Some(standard_maximum), Some(maximum)) = (standard.maximum, spec.maximum) {
        if maximum > standard_maximum {
            errors.push(format!("{label} widens the task-standard maximum"));
        }
    }
    if let Some(standard_multiple) = standard.multiple_of {
        match spec.multiple_of {
            None => errors.push(format!("{label} omits the task-standard multiple_of")),
            Some(multiple) if multiple.is_finite() && multiple > 0.0 => {
                let quotient = multiple / standard_multiple;
                let tolerance = f64::EPSILON * quotient.abs().max(1.0) * 8.0;
                if (quotient - quotient.round()).abs() > tolerance {
                    errors.push(format!(
                        "{label} widens the task-standard multiple_of constraint"
                    ));
                }
            }
            Some(_) => {}
        }
    }
    if standard.min_length.is_some() && spec.min_length.is_none() {
        errors.push(format!("{label} omits the task-standard minimum length"));
    } else if let (Some(standard_minimum), Some(minimum)) = (standard.min_length, spec.min_length) {
        if minimum < standard_minimum {
            errors.push(format!("{label} widens the task-standard minimum length"));
        }
    }
    if standard.max_length.is_some() && spec.max_length.is_none() {
        errors.push(format!("{label} omits the task-standard maximum length"));
    } else if let (Some(standard_maximum), Some(maximum)) = (standard.max_length, spec.max_length) {
        if maximum > standard_maximum {
            errors.push(format!("{label} widens the task-standard maximum length"));
        }
    }
    if standard.min_items.is_some() && spec.min_items.is_none() {
        errors.push(format!(
            "{label} omits the task-standard minimum item count"
        ));
    } else if let (Some(standard_minimum), Some(minimum)) = (standard.min_items, spec.min_items) {
        if minimum < standard_minimum {
            errors.push(format!(
                "{label} widens the task-standard minimum item count"
            ));
        }
    }
    if standard.max_items.is_some() && spec.max_items.is_none() {
        errors.push(format!(
            "{label} omits the task-standard maximum item count"
        ));
    } else if let (Some(standard_maximum), Some(maximum)) = (standard.max_items, spec.max_items) {
        if maximum > standard_maximum {
            errors.push(format!(
                "{label} widens the task-standard maximum item count"
            ));
        }
    }
    if !standard.enum_values.is_empty() {
        if spec.enum_values.is_empty()
            || spec
                .enum_values
                .iter()
                .any(|value| !standard.enum_values.contains(value))
        {
            errors.push(format!("{label} widens or omits the task-standard enum"));
        }
    }
    let mut values = spec.calibration_values.iter().collect::<Vec<_>>();
    if let Some(default) = &spec.default {
        values.push(default);
    }
    if values.is_empty() {
        errors.push(format!(
            "{label} has no default or calibration value from which to generate conformance cases"
        ));
    }
    for value in values {
        if endpoint_calibration_marker_is_standard(value, standard) {
            continue;
        }
        if let Err(reason) = mayhem_proto::validate_endpoint_attribute_value(spec, value) {
            errors.push(format!("{label} has invalid declared test value: {reason}"));
        }
    }
}

fn endpoint_calibration_marker_is_standard(
    value: &Value,
    standard: &mayhem_proto::EndpointAttributeSpec,
) -> bool {
    let Some(marker) = value.as_str() else {
        return false;
    };
    marker.strip_prefix('$').is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    }) && (standard.calibration_values.contains(value) || standard.default.as_ref() == Some(value))
}

fn validate_endpoint_attribute_names(
    model_id: &str,
    family: &str,
    field: &str,
    attributes: &[String],
    errors: &mut Vec<String>,
) {
    if attributes.is_empty() || attributes.len() > 128 {
        errors.push(format!(
            "{model_id} endpoint family {family} {field} must contain 1..=128 entries"
        ));
    }
    let mut seen = BTreeSet::new();
    for attribute in attributes {
        if !valid_endpoint_attribute_name(attribute) {
            errors.push(format!(
                "{model_id} endpoint family {family} has invalid {field} entry {attribute}"
            ));
        }
        if !seen.insert(attribute) {
            errors.push(format!(
                "{model_id} endpoint family {family} duplicates {field} entry {attribute}"
            ));
        }
    }
}

fn valid_endpoint_attribute_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'[' | b']' | b'-')
        })
}

fn valid_endpoint_family(family: &str) -> bool {
    matches!(
        family,
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS
            | mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS
            | mayhem_proto::ENDPOINT_OPENAI_RESPONSES
            | mayhem_proto::ENDPOINT_HF_MULTIMODAL_CHAT
            | mayhem_proto::ENDPOINT_OPENAI_EMBEDDINGS
            | mayhem_proto::ENDPOINT_HF_FEATURE_EXTRACTION
            | mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS
            | mayhem_proto::ENDPOINT_HF_TEXT_TO_IMAGE
            | mayhem_proto::ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS
            | mayhem_proto::ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION
            | mayhem_proto::ENDPOINT_OPENAI_AUDIO_SPEECH
            | mayhem_proto::ENDPOINT_HF_TEXT_TO_SPEECH
            | mayhem_proto::ENDPOINT_OPENAI_VIDEOS
            | mayhem_proto::ENDPOINT_HF_TEXT_TO_VIDEO
            | mayhem_proto::ENDPOINT_MAYHEM_AUDIO_GENERATIONS
            | mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS
            | mayhem_proto::ENDPOINT_HF_TEXT_TO_AUDIO
    )
}

fn required_endpoint_families(model: &CatalogModel) -> BTreeSet<&'static str> {
    required_endpoint_family_names(&model.model_class, &model.adapter.modality_set)
}

fn required_endpoint_family_names(
    model_class: &str,
    modalities: &[String],
) -> BTreeSet<&'static str> {
    let mut required = match model_class {
        DEFAULT_MODEL_CLASS => BTreeSet::from([
            mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
            mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
        ]),
        MODEL_CLASS_EMBEDDING => BTreeSet::from([
            mayhem_proto::ENDPOINT_OPENAI_EMBEDDINGS,
            mayhem_proto::ENDPOINT_HF_FEATURE_EXTRACTION,
        ]),
        MODEL_CLASS_IMAGE_GENERATION => BTreeSet::from([
            mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_IMAGE,
        ]),
        MODEL_CLASS_VIDEO_GENERATION => BTreeSet::from([
            mayhem_proto::ENDPOINT_OPENAI_VIDEOS,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_VIDEO,
        ]),
        MODEL_CLASS_TTS => BTreeSet::from([
            mayhem_proto::ENDPOINT_OPENAI_AUDIO_SPEECH,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_SPEECH,
        ]),
        MODEL_CLASS_STT => BTreeSet::from([
            mayhem_proto::ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS,
            mayhem_proto::ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION,
        ]),
        MODEL_CLASS_AUDIO_GENERATION => BTreeSet::from([
            mayhem_proto::ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_AUDIO,
        ]),
        MODEL_CLASS_MUSIC_GENERATION => BTreeSet::from([
            mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS,
            mayhem_proto::ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_AUDIO,
        ]),
        _ => BTreeSet::new(),
    };
    if model_class == DEFAULT_MODEL_CLASS
        && modalities
            .iter()
            .any(|modality| matches!(modality.as_str(), "image" | "audio" | "video"))
    {
        required.insert(mayhem_proto::ENDPOINT_HF_MULTIMODAL_CHAT);
    }
    required
}

fn endpoint_family_allowed_for_model(model: &CatalogModel, family: &str) -> bool {
    match model.model_class.as_str() {
        DEFAULT_MODEL_CLASS => {
            matches!(
                family,
                mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS
                    | mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS
                    | mayhem_proto::ENDPOINT_OPENAI_RESPONSES
            ) || (family == mayhem_proto::ENDPOINT_HF_MULTIMODAL_CHAT
                && model
                    .adapter
                    .modality_set
                    .iter()
                    .any(|modality| matches!(modality.as_str(), "image" | "audio" | "video")))
        }
        MODEL_CLASS_EMBEDDING => matches!(
            family,
            mayhem_proto::ENDPOINT_OPENAI_EMBEDDINGS | mayhem_proto::ENDPOINT_HF_FEATURE_EXTRACTION
        ),
        MODEL_CLASS_IMAGE_GENERATION => matches!(
            family,
            mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS
                | mayhem_proto::ENDPOINT_HF_TEXT_TO_IMAGE
        ),
        MODEL_CLASS_VIDEO_GENERATION => matches!(
            family,
            mayhem_proto::ENDPOINT_OPENAI_VIDEOS | mayhem_proto::ENDPOINT_HF_TEXT_TO_VIDEO
        ),
        MODEL_CLASS_TTS => matches!(
            family,
            mayhem_proto::ENDPOINT_OPENAI_AUDIO_SPEECH | mayhem_proto::ENDPOINT_HF_TEXT_TO_SPEECH
        ),
        MODEL_CLASS_STT => matches!(
            family,
            mayhem_proto::ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS
                | mayhem_proto::ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION
        ),
        MODEL_CLASS_AUDIO_GENERATION => matches!(
            family,
            mayhem_proto::ENDPOINT_MAYHEM_AUDIO_GENERATIONS
                | mayhem_proto::ENDPOINT_HF_TEXT_TO_AUDIO
        ),
        MODEL_CLASS_MUSIC_GENERATION => matches!(
            family,
            mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS
                | mayhem_proto::ENDPOINT_MAYHEM_AUDIO_GENERATIONS
                | mayhem_proto::ENDPOINT_HF_TEXT_TO_AUDIO
        ),
        _ => false,
    }
}

fn valid_adapter_modality(modality: &str) -> bool {
    matches!(modality, "text" | "embedding" | "image" | "video" | "audio")
}

fn adapter_modality_allowed(model: &CatalogModel, modality: &str) -> bool {
    model
        .modality_assessment
        .detected
        .iter()
        .any(|detected| detected == modality)
}

fn validate_artifact(
    model_id: &str,
    tier: &str,
    name: &str,
    artifact: &CatalogArtifact,
    errors: &mut Vec<String>,
) {
    if !matches!(
        artifact.engine.as_str(),
        "llama.cpp"
            | "mlx"
            | "trt-llm"
            | "vllm"
            | "diffusers"
            | "stable-diffusion.cpp"
            | "comfyui"
            | "ace-step"
            | "transformers-asr"
            | "whisper.cpp"
            | "piper"
            | "kokoro"
    ) {
        errors.push(format!(
            "{model_id}/{name} has unsupported engine {}",
            artifact.engine
        ));
    }
    validate_source(
        model_id,
        &format!("artifacts.{name}.source"),
        &artifact.source,
        errors,
    );
    if let Some(source) = &artifact.upstream_source {
        validate_source(
            model_id,
            &format!("artifacts.{name}.upstream_source"),
            source,
            errors,
        );
    }
    if artifact.path.trim().is_empty() {
        errors.push(format!("{model_id}/{name} path is required"));
    } else {
        errors.extend(
            validate_huggingface_path(model_id, &format!("artifacts.{name}.path"), &artifact.path)
                .err()
                .unwrap_or_default(),
        );
    }
    if !is_hex_len(&artifact.artifact_root, 64) {
        errors.push(format!(
            "{model_id}/{name} artifact_root must be 32-byte hex"
        ));
    }
    if artifact.artifact_root_kind.trim().is_empty() {
        errors.push(format!("{model_id}/{name} artifact_root_kind is required"));
    }
    if tier == "launch" && artifact.artifact_root_kind != "blake3_merkle_v1" {
        errors.push(format!(
            "{model_id}/{name} launch artifact_root_kind must be blake3_merkle_v1"
        ));
    }
    if artifact.weights_bytes == 0 {
        errors.push(format!("{model_id}/{name} weights_bytes must be positive"));
    }
    if let Some(value) = &artifact.source_sha256 {
        if !is_hex_len(value, 64) {
            errors.push(format!(
                "{model_id}/{name} source_sha256 must be 32-byte hex"
            ));
        }
    }
    if tier == "launch" && artifact.source_sha256.is_none() {
        errors.push(format!(
            "{model_id}/{name} launch source_sha256 is required"
        ));
    }
    if let Some(value) = &artifact.tokenizer_sha256 {
        if !is_hex_len(value, 64) {
            errors.push(format!(
                "{model_id}/{name} tokenizer_sha256 must be 32-byte hex"
            ));
        }
    }
    if let Some(value) = &artifact.chat_template_sha256 {
        if !is_hex_len(value, 64) {
            errors.push(format!(
                "{model_id}/{name} chat_template_sha256 must be 32-byte hex"
            ));
        }
    }
    if matches!(artifact.engine.as_str(), "trt-llm" | "vllm") && artifact.min_compute_cap.is_none()
    {
        errors.push(format!(
            "{model_id}/{name} {} artifact needs min_compute_cap",
            artifact.engine
        ));
    }
    if artifact.engine == "vllm" {
        for required in [
            "vllm_config",
            "vllm_tokenizer_json",
            "vllm_tokenizer_config",
        ] {
            if !artifact.sidecars.contains_key(required) {
                errors.push(format!(
                    "{model_id}/{name} vllm artifact needs sidecar {required}"
                ));
            }
        }
    }
    if artifact.engine == "transformers-asr" {
        for (required, expected_path) in [
            ("transformers_config", "config.json"),
            ("transformers_generation_config", "generation_config.json"),
            ("transformers_processor_config", "processor_config.json"),
            ("transformers_tokenizer_json", "tokenizer.json"),
            ("transformers_tokenizer_config", "tokenizer_config.json"),
        ] {
            match artifact.sidecars.get(required) {
                Some(sidecar) if sidecar.path == expected_path => {}
                Some(sidecar) => errors.push(format!(
                    "{model_id}/{name} transformers-asr sidecar {required} must use path {expected_path}, got {}",
                    sidecar.path
                )),
                None => errors.push(format!(
                    "{model_id}/{name} transformers-asr artifact needs sidecar {required}"
                )),
            }
        }
    }
    if artifact.engine == "ace-step" {
        if artifact.path != "model.safetensors" {
            errors.push(format!(
                "{model_id}/{name} ace-step primary artifact must use path model.safetensors, got {}",
                artifact.path
            ));
        }
        for (required, expected_path) in ACE_STEP_REQUIRED_SIDECARS {
            match artifact.sidecars.get(*required) {
                Some(sidecar) if sidecar.path == *expected_path => {}
                Some(sidecar) => errors.push(format!(
                    "{model_id}/{name} ace-step sidecar {required} must use path {expected_path}, got {}",
                    sidecar.path
                )),
                None => errors.push(format!(
                    "{model_id}/{name} ace-step artifact needs sidecar {required}"
                )),
            }
        }
        let required = ACE_STEP_REQUIRED_SIDECARS
            .iter()
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>();
        for sidecar_name in artifact.sidecars.keys() {
            if !required.contains(sidecar_name.as_str()) {
                errors.push(format!(
                    "{model_id}/{name} ace-step artifact has unapproved sidecar {sidecar_name}"
                ));
            }
        }
    }
    match (&artifact.stable_diffusion_cpp, artifact.engine.as_str()) {
        (Some(config), "stable-diffusion.cpp") => {
            if let Err(error) = config.validate() {
                errors.push(format!(
                    "{model_id}/{name} has invalid stable_diffusion_cpp config: {error}"
                ));
            }
            if config.separate_diffusion_model {
                for required in ["text_encoder", "vae"] {
                    if !artifact.sidecars.contains_key(required) {
                        errors.push(format!(
                            "{model_id}/{name} split stable-diffusion.cpp artifact needs sidecar {required}"
                        ));
                    }
                }
            }
        }
        (None, "stable-diffusion.cpp") => errors.push(format!(
            "{model_id}/{name} stable-diffusion.cpp artifact must declare stable_diffusion_cpp semantics"
        )),
        (Some(_), _) => errors.push(format!(
            "{model_id}/{name} declares stable_diffusion_cpp config for engine {}",
            artifact.engine
        )),
        (None, _) => {}
    }
    for (sidecar_name, sidecar) in &artifact.sidecars {
        validate_artifact_sidecar(model_id, name, tier, sidecar_name, sidecar, errors);
    }
    let _ = (artifact.download_check, &artifact.notes);
}

const ACE_STEP_REQUIRED_SIDECARS: &[(&str, &str)] = &[
    ("ace_dit_config", "config.json"),
    ("ace_dit_configuration", "configuration_acestep_v15.py"),
    ("ace_dit_modeling", "modeling_acestep_v15_base.py"),
    ("ace_dit_apg_guidance", "apg_guidance.py"),
    ("ace_dit_silence_latent", "silence_latent.pt"),
    (
        "ace_embedding_added_tokens",
        "Qwen3-Embedding-0.6B/added_tokens.json",
    ),
    (
        "ace_embedding_chat_template",
        "Qwen3-Embedding-0.6B/chat_template.jinja",
    ),
    ("ace_embedding_config", "Qwen3-Embedding-0.6B/config.json"),
    ("ace_embedding_merges", "Qwen3-Embedding-0.6B/merges.txt"),
    (
        "ace_embedding_model",
        "Qwen3-Embedding-0.6B/model.safetensors",
    ),
    (
        "ace_embedding_special_tokens",
        "Qwen3-Embedding-0.6B/special_tokens_map.json",
    ),
    (
        "ace_embedding_tokenizer",
        "Qwen3-Embedding-0.6B/tokenizer.json",
    ),
    (
        "ace_embedding_tokenizer_config",
        "Qwen3-Embedding-0.6B/tokenizer_config.json",
    ),
    ("ace_embedding_vocab", "Qwen3-Embedding-0.6B/vocab.json"),
    (
        "ace_lm_added_tokens",
        "acestep-5Hz-lm-1.7B/added_tokens.json",
    ),
    (
        "ace_lm_chat_template",
        "acestep-5Hz-lm-1.7B/chat_template.jinja",
    ),
    ("ace_lm_config", "acestep-5Hz-lm-1.7B/config.json"),
    ("ace_lm_merges", "acestep-5Hz-lm-1.7B/merges.txt"),
    ("ace_lm_model", "acestep-5Hz-lm-1.7B/model.safetensors"),
    (
        "ace_lm_special_tokens",
        "acestep-5Hz-lm-1.7B/special_tokens_map.json",
    ),
    ("ace_lm_tokenizer", "acestep-5Hz-lm-1.7B/tokenizer.json"),
    (
        "ace_lm_tokenizer_config",
        "acestep-5Hz-lm-1.7B/tokenizer_config.json",
    ),
    ("ace_lm_vocab", "acestep-5Hz-lm-1.7B/vocab.json"),
    ("ace_vae_config", "vae/config.json"),
    ("ace_vae_model", "vae/diffusion_pytorch_model.safetensors"),
];

fn validate_artifact_sidecar(
    model_id: &str,
    artifact_name: &str,
    tier: &str,
    sidecar_name: &str,
    sidecar: &CatalogArtifactSidecar,
    errors: &mut Vec<String>,
) {
    if !is_safe_huggingface_path_segment(sidecar_name) {
        errors.push(format!(
            "{model_id}/{artifact_name} sidecar name {sidecar_name} must be a safe path segment"
        ));
    }
    validate_source(
        model_id,
        &format!("artifacts.{artifact_name}.sidecars.{sidecar_name}.source"),
        &sidecar.source,
        errors,
    );
    if let Some(upstream_source) = &sidecar.upstream_source {
        validate_source(
            model_id,
            &format!("artifacts.{artifact_name}.sidecars.{sidecar_name}.upstream_source"),
            upstream_source,
            errors,
        );
    }
    if sidecar.path.trim().is_empty() {
        errors.push(format!(
            "{model_id}/{artifact_name} sidecar {sidecar_name} path is required"
        ));
    } else {
        errors.extend(
            validate_huggingface_path(
                model_id,
                &format!("artifacts.{artifact_name}.sidecars.{sidecar_name}.path"),
                &sidecar.path,
            )
            .err()
            .unwrap_or_default(),
        );
    }
    if !is_hex_len(&sidecar.artifact_root, 64) {
        errors.push(format!(
            "{model_id}/{artifact_name} sidecar {sidecar_name} artifact_root must be 32-byte hex"
        ));
    }
    if sidecar.artifact_root_kind.trim().is_empty() {
        errors.push(format!(
            "{model_id}/{artifact_name} sidecar {sidecar_name} artifact_root_kind is required"
        ));
    }
    if tier == "launch" && sidecar.artifact_root_kind != "blake3_merkle_v1" {
        errors.push(format!(
            "{model_id}/{artifact_name} launch sidecar {sidecar_name} artifact_root_kind must be blake3_merkle_v1"
        ));
    }
    if sidecar.weights_bytes == 0 {
        errors.push(format!(
            "{model_id}/{artifact_name} sidecar {sidecar_name} weights_bytes must be positive"
        ));
    }
    if !is_hex_len(&sidecar.source_sha256, 64) {
        errors.push(format!(
            "{model_id}/{artifact_name} sidecar {sidecar_name} source_sha256 must be 32-byte hex"
        ));
    }
}

fn validate_source(model_id: &str, label: &str, source: &SourceRef, errors: &mut Vec<String>) {
    errors.extend(
        validate_huggingface_source(model_id, label, source)
            .err()
            .unwrap_or_default(),
    );
}

fn validate_huggingface_source(
    model_id: &str,
    label: &str,
    source: &SourceRef,
) -> std::result::Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if source.kind != "huggingface" {
        errors.push(format!("{model_id} {label}.kind must be huggingface"));
    }
    if !is_safe_huggingface_repo(&source.repo) {
        errors.push(format!(
            "{model_id} {label}.repo must be a safe namespace/name repo id"
        ));
    }
    if !is_hex_len(&source.revision, 40) {
        errors.push(format!(
            "{model_id} {label}.revision must be a 20-byte git commit hex"
        ));
    }
    let _ = &source.publisher_key;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_huggingface_path(
    model_id: &str,
    label: &str,
    path: &str,
) -> std::result::Result<(), Vec<String>> {
    if is_safe_huggingface_path(path) {
        Ok(())
    } else {
        Err(vec![format!(
            "{model_id} {label} must be a relative Hugging Face artifact path without traversal or URL syntax"
        )])
    }
}

fn is_safe_huggingface_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let Some(namespace) = parts.next() else {
        return false;
    };
    let Some(name) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && is_safe_huggingface_component(namespace)
        && is_safe_huggingface_component(name)
}

fn is_safe_huggingface_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 96
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !matches!(bytes.first(), Some(b'-' | b'.'))
        && !matches!(bytes.last(), Some(b'-' | b'.'))
        && !value.contains("..")
        && !value.contains("--")
}

fn is_safe_huggingface_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains('\\')
        && !path.contains('?')
        && !path.contains('#')
        && !path.contains('%')
        && !path.bytes().any(|byte| byte.is_ascii_control())
        && path.split('/').all(is_safe_huggingface_path_segment)
}

fn is_safe_huggingface_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
}

fn validate_canary_set(canaries_dir: &Path, set_id: &str, errors: &mut Vec<String>) {
    let path = canaries_dir.join(format!("{set_id}.json"));
    match fs::read_to_string(&path)
        .with_context(|| format!("reading canary set {}", path.display()))
        .and_then(|text| {
            serde_json::from_str::<CanarySet>(&text)
                .with_context(|| format!("parsing canary set {}", path.display()))
        }) {
        Ok(canary) => {
            if canary.set_id != set_id {
                errors.push(format!(
                    "canary set file {} declares {}",
                    set_id, canary.set_id
                ));
            }
            if canary.prompts.is_empty() {
                errors.push(format!("canary set {set_id} has no prompts"));
            }
            let mut prompt_ids = BTreeSet::new();
            for prompt in &canary.prompts {
                if prompt.id.trim().is_empty() {
                    errors.push(format!("canary set {set_id} has an empty prompt id"));
                } else if !prompt_ids.insert(prompt.id.as_str()) {
                    errors.push(format!(
                        "canary set {set_id} duplicates prompt id {}",
                        prompt.id
                    ));
                }
                if prompt.max_tokens == Some(0) {
                    errors.push(format!(
                        "canary prompt {} in {set_id} must use positive max_tokens",
                        prompt.id
                    ));
                }
                if prompt
                    .temperature
                    .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
                {
                    errors.push(format!(
                        "canary prompt {} in {set_id} has invalid temperature",
                        prompt.id
                    ));
                }
                if prompt
                    .top_p
                    .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1.0)
                {
                    errors.push(format!(
                        "canary prompt {} in {set_id} has invalid top_p",
                        prompt.id
                    ));
                }
                if prompt
                    .top_k
                    .is_some_and(|value| !(0..=1_000_000).contains(&value))
                {
                    errors.push(format!(
                        "canary prompt {} in {set_id} has invalid top_k",
                        prompt.id
                    ));
                }
                if prompt
                    .min_p
                    .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
                {
                    errors.push(format!(
                        "canary prompt {} in {set_id} has invalid min_p",
                        prompt.id
                    ));
                }
                if prompt
                    .repeat_penalty
                    .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 10.0)
                {
                    errors.push(format!(
                        "canary prompt {} in {set_id} has invalid repeat_penalty",
                        prompt.id
                    ));
                }
                if prompt
                    .frequency_penalty
                    .is_some_and(|value| !value.is_finite() || !(-2.0..=2.0).contains(&value))
                {
                    errors.push(format!(
                        "canary prompt {} in {set_id} has invalid frequency_penalty",
                        prompt.id
                    ));
                }
                if prompt
                    .presence_penalty
                    .is_some_and(|value| !value.is_finite() || !(-2.0..=2.0).contains(&value))
                {
                    errors.push(format!(
                        "canary prompt {} in {set_id} has invalid presence_penalty",
                        prompt.id
                    ));
                }
                if prompt.temperature.is_some_and(|value| value > 0.0) && prompt.seed.is_none() {
                    errors.push(format!(
                        "canary prompt {} in {set_id} must pin seed when temperature is non-zero",
                        prompt.id
                    ));
                }
                if prompt.seed.is_some_and(|seed| seed > u64::from(u32::MAX)) {
                    errors.push(format!(
                        "canary prompt {} in {set_id} seed exceeds u32",
                        prompt.id
                    ));
                }
            }
        }
        Err(err) => errors.push(err.to_string()),
    }
}

fn validate_model_canary_modality_coverage(
    canaries_dir: &Path,
    model: &CatalogModel,
    errors: &mut Vec<String>,
) {
    let path = canaries_dir.join(format!("{}.json", model.canary.set_id));
    let canary = match fs::read_to_string(&path)
        .with_context(|| format!("reading canary set {}", path.display()))
        .and_then(|text| {
            serde_json::from_str::<CanarySet>(&text)
                .with_context(|| format!("parsing canary set {}", path.display()))
        }) {
        Ok(canary) => canary,
        Err(_) => return,
    };
    let mut covered = BTreeSet::new();
    for prompt in &canary.prompts {
        covered.extend(canary_prompt_modalities(model, prompt));
    }
    for modality in &model.adapter.modality_set {
        if !covered.contains(modality.as_str()) {
            errors.push(format!(
                "{} served modality {} has no functional prompt in canary set {} using {}",
                model.model_id, modality, model.canary.set_id, model.canary.verification_method
            ));
        }
    }
}

fn canary_prompt_modalities<'a>(
    model: &'a CatalogModel,
    prompt: &'a CanarySetPrompt,
) -> BTreeSet<&'a str> {
    let mut modalities = BTreeSet::new();
    match model.canary.verification_method.as_str() {
        VERIFICATION_TOKEN_FINGERPRINT => {
            if !prompt.messages.is_empty() {
                modalities.insert("text");
            }
            for message in &prompt.messages {
                for part in message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("image_url") => {
                            modalities.insert("image");
                        }
                        Some("input_audio") => {
                            modalities.insert("audio");
                        }
                        Some("video") | Some("video_url") | Some("input_video") => {
                            modalities.insert("video");
                        }
                        _ => {}
                    }
                }
            }
        }
        VERIFICATION_SEED_PERCEPTUAL_HASH if prompt.prompt.is_some() => {
            modalities.insert("image");
        }
        VERIFICATION_EMBEDDING_COSINE if prompt.input.is_some() || prompt.prompt.is_some() => {
            modalities.insert("embedding");
        }
        VERIFICATION_TRANSCRIPT_MATCH if prompt.audio_b64.is_some() => {
            modalities.insert("audio");
            modalities.insert("text");
        }
        VERIFICATION_AUDIO_FINGERPRINT if prompt.input.is_some() || prompt.prompt.is_some() => {
            modalities.insert("audio");
        }
        _ => {}
    }
    modalities
}

fn run_download_checks(
    catalog: &CatalogDocument,
    hf_token_file: Option<&Path>,
) -> Result<Vec<DownloadCheckReport>> {
    let token = read_hf_token(hf_token_file)?;
    let mut reports = Vec::new();
    for model in &catalog.models {
        for (artifact_name, artifact) in &model.artifacts {
            if !artifact.download_check {
                continue;
            }
            let ok = check_hf_artifact(
                &artifact.source.repo,
                &artifact.source.revision,
                &artifact.path,
                &token,
            )
            .with_context(|| {
                format!(
                    "checking download for {} {} from {}",
                    model.model_id, artifact_name, artifact.source.repo
                )
            })?;
            reports.push(DownloadCheckReport {
                model_id: model.model_id.clone(),
                artifact: artifact_name.clone(),
                repo: artifact.source.repo.clone(),
                revision: artifact.source.revision.clone(),
                path: artifact.path.clone(),
                ok,
            });
        }
    }
    if reports.is_empty() {
        bail!("no artifacts in the catalog are marked download_check=true");
    }
    Ok(reports)
}

fn run_source_checks(
    catalog: &CatalogDocument,
    hf_token_file: Option<&Path>,
    launch_only: bool,
) -> Result<Vec<SourceCheckReport>> {
    let token = read_hf_token(hf_token_file)?;
    let mut reports = Vec::new();
    for model in &catalog.models {
        if launch_only && model.tier != "launch" {
            continue;
        }
        for (artifact_name, artifact) in &model.artifacts {
            reports.push(source_check_report(
                model.model_id.clone(),
                artifact_name,
                &artifact.source,
                &artifact.path,
                &artifact.artifact_root_kind,
                artifact.source_sha256.clone(),
                if model.tier == "launch" {
                    launch_source_metadata_errors(model, artifact_name, artifact)
                } else {
                    Vec::new()
                },
                &token,
            )?);
            for (sidecar_name, sidecar) in &artifact.sidecars {
                let label = format!("{artifact_name}.sidecar.{sidecar_name}");
                reports.push(source_check_report(
                    model.model_id.clone(),
                    &label,
                    &sidecar.source,
                    &sidecar.path,
                    &sidecar.artifact_root_kind,
                    Some(sidecar.source_sha256.clone()),
                    if model.tier == "launch" {
                        launch_sidecar_metadata_errors(model, artifact_name, sidecar_name, sidecar)
                    } else {
                        Vec::new()
                    },
                    &token,
                )?);
            }
        }
    }
    if reports.is_empty() {
        bail!(
            "no {}artifacts in the catalog to source-check",
            if launch_only { "launch " } else { "" }
        );
    }
    Ok(reports)
}

fn source_check_report(
    model_id: String,
    artifact_label: &str,
    source: &SourceRef,
    path: &str,
    artifact_root_kind: &str,
    source_sha256: Option<String>,
    metadata_errors: Vec<String>,
    token: &str,
) -> Result<SourceCheckReport> {
    let url = huggingface_resolve_url(source, path)?;
    let check = check_hf_artifact_head(&source.repo, &source.revision, path, token);
    let (status, ok, error) = match check {
        Ok(status) => (
            Some(status),
            source_status_ok(status) && metadata_errors.is_empty(),
            None,
        ),
        Err(err) => (None, false, Some(err.to_string())),
    };
    Ok(SourceCheckReport {
        model_id,
        artifact: artifact_label.to_owned(),
        repo: source.repo.clone(),
        revision: source.revision.clone(),
        path: path.to_owned(),
        url,
        artifact_root_kind: artifact_root_kind.to_owned(),
        source_sha256,
        status,
        ok,
        error,
        metadata_errors,
    })
}

fn launch_source_metadata_errors(
    model: &CatalogModel,
    artifact_name: &str,
    artifact: &CatalogArtifact,
) -> Vec<String> {
    let mut errors = Vec::new();
    if artifact.artifact_root_kind != "blake3_merkle_v1" {
        errors.push(format!(
            "{} / {} artifact_root_kind must be blake3_merkle_v1 for launch calibration, got {}",
            model.model_id, artifact_name, artifact.artifact_root_kind
        ));
    }
    match artifact.source_sha256.as_deref() {
        Some(value) if is_hex_len(value, 64) => {}
        Some(value) => errors.push(format!(
            "{} / {} source_sha256 must be 32-byte hex for launch calibration, got {}",
            model.model_id, artifact_name, value
        )),
        None => errors.push(format!(
            "{} / {} source_sha256 is required before launch calibration",
            model.model_id, artifact_name
        )),
    }
    errors
}

fn launch_sidecar_metadata_errors(
    model: &CatalogModel,
    artifact_name: &str,
    sidecar_name: &str,
    sidecar: &CatalogArtifactSidecar,
) -> Vec<String> {
    let mut errors = Vec::new();
    if sidecar.artifact_root_kind != "blake3_merkle_v1" {
        errors.push(format!(
            "{} / {} sidecar {} artifact_root_kind must be blake3_merkle_v1 for launch calibration, got {}",
            model.model_id, artifact_name, sidecar_name, sidecar.artifact_root_kind
        ));
    }
    if !is_hex_len(&sidecar.source_sha256, 64) {
        errors.push(format!(
            "{} / {} sidecar {} source_sha256 must be 32-byte hex for launch calibration, got {}",
            model.model_id, artifact_name, sidecar_name, sidecar.source_sha256
        ));
    }
    errors
}

fn read_hf_token(path: Option<&Path>) -> Result<String> {
    if let Some(path) = path {
        return fs::read_to_string(path)
            .with_context(|| format!("reading HF token file {}", path.display()))
            .map(|value| value.trim().to_owned());
    }
    if let Ok(token) = std::env::var("HF_TOKEN") {
        let token = token.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    bail!("set HF_TOKEN or pass --hf-token-file for catalog source/download checks")
}

fn check_hf_artifact(repo: &str, revision: &str, path: &str, token: &str) -> Result<bool> {
    let url = format!("https://huggingface.co/{repo}/resolve/{revision}/{path}");
    let config = format!(
        concat!(
            "fail\n",
            "silent\n",
            "show-error\n",
            "location\n",
            "range = \"0-0\"\n",
            "output = \"/dev/null\"\n",
            "url = \"{}\"\n",
            "header = \"Authorization: Bearer {}\"\n"
        ),
        curl_config_escape(&url),
        curl_config_escape(token)
    );
    let mut child = Command::new("curl")
        .arg("-K")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning curl for Hugging Face download check")?;
    {
        let stdin = child.stdin.as_mut().context("opening curl stdin")?;
        stdin
            .write_all(config.as_bytes())
            .context("writing curl config")?;
    }
    let output = child.wait_with_output().context("waiting for curl")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("curl failed: {}", stderr.trim());
    }
    Ok(true)
}

fn check_hf_artifact_head(repo: &str, revision: &str, path: &str, token: &str) -> Result<u16> {
    let url = format!("https://huggingface.co/{repo}/resolve/{revision}/{path}");
    let config = format!(
        concat!(
            "silent\n",
            "show-error\n",
            "location\n",
            "head\n",
            "output = \"/dev/null\"\n",
            "write-out = \"%{{http_code}}\"\n",
            "url = \"{}\"\n",
            "header = \"Authorization: Bearer {}\"\n"
        ),
        curl_config_escape(&url),
        curl_config_escape(token)
    );
    let mut child = Command::new("curl")
        .arg("-K")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning curl for Hugging Face source check")?;
    {
        let stdin = child.stdin.as_mut().context("opening curl stdin")?;
        stdin
            .write_all(config.as_bytes())
            .context("writing curl config")?;
    }
    let output = child.wait_with_output().context("waiting for curl")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("curl failed: {}", stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status = stdout
        .trim()
        .parse::<u16>()
        .with_context(|| format!("parsing curl HTTP status {}", stdout.trim()))?;
    Ok(status)
}

fn curl_config_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn source_status_ok(status: u16) -> bool {
    (200..400).contains(&status)
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn hex_to_array<const N: usize>(value: &str) -> Result<[u8; N]> {
    let bytes = hex_to_vec(value)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected {} bytes of hex", N))
}

fn hex_to_vec(value: &str) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        bail!("invalid hex");
    }
    let mut out = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("invalid hex"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;

    #[test]
    fn signature_verification_rejects_tampering() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let bytes = br#"{"schema_version":1}"#;
        let sig = signing_key.sign(bytes);
        let public_key = hex_string(&signing_key.verifying_key().to_bytes());
        let signature = CatalogSignature {
            schema_version: 1,
            alg: "ed25519".to_owned(),
            signed_path: "catalog/models.json".to_owned(),
            key_id: "test".to_owned(),
            public_key,
            blake3: blake3::hash(bytes).to_hex().to_string(),
            sig: hex_string(&sig.to_bytes()),
        };

        verify_signature_bytes(bytes, &signature).unwrap();
        assert!(verify_signature_bytes(br#"{"schema_version":2}"#, &signature).is_err());
    }

    #[test]
    fn signature_verification_rejects_low_order_forgery() {
        let mut weak_key = [0_u8; 32];
        weak_key[0] = 1;
        let mut weak_signature = [0_u8; 64];
        weak_signature[0] = 1;
        let bytes = br#"{"schema_version":1}"#;
        let signature = CatalogSignature {
            schema_version: 1,
            alg: "ed25519".to_owned(),
            signed_path: "catalog/models.json".to_owned(),
            key_id: "test".to_owned(),
            public_key: hex_string(&weak_key),
            blake3: blake3::hash(bytes).to_hex().to_string(),
            sig: hex_string(&weak_signature),
        };

        verify_signature_bytes(bytes, &signature)
            .expect_err("low-order catalog signature forgery must fail strict verification");
    }

    #[test]
    fn hex_parser_rejects_bad_input() {
        assert_eq!(hex_to_vec("00ff").unwrap(), vec![0, 255]);
        assert!(hex_to_vec("0").is_err());
        assert!(hex_to_vec("zz").is_err());
    }

    #[test]
    fn absent_catalog_attestation_authority_is_tier1_only() {
        let authority: CatalogAttestationAuthority =
            serde_json::from_value(serde_json::json!({ "models": [] })).unwrap();
        let mut errors = Vec::new();
        validate_catalog_attestation_authority(&authority, &mut errors);

        assert!(authority.is_tier1_only());
        assert!(errors.is_empty());
    }

    #[test]
    fn invalid_catalog_attestation_authority_degrades_to_tier1_only() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "attestation_policy_chain": [],
            "enclave_attestation_bindings": []
        }))
        .unwrap();
        let (authority, errors) = validated_catalog_attestation_authority(&bytes);

        assert!(authority.is_tier1_only());
        assert!(errors
            .iter()
            .any(|error| error.contains("must not be present but empty")));
    }

    #[test]
    fn catalog_attestation_authority_uses_canonical_chain_validator() {
        let binding = AdminEnclaveAttestationBinding {
            enclave_id: "11".repeat(32),
            kind: mayhem_proto::HardwareQuoteKind::Tpm2QuoteEk,
            platform: Some("windows-tpm2".to_owned()),
            measurement_trust_data: BTreeMap::new(),
        };
        let mut errors = Vec::new();
        validate_catalog_attestation_authority(
            &CatalogAttestationAuthority {
                attestation_policy_chain: None,
                enclave_attestation_bindings: vec![binding],
            },
            &mut errors,
        );

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("require an attestation policy chain"));
    }

    #[test]
    fn huggingface_resolve_url_requires_safe_pinned_catalog_reference() {
        let source = SourceRef {
            kind: "huggingface".to_owned(),
            repo: "admin-approved/model-4bit".to_owned(),
            revision: "1".repeat(40),
            publisher_key: None,
        };

        assert_eq!(
            huggingface_resolve_url(&source, "snapshots/model.safetensors").unwrap(),
            format!(
                "https://huggingface.co/admin-approved/model-4bit/resolve/{}/snapshots/model.safetensors",
                "1".repeat(40)
            )
        );

        let mut branch = source.clone();
        branch.revision = "main".to_owned();
        assert!(huggingface_resolve_url(&branch, "model.safetensors").is_err());

        let mut unsafe_repo = source.clone();
        unsafe_repo.repo = "admin/model/extra".to_owned();
        assert!(huggingface_resolve_url(&unsafe_repo, "model.safetensors").is_err());

        assert!(huggingface_resolve_url(&source, "../model.safetensors").is_err());
        assert!(huggingface_resolve_url(&source, "/model.safetensors").is_err());
        assert!(huggingface_resolve_url(&source, "model.safetensors?download=1").is_err());
    }

    #[test]
    fn source_status_ok_accepts_success_and_redirect_only() {
        assert!(source_status_ok(200));
        assert!(source_status_ok(302));
        assert!(!source_status_ok(401));
        assert!(!source_status_ok(404));
        assert!(!source_status_ok(500));
    }

    #[test]
    fn ace_step_artifact_requires_the_exact_signed_component_inventory() {
        let mut model = verification_test_model(
            "admin/ace-step@sft",
            MODEL_CLASS_MUSIC_GENERATION,
            "ace-step",
            CanaryRef {
                set_id: "canary-music-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_AUDIO_FINGERPRINT.to_owned(),
                verification_tolerance_bps: Some(1_000),
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([(
                        "music".to_owned(),
                        format!("audiospec-v1:1000:{}", "01".repeat(256)),
                    )]),
                )]),
            },
        );
        let artifact = model.artifacts.get_mut("fixture").unwrap();
        artifact.sidecars = ACE_STEP_REQUIRED_SIDECARS
            .iter()
            .enumerate()
            .map(|(index, (name, path))| {
                (
                    (*name).to_owned(),
                    CatalogArtifactSidecar {
                        source: artifact.source.clone(),
                        upstream_source: None,
                        path: (*path).to_owned(),
                        artifact_root: format!("{index:064x}"),
                        artifact_root_kind: "blake3_merkle_v1".to_owned(),
                        weights_bytes: 1,
                        source_sha256: format!("{:064x}", index + 1),
                    },
                )
            })
            .collect();

        let mut errors = Vec::new();
        validate_artifact(
            &model.model_id,
            &model.tier,
            "fixture",
            model.artifacts.get("fixture").unwrap(),
            &mut errors,
        );
        assert!(errors.is_empty(), "{errors:#?}");

        let mut missing = model.artifacts["fixture"].clone();
        missing.sidecars.remove("ace_vae_model");
        let mut errors = Vec::new();
        validate_artifact(
            &model.model_id,
            &model.tier,
            "fixture",
            &missing,
            &mut errors,
        );
        assert!(errors
            .iter()
            .any(|error| error.contains("needs sidecar ace_vae_model")));

        let mut extra = model.artifacts["fixture"].clone();
        extra.sidecars.insert(
            "provider_code".to_owned(),
            CatalogArtifactSidecar {
                source: extra.source.clone(),
                upstream_source: None,
                path: "provider.py".to_owned(),
                artifact_root: "b".repeat(64),
                artifact_root_kind: "blake3_merkle_v1".to_owned(),
                weights_bytes: 1,
                source_sha256: "c".repeat(64),
            },
        );
        let mut errors = Vec::new();
        validate_artifact(&model.model_id, &model.tier, "fixture", &extra, &mut errors);
        assert!(errors
            .iter()
            .any(|error| error.contains("unapproved sidecar provider_code")));
    }

    #[test]
    fn model_min_app_version_must_be_semver_when_present() {
        let mut model = verification_test_model(
            "admin/model@4bit",
            DEFAULT_MODEL_CLASS,
            "llama.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::from([("fixture".to_owned(), "a".repeat(64))]),
                token_prefixes: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([(
                        "fixed-text".to_owned(),
                        vec![1; MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS],
                    )]),
                )]),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.min_app_version = Some("0.1.0".to_owned());
        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(
            !errors.iter().any(|error| error.contains("min_app_version")),
            "{errors:?}"
        );

        model.min_app_version = Some("next".to_owned());
        errors.clear();
        validate_model(&model, &mut errors);
        assert!(errors
            .iter()
            .any(|error| error.contains("min_app_version must be a semantic version")));
    }

    #[test]
    fn speciality_catalog_validation_requires_complete_per_artifact_levels() {
        let mut model = verification_test_model(
            "admin/model@4bit",
            DEFAULT_MODEL_CLASS,
            "llama.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::from([("fixture".to_owned(), "a".repeat(64))]),
                token_prefixes: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([(
                        "fixed-text".to_owned(),
                        vec![1; MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS],
                    )]),
                )]),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        let descriptor = ModelSpecialityDescriptor {
            name: "reasoning_effort".to_owned(),
            mechanism: "enum".to_owned(),
            default_level: "low".to_owned(),
            levels: vec![
                mayhem_proto::ModelSpecialityLevel {
                    name: "low".to_owned(),
                    rank: 0,
                    native_value: Value::String("low".to_owned()),
                    default_max_output_tokens: Some(8),
                    max_reasoning_tokens: Some(4),
                },
                mayhem_proto::ModelSpecialityLevel {
                    name: "high".to_owned(),
                    rank: 1,
                    native_value: Value::String("high".to_owned()),
                    default_max_output_tokens: Some(32),
                    max_reasoning_tokens: Some(24),
                },
            ],
            calibration_modalities: Vec::new(),
            research_evidence: vec!["pinned family documentation".to_owned()],
        };
        for contract in &mut model.adapter.endpoint_families {
            contract
                .request_attributes
                .push("reasoning_effort".to_owned());
            let mut spec = mayhem_proto::EndpointAttributeSpec::new(EndpointValueType::String);
            spec.default = Some(Value::String("low".to_owned()));
            spec.enum_values = vec![
                Value::String("low".to_owned()),
                Value::String("high".to_owned()),
            ];
            spec.calibration_values = spec.enum_values.clone();
            contract
                .request_attribute_specs
                .insert("reasoning_effort".to_owned(), spec);
            contract.speciality_mappings.insert(
                "reasoning_effort".to_owned(),
                mayhem_proto::EndpointSpecialityMapping {
                    request_path: "reasoning_effort".to_owned(),
                    target: EndpointSpecialityTarget::ChatTemplateKwarg,
                    native_path: "reasoning_effort".to_owned(),
                },
            );
        }
        model.adapter.specialities = vec![descriptor];
        model.speciality_assessment.detected = vec!["reasoning_effort".to_owned()];
        model.speciality_assessment.evidence = vec!["pinned family documentation".to_owned()];
        model.speciality_assessment.calibrated.insert(
            "fixture".to_owned(),
            BTreeMap::from([(
                "reasoning_effort".to_owned(),
                BTreeMap::from([
                    (
                        "low".to_owned(),
                        CatalogSpecialityCalibration {
                            fingerprint: "b".repeat(64),
                            token_prefixes: BTreeMap::from([(
                                "fixed-text".to_owned(),
                                vec![1; MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS],
                            )]),
                            output_tokens_min: 8,
                            output_tokens_max: 8,
                            reasoning_tokens_min: 2,
                            reasoning_tokens_max: 2,
                        },
                    ),
                    (
                        "high".to_owned(),
                        CatalogSpecialityCalibration {
                            fingerprint: "c".repeat(64),
                            token_prefixes: BTreeMap::from([(
                                "fixed-text".to_owned(),
                                vec![2; MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS],
                            )]),
                            output_tokens_min: 24,
                            output_tokens_max: 24,
                            reasoning_tokens_min: 16,
                            reasoning_tokens_max: 16,
                        },
                    ),
                ]),
            )]),
        );

        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(
            !errors.iter().any(|error| error.contains("speciality")),
            "{errors:?}"
        );

        model
            .speciality_assessment
            .calibrated
            .get_mut("fixture")
            .unwrap()
            .get_mut("reasoning_effort")
            .unwrap()
            .remove("high");
        errors.clear();
        validate_model(&model, &mut errors);
        assert!(errors.iter().any(|error| {
            error.contains("speciality calibration") && error.contains("missing level high")
        }));
    }

    #[test]
    fn launch_source_metadata_requires_merkle_root_and_source_sha() {
        let mut model = CatalogModel {
            model_id: "admin/model@4bit".to_owned(),
            model_class: DEFAULT_MODEL_CLASS.to_owned(),
            family: "admin".to_owned(),
            params_b: 4.0,
            tier: "launch".to_owned(),
            min_app_version: None,
            provenance: Provenance {
                source: SourceRef {
                    kind: "huggingface".to_owned(),
                    repo: "admin/source".to_owned(),
                    revision: "1".repeat(40),
                    publisher_key: None,
                },
                conversion: Vec::new(),
                license: "apache-2.0".to_owned(),
                license_sha256: "a".repeat(64),
            },
            artifacts: BTreeMap::new(),
            caps: CatalogCaps {
                tools: true,
                json: true,
                ctx_max: 1024,
                vision: false,
                image: false,
                video: false,
                audio: false,
                output_modality: Some("text".to_owned()),
                output_modalities: vec!["text".to_owned()],
            },
            requirements: CatalogRequirements {
                min_ram_gb: 8,
                min_vram_gb_full_offload: 0,
                cpu_flags: Vec::new(),
                backends: vec!["llama.cpp".to_owned()],
            },
            adapter: CatalogAdapter::default(),
            modality_assessment: CatalogModalityAssessment {
                detected: vec!["text".to_owned()],
                evidence: vec!["test fixture".to_owned()],
                calibrated_fingerprints: BTreeMap::new(),
                resource_profiles: BTreeMap::new(),
            },
            speciality_assessment: CatalogSpecialityAssessment {
                evidence: vec!["test fixture researched: no model specialities".to_owned()],
                ..CatalogSpecialityAssessment::default()
            },
            sampling: CatalogSamplingProfile::default(),
            canary: CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
            price_ref_au: PriceRef {
                denom: "au_usd".to_owned(),
                in_per_1k: 1,
                out_per_1k: 1,
                rate_map: Vec::new(),
            },
        };
        let mut artifact = CatalogArtifact {
            engine: "llama.cpp".to_owned(),
            stable_diffusion_cpp: None,
            source: SourceRef {
                kind: "huggingface".to_owned(),
                repo: "admin/model".to_owned(),
                revision: "2".repeat(40),
                publisher_key: None,
            },
            upstream_source: None,
            path: "model.gguf".to_owned(),
            artifact_root: "b".repeat(64),
            artifact_root_kind: "blake3_descriptor_until_p2_4".to_owned(),
            weights_bytes: 1,
            source_sha256: None,
            tokenizer_sha256: None,
            chat_template_sha256: None,
            min_compute_cap: None,
            download_check: false,
            notes: None,
            sidecars: BTreeMap::new(),
        };

        let errors = launch_source_metadata_errors(&model, "gguf-q4_k_m", &artifact);
        assert_eq!(errors.len(), 2);
        assert!(errors
            .iter()
            .any(|error| error.contains("artifact_root_kind must be blake3_merkle_v1")));
        assert!(errors
            .iter()
            .any(|error| error.contains("source_sha256 is required")));

        artifact.artifact_root_kind = "blake3_merkle_v1".to_owned();
        artifact.source_sha256 = Some("c".repeat(64));
        model
            .artifacts
            .insert("gguf-q4_k_m".to_owned(), artifact.clone());
        assert!(launch_source_metadata_errors(&model, "gguf-q4_k_m", &artifact).is_empty());
    }

    #[test]
    fn canary_verification_descriptor_is_class_specific() {
        let text = verification_test_model(
            "admin/text@fixture",
            DEFAULT_MODEL_CLASS,
            "llama.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::from([("fixture".to_owned(), "a".repeat(64))]),
                token_prefixes: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([(
                        "fixed-text".to_owned(),
                        vec![1; MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS],
                    )]),
                )]),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        let mut errors = Vec::new();
        validate_model(&text, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let image = verification_test_model(
            "admin/image@fixture",
            "image-generation",
            "diffusers",
            CanaryRef {
                set_id: "canary-image-v1".to_owned(),
                match_min: 0.95,
                verification_method: VERIFICATION_SEED_PERCEPTUAL_HASH.to_owned(),
                verification_tolerance_bps: Some(500),
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([("fixed-image".to_owned(), "f".repeat(16))]),
                )]),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        let mut errors = Vec::new();
        validate_model(&image, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut invalid_image = image.clone();
        invalid_image.canary.verification_method = VERIFICATION_TOKEN_FINGERPRINT.to_owned();
        invalid_image.canary.verification_tolerance_bps = None;
        invalid_image.canary.fingerprints =
            BTreeMap::from([("fixture".to_owned(), "b".repeat(64))]);
        invalid_image.canary.token_prefixes = BTreeMap::from([(
            "fixture".to_owned(),
            BTreeMap::from([("fixed-image".to_owned(), vec![4, 5, 6])]),
        )]);
        invalid_image.canary.perceptual_hashes = BTreeMap::new();
        let mut errors = Vec::new();
        validate_model(&invalid_image, &mut errors);
        assert!(errors.iter().any(|error| error.contains(
            "canary.verification_method token_fingerprint is not allowed for model_class image-generation"
        )));

        let mut missing_tolerance = image.clone();
        missing_tolerance.canary.verification_tolerance_bps = None;
        let mut errors = Vec::new();
        validate_model(&missing_tolerance, &mut errors);
        assert!(errors
            .iter()
            .any(|error| error.contains("requires verification_tolerance_bps")));

        let embedding = verification_test_model(
            "admin/embedding@fixture",
            MODEL_CLASS_EMBEDDING,
            "llama.cpp",
            CanaryRef {
                set_id: "canary-embedding-v1".to_owned(),
                match_min: 0.98,
                verification_method: VERIFICATION_EMBEDDING_COSINE.to_owned(),
                verification_tolerance_bps: Some(25),
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([("fixed-embedding".to_owned(), vec![0.1, 0.2, 0.3])]),
                )]),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        let mut errors = Vec::new();
        validate_model(&embedding, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut attested_embedding = embedding.clone();
        attested_embedding.canary.verification_method =
            VERIFICATION_ATTESTATION_OF_COMPUTE.to_owned();
        attested_embedding.canary.verification_tolerance_bps = None;
        attested_embedding.canary.embedding_vectors.clear();
        let mut errors = Vec::new();
        validate_model(&attested_embedding, &mut errors);
        assert!(errors.iter().any(|error| {
            error.contains(
                "launch model_class embedding requires output canary method embedding_cosine",
            )
        }));

        let stt = verification_test_model(
            "admin/stt@fixture",
            MODEL_CLASS_STT,
            "whisper.cpp",
            CanaryRef {
                set_id: "canary-stt-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TRANSCRIPT_MATCH.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([("fixed-audio".to_owned(), "hello mayhem".to_owned())]),
                )]),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        let mut errors = Vec::new();
        validate_model(&stt, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let audio = verification_test_model(
            "admin/music@fixture",
            MODEL_CLASS_MUSIC_GENERATION,
            "comfyui",
            CanaryRef {
                set_id: "canary-music-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_AUDIO_FINGERPRINT.to_owned(),
                verification_tolerance_bps: Some(1_000),
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([(
                        "fixed-audio".to_owned(),
                        format!("audiospec-v1:1000:{}", "01".repeat(256)),
                    )]),
                )]),
            },
        );
        let mut errors = Vec::new();
        validate_model(&audio, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut unknown_marker = audio.clone();
        unknown_marker
            .adapter
            .endpoint_families
            .iter_mut()
            .find(|contract| contract.family == mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .unwrap()
            .request_attribute_specs
            .get_mut("source_audio.data")
            .unwrap()
            .calibration_values = vec![Value::String("$UNKNOWN_AUDIO_FIXTURE".to_owned())];
        let mut errors = Vec::new();
        validate_model(&unknown_marker, &mut errors);
        assert!(errors
            .iter()
            .any(|error| error.contains("has invalid declared test value")));

        let mut attested_audio = audio.clone();
        attested_audio.canary.verification_method = VERIFICATION_ATTESTATION_OF_COMPUTE.to_owned();
        attested_audio.canary.audio_fingerprints.clear();
        let mut errors = Vec::new();
        validate_model(&attested_audio, &mut errors);
        assert!(errors.iter().any(|error| {
            error.contains(
                "launch model_class music-generation requires output canary method audio_fingerprint"
            )
        }));
    }

    #[test]
    fn adapter_descriptor_validates_registry_keys_and_modalities() {
        let mut model = verification_test_model(
            "admin/text@adapter",
            DEFAULT_MODEL_CLASS,
            "llama.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::from([("fixture".to_owned(), "a".repeat(64))]),
                token_prefixes: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([(
                        "fixed-text".to_owned(),
                        vec![1; MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS],
                    )]),
                )]),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.adapter.tool_call_strategy = "openai_tool_calls".to_owned();
        model.adapter.chat_template_id = "qwen3.5-instruct".to_owned();
        model.adapter.reasoning_passthrough = "preserve".to_owned();
        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut vision_input = model.clone();
        vision_input.caps.vision = true;
        vision_input.adapter.modality_set = vec!["text".to_owned(), "image".to_owned()];
        vision_input
            .adapter
            .endpoint_families
            .push(endpoint_contract_template(mayhem_proto::ENDPOINT_HF_MULTIMODAL_CHAT).unwrap());
        vision_input
            .modality_assessment
            .detected
            .push("image".to_owned());
        vision_input
            .modality_assessment
            .calibrated_fingerprints
            .get_mut("fixture")
            .unwrap()
            .insert("image".to_owned(), "e".repeat(64));
        vision_input
            .modality_assessment
            .resource_profiles
            .entry("fixture".to_owned())
            .or_default()
            .insert(
                "image".to_owned(),
                verification_test_resource_profile("image"),
            );
        vision_input.price_ref_au.rate_map = vec![
            CatalogRateMapEntry {
                unit: "input_token".to_owned(),
                per_unit_au: 1,
                granularity: 1000,
            },
            CatalogRateMapEntry {
                unit: "output_token".to_owned(),
                per_unit_au: 1,
                granularity: 1000,
            },
        ];
        let mut errors = Vec::new();
        validate_model(&vision_input, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut double_billed_vision = vision_input.clone();
        double_billed_vision
            .price_ref_au
            .rate_map
            .push(CatalogRateMapEntry {
                unit: "image".to_owned(),
                per_unit_au: 1,
                granularity: 1,
            });
        let mut errors = Vec::new();
        validate_model(&double_billed_vision, &mut errors);
        assert!(errors.iter().any(
            |error| error.contains("multimodal LLM media input is billed through input_token")
        ));

        let mut invalid = model.clone();
        invalid.adapter.endpoint_families[0].family = "provider_native".to_owned();
        invalid.adapter.chat_template_id = String::new();
        invalid.adapter.tool_call_strategy = "provider_custom".to_owned();
        invalid.adapter.reasoning_passthrough = "leak".to_owned();
        invalid.adapter.modality_set = vec![
            "text".to_owned(),
            "text".to_owned(),
            "image".to_owned(),
            "smell".to_owned(),
        ];
        let mut errors = Vec::new();
        validate_model(&invalid, &mut errors);
        assert!(errors
            .iter()
            .any(|error| error.contains("endpoint_families")));
        assert!(errors
            .iter()
            .any(|error| error.contains("chat_template_id")));
        assert!(errors
            .iter()
            .any(|error| error.contains("tool_call_strategy")));
        assert!(errors
            .iter()
            .any(|error| error.contains("reasoning_passthrough")));
        assert!(errors
            .iter()
            .any(|error| error.contains("adapter.modality_set duplicates text")));
        assert!(errors.iter().any(|error| {
            error.contains("adapter.modality_set entry image is not allowed by model caps")
        }));
        assert!(errors
            .iter()
            .any(|error| error.contains("adapter.modality_set entry is unsupported: smell")));
    }

    #[test]
    fn token_canary_video_shape_counts_as_video_input() {
        let model = verification_test_model(
            "admin/video-chat@fixture",
            DEFAULT_MODEL_CLASS,
            "vllm",
            CanaryRef {
                set_id: "canary-video-chat-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        let prompt = CanarySetPrompt {
            id: "video".to_owned(),
            messages: vec![serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {"type": "video", "video": {"data": "AAAA", "num_frames": 1}}
                ]
            })],
            prompt: None,
            input: None,
            audio_b64: None,
            temperature: Some(0.0),
            top_p: None,
            top_k: None,
            min_p: None,
            repeat_penalty: None,
            frequency_penalty: None,
            presence_penalty: None,
            seed: None,
            max_tokens: Some(8),
        };

        assert_eq!(
            canary_prompt_modalities(&model, &prompt),
            BTreeSet::from(["text", "video"])
        );
    }

    #[test]
    fn embedding_adapter_and_input_only_pricing_validate() {
        let mut model = verification_test_model(
            "admin/embed@q8",
            "embedding",
            "llama.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_EMBEDDING_COSINE.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([("fixed-embedding".to_owned(), vec![0.1, 0.2, 0.3])]),
                )]),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.price_ref_au.out_per_1k = 0;

        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut wrong_shape = model.clone();
        wrong_shape.adapter.endpoint_families = vec![default_chat_endpoint_contract()];
        let mut errors = Vec::new();
        validate_model(&wrong_shape, &mut errors);
        assert!(errors.iter().any(|error| error.contains(
            "adapter.endpoint_families missing required compatible family openai_embeddings"
        )));
        assert!(errors.iter().any(|error| error.contains(
            "endpoint family openai_chat_completions is not compatible with model_class embedding"
        )));

        let mut text_with_zero_output = verification_test_model(
            "admin/text@zero-output",
            DEFAULT_MODEL_CLASS,
            "llama.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::from([("fixture".to_owned(), "a".repeat(64))]),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        text_with_zero_output.price_ref_au.out_per_1k = 0;
        let mut errors = Vec::new();
        validate_model(&text_with_zero_output, &mut errors);
        assert!(errors.iter().any(|error| error
            .contains("price_ref_au.out_per_1k must be positive for non-embedding models")));
    }

    #[test]
    fn image_generation_adapter_shape_validates() {
        let mut model = verification_test_model(
            "admin/sd-turbo@small",
            "image-generation",
            "stable-diffusion.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_SEED_PERCEPTUAL_HASH.to_owned(),
                verification_tolerance_bps: Some(128),
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([("fixed-image".to_owned(), "a".repeat(16))]),
                )]),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.adapter.tool_call_strategy = "none".to_owned();
        model.adapter.modality_set = vec!["image".to_owned()];
        model.price_ref_au.in_per_1k = 0;
        model.price_ref_au.out_per_1k = 0;
        model.price_ref_au.rate_map = vec![
            CatalogRateMapEntry {
                unit: USAGE_IMAGE.to_owned(),
                per_unit_au: 500,
                granularity: 1,
            },
            CatalogRateMapEntry {
                unit: USAGE_STEP.to_owned(),
                per_unit_au: 2,
                granularity: 1,
            },
        ];

        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut offset_model = model.clone();
        offset_model
            .artifacts
            .get_mut("fixture")
            .unwrap()
            .stable_diffusion_cpp = Some(mayhem_engine::StableDiffusionCppConfig {
            separate_diffusion_model: false,
            guidance_scale_offset: 1,
            steps_offset: -1,
        });
        let mut errors = Vec::new();
        validate_model(&offset_model, &mut errors);
        assert!(errors
            .iter()
            .any(|error| { error.contains("steps range 1..=150 with backend offset -1") }));
        assert!(errors
            .iter()
            .any(|error| { error.contains("cfg_scale range 0..=50 with backend offset 1") }));
        for contract in &mut offset_model.adapter.endpoint_families {
            let (steps, guidance) = match contract.family.as_str() {
                mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS => ("steps", "cfg_scale"),
                mayhem_proto::ENDPOINT_HF_TEXT_TO_IMAGE => (
                    "parameters.num_inference_steps",
                    "parameters.guidance_scale",
                ),
                _ => continue,
            };
            contract
                .request_attribute_specs
                .get_mut(steps)
                .unwrap()
                .minimum = Some(2.0);
            contract
                .request_attribute_specs
                .get_mut(guidance)
                .unwrap()
                .maximum = Some(49.0);
        }
        let mut errors = Vec::new();
        validate_model(&offset_model, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut missing_step_price = model.clone();
        missing_step_price
            .price_ref_au
            .rate_map
            .retain(|entry| entry.unit != USAGE_STEP);
        let mut errors = Vec::new();
        validate_model(&missing_step_price, &mut errors);
        assert!(errors
            .iter()
            .any(|error| { error.contains("price_ref_au.rate_map missing required unit step") }));

        let mut wrong_shape = model.clone();
        wrong_shape.adapter.endpoint_families = vec![default_chat_endpoint_contract()];
        let mut errors = Vec::new();
        validate_model(&wrong_shape, &mut errors);
        assert!(errors.iter().any(|error| error.contains(
            "adapter.endpoint_families missing required compatible family openai_image_generations"
        )));
        assert!(errors.iter().any(|error| error.contains(
            "endpoint family openai_chat_completions is not compatible with model_class image-generation"
        )));

        let mut text_with_image_shape = verification_test_model(
            "admin/text@bad-image-shape",
            DEFAULT_MODEL_CLASS,
            "llama.cpp",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::from([("fixture".to_owned(), "a".repeat(64))]),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        text_with_image_shape.adapter.endpoint_families =
            vec![
                endpoint_contract_template(mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS)
                    .unwrap(),
            ];
        let mut errors = Vec::new();
        validate_model(&text_with_image_shape, &mut errors);
        assert!(errors.iter().any(|error| error.contains(
            "endpoint family openai_image_generations is not compatible with model_class text-generation"
        )));
    }

    #[test]
    fn non_llm_roster_rate_maps_cover_every_required_machine_unit() {
        let mut model = verification_test_model(
            "admin/non-llm-rate-map-fixture",
            DEFAULT_MODEL_CLASS,
            "fixture",
            CanaryRef {
                set_id: "canary-launch-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        let rate = |unit: &str, per_unit_au, granularity| CatalogRateMapEntry {
            unit: unit.to_owned(),
            per_unit_au,
            granularity,
        };
        let cases = [
            (
                MODEL_CLASS_IMAGE_GENERATION,
                vec![
                    rate(USAGE_IMAGE, 1, 1),
                    rate(USAGE_STEP, 2_499_999_999_999_999, 36),
                ],
            ),
            (
                MODEL_CLASS_STT,
                vec![rate(USAGE_AUDIO_SECOND, 1_000_000_000_000_000, 60)],
            ),
            (
                MODEL_CLASS_TTS,
                vec![
                    rate(USAGE_INPUT_CHARACTER, 1, 1),
                    rate(USAGE_AUDIO_SECOND, 18_000_000_000_000, 1),
                ],
            ),
            (
                MODEL_CLASS_TTS,
                vec![
                    rate(USAGE_INPUT_CHARACTER, 1, 1),
                    rate(USAGE_AUDIO_SECOND, 60_000_000_000_000, 1),
                ],
            ),
            (
                MODEL_CLASS_MUSIC_GENERATION,
                vec![
                    rate(USAGE_INPUT_CHARACTER, 1, 1),
                    rate(USAGE_AUDIO_SECOND, 100_000_000_000_000, 1),
                ],
            ),
            (
                MODEL_CLASS_EMBEDDING,
                vec![rate(USAGE_INPUT_TOKEN, 4_000_000_000_000_000, 1_000_000)],
            ),
            (
                MODEL_CLASS_EMBEDDING,
                vec![rate(USAGE_INPUT_TOKEN, 10_000_000_000_000_000, 1_000_000)],
            ),
        ];

        for (model_class, rate_map) in cases {
            model.model_class = model_class.to_owned();
            model.price_ref_au.rate_map = rate_map;
            let mut errors = Vec::new();
            validate_required_modality_price_units(&model, &mut errors);
            validate_price_rate_map(&model, &mut errors);
            assert!(errors.is_empty(), "{model_class}: {errors:#?}");
        }
    }

    #[test]
    fn video_generation_requires_video_units_in_every_price_validator() {
        let mut model = verification_test_model(
            "admin/video@small",
            MODEL_CLASS_VIDEO_GENERATION,
            "diffusers",
            CanaryRef {
                set_id: "canary-video-v1".to_owned(),
                match_min: 0.9,
                verification_method: VERIFICATION_SEED_PERCEPTUAL_HASH.to_owned(),
                verification_tolerance_bps: Some(128),
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([("fixed-video".to_owned(), "a".repeat(16))]),
                )]),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.price_ref_au.in_per_1k = 0;
        model.price_ref_au.out_per_1k = 0;
        model.price_ref_au.rate_map = vec![
            CatalogRateMapEntry {
                unit: USAGE_VIDEO_SECOND.to_owned(),
                per_unit_au: 500,
                granularity: 1,
            },
            CatalogRateMapEntry {
                unit: USAGE_FRAME.to_owned(),
                per_unit_au: 2,
                granularity: 1,
            },
        ];

        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        model.price_ref_au.rate_map = vec![
            CatalogRateMapEntry {
                unit: USAGE_INPUT_TOKEN.to_owned(),
                per_unit_au: 500,
                granularity: 1,
            },
            CatalogRateMapEntry {
                unit: USAGE_OUTPUT_TOKEN.to_owned(),
                per_unit_au: 2,
                granularity: 1,
            },
        ];
        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(errors
            .iter()
            .any(|error| error.contains("missing required unit video_second")));
        assert!(errors
            .iter()
            .any(|error| error.contains("missing required unit frame")));
    }

    fn verification_test_resource_profile(modality: &str) -> CatalogModalityResourceProfile {
        let measured_working_set_bytes = match modality {
            "image" => 3 * 4,
            "audio" => 48_000 * 4,
            "video" => 224 * 224 * 3 * 4,
            _ => 1,
        }
        .max(10);
        let calibration_baseline_memory_bytes = 1_000_000;
        let calibration_peak_memory_bytes =
            calibration_baseline_memory_bytes + measured_working_set_bytes;
        CatalogModalityResourceProfile {
            unit: match modality {
                "image" => "pixel",
                "audio" => "second",
                "video" => "frame",
                "embedding" => "input_token",
                _ => "",
            }
            .to_owned(),
            measurement_source: "test-fixture".to_owned(),
            max_item_bytes: 1,
            max_item_units: 1,
            measured_item_bytes: 1,
            measured_item_units: 1,
            measured_working_set_bytes,
            calibration_baseline_memory_bytes,
            calibration_peak_memory_bytes,
            calibration_f13_budget_bytes: calibration_peak_memory_bytes + 1_000_000,
            default_max_inflight_items: 1,
            default_max_items_per_request: 1,
        }
    }

    fn verification_test_model(
        model_id: &str,
        model_class: &str,
        engine: &str,
        canary: CanaryRef,
    ) -> CatalogModel {
        let output_modality = match model_class {
            MODEL_CLASS_EMBEDDING => "embedding",
            MODEL_CLASS_IMAGE_GENERATION => "image",
            MODEL_CLASS_VIDEO_GENERATION => "video",
            MODEL_CLASS_TTS | MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION => {
                "audio"
            }
            _ => "text",
        };
        let modality_set = vec![output_modality.to_owned()];
        let mut endpoint_families = required_endpoint_family_names(model_class, &modality_set)
            .into_iter()
            .map(|family| endpoint_contract_template(family).unwrap())
            .collect::<Vec<_>>();
        for contract in &mut endpoint_families {
            match contract.family.as_str() {
                mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS => {
                    for (path, value) in [
                        ("n", serde_json::json!(1)),
                        ("width", serde_json::json!(1024)),
                        ("height", serde_json::json!(1024)),
                        ("steps", serde_json::json!(9)),
                        ("cfg_scale", serde_json::json!(0.0)),
                        ("response_format", serde_json::json!("b64_json")),
                    ] {
                        contract
                            .request_attribute_specs
                            .get_mut(path)
                            .unwrap()
                            .default = Some(value);
                    }
                }
                mayhem_proto::ENDPOINT_HF_TEXT_TO_IMAGE => {
                    for (path, value) in [
                        ("parameters.width", serde_json::json!(1024)),
                        ("parameters.height", serde_json::json!(1024)),
                        ("parameters.num_inference_steps", serde_json::json!(9)),
                        ("parameters.guidance_scale", serde_json::json!(0.0)),
                    ] {
                        contract
                            .request_attribute_specs
                            .get_mut(path)
                            .unwrap()
                            .default = Some(value);
                    }
                }
                _ => {}
            }
        }
        CatalogModel {
            model_id: model_id.to_owned(),
            model_class: model_class.to_owned(),
            family: "fixture".to_owned(),
            params_b: 1.0,
            tier: "launch".to_owned(),
            min_app_version: None,
            provenance: Provenance {
                source: SourceRef {
                    kind: "huggingface".to_owned(),
                    repo: "admin/source".to_owned(),
                    revision: "1".repeat(40),
                    publisher_key: None,
                },
                conversion: vec![ConversionRef {
                    tool: "fixture-convert".to_owned(),
                    method: "fixture".to_owned(),
                    input_sha256: "2".repeat(64),
                    output_sha256: "3".repeat(64),
                }],
                license: "apache-2.0".to_owned(),
                license_sha256: "4".repeat(64),
            },
            artifacts: BTreeMap::from([(
                "fixture".to_owned(),
                CatalogArtifact {
                    engine: engine.to_owned(),
                    stable_diffusion_cpp: (engine == "stable-diffusion.cpp")
                        .then_some(mayhem_engine::StableDiffusionCppConfig::default()),
                    source: SourceRef {
                        kind: "huggingface".to_owned(),
                        repo: "admin/model".to_owned(),
                        revision: "5".repeat(40),
                        publisher_key: None,
                    },
                    upstream_source: None,
                    path: "model.safetensors".to_owned(),
                    artifact_root: "6".repeat(64),
                    artifact_root_kind: "blake3_merkle_v1".to_owned(),
                    weights_bytes: 1,
                    source_sha256: Some("7".repeat(64)),
                    tokenizer_sha256: None,
                    chat_template_sha256: None,
                    min_compute_cap: None,
                    download_check: false,
                    notes: None,
                    sidecars: BTreeMap::new(),
                },
            )]),
            caps: CatalogCaps {
                tools: model_class == DEFAULT_MODEL_CLASS,
                json: model_class == DEFAULT_MODEL_CLASS,
                ctx_max: 1024,
                vision: false,
                image: model_class == MODEL_CLASS_IMAGE_GENERATION,
                video: model_class == MODEL_CLASS_VIDEO_GENERATION,
                audio: matches!(
                    model_class,
                    MODEL_CLASS_TTS | MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION
                ),
                output_modality: Some(output_modality.to_owned()),
                output_modalities: vec![output_modality.to_owned()],
            },
            requirements: CatalogRequirements {
                min_ram_gb: 1,
                min_vram_gb_full_offload: 0,
                cpu_flags: Vec::new(),
                backends: vec![engine.to_owned()],
            },
            adapter: CatalogAdapter {
                endpoint_families,
                modality_set,
                tool_call_strategy: if model_class == DEFAULT_MODEL_CLASS {
                    "mayhem_json".to_owned()
                } else {
                    "none".to_owned()
                },
                ..CatalogAdapter::default()
            },
            modality_assessment: CatalogModalityAssessment {
                detected: vec![output_modality.to_owned()],
                evidence: vec!["test fixture".to_owned()],
                calibrated_fingerprints: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([(output_modality.to_owned(), "f".repeat(64))]),
                )]),
                resource_profiles: if output_modality == "text" {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([(
                        "fixture".to_owned(),
                        BTreeMap::from([(
                            output_modality.to_owned(),
                            verification_test_resource_profile(output_modality),
                        )]),
                    )])
                },
            },
            speciality_assessment: CatalogSpecialityAssessment {
                evidence: vec!["test fixture researched: no model specialities".to_owned()],
                ..CatalogSpecialityAssessment::default()
            },
            sampling: CatalogSamplingProfile::default(),
            canary,
            price_ref_au: PriceRef {
                denom: "au_usd".to_owned(),
                in_per_1k: 1,
                out_per_1k: 1,
                rate_map: Vec::new(),
            },
        }
    }

    fn hex_string(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}
