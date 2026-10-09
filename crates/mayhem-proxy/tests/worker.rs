#![cfg(unix)]

use mayhem_proto::proxy::{ProxyEndpoint, ProxyRail};
use mayhem_proxy::{
    attempts::{self, Binding, Digest, Identity, Journal, Record},
    connector::{config::ErrorProfile, failure::Code, http::WireFormat},
    worker::{
        self,
        host::{Active, Pool, PoolLimits},
        DecodeLimits, Decoded, Init,
    },
};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn d(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
fn journal(path: &Path) -> Journal {
    Journal::open(
        path,
        Identity {
            network_id: "918".into(),
            msb_bootstrap: d(10),
            subnet_bootstrap: d(11),
            controller_pubkey: d(12),
        },
        attempts::Limits {
            max_records: 200,
            max_unfinished: 200,
            closed_retention_ms: 1000,
            max_payload_bytes: 128 * 1024 * 1024,
        },
    )
    .unwrap()
}
fn record(j: &Journal, n: u64) -> Record {
    j.prepare(
        d(n),
        Binding {
            request_hash: d(1),
            endpoint: ProxyEndpoint::Chat,
            contract_version: 30,
            provider_pubkey: d(2),
            market_id: d(3),
            offer_digest: d(4),
            endpoint_contract: d(5),
            metering_policy: d(6),
            accepted_terms: d(7),
            reservation: d(8),
            capacity_lease: d(9),
            connection_digest: d(10),
            connection_revision: 1,
            recipe_digest: d(11),
            rail: ProxyRail::Fiat,
        },
        100,
    )
    .unwrap()
}
fn limits(children: usize) -> PoolLimits {
    PoolLimits {
        max_children: children,
        max_buffer_bytes: 256 * 1024 * 1024,
        startup_timeout: Duration::from_secs(3),
        processing_timeout: Duration::from_secs(2),
    }
}
fn init(r: &Record, format: WireFormat) -> Init {
    Init::new(
        r,
        format,
        ErrorProfile::OpenAi,
        DecodeLimits {
            max_total_bytes: 2 * 1024 * 1024,
            max_event_bytes: 1024 * 1024,
        },
    )
    .unwrap()
}
fn pool(work: &Path, limits: PoolLimits) -> Pool {
    Pool::new(env!("CARGO_BIN_EXE_mayhem-proxy-worker"), work, limits).unwrap()
}

#[tokio::test]
async fn semantic_policy_is_bound_and_must_compile_before_worker_activation() {
    use mayhem_proxy::semantics::{Output, Policy};
    let store = dir();
    let work = dir();
    let journal = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(2));
    let record = record(&journal, 1);
    let policy = Policy {
        endpoint: mayhem_proto::proxy::ProxyEndpoint::Chat,
        request_hash: record.binding.request_hash.clone(),
        tools: Default::default(),
        output: Output::JsonObject,
        recipe: None,
        recipe_response_bytes: None,
    };
    let init = init(&record, WireFormat::Json)
        .with_semantics(&policy)
        .unwrap();
    let prepared = pool.start(init.clone()).await.unwrap();
    let ticket = journal
        .begin_dispatch(&record.invocation, record.generation, 101)
        .unwrap();
    assert!(
        matches!(prepared.attach(ticket), Err(worker::Error::Configuration)),
        "policy compilation cannot be skipped"
    );
    let prepared = pool.start(init).await.unwrap();
    let mut different = policy;
    different.output = Output::Text;
    assert!(
        matches!(
            prepared.configure_semantics(&different).await,
            Err(worker::Error::Identity)
        ),
        "cannot swap output constraints"
    );
}
async fn active(pool: &Pool, j: &Journal, r: &Record, format: WireFormat) -> Active {
    let ready = pool.start(init(r, format)).await.unwrap();
    let ticket = j.begin_dispatch(&r.invocation, r.generation, 101).unwrap();
    ready.attach(ticket).unwrap()
}
async fn ignored(_: Decoded) -> worker::Result<()> {
    Ok(())
}

