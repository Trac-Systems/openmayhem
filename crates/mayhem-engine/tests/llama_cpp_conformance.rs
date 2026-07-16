#![cfg(feature = "llama-cpp")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mayhem_engine::{
    verify_artifact, CancellationToken, EmbeddingRequest, EngineBackend, EngineError,
    GenerateRequest, GrammarSpec, LlamaCppBackend, LoadConfig, MediaInput, ModelArtifact, ToolSpec,
    MTMD_MEDIA_MARKER,
};
use serde_json::json;

const RUN_ENV: &str = "MAYHEM_RUN_LLAMACPP_TESTS";
const MODEL_ENV: &str = "MAYHEM_LLAMACPP_MODEL";
const GPU_LAYERS_ENV: &str = "MAYHEM_LLAMACPP_GPU_LAYERS";
const HF_TOKEN_FILE_ENV: &str = "HF_TOKEN_FILE";
const DEFAULT_HF_TOKEN_FILE: &str = ".mayhem-local/secrets/hf.txt";
const REPO: &str = "lmstudio-community/Qwen3.5-4B-GGUF";
const REVISION: &str = "f9f88ac3e234be915e23811a6d28ea287bdb927e";
const FILE_NAME: &str = "Qwen3.5-4B-Q4_K_M.gguf";
const EMBED_REPO: &str = "ggml-org/bge-small-en-v1.5-Q8_0-GGUF";
const EMBED_REVISION: &str = "f2068edd9b54f2a369549ccc71f70ed273a2a801";
const EMBED_FILE_NAME: &str = "bge-small-en-v1.5-q8_0.gguf";
const VISION_REPO: &str = "ggml-org/SmolVLM2-256M-Video-Instruct-GGUF";
const VISION_REVISION: &str = "7b6af08ec3a985c86542c095a63bab2e68510461";
const VISION_FILE_NAME: &str = "SmolVLM2-256M-Video-Instruct-Q8_0.gguf";
const VISION_MMPROJ_FILE_NAME: &str = "mmproj-SmolVLM2-256M-Video-Instruct-Q8_0.gguf";
const RED_SQUARE_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAAZklEQVR42u3QMREAAAgEIEN8/3jW0ByeDBSgOpnPSoAAAQIECBAgQIAAAQIECBAgQIAAAQIECBAgQIAAAQIECBAgQIAAAQIECBAgQIAAAQIECBAgQIAAAQIECBAgQIAAAQIECLhvAa870eGT0af7AAAAAElFTkSuQmCC";

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
    config.gpu_layers = optional_positive_u32(GPU_LAYERS_ENV)?;
    eprintln!(
        "llama.cpp conformance model={} gpu_layers={:?}",
        model_path.display(),
        config.gpu_layers
    );
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
        &CancellationToken::new(),
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
        &CancellationToken::new(),
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

    let cancellation = CancellationToken::new();
    let cancel_from_sink = cancellation.clone();
    let mut cancelled_chunks = 0_usize;
    let mut cancellable = GenerateRequest::new(
        "Continue with a long numbered list of distinct short words without stopping.",
    )
    .with_max_new_tokens(128);
    cancellable.ignore_eos = true;
    let error = backend
        .generate(
            cancellable,
            &mut |_chunk| {
                cancelled_chunks += 1;
                cancel_from_sink.cancel();
                Ok(())
            },
            &cancellation,
        )
        .expect_err("llama.cpp generation must stop after cancellation");
    assert!(matches!(error, EngineError::Cancelled), "{error}");
    assert_eq!(cancelled_chunks, 1, "cancellation exceeded one decode step");

    let recovered = backend.generate(
        GenerateRequest::new("Reply with ok.").with_max_new_tokens(8),
        &mut |_chunk| Ok(()),
        &CancellationToken::new(),
    )?;
    assert!(
        !recovered.text.trim().is_empty(),
        "llama.cpp backend did not remain usable after cancellation"
    );

    Ok(())
}

