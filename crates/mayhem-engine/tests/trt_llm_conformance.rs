#![cfg(feature = "trt-llm")]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[cfg(feature = "llama-cpp")]
use mayhem_engine::LlamaCppBackend;
use mayhem_engine::{
    verify_artifact, EngineBackend, GenerateOutput, GenerateRequest, GrammarSpec, LoadConfig,
    ModelArtifact, ToolSpec, TrtLlmBackend,
};
use serde_json::json;

const RUN_ENV: &str = "MAYHEM_RUN_TRTLLM_TESTS";
const BENCH_ENV: &str = "MAYHEM_RUN_TRTLLM_BENCH";
const MODEL_ENV: &str = "MAYHEM_TRTLLM_MODEL";
const ENGINE_DIR_ENV: &str = "MAYHEM_TRTLLM_ENGINE_DIR";
const KV_CACHE_DTYPE_ENV: &str = "MAYHEM_TRTLLM_KV_CACHE_DTYPE";
const PYTHON_ENV: &str = "MAYHEM_TRTLLM_PYTHON";
#[cfg(feature = "llama-cpp")]
const BASELINE_GGUF_ENV: &str = "MAYHEM_TRTLLM_BASELINE_GGUF";

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn trt_llm_checkpoint_smoke_generates_constrains_and_canaries() -> TestResult {
    if env::var(RUN_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping TensorRT-LLM conformance; set {RUN_ENV}=1 to run");
        return Ok(());
    }

    let python = python_bin();
    ensure_trt_python(&python)?;
    let model_path = trt_model_path()?;
    verify_artifact(&ModelArtifact::trt_llm_checkpoint(&model_path))?;

    let mut backend = TrtLlmBackend::new()?;
    let mut config = LoadConfig::trt_llm_checkpoint(&model_path);
    config.ctx_size = 1024;
    config.trt_engine_dir = Some(engine_dir(&model_path));
    config.trt_tensor_parallel = Some(1);
    config.trt_kv_cache_dtype = kv_cache_dtype(&model_path);
    let info = backend.load(config)?;
    assert_eq!(info.backend, "trt-llm");
    assert_eq!(info.artifact.path, model_path);
    assert!(
        info.n_vocab > 0,
        "TensorRT tokenizer did not report a vocabulary"
    );

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
    assert_usage(&output);

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
    assert!(parsed["arguments"].is_object());

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
    )?;
    assert_usage(&canary);
    assert!(!canary_chunks.is_empty(), "canary produced no token chunks");
    println!(
        "TensorRT-LLM canary fingerprint: {}",
        token_fingerprint(canary_chunks.iter().map(|chunk| chunk.token_id))
    );

    Ok(())
}

#[cfg(feature = "llama-cpp")]
#[test]
fn trt_llm_nvfp4_beats_llama_cpp_baseline_by_5x() -> TestResult {
    if env::var(BENCH_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping TensorRT-LLM benchmark; set {BENCH_ENV}=1 to run");
        return Ok(());
    }

    let model_path = trt_model_path()?;
    let gguf_path = PathBuf::from(env::var_os(BASELINE_GGUF_ENV).ok_or(format!(
        "{BASELINE_GGUF_ENV} must point to the same model's llama.cpp GGUF baseline"
    ))?);

    let mut trt = TrtLlmBackend::new()?;
    let mut trt_config = LoadConfig::trt_llm_checkpoint(&model_path);
    trt_config.ctx_size = 1024;
    trt_config.trt_engine_dir = Some(engine_dir(&model_path));
    trt_config.trt_tensor_parallel = Some(1);
    trt_config.trt_kv_cache_dtype = kv_cache_dtype(&model_path);
    trt.load(trt_config)?;

    let mut llama = LlamaCppBackend::new()?;
    let mut llama_config = LoadConfig::gguf(&gguf_path);
    llama_config.ctx_size = 1024;
    llama_config.batch_size = 256;
    llama_config.ubatch_size = 256;
    llama_config.threads = Some(8);
    llama_config.gpu_layers = Some(0);
    llama.load(llama_config)?;

    let prompt = "Write a numbered list of short words about audited inference receipts.";
    let _ = timed_generate(&mut trt, "Warm up with three short words.", 8)?;
    let _ = timed_generate(&mut llama, "Warm up with three short words.", 8)?;

    let trt_samples = throughput_samples(&mut trt, prompt, 128, 3)?;
    let llama_samples = throughput_samples(&mut llama, prompt, 128, 3)?;
    let trt_tps = median(&trt_samples);
    let llama_tps = median(&llama_samples);
    println!(
        "TensorRT-LLM tok/s median: {trt_tps:.2} samples={trt_samples:?}; llama.cpp tok/s median: {llama_tps:.2} samples={llama_samples:?}"
    );
    assert!(
        trt_tps >= llama_tps * 5.0,
        "TensorRT-LLM tok/s ({trt_tps:.2}) was not >= 5x llama.cpp ({llama_tps:.2})"
    );

    Ok(())
}

#[cfg(not(feature = "llama-cpp"))]
#[test]
fn trt_llm_benchmark_requires_llama_cpp_feature() {
    if env::var(BENCH_ENV).ok().as_deref() == Some("1") {
        panic!("TensorRT-LLM benchmark requires enabling the llama-cpp feature");
    }
}

fn timed_generate<B: EngineBackend>(
    backend: &mut B,
    prompt: &str,
    max_new_tokens: u32,
) -> Result<(GenerateOutput, Duration), mayhem_engine::EngineError> {
    let start = Instant::now();
    let output = backend.generate(
        GenerateRequest::new(prompt).with_max_new_tokens(max_new_tokens),
        &mut |_chunk| Ok(()),
    )?;
    Ok((output, start.elapsed()))
}

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

fn tokens_per_second(output: &GenerateOutput, elapsed: Duration) -> f64 {
    output.usage.completion_tokens as f64 / elapsed.as_secs_f64().max(0.001)
}

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

fn token_fingerprint(tokens: impl IntoIterator<Item = i32>) -> String {
    let mut hasher = blake3::Hasher::new();
    for token in tokens {
        hasher.update(&token.to_be_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn trt_model_path() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    env::var_os(MODEL_ENV).map(PathBuf::from).ok_or_else(|| {
        format!("{MODEL_ENV} must point to a TensorRT-LLM checkpoint directory").into()
    })
}

fn engine_dir(model_path: &Path) -> PathBuf {
    env::var_os(ENGINE_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let root = if model_path.is_dir() {
                model_path
            } else {
                model_path.parent().unwrap_or_else(|| Path::new("."))
            };
            root.join(".mayhem-trtllm-engines").join("conformance")
        })
}

fn kv_cache_dtype(model_path: &Path) -> Option<String> {
    if let Some(value) = env::var_os(KV_CACHE_DTYPE_ENV) {
        return Some(value.to_string_lossy().to_string());
    }
    let lower = model_path.to_string_lossy().to_ascii_lowercase();
    if lower.contains("nvfp4") {
        Some("nvfp4".to_owned())
    } else if lower.contains("fp8") {
        Some("fp8".to_owned())
    } else {
        None
    }
}

fn python_bin() -> PathBuf {
    env::var_os(PYTHON_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("python3"))
}

fn ensure_trt_python(python: &Path) -> TestResult {
    let status = Command::new(python)
        .arg("-c")
        .arg("import tensorrt_llm, transformers")
        .status()?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "TensorRT-LLM Python dependencies missing for {}; install tensorrt_llm or set {PYTHON_ENV}",
        python.display()
    )
    .into())
}
