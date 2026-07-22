#![cfg(feature = "mlx")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "llama-cpp")]
use std::time::{Duration, Instant};

#[cfg(feature = "llama-cpp")]
use mayhem_engine::LlamaCppBackend;
use mayhem_engine::{
    verify_artifact, CancellationToken, EngineBackend, GenerateOutput, GenerateRequest,
    GrammarSpec, LoadConfig, MlxBackend, ModelArtifact, ToolSpec,
};
use serde_json::json;

const RUN_ENV: &str = "MAYHEM_RUN_MLX_TESTS";
const BENCH_ENV: &str = "MAYHEM_RUN_MLX_BENCH";
const PYTHON_ENV: &str = "MAYHEM_MLX_PYTHON";
const HF_TOKEN_FILE_ENV: &str = "HF_TOKEN_FILE";
const DEFAULT_HF_TOKEN_FILE: &str = ".mayhem-local/secrets/hf.txt";
const MLX_REPO: &str = "mlx-community/Qwen3.5-4B-MLX-4bit";
const MLX_REVISION: &str = "32f3e8ecf65426fc3306969496342d504bfa13f3";
const MLX_WEIGHTS: &str = "model.safetensors";
#[cfg(feature = "llama-cpp")]
const GGUF_REPO: &str = "lmstudio-community/Qwen3.5-4B-GGUF";
#[cfg(feature = "llama-cpp")]
const GGUF_REVISION: &str = "f9f88ac3e234be915e23811a6d28ea287bdb927e";
#[cfg(feature = "llama-cpp")]
const GGUF_FILE_NAME: &str = "Qwen3.5-4B-Q4_K_M.gguf";

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn mlx_dev_model_smoke_generates_constrains_and_canaries() -> TestResult {
    if env::var(RUN_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping MLX conformance; set {RUN_ENV}=1 to run");
        return Ok(());
    }

    let python = python_bin();
    ensure_mlx_python(&python)?;
    let model_dir = ensure_mlx_snapshot(&python)?;
    let model_path = model_dir.join(MLX_WEIGHTS);

    let mut backend = MlxBackend::new()?;
    let mut config = LoadConfig::mlx_safetensors(&model_path);
    config.ctx_size = 1024;
    config.batch_size = 256;
    config.ubatch_size = 256;
    let info = backend.load(config)?;
    assert_eq!(info.backend, "mlx");
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
        &CancellationToken::new(),
    )?;
    assert!(!chunks.is_empty(), "streaming sink received no tokens");
    assert!(!output.text.trim().is_empty(), "model returned empty text");
    assert_usage(&output);
    assert_stream_matches_output(&chunks, &output);

    let tool_call = backend.generate(
        GenerateRequest::new("Return the lookup tool call.")
            .with_max_new_tokens(96)
            .with_grammar(GrammarSpec::ToolCall {
                tools: vec![ToolSpec::new("lookup", json!({"type": "object"}))],
            }),
        &mut |_chunk| Ok(()),
        &CancellationToken::new(),
    )?;
    let tool_call: serde_json::Value = serde_json::from_str(tool_call.text.trim())?;
    assert_eq!(tool_call["tool"], json!("lookup"));
    assert!(tool_call["arguments"].is_object());

    let mut canary_chunks = Vec::new();
    let canary = backend.generate(
        GenerateRequest::new(
            "Return compact JSON only. What is 17 + 25? Use exactly the key answer.",
        )
        .with_max_new_tokens(32),
        &mut |chunk| {
            canary_chunks.push(chunk);
            Ok(())
        },
        &CancellationToken::new(),
    )?;
    assert_usage(&canary);
    assert!(!canary_chunks.is_empty(), "canary produced no token chunks");
    assert_stream_matches_output(&canary_chunks, &canary);
    println!(
        "MLX dev canary fingerprint: {}",
        token_fingerprint(canary_chunks.iter().map(|chunk| chunk.token_id))
    );

    Ok(())
}