#[tokio::test]
async fn real_json_worker_preserves_values_and_binds_exact_attempt_without_settling() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    let r = record(&j, 1);
    let mut worker = active(&pool, &j, &r, WireFormat::Json).await;
    let body = br#"{"choices":[{"message":{"content":"hello","tool_calls":[{"id":"a","function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"completion_tokens":5}}"#;
    let output = Arc::new(Mutex::new(Vec::new()));
    for chunk in body.chunks(7) {
        worker.push(chunk, ignored).await.unwrap();
    }
    worker
        .finish(|value| {
            let out = output.clone();
            async move {
                out.lock().unwrap().push(value);
                Ok(())
            }
        })
        .await
        .unwrap();
    let outputs = output.lock().unwrap();
    assert_eq!(outputs.len(), 1);
    let Decoded::Json { value } = &outputs[0] else {
        panic!("wrong frame");
    };
    assert_eq!(
        *value,
        serde_json::from_slice::<serde_json::Value>(body).unwrap()
    );
    assert_eq!(
        j.get(&r.invocation).unwrap().unwrap().phase,
        attempts::Phase::Dispatched,
        "framing is not settlement proof"
    );
    assert!(matches!(
        worker.push(b"x", ignored).await,
        Err(worker::Error::Stopped)
    ));
}

#[tokio::test]
async fn sse_and_ndjson_handle_split_utf8_errors_and_incomplete_eof() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(2));
    let r = record(&j, 1);
    let mut stream = active(&pool, &j, &r, WireFormat::Sse).await;
    let count = Arc::new(Mutex::new(Vec::new()));
    let mut emit = |v| {
        let count = count.clone();
        async move {
            count.lock().unwrap().push(v);
            Ok(())
        }
    };
    for b in "event: message\r\ndata: {\"text\":\"café\"}\r\n\r\ndata: [DONE]\n\n".as_bytes() {
        stream.push(&[*b], &mut emit).await.unwrap();
    }
    stream.finish(&mut emit).await.unwrap();
    assert_eq!(count.lock().unwrap().len(), 2);
    let r = record(&j, 2);
    let mut ndjson = active(&pool, &j, &r, WireFormat::Ndjson).await;
    ndjson.push(br#"{"text":"ok"}"#, ignored).await.unwrap();
    ndjson.finish(ignored).await.unwrap();
    let r = record(&j, 3);
    let mut stream = active(&pool, &j, &r, WireFormat::Sse).await;
    stream
        .push(b"data: {\"text\":\"cut off\"}", ignored)
        .await
        .unwrap();
    assert!(
        matches!(stream.finish(ignored).await, Err(worker::Error::Upstream(f)) if f.code == Code::UpstreamProtocol)
    );
    stream.stop().await.unwrap();
    let r = record(&j, 4);
    let mut stream = active(&pool, &j, &r, WireFormat::Sse).await;
    assert!(
        matches!(stream.push(b"data: {\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"secret vendor text\"}}\n\n", ignored).await, Err(worker::Error::Upstream(f)) if f.code == Code::UpstreamRateLimited && !f.to_string().contains("secret"))
    );
}

#[tokio::test]
async fn upstream_error_envelopes_never_become_answers_even_with_large_private_messages() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    for (n, size) in [(1, 30), (2, 20000)] {
        let r = record(&j, n);
        let mut worker = active(&pool, &j, &r, WireFormat::Json).await;
        let body = serde_json::to_vec(
            &serde_json::json!({"error":{"code":"invalid_api_key","message":"x".repeat(size)}}),
        )
        .unwrap();
        worker
            .push(&body, |_| async { panic!("error body emitted as answer") })
            .await
            .unwrap();
        assert!(matches!(
            worker
                .finish(|_| async { panic!("error body emitted as answer") })
                .await,
            Err(worker::Error::Upstream(_))
        ));
        worker.stop().await.unwrap();
    }
    let r = record(&j, 3);
    let mut worker = active(&pool, &j, &r, WireFormat::Json).await;
    worker
        .push(
            br#"{"refusal":"I cannot help with that request."}"#,
            ignored,
        )
        .await
        .unwrap();
    worker.finish(ignored).await.unwrap();
}

