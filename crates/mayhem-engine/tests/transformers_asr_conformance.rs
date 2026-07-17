#![cfg(all(feature = "transformers-asr", unix))]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mayhem_engine::{
    AudioTranscriptionRequest, CancellationToken, EngineBackend, EngineError, LoadConfig,
    TransformersAsrBackend,
};

fn test_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "mayhem-transformers-asr-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ))
}

fn write_model(root: &Path) -> PathBuf {
    let model = root.join("model");
    fs::create_dir_all(&model).expect("model dir");
    let header = br#"{"weight":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
    let mut weights = Vec::new();
    weights.extend_from_slice(&(header.len() as u64).to_le_bytes());
    weights.extend_from_slice(header);
    weights.extend_from_slice(&0_f32.to_le_bytes());
    fs::write(model.join("model.safetensors"), weights).expect("weights");
    for sidecar in [
        "config.json",
        "processor_config.json",
        "tokenizer.json",
        "tokenizer_config.json",
    ] {
        fs::write(model.join(sidecar), "{}\n").expect("sidecar");
    }
    model
}

fn write_worker(root: &Path) -> PathBuf {
    let worker = root.join("fake-python");
    fs::write(
        &worker,
        r#"#!/usr/bin/env python3
import json
import sys
import time

for line in sys.stdin:
    request = json.loads(line)
    request_id = request["id"]
    operation = request["op"]
    payload = request.get("payload") or {}
    if operation == "load":
        result = {"n_ctx_train": 0, "n_vocab": 8192}
    elif operation == "transcribe":
        if payload.get("content_type") == "audio/slow":
            time.sleep(30)
        result = {
            "text": "Hello Mayhem.",
            "audio_seconds": 1,
            "duration_seconds": 0.5,
            "words": [
                {"text": "Hello", "start": 0.08, "end": 0.24},
                {"text": "Mayhem.", "start": 0.24, "end": 0.48},
            ],
            "segments": [
                {"text": "Hello Mayhem.", "start": 0.08, "end": 0.48}
            ],
        }
    elif operation == "shutdown":
        result = {}
    else:
        print(json.dumps({"id": request_id, "ok": False, "error": "bad op"}), flush=True)
        continue
    print(json.dumps({"id": request_id, "ok": True, "result": result}), flush=True)
    if operation == "shutdown":
        break
"#,
    )
    .expect("worker");
    let mut permissions = fs::metadata(&worker)
        .expect("worker metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&worker, permissions).expect("worker permissions");
    worker
}

fn request(content_type: &str) -> AudioTranscriptionRequest {
    AudioTranscriptionRequest {
        audio: vec![1, 2, 3, 4],
        content_type: Some(content_type.to_owned()),
        language: None,
        prompt: None,
    }
}

#[test]
fn transformers_asr_loads_local_artifact_and_returns_timestamps() {
    let root = test_root("timestamps");
    let model = write_model(&root);
    let worker = write_worker(&root);
    let mut backend = TransformersAsrBackend::with_python(worker).expect("backend");
    backend
        .load(LoadConfig::transformers_safetensors(&model))
        .expect("load");

    let output = backend
        .transcribe(request("audio/wav"), &CancellationToken::new())
        .expect("transcribe");

    assert_eq!(output.text, "Hello Mayhem.");
    assert_eq!(output.words.len(), 2);
    assert_eq!(output.segments.len(), 1);
    assert_eq!(output.duration_seconds, Some(0.5));
    assert!(backend.component_healthy());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn transformers_asr_cancellation_kills_compute_and_reloads_for_next_request() {
    let root = test_root("cancel");
    let model = write_model(&root);
    let worker = write_worker(&root);
    let cancellation = CancellationToken::new();
    let cancel_from_test = cancellation.clone();

    let handle = thread::spawn(move || {
        let mut backend = TransformersAsrBackend::with_python(worker).expect("backend");
        backend
            .load(LoadConfig::transformers_safetensors(&model))
            .expect("load");
        let error = backend
            .transcribe(request("audio/slow"), &cancellation)
            .expect_err("cancelled request");
        assert!(matches!(error, EngineError::Cancelled));

        let output = backend
            .transcribe(request("audio/wav"), &CancellationToken::new())
            .expect("automatic reload");
        assert_eq!(output.text, "Hello Mayhem.");
    });
    thread::sleep(Duration::from_millis(150));
    cancel_from_test.cancel();
    handle.join().expect("worker thread");
    fs::remove_dir_all(root).expect("cleanup");
}