#[cfg(feature = "llama-cpp")]
#[test]
fn mlx_dev_model_beats_llama_cpp_metal_baseline() -> TestResult {
    if env::var(BENCH_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping MLX benchmark; set {BENCH_ENV}=1 to run");
        return Ok(());
    }

    let python = python_bin();
    ensure_mlx_python(&python)?;
    let mlx_model_dir = ensure_mlx_snapshot(&python)?;
    let gguf_path = ensure_gguf_dev_model()?;

    let mut mlx = MlxBackend::new()?;
    let mut mlx_config = LoadConfig::mlx_safetensors(mlx_model_dir.join(MLX_WEIGHTS));
    mlx_config.ctx_size = 1024;
    mlx.load(mlx_config)?;

    let mut llama = LlamaCppBackend::new()?;
    let mut llama_config = LoadConfig::gguf(&gguf_path);
    llama_config.ctx_size = 1024;
    llama_config.batch_size = 256;
    llama_config.ubatch_size = 256;
    llama_config.threads = Some(4);
    llama_config.gpu_layers = Some(99);
    llama.load(llama_config)?;

    let prompt = "Write a numbered list of short words about audited inference receipts.";
    let _ = timed_generate(&mut mlx, "Warm up with three short words.", 8)?;
    let _ = timed_generate(&mut llama, "Warm up with three short words.", 8)?;

    let mlx_samples = throughput_samples(&mut mlx, prompt, 128, 3)?;
    let llama_samples = throughput_samples(&mut llama, prompt, 128, 3)?;
    let mlx_tps = median(&mlx_samples);
    let llama_tps = median(&llama_samples);
    println!(
        "MLX tok/s median: {mlx_tps:.2} samples={mlx_samples:?}; llama.cpp-Metal tok/s median: {llama_tps:.2} samples={llama_samples:?}"
    );
    assert!(
        mlx_tps > llama_tps,
        "MLX tok/s ({mlx_tps:.2}) did not beat llama.cpp-Metal ({llama_tps:.2})"
    );

    Ok(())
}

#[cfg(not(feature = "llama-cpp"))]
#[test]
fn mlx_benchmark_requires_llama_cpp_feature() {
    if env::var(BENCH_ENV).ok().as_deref() == Some("1") {
        panic!("MLX benchmark requires enabling the llama-cpp-metal feature");
    }
}

#[cfg(feature = "llama-cpp")]
fn timed_generate<B: EngineBackend>(
    backend: &mut B,
    prompt: &str,
    max_new_tokens: u32,
) -> Result<(GenerateOutput, Duration), mayhem_engine::EngineError> {
    let start = Instant::now();
    let output = backend.generate(
        GenerateRequest::new(prompt).with_max_new_tokens(max_new_tokens),
        &mut |_chunk| Ok(()),
        &CancellationToken::new(),
    )?;
    Ok((output, start.elapsed()))
}

#[cfg(feature = "llama-cpp")]
fn throughput_samples<B: EngineBackend>(
    backend: &mut B,
    prompt: &str,
    max_new_tokens: u32,
    samples: usize,
) -> Result<Vec<f64>, mayhem_engine::EngineError> {
    let mut results = Vec::with_capacity(samples);
    for _ in 0..samples {
        let (output, elapsed) = timed_generate(backend, prompt, max_new_tokens)?;
        results.push(tokens_per_second(&output, elapsed));
    }
    Ok(results)
}

#[cfg(feature = "llama-cpp")]
fn tokens_per_second(output: &GenerateOutput, elapsed: Duration) -> f64 {
    output.usage.completion_tokens as f64 / elapsed.as_secs_f64().max(0.001)
}

#[cfg(feature = "llama-cpp")]
fn median(values: &[f64]) -> f64 {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn assert_usage(output: &GenerateOutput) {
    assert!(output.usage.prompt_tokens > 0);
    assert!(output.usage.completion_tokens > 0);
    assert_eq!(
        output.usage.total_tokens,
        output.usage.prompt_tokens + output.usage.completion_tokens
    );
}

fn assert_stream_matches_output(chunks: &[mayhem_engine::TokenChunk], output: &GenerateOutput) {
    assert_eq!(
        chunks.len(),
        output.usage.completion_tokens as usize,
        "streamed token ids must exactly match billed completion tokens"
    );
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<String>(),
        output.text,
        "streamed text must exactly match the final generated output"
    );
}