#[tokio::test]
async fn cancellation_interrupts_backpressure_and_leaves_other_attempts_running() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(2));
    let r1 = record(&j, 1);
    let mut a = active(&pool, &j, &r1, WireFormat::Sse).await;
    let r2 = record(&j, 2);
    let mut b = active(&pool, &j, &r2, WireFormat::Json).await;
    let cancel = a.cancellation();
    let (started, ready) = tokio::sync::oneshot::channel();
    let mut started = Some(started);
    let call = a.push(b"data: {\"text\":\"one\"}\n\n", move |_| {
        let started = started.take();
        async move {
            let _ = started.unwrap().send(());
            std::future::pending::<worker::Result<()>>().await
        }
    });
    let cancel_call = async {
        ready.await.unwrap();
        cancel.cancel();
    };
    let (result, _) = tokio::join!(call, cancel_call);
    assert!(matches!(result, Err(worker::Error::Cancelled)));
    a.stop().await.unwrap();
    b.push(b"{}", ignored).await.unwrap();
    b.finish(ignored).await.unwrap();
    assert_eq!(
        j.get(&r1.invocation).unwrap().unwrap().phase,
        attempts::Phase::Dispatched
    );
    assert_eq!(j.recovery_page(None, 10).unwrap().records.len(), 2);
    let r3 = record(&j, 3);
    pool.start(init(&r3, WireFormat::Json))
        .await
        .unwrap()
        .stop()
        .await
        .unwrap();
}

#[tokio::test]
async fn dropped_exchange_poison_is_not_reused_and_child_is_reaped() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    let r = record(&j, 1);
    let mut a = active(&pool, &j, &r, WireFormat::Sse).await;
    let result = tokio::time::timeout(
        Duration::from_millis(25),
        a.push(b"data: {}\n\n", |_| {
            std::future::pending::<worker::Result<()>>()
        }),
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(
        a.push(b"data: {}\n\n", ignored).await,
        Err(worker::Error::Stopped)
    ));
    a.stop().await.unwrap();
    let r = record(&j, 2);
    let ready = pool.start(init(&r, WireFormat::Json)).await.unwrap();
    ready.stop().await.unwrap();
}

#[tokio::test]
async fn inference_idle_and_consumer_backpressure_are_not_decoder_timeouts() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(
        work.path(),
        PoolLimits {
            processing_timeout: Duration::from_millis(100),
            ..limits(1)
        },
    );
    let r = record(&j, 1);
    let mut a = active(&pool, &j, &r, WireFormat::Sse).await;
    a.push(b"data: {\"a\":1}\n\n", ignored).await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    a.push(b"data: {\"a\":2}\n\n", |_| async {
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok(())
    })
    .await
    .unwrap();
    a.finish(ignored).await.unwrap();
}

#[tokio::test]
async fn wrong_ticket_release_or_abi_never_activates_a_decoder() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(2));
    let r = record(&j, 1);
    for bad in ["release", "abi"] {
        let mut config = init(&r, WireFormat::Json);
        if bad == "release" {
            config.release = "0.0.0".into();
        } else {
            config.abi += 1;
        }
        assert!(matches!(
            pool.start(config).await,
            Err(worker::Error::Identity)
        ));
    }
    let ready = pool.start(init(&r, WireFormat::Json)).await.unwrap();
    let other = record(&j, 2);
    let ticket = j
        .begin_dispatch(&other.invocation, other.generation, 101)
        .unwrap();
    assert!(matches!(ready.attach(ticket), Err(worker::Error::Identity)));
    assert_eq!(
        j.get(&r.invocation).unwrap().unwrap().phase,
        attempts::Phase::Prepared
    );
}

#[tokio::test]
async fn process_and_buffer_capacity_refuse_before_queueing_or_dispatch() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    let r = record(&j, 1);
    let ready = pool.start(init(&r, WireFormat::Json)).await.unwrap();
    let r2 = record(&j, 2);
    assert!(matches!(
        pool.start(init(&r2, WireFormat::Json)).await,
        Err(worker::Error::Capacity)
    ));
    ready.stop().await.unwrap();
    let pool = Pool::new(
        env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
        work.path(),
        PoolLimits {
            max_buffer_bytes: 64 * 1024,
            ..limits(2)
        },
    )
    .unwrap();
    assert!(matches!(
        pool.start(init(&r2, WireFormat::Json)).await,
        Err(worker::Error::Capacity)
    ));
    assert_eq!(
        j.get(&r2.invocation).unwrap().unwrap().phase,
        attempts::Phase::Prepared
    );
}

