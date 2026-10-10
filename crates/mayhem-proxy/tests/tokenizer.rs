//! Actual bundled contained tokenizer, no upstream/provider requests.
#![cfg(unix)]
use mayhem_proxy::{
    attempts::Digest,
    health::native::{Limits, Source},
    worker::host::{Pool, PoolLimits},
};
use serde_json::json;
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn data() -> Vec<u8> {
    serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],
        "normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,
        "model":{"type":"WordLevel","vocab":{"[UNK]":0,"one":1,"two":2,"三":3},"unk_token":"[UNK]"}})).unwrap()
}
fn limits() -> Limits {
    Limits {
        artifact_bytes: 1024 * 1024,
        output_bytes: 1024 * 1024,
        channels: 8,
        workers: 1,
        minimum_tokens: 2,
    }
}
fn digest(data: &[u8]) -> Digest {
    Digest::new(blake3::hash(data).to_hex().as_str()).unwrap()
}
fn pool(dir: &std::path::Path) -> Pool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    Pool::new(
        env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
        dir,
        PoolLimits {
            max_children: 1,
            max_buffer_bytes: 8 * 1024 * 1024,
            startup_timeout: Duration::from_secs(5),
            processing_timeout: Duration::from_secs(5),
        },
    )
    .unwrap()
}
#[test]
fn provisioning_requires_exact_valid_artifact_and_explicit_launcher() {
    let dir = tempfile::tempdir().unwrap();
    let p = pool(dir.path());
    let b = data();
    let d = digest(&b);
    let source = Source::from_bytes(&b, d.clone(), d.clone(), d, limits()).unwrap();
    source.validate(&p).unwrap();
    source.validate(&p).unwrap();
    for mode in ["malformed", "padding", "truncation", "dropout", "regex"] {
        let mut value: serde_json::Value = serde_json::from_slice(&b).unwrap();
        match mode {
            "padding" | "truncation" => value[mode] = json!({}),
            "dropout" => value["model"]["dropout"] = json!(0.5),
            "regex" => {
                value["normalizer"] = json!({"type":"Replace","pattern":{"Regex":"("},"content":""})
            }
            _ => (),
        }
        let bytes = if mode == "malformed" {
            b"{".to_vec()
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        let d = digest(&bytes);
        let source = Source::from_bytes(&bytes, d.clone(), d.clone(), d, limits()).unwrap();
        assert!(source.validate(&p).is_err(), "{mode}");
    }
    let other = tempfile::tempdir().unwrap();
    assert!(source.bind_pool(&pool(other.path())).is_err());
    assert!(Source::from_bytes(&b, digest(b"changed"), digest(&b), digest(&b), limits()).is_err());
}
fn frame(bytes: &[u8], out: &mut Vec<u8>) {
    out.extend((bytes.len() as u32).to_le_bytes());
    out.extend(bytes);
}
fn message(bytes: &[u8], fields: &[(&str, u32)]) -> Vec<u8> {
    let d = digest(bytes);
    let mut wire = Vec::new();
    frame(&serde_json::to_vec(&json!({"abi":1,"release":mayhem_proxy::worker::RELEASE,"nonce":digest(b"nonce"),"digest":d,"limits":limits(),"fields":fields.len()})).unwrap(), &mut wire);
    frame(bytes, &mut wire);
    for (text, first) in fields {
        wire.extend(first.to_le_bytes());
        frame(text.as_bytes(), &mut wire);
    }
    wire
}
async fn execute(wire: Vec<u8>) -> std::process::Output {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mayhem-proxy-worker"))
        .arg("--tokenizer-stdio-v1")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let writer = tokio::spawn(async move {
        let _ = input.write_all(&wire).await;
        let _ = input.shutdown().await;
    });
    let result = tokio::time::timeout(Duration::from_secs(12), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    writer.await.unwrap();
    result
}
#[tokio::test]
async fn actual_worker_reproduces_utf8_offsets_and_rejects_unbounded_or_unbound_frames() {
    let b = data();
    let result = execute(message(&b, &[("one two 三", 4)])).await;
    assert!(result.status.success());
    let length = u32::from_le_bytes(result.stdout[..4].try_into().unwrap()) as usize;
    assert_eq!(length + 4, result.stdout.len());
    let reply: serde_json::Value = serde_json::from_slice(&result.stdout[4..]).unwrap();
    assert_eq!(reply["tokens"], 2);
    assert_eq!(reply["digest"], json!(digest(&b)));
    for wire in [
        vec![255; 4],
        message(&b, &[("one 三", 5)]),
        {
            let mut v = message(&b, &[]);
            v.push(1);
            v
        },
        {
            let mut v = message(&b, &[]);
            v.truncate(v.len() - 1);
            v
        },
    ] {
        let result = execute(wire).await;
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
}
#[tokio::test]
async fn premature_or_invalid_mode_has_no_parser_output() {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mayhem-proxy-worker"))
        .arg("--uncontained-tokenizer")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut output)
        .await
        .unwrap();
    assert!(!child.wait().await.unwrap().success());
    assert!(output.is_empty());
}

#[tokio::test]
async fn adversarial_normalization_cannot_escape_heap_ceiling_or_poison_the_next_job() {
    let mut value: serde_json::Value = serde_json::from_slice(&data()).unwrap();
    // Tiny imported data can expand bounded caller text into gigabytes. Byte
    // framing alone would not protect the host or the worker's heap.
    value["normalizer"] =
        json!({"type":"Replace","pattern":{"String":"a"},"content":"a".repeat(8192)});
    let artifact = serde_json::to_vec(&value).unwrap();
    let text = "a".repeat(256 * 1024);
    let result = execute(message(&artifact, &[(&text, 0)])).await;
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(execute(message(&data(), &[("one two 三", 4)]))
        .await
        .status
        .success());
}
