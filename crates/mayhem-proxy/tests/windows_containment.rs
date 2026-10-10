//! Run on a native supported Windows host. Cross-checking this file does not
//! establish LPAC enforcement, decoder compatibility, or provider setup support.
#![cfg(windows)]
#[path = "support/windows_fixture.rs"]
mod windows_fixture;

use mayhem_proto::proxy::ProxyEndpoint;
use mayhem_proxy::{
    attempts::Digest,
    connector::{config::ErrorProfile, http::WireFormat},
    semantics::{Output, Policy},
    worker::{
        host::{Pool, PoolLimits},
        DecodeLimits, Init, Session, ABI, RELEASE,
    },
};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn uncontained_launch_refuses_before_reading_ipc_or_ready() {
    for mode in ["--stdio-v1", "--tokenizer-stdio-v1", "--tokenizer-stdio-v2"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mayhem-proxy-worker"))
            .arg(mode)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Keep stdin open: an unrestricted fallback would wait for input.
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= until {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("uncontained worker continued: {mode}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{mode}");
        assert!(output.stdout.is_empty(), "READY must require containment");
        assert!(output.stderr.is_empty());
    }
}

#[tokio::test]
async fn actual_worker_prepares_schema_and_releases_its_process_slot_after_stop() {
    let work = windows_fixture::PrivateDirectory::new();
    let pool = Pool::new(
        env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
        work.path(),
        PoolLimits {
            max_children: 1,
            max_buffer_bytes: 32 * 1024 * 1024,
            startup_timeout: Duration::from_secs(3),
            processing_timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    let digest = |n: char| Digest::new(n.to_string().repeat(64)).unwrap();
    let policy = Policy {
        endpoint: ProxyEndpoint::Chat,
        request_hash: digest('3'),
        tools: Default::default(),
        output: Output::JsonSchema {
            schema: serde_json::json!({"type":"object","properties":{"text":{"type":"string","pattern":"^[a-z]+$"}},"required":["text"],"additionalProperties":false}),
        },
        recipe: None,
        recipe_response_bytes: None,
    };
    let init = Init {
        abi: ABI,
        release: RELEASE.into(),
        session: Session {
            invocation: digest('1'),
            attempt: 1,
            binding_hash: digest('2'),
        },
        format: WireFormat::Json,
        error_profile: ErrorProfile::OpenAi,
        limits: DecodeLimits {
            max_total_bytes: 1024 * 1024,
            max_event_bytes: 1024 * 1024,
        },
        semantic_policy: Some(policy.digest().unwrap()),
    };
    let ready = pool
        .start(init.clone())
        .await
        .unwrap()
        .configure_semantics(&policy)
        .await
        .unwrap();
    assert!(matches!(
        pool.start(init.clone()).await,
        Err(mayhem_proxy::worker::Error::Capacity)
    ));
    ready.stop().await.unwrap();
    let until = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match pool.start(init.clone()).await {
            Ok(ready) => {
                ready
                    .configure_semantics(&policy)
                    .await
                    .unwrap()
                    .stop()
                    .await
                    .unwrap();
                break;
            }
            Err(mayhem_proxy::worker::Error::Capacity) if tokio::time::Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => panic!("process slot not safely reusable: {error}"),
        }
    }
}