fn token_fingerprint(tokens: impl IntoIterator<Item = i32>) -> String {
    let mut hasher = blake3::Hasher::new();
    for token in tokens {
        hasher.update(&token.to_be_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

#[test]
fn token_fingerprint_uses_auditor_catalog_format() {
    assert_eq!(
        token_fingerprint([]),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    assert_eq!(
        token_fingerprint([1, 2, 3]),
        "04a03410338d287acb82ba338ec1aea060eac0650f256eddc814f743c731cf33"
    );
    assert_eq!(
        token_fingerprint([-1]),
        "650e93bacca01942a5a787f2f3ec4ce560998eb7c250733601a880d7f0c11178"
    );
}

fn ensure_mlx_python(python: &Path) -> TestResult {
    let status = Command::new(python)
        .arg("-c")
        .arg("import mlx, mlx_lm, huggingface_hub")
        .status()?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "MLX Python dependencies missing for {}; install mlx-lm or set {PYTHON_ENV}",
        python.display()
    )
    .into())
}

fn ensure_mlx_snapshot(python: &Path) -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let path = mlx_cache_path()?;
    if mlx_snapshot_ready(&path) {
        verify_artifact(&ModelArtifact::mlx_safetensors(&path))?;
        return Ok(path);
    }

    fs::create_dir_all(&path)?;
    let script = r#"
import os
import sys
from huggingface_hub import snapshot_download

snapshot_download(
    repo_id=sys.argv[1],
    revision=sys.argv[2],
    local_dir=sys.argv[3],
    allow_patterns=[
        "*.json",
        "*.model",
        "*.py",
        "*.safetensors",
        "*.tiktoken",
        "*.txt",
        "tokenizer*",
    ],
    token=os.environ.get("HF_TOKEN") or None,
)
"#;
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(script)
        .arg(MLX_REPO)
        .arg(MLX_REVISION)
        .arg(&path);
    if let Some(token) = hf_token() {
        command.env("HF_TOKEN", token);
    }
    let status = command.status()?;
    if !status.success() {
        return Err(format!("snapshot download failed for {MLX_REPO}: {status}").into());
    }
    verify_artifact(&ModelArtifact::mlx_safetensors(&path))?;
    Ok(path)
}

fn mlx_snapshot_ready(path: &Path) -> bool {
    path.join(MLX_WEIGHTS).is_file()
        && path.join("config.json").is_file()
        && (path.join("tokenizer.json").is_file() || path.join("tokenizer.model").is_file())
}

#[cfg(feature = "llama-cpp")]
fn ensure_gguf_dev_model() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let path = gguf_cache_path()?;
    let artifact = ModelArtifact::gguf(&path);
    if path.exists() {
        if verify_artifact(&artifact).is_ok() {
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }

    fs::create_dir_all(path.parent().expect("cache path has a parent"))?;
    let url =
        format!("https://huggingface.co/{GGUF_REPO}/resolve/{GGUF_REVISION}/{GGUF_FILE_NAME}");
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
        return Err(format!("curl failed to download {GGUF_FILE_NAME}: {status}").into());
    }
    verify_artifact(&artifact)?;
    Ok(path)
}

fn mlx_cache_path() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(Path::new(&home)
        .join(".mayhem")
        .join("cache")
        .join("huggingface")
        .join(MLX_REPO)
        .join(MLX_REVISION)
        .join("snapshot"))
}

#[cfg(feature = "llama-cpp")]
fn gguf_cache_path() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(Path::new(&home)
        .join(".mayhem")
        .join("cache")
        .join("huggingface")
        .join(GGUF_REPO)
        .join(GGUF_REVISION)
        .join(GGUF_FILE_NAME))
}

fn python_bin() -> PathBuf {
    env::var_os(PYTHON_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("python3"))
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
