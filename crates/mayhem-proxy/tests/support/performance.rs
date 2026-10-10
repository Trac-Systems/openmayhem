//! Opt-in, bounded local performance evidence. This is not a GPU benchmark,
//! payment/ledger acceptance, or a production capacity claim.
use super::*;
use serde::{Deserialize, Serialize};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, time::Instant};

const FIXTURE: &str = "proxy-http-durable-perf-v1";
const QUESTIONS: usize = 128;
const STREAM_ROUNDS: usize = 3;
const DECISION_ROUNDS: usize = 8;

#[derive(Clone, Serialize, Deserialize)]
struct Sample {
    /// Synchronous request/acceptance retention before Executor dispatch.
    prepare_ms: f64,
    first_ms: f64,
    last_ms: f64,
    complete_ms: f64,
    units: usize,
    output_bytes: usize,
}
#[derive(Default)]
struct Content {
    first: Option<Duration>,
    last: Duration,
    count: usize,
    bytes: usize,
    hash: blake3::Hasher,
}
impl Content {
    fn push(&mut self, value: &Value, start: Instant) {
        if let Some(text) = value["choices"][0]["delta"]["content"].as_str() {
            if !text.is_empty() {
                let at = start.elapsed();
                self.first.get_or_insert(at);
                self.last = at;
                self.count += 1;
                self.bytes += text.len();
                self.hash.update(text.as_bytes());
            }
        }
    }
    fn finish(&self, start: Instant, expected: &str, events: usize) -> Sample {
        assert_eq!(self.count, events);
        assert_eq!(self.bytes, expected.len());
        assert_eq!(self.hash.finalize(), blake3::hash(expected.as_bytes()));
        Sample {
            prepare_ms: 0.,
            first_ms: ms(self.first.unwrap()),
            last_ms: ms(self.last),
            complete_ms: ms(start.elapsed()),
            units: events,
            output_bytes: self.bytes,
        }
    }
}
fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

