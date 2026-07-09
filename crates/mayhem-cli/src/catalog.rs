use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use mayhem_proto::{
    default_model_class, MoneyAu, DEFAULT_MODEL_CLASS, USAGE_AUDIO_SECOND, USAGE_IMAGE,
    USAGE_INPUT_CHARACTER, USAGE_INPUT_TOKEN, USAGE_OUTPUT_TOKEN, USAGE_STEP,
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
    #[serde(default)]
    pub(crate) adapter: CatalogAdapter,
    pub(crate) canary: CanaryRef,
    pub(crate) price_ref_au: PriceRef,
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
    pub(crate) source: SourceRef,
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct CatalogAdapter {
    #[serde(default = "default_request_shape_family")]
    pub(crate) request_shape_family: String,
    #[serde(default = "default_chat_template_id")]
    pub(crate) chat_template_id: String,
    #[serde(default = "default_tool_call_strategy")]
    pub(crate) tool_call_strategy: String,
    #[serde(default = "default_reasoning_passthrough")]
    pub(crate) reasoning_passthrough: String,
    #[serde(default = "default_modality_set")]
    pub(crate) modality_set: Vec<String>,
    #[serde(default = "default_response_normalization")]
    pub(crate) response_normalization: String,
}

impl Default for CatalogAdapter {
    fn default() -> Self {
        Self {
            request_shape_family: default_request_shape_family(),
            chat_template_id: default_chat_template_id(),
            tool_call_strategy: default_tool_call_strategy(),
            reasoning_passthrough: default_reasoning_passthrough(),
            modality_set: default_modality_set(),
            response_normalization: default_response_normalization(),
        }
    }
}

fn default_request_shape_family() -> String {
    "openai_chat".to_owned()
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

fn default_response_normalization() -> String {
    "openai_chat".to_owned()
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
    prompts: Vec<Value>,
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
    if dev_model_count < 2 {
        errors.push("catalog must contain at least two dev entries".to_owned());
    }
    if launch_model_count < 1 {
        errors.push("catalog must contain at least one launch entry".to_owned());
    }

    for set_id in &canary_sets {
        validate_canary_set(&options.canaries_dir, set_id, &mut errors);
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
    key.verify(catalog_bytes, &sig)
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
    validate_model_adapter(model, errors);
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

fn validate_price_ref(model: &CatalogModel, errors: &mut Vec<String>) {
    if !model.price_ref_au.rate_map.is_empty() {
        validate_price_rate_map(model, errors);
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
    if model.canary.verification_tolerance_bps.is_some() {
        errors.push(format!(
            "{} audio_fingerprint canary must not set verification_tolerance_bps",
            model.model_id
        ));
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
            if !is_hex_len(fingerprint, 64) {
                errors.push(format!(
                    "{model_id} canary audio_fingerprints for {artifact} prompt {prompt_id} must be 32-byte hex"
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
    if !matches!(
        adapter.request_shape_family.as_str(),
        "openai_chat"
            | "openai_embeddings"
            | "openai_images"
            | "openai_audio_speech"
            | "openai_audio_transcriptions"
            | "openai_audio_generations"
    ) {
        errors.push(format!(
            "{} adapter.request_shape_family is unsupported: {}",
            model.model_id, adapter.request_shape_family
        ));
    }
    if model.model_class == MODEL_CLASS_EMBEDDING
        && adapter.request_shape_family != "openai_embeddings"
    {
        errors.push(format!(
            "{} embedding model must use adapter.request_shape_family openai_embeddings",
            model.model_id
        ));
    }
    if adapter.request_shape_family == "openai_embeddings"
        && model.model_class != MODEL_CLASS_EMBEDDING
    {
        errors.push(format!(
            "{} adapter.request_shape_family openai_embeddings is only allowed for model_class embedding",
            model.model_id
        ));
    }
    if model.model_class == MODEL_CLASS_IMAGE_GENERATION
        && adapter.request_shape_family != "openai_images"
    {
        errors.push(format!(
            "{} image-generation model must use adapter.request_shape_family openai_images",
            model.model_id
        ));
    }
    if adapter.request_shape_family == "openai_images"
        && model.model_class != MODEL_CLASS_IMAGE_GENERATION
    {
        errors.push(format!(
            "{} adapter.request_shape_family openai_images is only allowed for model_class image-generation",
            model.model_id
        ));
    }
    if model.model_class == MODEL_CLASS_TTS && adapter.request_shape_family != "openai_audio_speech"
    {
        errors.push(format!(
            "{} tts model must use adapter.request_shape_family openai_audio_speech",
            model.model_id
        ));
    }
    if adapter.request_shape_family == "openai_audio_speech" && model.model_class != MODEL_CLASS_TTS
    {
        errors.push(format!(
            "{} adapter.request_shape_family openai_audio_speech is only allowed for model_class tts",
            model.model_id
        ));
    }
    if model.model_class == MODEL_CLASS_STT
        && adapter.request_shape_family != "openai_audio_transcriptions"
    {
        errors.push(format!(
            "{} stt model must use adapter.request_shape_family openai_audio_transcriptions",
            model.model_id
        ));
    }
    if adapter.request_shape_family == "openai_audio_transcriptions"
        && model.model_class != MODEL_CLASS_STT
    {
        errors.push(format!(
            "{} adapter.request_shape_family openai_audio_transcriptions is only allowed for model_class stt",
            model.model_id
        ));
    }
    if matches!(
        model.model_class.as_str(),
        MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION
    ) && adapter.request_shape_family != "openai_audio_generations"
    {
        errors.push(format!(
            "{} {} model must use adapter.request_shape_family openai_audio_generations",
            model.model_id, model.model_class
        ));
    }
    if adapter.request_shape_family == "openai_audio_generations"
        && !matches!(
            model.model_class.as_str(),
            MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION
        )
    {
        errors.push(format!(
            "{} adapter.request_shape_family openai_audio_generations is only allowed for model_class audio-generation or music-generation",
            model.model_id
        ));
    }
    if !matches!(
        adapter.chat_template_id.as_str(),
        "generic_chatml" | "llama3-instruct" | "qwen2.5-instruct" | "smolvlm2-instruct"
    ) {
        errors.push(format!(
            "{} adapter.chat_template_id is unsupported: {}",
            model.model_id, adapter.chat_template_id
        ));
    }
    if !matches!(
        adapter.tool_call_strategy.as_str(),
        "none" | "mayhem_json" | "openai_tool_calls"
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
    let mut caps_modalities = BTreeSet::new();
    if let Some(modality) = model.caps.output_modality.as_deref() {
        caps_modalities.insert(modality);
    }
    for modality in &model.caps.output_modalities {
        caps_modalities.insert(modality.as_str());
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
    if !matches!(
        adapter.response_normalization.as_str(),
        "openai_chat"
            | "openai_embeddings"
            | "openai_images"
            | "openai_audio_speech"
            | "openai_audio_transcriptions"
            | "openai_audio_generations"
    ) {
        errors.push(format!(
            "{} adapter.response_normalization is unsupported: {}",
            model.model_id, adapter.response_normalization
        ));
    }
    if model.model_class == MODEL_CLASS_EMBEDDING
        && adapter.response_normalization != "openai_embeddings"
    {
        errors.push(format!(
            "{} embedding model must use adapter.response_normalization openai_embeddings",
            model.model_id
        ));
    }
    if adapter.response_normalization == "openai_embeddings"
        && model.model_class != MODEL_CLASS_EMBEDDING
    {
        errors.push(format!(
            "{} adapter.response_normalization openai_embeddings is only allowed for model_class embedding",
            model.model_id
        ));
    }
    if model.model_class == MODEL_CLASS_IMAGE_GENERATION
        && adapter.response_normalization != "openai_images"
    {
        errors.push(format!(
            "{} image-generation model must use adapter.response_normalization openai_images",
            model.model_id
        ));
    }
    if adapter.response_normalization == "openai_images"
        && model.model_class != MODEL_CLASS_IMAGE_GENERATION
    {
        errors.push(format!(
            "{} adapter.response_normalization openai_images is only allowed for model_class image-generation",
            model.model_id
        ));
    }
    if model.model_class == MODEL_CLASS_TTS
        && adapter.response_normalization != "openai_audio_speech"
    {
        errors.push(format!(
            "{} tts model must use adapter.response_normalization openai_audio_speech",
            model.model_id
        ));
    }
    if adapter.response_normalization == "openai_audio_speech"
        && model.model_class != MODEL_CLASS_TTS
    {
        errors.push(format!(
            "{} adapter.response_normalization openai_audio_speech is only allowed for model_class tts",
            model.model_id
        ));
    }
    if model.model_class == MODEL_CLASS_STT
        && adapter.response_normalization != "openai_audio_transcriptions"
    {
        errors.push(format!(
            "{} stt model must use adapter.response_normalization openai_audio_transcriptions",
            model.model_id
        ));
    }
    if adapter.response_normalization == "openai_audio_transcriptions"
        && model.model_class != MODEL_CLASS_STT
    {
        errors.push(format!(
            "{} adapter.response_normalization openai_audio_transcriptions is only allowed for model_class stt",
            model.model_id
        ));
    }
    if matches!(
        model.model_class.as_str(),
        MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION
    ) && adapter.response_normalization != "openai_audio_generations"
    {
        errors.push(format!(
            "{} {} model must use adapter.response_normalization openai_audio_generations",
            model.model_id, model.model_class
        ));
    }
    if adapter.response_normalization == "openai_audio_generations"
        && !matches!(
            model.model_class.as_str(),
            MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION
        )
    {
        errors.push(format!(
            "{} adapter.response_normalization openai_audio_generations is only allowed for model_class audio-generation or music-generation",
            model.model_id
        ));
    }
}

fn valid_adapter_modality(modality: &str) -> bool {
    matches!(modality, "text" | "embedding" | "image" | "video" | "audio")
}

fn adapter_modality_allowed(model: &CatalogModel, modality: &str) -> bool {
    let caps_modalities = model_output_modalities(model);
    match modality {
        "image" => model.caps.vision || caps_modalities.contains("image"),
        "audio" => model.caps.audio || caps_modalities.contains("audio"),
        other => caps_modalities.is_empty() || caps_modalities.contains(other),
    }
}

fn model_output_modalities(model: &CatalogModel) -> BTreeSet<&str> {
    let mut caps_modalities = BTreeSet::new();
    if let Some(modality) = model.caps.output_modality.as_deref() {
        caps_modalities.insert(modality);
    }
    for modality in &model.caps.output_modalities {
        caps_modalities.insert(modality.as_str());
    }
    caps_modalities
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
    for (sidecar_name, sidecar) in &artifact.sidecars {
        validate_artifact_sidecar(model_id, name, tier, sidecar_name, sidecar, errors);
    }
    let _ = (artifact.download_check, &artifact.notes);
}

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
        }
        Err(err) => errors.push(err.to_string()),
    }
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
    fn hex_parser_rejects_bad_input() {
        assert_eq!(hex_to_vec("00ff").unwrap(), vec![0, 255]);
        assert!(hex_to_vec("0").is_err());
        assert!(hex_to_vec("zz").is_err());
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
                    BTreeMap::from([("fixed-text".to_owned(), vec![1, 2, 3])]),
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
            source: SourceRef {
                kind: "huggingface".to_owned(),
                repo: "admin/model".to_owned(),
                revision: "2".repeat(40),
                publisher_key: None,
            },
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
                    BTreeMap::from([("fixed-text".to_owned(), vec![1, 2, 3])]),
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
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::from([(
                    "fixture".to_owned(),
                    BTreeMap::from([("fixed-audio".to_owned(), "c".repeat(64))]),
                )]),
            },
        );
        let mut errors = Vec::new();
        validate_model(&audio, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

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
                    BTreeMap::from([("fixed-text".to_owned(), vec![1, 2, 3])]),
                )]),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.adapter.tool_call_strategy = "openai_tool_calls".to_owned();
        model.adapter.chat_template_id = "qwen2.5-instruct".to_owned();
        model.adapter.reasoning_passthrough = "preserve".to_owned();
        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut vision_input = model.clone();
        vision_input.caps.vision = true;
        vision_input.adapter.modality_set = vec!["text".to_owned(), "image".to_owned()];
        let mut errors = Vec::new();
        validate_model(&vision_input, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut invalid = model.clone();
        invalid.adapter.request_shape_family = "provider_native".to_owned();
        invalid.adapter.chat_template_id = String::new();
        invalid.adapter.tool_call_strategy = "provider_custom".to_owned();
        invalid.adapter.reasoning_passthrough = "leak".to_owned();
        invalid.adapter.modality_set = vec![
            "text".to_owned(),
            "text".to_owned(),
            "image".to_owned(),
            "smell".to_owned(),
        ];
        invalid.adapter.response_normalization = "raw".to_owned();
        let mut errors = Vec::new();
        validate_model(&invalid, &mut errors);
        assert!(errors
            .iter()
            .any(|error| error.contains("request_shape_family")));
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
        assert!(errors
            .iter()
            .any(|error| error.contains("response_normalization")));
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
                verification_method: VERIFICATION_ATTESTATION_OF_COMPUTE.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.adapter.request_shape_family = "openai_embeddings".to_owned();
        model.adapter.response_normalization = "openai_embeddings".to_owned();
        model.price_ref_au.out_per_1k = 0;

        let mut errors = Vec::new();
        validate_model(&model, &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let mut wrong_shape = model.clone();
        wrong_shape.adapter.request_shape_family = "openai_chat".to_owned();
        wrong_shape.adapter.response_normalization = "openai_chat".to_owned();
        let mut errors = Vec::new();
        validate_model(&wrong_shape, &mut errors);
        assert!(errors.iter().any(|error| error
            .contains("embedding model must use adapter.request_shape_family openai_embeddings")));
        assert!(errors.iter().any(|error| error.contains(
            "embedding model must use adapter.response_normalization openai_embeddings"
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
                verification_method: VERIFICATION_ATTESTATION_OF_COMPUTE.to_owned(),
                verification_tolerance_bps: None,
                fingerprints: BTreeMap::new(),
                token_prefixes: BTreeMap::new(),
                perceptual_hashes: BTreeMap::new(),
                embedding_vectors: BTreeMap::new(),
                transcripts: BTreeMap::new(),
                audio_fingerprints: BTreeMap::new(),
            },
        );
        model.adapter.request_shape_family = "openai_images".to_owned();
        model.adapter.response_normalization = "openai_images".to_owned();
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
        wrong_shape.adapter.request_shape_family = "openai_chat".to_owned();
        wrong_shape.adapter.response_normalization = "openai_chat".to_owned();
        let mut errors = Vec::new();
        validate_model(&wrong_shape, &mut errors);
        assert!(errors.iter().any(|error| error.contains(
            "image-generation model must use adapter.request_shape_family openai_images"
        )));
        assert!(errors.iter().any(|error| error.contains(
            "image-generation model must use adapter.response_normalization openai_images"
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
        text_with_image_shape.adapter.request_shape_family = "openai_images".to_owned();
        text_with_image_shape.adapter.response_normalization = "openai_images".to_owned();
        let mut errors = Vec::new();
        validate_model(&text_with_image_shape, &mut errors);
        assert!(errors.iter().any(|error| error.contains(
            "adapter.request_shape_family openai_images is only allowed for model_class image-generation"
        )));
        assert!(errors.iter().any(|error| error.contains(
            "adapter.response_normalization openai_images is only allowed for model_class image-generation"
        )));
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
        let request_shape_family = match model_class {
            MODEL_CLASS_EMBEDDING => "openai_embeddings",
            MODEL_CLASS_IMAGE_GENERATION => "openai_images",
            MODEL_CLASS_TTS => "openai_audio_speech",
            MODEL_CLASS_STT => "openai_audio_transcriptions",
            MODEL_CLASS_AUDIO_GENERATION | MODEL_CLASS_MUSIC_GENERATION => {
                "openai_audio_generations"
            }
            _ => "openai_chat",
        };
        let response_normalization = request_shape_family;
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
                    source: SourceRef {
                        kind: "huggingface".to_owned(),
                        repo: "admin/model".to_owned(),
                        revision: "5".repeat(40),
                        publisher_key: None,
                    },
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
                modality_set: vec![output_modality.to_owned()],
                request_shape_family: request_shape_family.to_owned(),
                response_normalization: response_normalization.to_owned(),
                tool_call_strategy: if model_class == DEFAULT_MODEL_CLASS {
                    "mayhem_json".to_owned()
                } else {
                    "none".to_owned()
                },
                ..CatalogAdapter::default()
            },
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
