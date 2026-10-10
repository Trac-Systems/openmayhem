//! Native Windows acceptance only. Cross-compilation is not enforcement proof.
//! Uses the actual bundled worker and LPAC/Job launcher with synthetic data;
//! no provider, upstream, filesystem tokenizer import or payment is involved.
#![cfg(windows)]
#[path = "support/windows_fixture.rs"]
mod windows_fixture;

use mayhem_proxy::{
    attempts::Digest,
    health::native::{engine, Limits, Source},
    worker::{
        host::{Pool, PoolLimits},
        RELEASE,
    },
};
use mayhem_windows_sandbox::{DecoderChild, DecoderLauncher, DecoderMode};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn artifact() -> Vec<u8> {
    serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],
        "normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,
        "model":{"type":"WordLevel","vocab":{"[UNK]":0,"one":1,"two":2,"三":3},"unk_token":"[UNK]"}})).unwrap()
}
fn digest(bytes: &[u8]) -> Digest {
    Digest::new(blake3::hash(bytes).to_hex().as_str()).unwrap()
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
fn frame(bytes: &[u8], out: &mut Vec<u8>) {
    out.extend((bytes.len() as u32).to_le_bytes());
    out.extend(bytes);
}
struct Worker {
    child: DecoderChild,
    input: tokio::fs::File,
    output: tokio::fs::File,
    _directory: windows_fixture::PrivateDirectory,
}
impl Worker {
    fn new() -> Self {
        let directory = windows_fixture::PrivateDirectory::new();
        let launcher = DecoderLauncher::new(
            std::path::Path::new(env!("CARGO_BIN_EXE_mayhem-proxy-worker")),
            directory.path(),
        )
        .unwrap();
        let mut child = launcher.spawn(DecoderMode::Tokenizer).unwrap();
        let mut input = tokio::fs::File::from_std(child.stdin.take().unwrap());
        let mut output = tokio::fs::File::from_std(child.stdout.take().unwrap());
        input.set_max_buf_size(65536);
        output.set_max_buf_size(65536);
        Self {
            child,
            input,
            output,
            _directory: directory,
        }
    }
    async fn initialize(&mut self, data: &[u8], pin: Digest) {
        let mut wire = Vec::new();
        frame(&serde_json::to_vec(&json!({"abi":2,"release":RELEASE,"nonce":digest(b"init"),"digest":pin,"limits":limits(),"fields":0})).unwrap(),&mut wire);
        frame(data, &mut wire);
        self.input.write_all(&wire).await.unwrap();
        self.input.flush().await.unwrap();
    }
    async fn reply(&mut self) -> Value {
        let n = self.output.read_u32_le().await.unwrap() as usize;
        assert!(n <= engine::CONTROL);
        let mut bytes = vec![0; n];
        self.output.read_exact(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    async fn count(&mut self, text: &str, first: u32, nonce: Digest) {
        let mut wire = Vec::new();
        frame(
            &serde_json::to_vec(&json!({"nonce":nonce,"fields":1})).unwrap(),
            &mut wire,
        );
        wire.extend(first.to_le_bytes());
        frame(text.as_bytes(), &mut wire);
        self.input.write_all(&wire).await.unwrap();
        self.input.flush().await.unwrap();
    }
    async fn failed_without_reply(&mut self) {
        loop {
            if let Some(code) = self.child.try_wait().unwrap() {
                assert_ne!(code, 0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut bytes = Vec::new();
        (&mut self.output)
            .take(4096)
            .read_to_end(&mut bytes)
            .await
            .unwrap();
        assert!(bytes.is_empty());
    }
}

#[test]
fn windows_tokenizer_policy_refuses_unrestricted_and_legacy_modes() {
    assert!(engine::resource_limits(false).is_err());
    assert!(engine::resource_limits(true).is_err());
    assert_eq!(engine::HEAP_BYTES, 512 * 1024 * 1024);
}

#[test]
fn windows_source_provisioning_validates_exact_pin_through_the_configured_pool() {
    let directory = windows_fixture::PrivateDirectory::new();
    let pool = Pool::new(
        env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
        directory.path(),
        PoolLimits {
            max_children: 1,
            max_buffer_bytes: 8 * 1024 * 1024,
            startup_timeout: Duration::from_secs(5),
            processing_timeout: Duration::from_secs(5),
        },
    )
    .unwrap();
    let data = artifact();
    let pin = digest(&data);
    let source = Source::from_bytes(&data, pin.clone(), pin.clone(), pin, limits()).unwrap();
    source.validate(&pool).unwrap();
    source.validate(&pool).unwrap();
    assert!(Source::from_bytes(
        &data,
        digest(b"wrong pin"),
        digest(&data),
        digest(&data),
        limits()
    )
    .is_err());
}

#[tokio::test]
async fn windows_retained_tokenizer_counts_utf8_and_rejects_wrong_pin_before_ready() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let data = artifact();
        let pin = digest(&data);
        let mut worker = Worker::new();
        worker.initialize(&data, pin.clone()).await;
        let initialized = worker.reply().await;
        assert_eq!(initialized["tokens"], 0);
        assert_eq!(initialized["digest"], json!(pin));
        assert_eq!(initialized["nonce"], json!(digest(b"init")));
        for (text, first, tokens, nonce) in [
            ("one two 三", 4, 2, digest(b"first")),
            ("one two 三", 0, 3, digest(b"second")),
        ] {
            worker.count(text, first, nonce.clone()).await;
            let reply = worker.reply().await;
            assert_eq!(reply["tokens"], tokens);
            assert_eq!(reply["nonce"], json!(nonce));
            assert_eq!(reply["digest"], json!(pin));
        }
        // Non-boundary byte offset must fail, not fabricate a native rate.
        worker.count("one 三", 5, digest(b"invalid offset")).await;
        worker.failed_without_reply().await;
        let mut wrong = Worker::new();
        wrong.initialize(&data, digest(b"wrong pin")).await;
        wrong.failed_without_reply().await;
    })
    .await
    .expect("bounded native Windows tokenizer acceptance exceeded deadline");
}

#[tokio::test]
async fn windows_tokenizer_heap_expansion_dies_without_poisoning_the_next_worker() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut data: Value = serde_json::from_slice(&artifact()).unwrap();
        data["normalizer"] =
            json!({"type":"Replace","pattern":{"String":"a"},"content":"a".repeat(8192)});
        let data = serde_json::to_vec(&data).unwrap();
        let mut worker = Worker::new();
        worker.initialize(&data, digest(&data)).await;
        assert_eq!(worker.reply().await["tokens"], 0);
        worker
            .count(&"a".repeat(256 * 1024), 0, digest(b"expansion"))
            .await;
        worker.failed_without_reply().await;
        let data = artifact();
        let mut fresh = Worker::new();
        fresh.initialize(&data, digest(&data)).await;
        assert_eq!(fresh.reply().await["tokens"], 0);
        fresh.count("one two 三", 4, digest(b"healthy")).await;
        assert_eq!(fresh.reply().await["tokens"], 2);
    })
    .await
    .expect("bounded native Windows heap acceptance exceeded deadline");
}
