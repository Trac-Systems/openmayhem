#![cfg(feature = "needle")]

use std::env;
use std::path::PathBuf;

use mayhem_engine::{CancellationToken, EngineBackend, GenerateRequest, LoadConfig, NeedleBackend};
use serde_json::{json, Value};

const NEEDLE_WEIGHTS_SHA256: &str =
    "c5f9a3016e4537e492c362da5cb8ba05107d8595bec0d5ea5d8a65801db46531";

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false
            }
        }
    })
}

fn run_case(backend: &mut NeedleBackend, prompt: &str, tools: Vec<Value>, seed: u32) -> Value {
    let mut request = GenerateRequest::new(prompt);
    request.tools = tools;
    request.max_new_tokens = 128;
    request.temperature = Some(0.0);
    request.top_p = Some(1.0);
    request.top_k = Some(1);
    request.seed = Some(seed);
    let mut chunks = Vec::new();
    let output = backend
        .generate(
            request,
            &mut |chunk| {
                chunks.push(chunk);
                Ok(())
            },
            &CancellationToken::new(),
        )
        .expect("real Needle generation");
    let calls: Vec<Value> =
        serde_json::from_str(&output.text).expect("Needle output must be a tool-call array");
    assert!(!calls.is_empty(), "proof prompt must produce a tool call");
    assert_eq!(
        chunks.len(),
        usize::try_from(output.usage.completion_tokens).expect("completion token count"),
        "backend must expose every real decoder token to the fingerprint sink"
    );
    assert_eq!(
        chunks.iter().filter(|chunk| !chunk.text.is_empty()).count(),
        1,
        "backend emits one complete canonical tool-call delta"
    );
    json!({
        "prompt": prompt,
        "output": calls,
        "usage": output.usage,
        "finish_reason": output.finish_reason.to_string(),
        "backend_evidence": backend.loaded_backend_evidence(),
    })
}

#[test]
#[ignore = "requires the immutable Needle artifact and locked Python runtime"]
fn real_needle_tool_call_and_throughput_proof() {
    let model_root = PathBuf::from(
        env::var("MAYHEM_NEEDLE_TEST_MODEL_ROOT")
            .expect("MAYHEM_NEEDLE_TEST_MODEL_ROOT is required"),
    );
    let python = env::var("MAYHEM_NEEDLE_PYTHON").expect("MAYHEM_NEEDLE_PYTHON is required");
    let device = env::var("MAYHEM_NEEDLE_DEVICE").unwrap_or_else(|_| "cpu".to_owned());

    let mut backend =
        NeedleBackend::with_python_for_device(python, &device).expect("valid Needle device");
    let weights = model_root.join("model.safetensors");
    let mut config = LoadConfig::transformers_safetensors(&weights);
    config.artifact = config
        .artifact
        .with_sha256(NEEDLE_WEIGHTS_SHA256)
        .with_sha256_path(&weights);
    config.ctx_size = 1_024;
    let loaded = backend.load(config).expect("load exact Needle artifact");

    let single = run_case(
        &mut backend,
        "What is the weather in Berlin?",
        vec![tool(
            "get_weather",
            "Get the current weather for a city.",
            json!({
                "city": {"type": "string", "minLength": 1}
            }),
            &["city"],
        )],
        7,
    );
    let parallel = run_case(
        &mut backend,
        "Set a timer for five minutes and turn off the lights.",
        vec![
            tool(
                "set_timer",
                "Set a timer.",
                json!({
                    "time_human": {"type": "string"}
                }),
                &["time_human"],
            ),
            tool(
                "toggle_lights",
                "Toggle lights on or off.",
                json!({
                    "state": {"type": "string", "enum": ["on", "off"]}
                }),
                &["state"],
            ),
        ],
        11,
    );
    assert!(
        parallel["output"]
            .as_array()
            .is_some_and(|calls| calls.len() >= 2),
        "parallel proof must retain every requested tool call: {parallel}"
    );

    println!(
        "NEEDLE_PROOF_JSON={}",
        serde_json::to_string(&json!({
            "schema_version": 1,
            "device": device,
            "loaded": loaded,
            "single": single,
            "parallel": parallel,
        }))
        .expect("serialize proof")
    );
}
