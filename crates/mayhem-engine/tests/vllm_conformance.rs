#![cfg(feature = "vllm")]

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
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
const MAX_NUM_SEQS_ENV: &str = "MAYHEM_VLLM_MAX_NUM_SEQS";
const CONCURRENT_CAPACITY_ENV: &str = "MAYHEM_VLLM_CONCURRENT_GENERATION_CAPACITY";
const MAX_BATCHED_TOKENS_ENV: &str = "MAYHEM_VLLM_MAX_NUM_BATCHED_TOKENS";
const CONCURRENT_PROMPT_TOKENS_ENV: &str = "MAYHEM_VLLM_CONCURRENT_PROMPT_TOKENS";
const DTYPE_ENV: &str = "MAYHEM_VLLM_DTYPE";
const KV_CACHE_DTYPE_ENV: &str = "MAYHEM_VLLM_KV_CACHE_DTYPE";

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
    let max_num_seqs = env_u32(MAX_NUM_SEQS_ENV, 1)?.max(1);
    let concurrent_capacity = env_u32(CONCURRENT_CAPACITY_ENV, 1)?.max(1);
    let max_num_batched_tokens = env_u32(MAX_BATCHED_TOKENS_ENV, ctx_size)?.max(1);
    let concurrent_prompt_tokens = env_u32(CONCURRENT_PROMPT_TOKENS_ENV, 0)?;
    if !(1..=100).contains(&utilization_pct) {
        return Err(format!("{UTILIZATION_PCT_ENV} must be between 1 and 100").into());
    }
    if concurrent_capacity > max_num_seqs {
        return Err(format!("{CONCURRENT_CAPACITY_ENV} cannot exceed {MAX_NUM_SEQS_ENV}").into());
    }

    let mut backend = VllmBackend::with_python(&python)?;
    let mut config = LoadConfig::vllm_safetensors(&model);
    config.ctx_size = ctx_size;
    config.batch_size = 1;
    config.ubatch_size = max_num_batched_tokens;
    config.vllm_dtype = env::var(DTYPE_ENV).ok().filter(|value| !value.is_empty());
    config.vllm_kv_cache_dtype = env::var(KV_CACHE_DTYPE_ENV)
        .ok()
        .filter(|value| !value.is_empty());
    config.vllm_gpu_memory_utilization_pct = Some(utilization_pct);
    config.vllm_gpu_memory_utilization_floor_pct = Some(utilization_pct);
    config.vllm_max_num_seqs = Some(max_num_seqs);
    config.vllm_concurrent_generation_capacity =
        (concurrent_capacity > 1).then_some(concurrent_capacity);
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

    if concurrent_capacity > 1 {
        run_concurrent_generation_proof(
            &backend,
            concurrent_capacity,
            concurrent_prompt_tokens,
            ctx_size,
        )?;
    }

    Ok(())
}

fn run_concurrent_generation_proof(
    backend: &VllmBackend,
    capacity: u32,
    target_prompt_tokens: u32,
    ctx_size: u32,
) -> TestResult {
    let concurrent = backend
        .concurrent_generation_backend()
        .ok_or("vLLM did not expose its configured concurrent generation backend")?;
    assert_eq!(concurrent.capacity(), capacity as usize);
    let max_prompt_tokens = ctx_size.saturating_sub(1024);
    let target_prompt_tokens = if target_prompt_tokens == 0 {
        1024.min(max_prompt_tokens)
    } else {
        target_prompt_tokens
    };
    if target_prompt_tokens > max_prompt_tokens {
        return Err(format!(
            "{CONCURRENT_PROMPT_TOKENS_ENV}={target_prompt_tokens} exceeds the safe prompt ceiling {max_prompt_tokens} for ctx {ctx_size}"
        )
        .into());
    }

    let mut prompts = Vec::with_capacity(capacity as usize);
    for index in 0..capacity {
        let marker = format!("lane-{index}");
        prompts.push(prompt_near_token_target(
            backend,
            &marker,
            target_prompt_tokens,
            max_prompt_tokens,
        )?);
    }

    let barrier = Arc::new(Barrier::new(capacity as usize + 1));
    let mut handles = Vec::with_capacity(capacity as usize);
    for (index, (prompt, prompt_tokens)) in prompts.into_iter().enumerate() {
        let concurrent = Arc::clone(&concurrent);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || -> Result<_, String> {
            barrier.wait();
            let started = Instant::now();
            let mut chunks = Vec::new();
            let mut request = GenerateRequest::new(format!(
                "{prompt}\nEnd of lane {index}. Reply with only OK."
            ))
            .with_max_new_tokens(8);
            request.temperature = Some(0.0);
            let output = concurrent
                .generate(
                    request,
                    &mut |chunk| {
                        chunks.push(chunk);
                        Ok(())
                    },
                    &CancellationToken::new(),
                )
                .map_err(|error| error.to_string())?;
            if chunks.is_empty() || output.text.trim().is_empty() {
                return Err(format!("concurrent lane {index} returned no output"));
            }
            Ok((
                index,
                prompt_tokens,
                output.usage.prompt_tokens,
                output.usage.completion_tokens,
                started.elapsed().as_millis(),
                output.text.trim().to_owned(),
            ))
        }));
    }

    let wall_started = Instant::now();
    barrier.wait();
    let mut results = handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .map_err(|_| "concurrent vLLM proof thread panicked".to_owned())?
        })
        .collect::<Result<Vec<_>, String>>()?;
    let wall_ms = wall_started.elapsed().as_millis();
    results.sort_by_key(|result| result.0);
    let summed_request_ms = results.iter().map(|result| result.4).sum::<u128>();
    assert!(
        wall_ms * 10 < summed_request_ms * 9,
        "configured vLLM requests did not overlap: wall_ms={wall_ms}, summed_request_ms={summed_request_ms}"
    );
    for (index, target_tokens, prompt_tokens, completion_tokens, elapsed_ms, text) in &results {
        println!(
            "vLLM concurrent lane={index} target_prompt_tokens={target_tokens} actual_prompt_tokens={prompt_tokens} completion_tokens={completion_tokens} elapsed_ms={elapsed_ms} output={text:?}"
        );
    }
    println!(
        "vLLM concurrent proof: capacity={capacity} wall_ms={wall_ms} summed_request_ms={summed_request_ms}"
    );
    Ok(())
}

fn prompt_near_token_target(
    backend: &VllmBackend,
    marker: &str,
    target_tokens: u32,
    max_tokens: u32,
) -> TestResult<(String, u32)> {
    let unit = format!(" calibration-{marker}");
    let unit_tokens = backend.tokenize(&unit)?.token_ids.len().max(1);
    let mut low = 1usize;
    let mut high = (target_tokens as usize).div_ceil(unit_tokens) + 1;
    while u32::try_from(backend.tokenize(&unit.repeat(high))?.token_ids.len())? < target_tokens {
        high = high.saturating_mul(2);
    }
    while low < high {
        let middle = low + (high - low) / 2;
        let candidate = unit.repeat(middle);
        let tokens = u32::try_from(backend.tokenize(&candidate)?.token_ids.len())?;
        if tokens < target_tokens {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let prompt = unit.repeat(low);
    let actual_tokens = u32::try_from(backend.tokenize(&prompt)?.token_ids.len())?;
    if actual_tokens < target_tokens || actual_tokens > max_tokens {
        return Err(format!(
            "could not construct {marker} prompt in token range {target_tokens}..={max_tokens}; got {actual_tokens}"
        )
        .into());
    }
    Ok((prompt, actual_tokens))
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