#[test]
fn gguf_embedding_model_smoke_returns_semantic_vectors() -> TestResult {
    if env::var(RUN_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping llama.cpp embedding conformance; set {RUN_ENV}=1 to run");
        return Ok(());
    }

    let model_path = ensure_embedding_model()?;
    let mut backend = LlamaCppBackend::new()?;
    let mut config = LoadConfig::gguf(&model_path);
    config.ctx_size = 512;
    config.batch_size = 128;
    config.ubatch_size = 128;
    config.threads = Some(4);
    config.gpu_layers = optional_positive_u32(GPU_LAYERS_ENV)?;
    let info = backend.load(config)?;
    assert_eq!(info.artifact.path, model_path);

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = backend
        .embed(
            EmbeddingRequest::many(["cancel this embedding request"]),
            &cancellation,
        )
        .expect_err("llama.cpp embedding must honor cancellation");
    assert!(matches!(error, EngineError::Cancelled), "{error}");

    let output = backend.embed(
        EmbeddingRequest::many([
            "a cat sits on a soft blanket",
            "a kitten rests on a warm blanket",
            "corporate debt refinancing terms",
        ]),
        &CancellationToken::new(),
    )?;
    assert_eq!(output.embeddings.len(), 3);
    assert!(output
        .embeddings
        .iter()
        .all(|embedding| embedding.len() > 64));
    assert!(output.usage.prompt_tokens > 0);
    assert_eq!(output.usage.completion_tokens, 0);

    let similar = cosine(&output.embeddings[0], &output.embeddings[1]);
    let dissimilar = cosine(&output.embeddings[0], &output.embeddings[2]);
    let fingerprint = embedding_fingerprint(&output.embeddings);
    eprintln!(
        "embedding fingerprint={fingerprint} similar={similar:.4} dissimilar={dissimilar:.4}"
    );
    assert!(
        similar > dissimilar,
        "expected similar cosine {similar:.4} > dissimilar {dissimilar:.4}"
    );

    Ok(())
}

#[test]
fn gguf_vision_model_smoke_describes_real_image() -> TestResult {
    if env::var(RUN_ENV).ok().as_deref() != Some("1") {
        eprintln!("skipping llama.cpp vision conformance; set {RUN_ENV}=1 to run");
        return Ok(());
    }

    let model_path = ensure_vision_model()?;
    let mmproj_path = ensure_vision_projector()?;
    let mut backend = LlamaCppBackend::new()?;
    let mut config = LoadConfig::gguf(&model_path);
    config.vision_projector = Some(ModelArtifact::gguf(&mmproj_path));
    config.ctx_size = 4096;
    config.batch_size = 512;
    config.ubatch_size = 512;
    config.threads = Some(4);
    config.gpu_layers = optional_positive_u32(GPU_LAYERS_ENV)?;
    let info = backend.load(config)?;
    assert_eq!(info.artifact.path, model_path);

    let prompt = format!(
        "<|im_start|>User:{MTMD_MEDIA_MARKER}What color is this image? Answer with one color word.<end_of_utterance>\nAssistant:"
    );
    let mut request = GenerateRequest::new(prompt).with_max_new_tokens(32);
    request.temperature = Some(0.0);
    request.media.push(MediaInput {
        kind: "image".to_owned(),
        content_type: Some("image/png".to_owned()),
        url: None,
        data: Some(RED_SQUARE_PNG_BASE64.to_owned()),
        frames: Vec::new(),
        num_frames: None,
        fps: None,
    });

    let cancellation = CancellationToken::new();
    let cancel_from_sink = cancellation.clone();
    let mut cancelled_chunks = 0_usize;
    let mut cancellable = request.clone();
    cancellable.max_new_tokens = 128;
    cancellable.ignore_eos = true;
    let error = backend
        .generate(
            cancellable,
            &mut |_chunk| {
                cancelled_chunks += 1;
                cancel_from_sink.cancel();
                Ok(())
            },
            &cancellation,
        )
        .expect_err("llama.cpp multimodal generation must stop after cancellation");
    assert!(matches!(error, EngineError::Cancelled), "{error}");
    assert_eq!(
        cancelled_chunks, 1,
        "multimodal cancellation exceeded one decode step"
    );

    let output = backend.generate(request, &mut |_chunk| Ok(()), &CancellationToken::new())?;
    eprintln!("vision output={:?}", output.text);
    assert!(
        !output.text.trim().is_empty(),
        "vision model returned empty text"
    );
    assert!(output.usage.prompt_tokens > 0);
    assert!(output.usage.completion_tokens > 0);
    assert!(
        output.text.to_ascii_lowercase().contains("red"),
        "expected vision output to name the red image; output={:?}",
        output.text
    );

    Ok(())
}

