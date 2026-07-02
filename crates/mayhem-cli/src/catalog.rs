use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct VerifyOptions {
    pub catalog_path: PathBuf,
    pub signature_path: PathBuf,
    pub keys_dir: PathBuf,
    pub canaries_dir: PathBuf,
    pub check_dev_downloads: bool,
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
    pub(crate) family: String,
    pub(crate) params_b: f64,
    pub(crate) tier: String,
    pub(crate) provenance: Provenance,
    pub(crate) artifacts: BTreeMap<String, CatalogArtifact>,
    pub(crate) caps: CatalogCaps,
    pub(crate) requirements: CatalogRequirements,
    pub(crate) canary: CanaryRef,
    pub(crate) price_ref_mu: PriceRef,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Provenance {
    pub(crate) source: SourceRef,
    #[serde(default)]
    pub(crate) conversion: Vec<ConversionRef>,
    pub(crate) license: String,
    pub(crate) license_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SourceRef {
    pub(crate) kind: String,
    pub(crate) repo: String,
    pub(crate) revision: String,
    #[serde(default)]
    pub(crate) publisher_key: Option<String>,
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
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogCaps {
    pub(crate) tools: bool,
    pub(crate) json: bool,
    pub(crate) ctx_max: u64,
    pub(crate) vision: bool,
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

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CanaryRef {
    pub(crate) set_id: String,
    pub(crate) match_min: f64,
    #[serde(default)]
    pub(crate) fingerprints: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PriceRef {
    pub(crate) denom: String,
    pub(crate) in_per_1k: u64,
    pub(crate) out_per_1k: u64,
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

    for set_id in &canary_sets {
        validate_canary_set(&options.canaries_dir, set_id, &mut errors);
    }

    let download_checks = if options.check_dev_downloads && errors.is_empty() {
        run_download_checks(&catalog, options.hf_token_file.as_deref())?
    } else {
        Vec::new()
    };

    Ok(CatalogVerifyReport {
        ok: errors.is_empty() && download_checks.iter().all(|check| check.ok),
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
    if catalog.models.len() < 7 {
        errors.push(
            "catalog must contain at least two dev entries and five launch entries".to_owned(),
        );
    }
}

fn validate_model(model: &CatalogModel, errors: &mut Vec<String>) {
    if model.model_id.trim().is_empty() {
        errors.push("model_id is required".to_owned());
    }
    if model.family.trim().is_empty() {
        errors.push(format!("{} has empty family", model.model_id));
    }
    if model.params_b <= 0.0 {
        errors.push(format!("{} params_b must be positive", model.model_id));
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
        validate_artifact(&model.model_id, name, artifact, errors);
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
    if model.price_ref_mu.denom != "mu_usd" {
        errors.push(format!(
            "{} price_ref_mu.denom must be mu_usd",
            model.model_id
        ));
    }
    if model.price_ref_mu.in_per_1k == 0 || model.price_ref_mu.out_per_1k == 0 {
        errors.push(format!(
            "{} price references must be positive",
            model.model_id
        ));
    }
}

fn validate_artifact(
    model_id: &str,
    name: &str,
    artifact: &CatalogArtifact,
    errors: &mut Vec<String>,
) {
    if !matches!(artifact.engine.as_str(), "llama.cpp" | "mlx" | "trt-llm") {
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
    }
    if !is_hex_len(&artifact.artifact_root, 64) {
        errors.push(format!(
            "{model_id}/{name} artifact_root must be 32-byte hex"
        ));
    }
    if artifact.artifact_root_kind.trim().is_empty() {
        errors.push(format!("{model_id}/{name} artifact_root_kind is required"));
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
    if artifact.engine == "trt-llm" && artifact.min_compute_cap.is_none() {
        errors.push(format!(
            "{model_id}/{name} trt-llm artifact needs min_compute_cap"
        ));
    }
    let _ = (artifact.download_check, &artifact.notes);
}

fn validate_source(model_id: &str, label: &str, source: &SourceRef, errors: &mut Vec<String>) {
    if source.kind != "huggingface" {
        errors.push(format!("{model_id} {label}.kind must be huggingface"));
    }
    if !source.repo.contains('/') {
        errors.push(format!("{model_id} {label}.repo must be namespace/name"));
    }
    if !is_hex_len(&source.revision, 40) {
        errors.push(format!(
            "{model_id} {label}.revision must be a 20-byte git commit hex"
        ));
    }
    let _ = &source.publisher_key;
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
    bail!("set HF_TOKEN or pass --hf-token-file for --check-dev-downloads")
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

fn curl_config_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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