// Accept concurrently; pacing is the same for direct and proxied consumers.
// Children belong to this task's JoinSet, so dropping Backend cancels them too.
async fn paced_backend(pieces: Vec<Vec<u8>>, mime: &'static str) -> Backend {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let pieces = Arc::new(pieces);
    let handle = tokio::spawn(async move {
        let mut children = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                connected = listener.accept() => {
                    let (mut socket, _) = connected.unwrap();
                    socket.set_nodelay(true).unwrap();
                    let pieces = pieces.clone();
                    let count = count.clone();
                    children.spawn(async move {
                        let mut bytes = Vec::new();
                        let mut buf = [0; 8192];
                        let boundary = loop {
                            let n = socket.read(&mut buf).await.unwrap();
                            if n == 0 { return; }
                            bytes.extend_from_slice(&buf[..n]);
                            assert!(bytes.len() <= 1024 * 1024);
                            if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") { break i + 4; }
                        };
                        let length = String::from_utf8_lossy(&bytes[..boundary]).lines()
                            .find_map(|l| l.to_lowercase().strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())).unwrap();
                        assert!(length <= 1024 * 1024);
                        while bytes.len() < boundary + length {
                            let n = socket.read(&mut buf).await.unwrap();
                            assert!(n > 0);
                            bytes.extend_from_slice(&buf[..n]);
                        }
                        let request: Value = serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap();
                        assert_eq!(request["model"], "upstream-model");
                        assert!(count.fetch_add(1, Ordering::SeqCst) < 64);
                        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", pieces.iter().map(Vec::len).sum::<usize>());
                        let at = tokio::time::Instant::now() + Duration::from_millis(40);
                        tokio::time::sleep_until(at).await;
                        socket.write_all(header.as_bytes()).await.unwrap();
                        for (i, piece) in pieces.iter().enumerate() {
                            tokio::time::sleep_until(at + Duration::from_millis(i as u64 * 8)).await;
                            socket.write_all(piece).await.unwrap();
                        }
                    });
                }
                result = children.join_next(), if !children.is_empty() => { result.unwrap().unwrap(); }
            }
        }
    });
    Backend {
        base,
        calls,
        bodies: Arc::new(Mutex::new(Vec::new())),
        handle,
    }
}
fn fixture(base: &str, endpoint: ProxyEndpoint) -> Fixture {
    let config = serde_json::from_value::<ConnectionConfig>(json!({
        "schema_version":1,"id":"perf","revision":1,"base_url":base,
        "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},
        "paths":{"chat_completions":"chat/completions","decisions":"decisions"},"error_profile":"open_ai"
    })).unwrap();
    Fixture::with_connection_limits(
        config,
        endpoint,
        32 * 1024 * 1024,
        "upstream-model".into(),
        Limits {
            request_bytes: 1024 * 1024,
            response_bytes: 1024 * 1024,
            choices: 8,
            tools: 16,
            questions: QUESTIONS,
            decision_options: 32,
        },
    )
}
fn public_body(stream: bool) -> Vec<u8> {
    let mut value = if stream {
        json!({"model":"public-model","messages":[{"role":"user","content":"Describe this synthetic document. ".repeat(1024)}],"stream":true})
    } else {
        let questions: serde_json::Map<String, Value> = (0..QUESTIONS).map(|n| (format!("q{n:03}"),
            json!({"type":"choice","instructions":format!("Choose the disposition for record {n}."),"criteria":{"accept":"valid","reject":"invalid"}}))).collect();
        json!({"model":"public-model","state":"Synthetic records for local protocol measurement. ".repeat(256),"questions":questions})
    };
    value["model"] = json!("public-model");
    serde_json::to_vec(&value).unwrap()
}
fn answers() -> Value {
    let values: serde_json::Map<String, Value> = (0..QUESTIONS).map(|n| (format!("q{n:03}"),
        if n % 2 == 0 { json!({"type":"choice","choice":"accept","probabilities":{"accept":0.8,"reject":0.2}}) }
        else { json!({"type":"choice","choice":"reject","probabilities":{"accept":0.3,"reject":0.7}}) })).collect();
    json!({"id":"decision-fixture","answers":values})
}
fn stream_pieces(events: usize) -> (Vec<Vec<u8>>, String) {
    let mut full = String::new();
    let mut chunks = Vec::new();
    for i in 0..events {
        let text = format!("{i:06}: {}\n", "synthetic-output ".repeat(3));
        full.push_str(&text);
        chunks.push(delta(&text, Value::Null));
    }
    let mut pieces: Vec<Vec<u8>> = chunks.chunks(64).map(|v| sse(v, false)).collect();
    pieces.push(sse(&[delta("", json!("stop"))], true));
    (pieces, full)
}
async fn direct(
    start: Instant,
    client: &reqwest::Client,
    base: &str,
    body: &[u8],
    expected: &str,
    events: usize,
) -> Sample {
    let mut request: Value = serde_json::from_slice(body).unwrap();
    request["model"] = json!("upstream-model");
    let path = if events > 0 {
        "chat/completions"
    } else {
        "decisions"
    };
    let mut reply = client
        .post(format!("{base}{path}"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 200);
    if events == 0 {
        let value: Value = reply.json().await.unwrap();
        assert_eq!(value["answers"], answers()["answers"]);
        let complete_ms = ms(start.elapsed());
        return Sample {
            prepare_ms: 0.,
            first_ms: complete_ms,
            last_ms: complete_ms,
            complete_ms,
            units: QUESTIONS,
            output_bytes: serde_json::to_vec(&value["answers"]).unwrap().len(),
        };
    }
    let mut content = Content::default();
    let mut pending = Vec::new();
    let mut done = false;
    while let Some(bytes) = reply.chunk().await.unwrap() {
        pending.extend_from_slice(&bytes);
        assert!(pending.len() < 2 * 1024 * 1024);
        let mut consumed = 0;
        while let Some(end) = pending[consumed..].windows(2).position(|p| p == b"\n\n") {
            let frame = &pending[consumed..consumed + end];
            let data = frame.strip_prefix(b"data: ").unwrap();
            if data == b"[DONE]" {
                done = true;
            } else {
                content.push(&serde_json::from_slice::<Value>(data).unwrap(), start);
            }
            consumed += end + 2;
        }
        pending.drain(..consumed);
    }
    assert!(done && pending.is_empty());
    content.finish(start, expected, events)
}
async fn proxied(
    start: Instant,
    f: &Fixture,
    n: u64,
    body: &[u8],
    expected: &str,
    events: usize,
) -> Sample {
    let prepare_start = Instant::now();
    let record = f.prepare_kind(n, body, ProxyRail::Fiat, events > 0);
    let prepare_ms = ms(prepare_start.elapsed());
    let mut content = Content::default();
    let result = if events > 0 {
        f.executor
            .execute_stream(&record.invocation, body, &Cancellation::default(), |v| {
                content.push(&v, start);
                async { Ok(()) }
            })
            .await
            .unwrap()
    } else {
        f.executor
            .execute_json(&record.invocation, body, &Cancellation::default())
            .await
            .unwrap()
    };
    let mut sample = if events > 0 {
        assert_eq!(
            result.reply.body["choices"][0]["message"]["content"],
            expected
        );
        content.finish(start, expected, events)
    } else {
        assert_eq!(result.reply.body["answers"], answers()["answers"]);
        let complete_ms = ms(start.elapsed());
        Sample {
            prepare_ms,
            first_ms: complete_ms,
            last_ms: complete_ms,
            complete_ms,
            units: QUESTIONS,
            output_bytes: serde_json::to_vec(&result.reply.body["answers"])
                .unwrap()
                .len(),
        }
    };
    sample.prepare_ms = prepare_ms;
    assert_eq!(
        result.reply.observed_usage.as_ref().unwrap().disposition,
        mayhem_proxy::metering::Disposition::Complete
    );
    let retained = f
        .journal
        .recover(&record.invocation, record.attempt)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(retained.digest, result.result_digest);
    assert_eq!(retained.reply.body, result.reply.body);
    sample
}
fn stats(samples: &[Sample]) -> Value {
    let percentile = |read: fn(&Sample) -> f64, p: f64| {
        let mut v: Vec<_> = samples.iter().map(read).collect();
        v.sort_by(f64::total_cmp);
        v[((v.len() as f64 * p).ceil() as usize).saturating_sub(1)]
    };
    let pair_span: f64 = samples
        .chunks_exact(2)
        .map(|p| p[0].complete_ms.max(p[1].complete_ms))
        .sum();
    let pair_skew = samples
        .chunks_exact(2)
        .map(|p| (p[0].complete_ms - p[1].complete_ms).abs())
        .fold(0., f64::max);
    json!({"samples":samples,"requests":samples.len(),"concurrency":2,
        "median_first_ms":percentile(|v|v.first_ms,0.5),"p95_first_ms":percentile(|v|v.first_ms,0.95),
        "median_complete_ms":percentile(|v|v.complete_ms,0.5),"p95_complete_ms":percentile(|v|v.complete_ms,0.95),
        "max_pair_completion_skew_ms":pair_skew,
        "output_units_per_second":samples.iter().map(|s|s.units).sum::<usize>() as f64 * 1000. / pair_span})
}
fn write_report(path: &Path, value: &Value) {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string_pretty(value).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in local timing; measure baseline and fix explicit envelope before comparing"]
async fn local_http_durable_performance() {
    let phase = std::env::var("MAYHEM_PROXY_PERF_PHASE").expect("baseline or compare");
    assert!(matches!(phase.as_str(), "baseline" | "compare"));
    // Read the prechosen budgets before dispatch, never derive them from the candidate.
    let envelope: Option<Value> = if phase == "compare" {
        Some(
            serde_json::from_slice(
                &std::fs::read(
                    std::env::var_os("MAYHEM_PROXY_PERF_ENVELOPE")
                        .expect("baseline-derived envelope path"),
                )
                .unwrap(),
            )
            .unwrap(),
        )
    } else {
        None
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30)) // Fixture fails instead of hanging on a server bug.
        .build()
        .unwrap();
    let mut cases = serde_json::Map::new();
    for events in [2048, 4096, 0] {
        let name = if events == 0 {
            "decisions_128".into()
        } else {
            format!("stream_{events}")
        };
        let (pieces, expected, mime, rounds) = if events > 0 {
            let (p, e) = stream_pieces(events);
            (p, e, "text/event-stream", STREAM_ROUNDS)
        } else {
            (
                vec![serde_json::to_vec(&answers()).unwrap()],
                String::new(),
                "application/json",
                DECISION_ROUNDS,
            )
        };
        let backend = paced_backend(pieces, mime).await;
        let endpoint = if events > 0 {
            ProxyEndpoint::Chat
        } else {
            ProxyEndpoint::Decisions
        };
        let body = public_body(events > 0);
        let f = fixture(&backend.base, endpoint);
        let mut direct_samples = Vec::new();
        let mut proxy_samples = Vec::new();
        for round in 0..rounds {
            // Include scheduler delay for both members of the simultaneous pair.
            // Separate per-call clocks would undercount the second caller's wait.
            let start = Instant::now();
            let (a, b) = tokio::join!(
                direct(start, &client, &backend.base, &body, &expected, events),
                direct(start, &client, &backend.base, &body, &expected, events)
            );
            direct_samples.extend([a, b]);
            if phase == "compare" {
                let start = Instant::now();
                let (a, b) = tokio::join!(
                    proxied(start, &f, (2 * round + 1) as u64, &body, &expected, events),
                    proxied(start, &f, (2 * round + 2) as u64, &body, &expected, events)
                );
                proxy_samples.extend([a, b]);
            }
        }
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            rounds * 2 * if phase == "compare" { 2 } else { 1 }
        );
        let mut case = json!({"direct":stats(&direct_samples),"request_bytes":body.len(),"upstream_calls":backend.calls.load(Ordering::SeqCst)});
        if !proxy_samples.is_empty() {
            case["proxy"] = stats(&proxy_samples);
        }
        cases.insert(name, case);
    }
    let mut report = json!({"fixture":FIXTURE,"phase":phase,"debug_assertions":cfg!(debug_assertions),
        "clock":"common simultaneous-pair start, includes both callers' scheduling delay",
        "scope":"loopback HTTP, isolated decoder, validation, independent metering, request/acceptance/result journal; no external inference, paid ledger, buyer gateway or API/site overhead",
        "units":"stream units are synthetic content events, decisions are questions; neither is native model tokens",
        "cases":cases});
    let mut failures = Vec::new();
    if let Some(envelope) = envelope {
        assert_eq!(envelope["fixture"], FIXTURE);
        for (name, case) in report["cases"].as_object().unwrap() {
            let budget = &envelope["cases"][name];
            for metric in [
                "p95_first_ms",
                "p95_complete_ms",
                "max_pair_completion_skew_ms",
            ] {
                let allowed = budget[metric]
                    .as_f64()
                    .expect("explicit finite positive numeric budget");
                assert!(allowed.is_finite() && allowed > 0.);
                let actual = case["proxy"][metric].as_f64().unwrap();
                if actual > allowed {
                    failures.push(format!("{name}/{metric}: {actual:.3} > {allowed:.3}"));
                }
            }
            let direct_max = budget["direct_p95_complete_max_ms"].as_f64().unwrap();
            if case["direct"]["p95_complete_ms"].as_f64().unwrap() > direct_max {
                failures.push(format!("{name}: direct control exceeds predeclared environmental variance; no candidate acceptance"));
            }
        }
        report["envelope"] = envelope;
        report["failures"] = json!(failures);
    }
    let path = std::env::var_os("MAYHEM_PROXY_PERF_REPORT").expect("new report path");
    write_report(Path::new(&path), &report);
    println!(
        "{}",
        json!({"fixture":FIXTURE,"phase":phase,"failures":failures})
    );
    assert!(
        failures.is_empty(),
        "performance evidence failed; keep the report, do not silently relax the envelope"
    );
}