#[tokio::test]
async fn byte_limits_invalid_json_and_crashed_worker_return_failures() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    let r = record(&j, 1);
    let mut config = init(&r, WireFormat::Json);
    config.limits.max_event_bytes = 4;
    let ready = pool.start(config).await.unwrap();
    let mut a = ready
        .attach(j.begin_dispatch(&r.invocation, r.generation, 101).unwrap())
        .unwrap();
    assert!(
        matches!(a.push(b"12345", ignored).await, Err(worker::Error::Upstream(f)) if f.code == Code::ResponseTooLarge)
    );
    a.stop().await.unwrap();
    let r = record(&j, 2);
    let mut a = active(&pool, &j, &r, WireFormat::Json).await;
    a.push(b"{broken", ignored).await.unwrap();
    assert!(
        matches!(a.finish(ignored).await, Err(worker::Error::Upstream(f)) if f.code == Code::UpstreamProtocol)
    );
    a.stop().await.unwrap();
    let fixture = store.path().join("exit-worker");
    std::fs::write(&fixture, b"#!/bin/sh\nexit 17\n").unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o700)).unwrap();
    let broken = Pool::new(&fixture, work.path(), limits(1)).unwrap();
    let r = record(&j, 3);
    assert!(matches!(
        broken.start(init(&r, WireFormat::Json)).await,
        Err(worker::Error::Stopped)
    ));
    assert_eq!(
        j.get(&r.invocation).unwrap().unwrap().phase,
        attempts::Phase::Prepared
    );
}

#[tokio::test]
async fn long_stream_is_incremental_and_reports_local_overhead() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    let r = record(&j, 1);
    let start = Instant::now();
    let mut a = active(&pool, &j, &r, WireFormat::Sse).await;
    let startup = start.elapsed();
    let count = Arc::new(Mutex::new(0));
    let text = "data: {\"text\":\"a\"}\n\n".repeat(10000);
    let decode_start = Instant::now();
    let mut emit = |_| {
        let count = count.clone();
        async move {
            *count.lock().unwrap() += 1;
            Ok(())
        }
    };
    for chunk in text.as_bytes().chunks(worker::CHUNK_BYTES) {
        a.push(chunk, &mut emit).await.unwrap();
    }
    a.finish(&mut emit).await.unwrap();
    assert_eq!(*count.lock().unwrap(), 10000);
    eprintln!(
        "decoder startup {:?}; 10000 events / {} bytes decoded in {:?}",
        startup,
        text.len(),
        decode_start.elapsed()
    );
}

#[tokio::test]
async fn a_finished_child_does_not_release_quota_while_consumer_is_blocked() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    let r = record(&j, 1);
    let mut a = active(&pool, &j, &r, WireFormat::Json).await;
    a.push(b"{}", ignored).await.unwrap();
    let next = record(&j, 2);
    a.finish(|_| async {
        // Child can emit its tiny END packet and exit while this callback waits.
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(matches!(
            pool.start(init(&next, WireFormat::Json)).await,
            Err(worker::Error::Capacity)
        ));
        Ok(())
    })
    .await
    .unwrap();
    pool.start(init(&next, WireFormat::Json))
        .await
        .unwrap()
        .stop()
        .await
        .unwrap();
}

#[tokio::test]
async fn warm_short_response_process_and_ipc_timings() {
    let store = dir();
    let work = dir();
    let j = journal(&store.path().join("attempts"));
    let pool = pool(work.path(), limits(1));
    let mut starts = Vec::new();
    let mut parses = Vec::new();
    for n in 1..=12 {
        let r = record(&j, n);
        let start = Instant::now();
        let ready = pool.start(init(&r, WireFormat::Json)).await.unwrap();
        let startup = start.elapsed();
        let ticket = j.begin_dispatch(&r.invocation, r.generation, 101).unwrap();
        let mut active = ready.attach(ticket).unwrap();
        let start = Instant::now();
        active
            .push(
                br#"{"choices":[{"text":"ok","finish_reason":"stop"}]}"#,
                ignored,
            )
            .await
            .unwrap();
        active.finish(ignored).await.unwrap();
        if n > 2 {
            starts.push(startup);
            parses.push(start.elapsed());
        }
    }
    starts.sort();
    parses.sort();
    eprintln!("warm decoder process+handshake median {:?}, max {:?}; short JSON IPC+decode+reap median {:?}, max {:?}", starts[starts.len()/2], starts.last().unwrap(), parses[parses.len()/2], parses.last().unwrap());
}
