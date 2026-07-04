#![cfg(feature = "llama-cpp")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mayhem_engine::{
    verify_artifact, EngineBackend, GenerateRequest, GrammarSpec, LlamaCppBackend, LoadConfig,
    ModelArtifact, ToolSpec,
};
use serde_json::json;

const RUN_ENV: &str = "MAYHEM_RUN_LLAMACPP_TESTS";
const HF_TOKEN_FILE_ENV: &str = "HF_TOKEN_FILE";
const DEFAULT_HF_TOKEN_FILE: &str = "/Applications/MAMP/htdocs/gpd/hf.txt";
const REPO: &str = "lmstudio-community/Qwen3.5-4B-GGUF";
const REVISION: &str = "f9f88ac3e234be915e23811a6d28ea287bdb927e";
const FILE_NAME: &str = "Qwen3.5-4B-Q4_K_M.gguf";

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn gguf_dev_model_smoke_generates_and_constrains_tool_call() -> TestResult {
    if env::var(RUN_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping llama.cpp conformance; set {RUN_ENV}=1 to run");
        return Ok(());
    }

    let model_path = ensure_dev_model()?;
    let mut backend = LlamaCppBackend::new()?;
    let mut config = LoadConfig::gguf(&model_path);
    config.ctx_size = 1024;
    config.batch_size = 256;
    config.ubatch_size = 256;
    config.threads = Some(4);
    let info = backend.load(config)?;
    assert_eq!(info.artifact.path, model_path);
    assert!(info.n_vocab > 0);

    let tokenization = backend.tokenize("Say ok.")?;
    assert!(!tokenization.is_empty());

    let mut chunks = Vec::new();
    let output = backend.generate(
        GenerateRequest::new("Reply with the word ok.").with_max_new_tokens(12),
        &mut |chunk| {
            chunks.push(chunk);
            Ok(())
        },
    )?;
    assert!(!chunks.is_empty(), "streaming sink received no tokens");
    assert!(!output.text.trim().is_empty(), "model returned empty text");
    assert!(output.usage.prompt_tokens > 0);
    assert!(output.usage.completion_tokens > 0);
    assert_eq!(
        output.usage.total_tokens,
        output.usage.prompt_tokens + output.usage.completion_tokens
    );

    let constrained = backend.generate(
        GenerateRequest::new("Return the lookup tool call.")
            .with_max_new_tokens(96)
            .with_grammar(GrammarSpec::ToolCall {
                tools: vec![ToolSpec::new("lookup", json!({"type": "object"}))],
            }),
        &mut |_chunk| Ok(()),
    )?;
    let parsed: serde_json::Value =
        serde_json::from_str(constrained.text.trim()).unwrap_or_else(|err| {
            panic!(
                "constrained tool-call output was not JSON: {err}; output={:?}",
                constrained.text
            )
        });
    assert_eq!(parsed["tool"], json!("lookup"));
    assert!(
        parsed["arguments"].is_object(),
        "tool arguments must be a JSON object: {parsed}"
    );

    Ok(())
}

fn ensure_dev_model() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let path = cache_path()?;
    let artifact = ModelArtifact::gguf(&path);
    if path.exists() {
        if verify_artifact(&artifact).is_ok() {
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }

    fs::create_dir_all(path.parent().expect("cache path has a parent"))?;
    let url = format!("https://huggingface.co/{REPO}/resolve/{REVISION}/{FILE_NAME}");
    let mut curl = Command::new("curl");
    curl.arg("--fail")
        .arg("--location")
        .arg("--retry")
        .arg("3")
        .arg("--retry-delay")
        .arg("2")
        .arg("--continue-at")
        .arg("-")
        .arg("--output")
        .arg(&path)
        .arg(url);

    if let Some(token) = hf_token() {
        curl.arg("--header")
            .arg(format!("Authorization: Bearer {token}"));
    }

    let status = curl.status()?;
    if !status.success() {
        return Err(format!("curl failed to download {FILE_NAME}: {status}").into());
    }
    verify_artifact(&artifact)?;
    Ok(path)
}

fn cache_path() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(Path::new(&home)
        .join(".mayhem")
        .join("cache")
        .join("huggingface")
        .join(REPO)
        .join(REVISION)
        .join(FILE_NAME))
}

fn hf_token() -> Option<String> {
    let path = env::var_os(HF_TOKEN_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_HF_TOKEN_FILE));
    let token = fs::read_to_string(path).ok()?;
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}
