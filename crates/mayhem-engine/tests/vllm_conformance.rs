#![cfg(feature = "vllm")]

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use mayhem_engine::{
    verify_artifact, CancellationToken, EngineBackend, GenerateRequest, LoadConfig, ModelArtifact,
    VllmBackend,
};

const RUN_ENV: &str = "MAYHEM_RUN_VLLM_TESTS";
const MODEL_ENV: &str = "MAYHEM_VLLM_MODEL";
const PYTHON_ENV: &str = "MAYHEM_VLLM_PYTHON";
const CTX_SIZE_ENV: &str = "MAYHEM_VLLM_CTX_SIZE";
const UTILIZATION_PCT_ENV: &str = "MAYHEM_VLLM_UTILIZATION_PCT";

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn vllm_checkpoint_smoke_loads_tokenizes_streams_and_generates() -> TestResult {
    if env::var(RUN_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping vLLM conformance; set {RUN_ENV}=1 to run");
        return Ok(());
    }

    let python = env::var_os(PYTHON_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("python3"));
    ensure_vllm_python(&python)?;
    let model = env::var_os(MODEL_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{MODEL_ENV} must point at a local vLLM checkpoint"))?;
    verify_artifact(&ModelArtifact::vllm_safetensors(&model))?;

    let ctx_size = env_u32(CTX_SIZE_ENV, 4096)?;
    let utilization_pct = env_u32(UTILIZATION_PCT_ENV, 10)?;
    if !(1..=100).contains(&utilization_pct) {
        return Err(format!("{UTILIZATION_PCT_ENV} must be between 1 and 100").into());
    }

    let mut backend = VllmBackend::with_python(&python)?;
    let mut config = LoadConfig::vllm_safetensors(&model);
    config.ctx_size = ctx_size;
    config.batch_size = 1;
    config.ubatch_size = ctx_size;
    config.vllm_dtype = Some("float16".to_owned());
    config.vllm_gpu_memory_utilization_pct = Some(utilization_pct);
    config.vllm_gpu_memory_utilization_floor_pct = Some(utilization_pct);
    let loaded = backend.load(config)?;
    assert_eq!(loaded.backend, "vllm");
    assert_eq!(loaded.ctx_size, ctx_size);
    assert!(loaded.n_vocab > 0, "vLLM tokenizer reported no vocabulary");

    let tokens = backend.tokenize("Say ok.")?;
    assert!(!tokens.token_ids.is_empty());

    let started = Instant::now();
    let mut chunks = Vec::new();
    let mut request = GenerateRequest::new("Reply with the single word OK.").with_max_new_tokens(8);
    request.temperature = Some(0.0);
    let output = backend.generate(
        request,
        &mut |chunk| {
            chunks.push(chunk);
            Ok(())
        },
        &CancellationToken::new(),
    )?;
    assert!(!chunks.is_empty(), "vLLM emitted no streaming token chunks");
    assert!(!output.text.trim().is_empty(), "vLLM returned empty text");
    assert!(output.usage.prompt_tokens > 0);
    assert!(output.usage.completion_tokens > 0);
    println!(
        "vLLM smoke: ctx={} prompt_tokens={} completion_tokens={} elapsed_ms={} output={:?}",
        loaded.ctx_size,
        output.usage.prompt_tokens,
        output.usage.completion_tokens,
        started.elapsed().as_millis(),
        output.text.trim()
    );

    Ok(())
}

fn ensure_vllm_python(python: &PathBuf) -> TestResult {
    let status = Command::new(python)
        .args(["-c", "import vllm; print(vllm.__version__)"])
        .status()?;
    if !status.success() {
        return Err(format!(
            "{} cannot import vllm; run the Mayhem managed runtime preflight first",
            python.display()
        )
        .into());
    }
    Ok(())
}

fn env_u32(name: &str, default: u32) -> TestResult<u32> {
    match env::var(name) {
        Ok(value) => Ok(value
            .parse::<u32>()
            .map_err(|_| format!("{name} must be an unsigned integer"))?),
        Err(_) => Ok(default),
    }
}