fn ensure_dev_model() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = env::var_os(MODEL_ENV).map(PathBuf::from) {
        verify_artifact(&ModelArtifact::gguf(&path))?;
        return Ok(path);
    }
    let path = cache_path(REPO, REVISION, FILE_NAME)?;
    let artifact = ModelArtifact::gguf(&path);
    if path.exists() {
        if verify_artifact(&artifact).is_ok() {
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }

    download_hf_file(REPO, REVISION, FILE_NAME, &path)?;
    verify_artifact(&artifact)?;
    Ok(path)
}

fn optional_positive_u32(
    name: &str,
) -> std::result::Result<Option<u32>, Box<dyn std::error::Error>> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .ok_or_else(|| format!("{name} is not valid UTF-8"))?;
    let parsed = value
        .parse::<u32>()
        .map_err(|err| format!("{name} must be a positive integer: {err}"))?;
    if parsed == 0 {
        return Err(format!("{name} must be a positive integer").into());
    }
    Ok(Some(parsed))
}

fn ensure_embedding_model() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = env::var("MAYHEM_EMBEDDING_GGUF") {
        return Ok(PathBuf::from(path));
    }
    let path = cache_path(EMBED_REPO, EMBED_REVISION, EMBED_FILE_NAME)?;
    let artifact = ModelArtifact::gguf(&path);
    if path.exists() {
        if verify_artifact(&artifact).is_ok() {
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }

    download_hf_file(EMBED_REPO, EMBED_REVISION, EMBED_FILE_NAME, &path)?;
    verify_artifact(&artifact)?;
    Ok(path)
}

fn ensure_vision_model() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = env::var("MAYHEM_VISION_GGUF") {
        return Ok(PathBuf::from(path));
    }
    ensure_hf_gguf(VISION_REPO, VISION_REVISION, VISION_FILE_NAME)
}

fn ensure_vision_projector() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = env::var("MAYHEM_VISION_MMPROJ") {
        return Ok(PathBuf::from(path));
    }
    ensure_hf_gguf(VISION_REPO, VISION_REVISION, VISION_MMPROJ_FILE_NAME)
}

fn ensure_hf_gguf(
    repo: &str,
    revision: &str,
    file_name: &str,
) -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let path = cache_path(repo, revision, file_name)?;
    let artifact = ModelArtifact::gguf(&path);
    if path.exists() {
        if verify_artifact(&artifact).is_ok() {
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }

    download_hf_file(repo, revision, file_name, &path)?;
    verify_artifact(&artifact)?;
    Ok(path)
}

fn download_hf_file(
    repo: &str,
    revision: &str,
    file_name: &str,
    path: &Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(path.parent().expect("cache path has a parent"))?;
    let url = format!("https://huggingface.co/{repo}/resolve/{revision}/{file_name}");
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
        .arg(&url);

    if let Some(token) = hf_token() {
        curl.arg("--header")
            .arg(format!("Authorization: Bearer {token}"));
    }

    let status = curl.status()?;
    if !status.success() {
        return Err(format!("curl failed to download {file_name}: {status}").into());
    }
    Ok(())
}

fn cache_path(
    repo: &str,
    revision: &str,
    file_name: &str,
) -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(Path::new(&home)
        .join(".mayhem")
        .join("cache")
        .join("huggingface")
        .join(repo)
        .join(revision)
        .join(file_name))
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let len = left.len().min(right.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for index in 0..len {
        dot += left[index] * right[index];
        left_norm += left[index] * left[index];
        right_norm += right[index] * right[index];
    }
    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

fn embedding_fingerprint(embeddings: &[Vec<f32>]) -> String {
    let mut hasher = blake3::Hasher::new();
    for embedding in embeddings {
        hasher.update(&(embedding.len() as u64).to_le_bytes());
        for value in embedding {
            hasher.update(&value.to_le_bytes());
        }
    }
    hasher.finalize().to_hex().to_string()
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
