use super::*;
fn recipe(endpoint: ProxyEndpoint, lookup: bool) -> mayhem_proxy::recipe::Signed {
    let mut r = recipe_fixture::recipe(endpoint).recipe;
    r.abi_min = 2;
    r.abi_max = 2;
    r.job=Some(serde_json::from_value(json!({"submit_status":202,"job_id_path":["job"],"poll_interval_ms":1000,"control_timeout_ms":1000,"status_path":["state"],"statuses":{"wait":"pending","ok":"ready","gone":"cancelled","bad":"failed","absent":"missing"},"poll":{"job_id_field":["job"],"response_id_path":["job"]},"result":{"job_id_field":["job"],"response_id_path":null},"cancel":{"control":{"job_id_field":["job"],"response_id_path":["job"]},"idempotent":false},"lookup":if lookup{json!({"submit_key_field":["original"],"lookup_key_field":["original"],"response_key_path":["original"]})}else{Value::Null}})).unwrap());
    recipe_fixture::sign(r)
}
fn fixture(base: &str, recipe: mayhem_proxy::recipe::Signed) -> Fixture {
    let mut f = custom(base, recipe_fixture::recipe(recipe.recipe.endpoint));
    f.adapter = Arc::new(
        Adapter::restore(f.adapter.snapshot())
            .unwrap()
            .with_recipe(recipe)
            .unwrap(),
    );
    f.connection=Arc::new(HttpConnection::new(serde_json::from_value::<ConnectionConfig>(json!({"schema_version":1,"id":"fixture","revision":1,"base_url":base,"network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},"paths":{"chat_completions":"submit","completions":"submit","responses":"submit","decisions":"submit","job_poll":"poll","job_result":"result","job_cancel":"cancel","job_lookup":"lookup"},"error_profile":"http_status"})).unwrap()).unwrap());
    f.executor = executor(&f, f.adapter.clone());
    f
}
fn executor(f: &Fixture, adapter: Arc<Adapter>) -> Executor {
    Executor::new(
        f.connection.clone(),
        adapter,
        Arc::new(
            Pool::new(
                env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
                f._work.path(),
                PoolLimits {
                    max_children: 2,
                    max_buffer_bytes: 64 * 1024 * 1024,
                    startup_timeout: Duration::from_secs(5),
                    processing_timeout: Duration::from_secs(3),
                },
            )
            .unwrap(),
        ),
        Arc::new(Storage::new(f.journal.clone(), 8).unwrap()),
    )
    .unwrap()
}
struct Server {
    base: String,
    seen: Arc<Mutex<Vec<(String, Value)>>>,
    pending: Arc<std::sync::atomic::AtomicBool>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    handle: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.handle.abort()
    }
}
impl Server {
    fn calls(&self, path: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .count()
    }
}
async fn server(result: Value, lose_ack: bool, cancelled: bool) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let received = seen.clone();
    let pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiting = pending.clone();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(cancelled));
    let stopped = cancelled.clone();
    let key = Arc::new(Mutex::new(Value::Null));
    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let seen = received.clone();
            let pending = waiting.clone();
            let cancelled = stopped.clone();
            let result = result.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                let boundary = loop {
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    assert!(bytes.len() < 2 * 1024 * 1024);
                    if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let header = String::from_utf8_lossy(&bytes[..boundary]);
                let path = header
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let length = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                while bytes.len() < boundary + length {
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let body: Value =
                    serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap();
                seen.lock().unwrap().push((path.clone(), body.clone()));
                let (status, value) = match path.as_str() {
                    "/submit" => {
                        *key.lock().unwrap() = body["original"].clone();
                        if lose_ack {
                            return;
                        }
                        (202, json!({"job":"original-job"}))
                    }
                    "/lookup" => {
                        assert_eq!(body["original"], *key.lock().unwrap());
                        (
                            200,
                            json!({"original":body["original"],"job":"original-job","state":"ok"}),
                        )
                    }
                    "/poll" => {
                        assert_eq!(body, json!({"job":"original-job"}));
                        (
                            200,
                            json!({"job":"original-job","state":if cancelled.load(Ordering::SeqCst){"gone"}else if pending.load(Ordering::SeqCst){"wait"}else{"ok"}}),
                        )
                    }
                    "/cancel" => {
                        assert_eq!(body, json!({"job":"original-job"}));
                        (200, json!({"job":"original-job","ack":true}))
                    }
                    "/result" => {
                        assert_eq!(body, json!({"job":"original-job"}));
                        (200, result)
                    }
                    _ => panic!("unexpected operation"),
                };
                let body = serde_json::to_vec(&value).unwrap();
                let header=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                let _ = socket.write_all(header.as_bytes()).await;
                for c in body.chunks(7) {
                    if socket.write_all(c).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    Server {
        base,
        seen,
        pending,
        cancelled,
        handle,
    }
}
#[tokio::test]
async fn async_four_endpoints_use_exact_job_id_worker_and_original_metering() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let recipe = recipe(endpoint, false);
        let sample = recipe.recipe.fixtures[0].clone();
        let server = server(sample.upstream_response.clone(), false, false).await;
        let f = fixture(&server.base, recipe);
        let bytes = serde_json::to_vec(&sample.request).unwrap();
        let record = f.prepare(930, &bytes, ProxyRail::Tnk);
        let result = f
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap();
        let expected = recipe_fixture::base(endpoint)
            .prepare_json(&bytes)
            .unwrap()
            .decode_json(
                sample.normalized_response,
                &format!("proxy_{}", record.invocation.as_str()),
                record.created_at_ms / 1000,
            )
            .unwrap();
        assert_eq!(result.reply.body, expected.body);
        assert_eq!(
            serde_json::to_value(result.reply.observed_usage).unwrap(),
            serde_json::to_value(expected.observed_usage).unwrap()
        );
        assert_eq!(result.reply.upstream_id.unwrap().as_str(), "original-job");
        assert_eq!(server.calls("/submit"), 1);
        assert_eq!(server.calls("/poll"), 1);
        assert_eq!(server.calls("/result"), 1);
        assert!(f
            .executor
            .resume_job(&record.invocation, record.attempt)
            .await
            .unwrap());
        assert_eq!(server.calls("/submit"), 1);
        assert!(result.attempt.closure.is_none());
    }
}
#[tokio::test]
async fn async_lost_submit_ack_lookup_and_concurrent_recovery_keep_original_recipe() {
    let recipe = recipe(ProxyEndpoint::Chat, true);
    let sample = recipe.recipe.fixtures[0].clone();
    let server = server(sample.upstream_response, true, false).await;
    let f = fixture(&server.base, recipe.clone());
    let bytes = serde_json::to_vec(&sample.request).unwrap();
    let record = f.prepare(931, &bytes, ProxyRail::Fiat);
    assert!(f
        .executor
        .execute_json(&record.invocation, &bytes, &Cancellation::default())
        .await
        .is_err());
    assert_eq!(server.calls("/submit"), 1);
    tokio::time::sleep(Duration::from_millis(6100)).await;
    let mut revised = recipe.recipe;
    revised.revision += 1;
    let changed = Arc::new(
        Adapter::restore(f.adapter.snapshot())
            .unwrap()
            .with_recipe(recipe_fixture::sign(revised))
            .unwrap(),
    );
    assert_ne!(changed.recipe_hash(), f.adapter.recipe_hash());
    let recovery = executor(&f, changed);
    let (a, b) = tokio::join!(
        recovery.resume_job(&record.invocation, record.attempt),
        recovery.resume_job(&record.invocation, record.attempt)
    );
    assert!(a.unwrap() || b.unwrap());
    assert_eq!(server.calls("/submit"), 1);
    assert_eq!(server.calls("/lookup"), 1);
    assert_eq!(server.calls("/result"), 1);
    let retained = f
        .journal
        .recover(&record.invocation, record.attempt)
        .unwrap();
    assert!(retained.result.is_some());
    assert_eq!(
        retained.record.binding.recipe_digest,
        *f.adapter.recipe_hash()
    );
    assert!(retained.record.closure.is_none());
}
#[tokio::test]
async fn async_lost_ack_without_explicit_lookup_never_resubmits() {
    let recipe = recipe(ProxyEndpoint::Chat, false);
    let sample = recipe.recipe.fixtures[0].clone();
    let server = server(sample.upstream_response, true, false).await;
    let f = fixture(&server.base, recipe);
    let bytes = serde_json::to_vec(&sample.request).unwrap();
    let record = f.prepare(932, &bytes, ProxyRail::Fiat);
    assert!(f
        .executor
        .execute_json(&record.invocation, &bytes, &Cancellation::default())
        .await
        .is_err());
    tokio::time::sleep(Duration::from_millis(6100)).await;
    assert!(matches!(
        f.executor
            .resume_job(&record.invocation, record.attempt)
            .await,
        Err(Error::RecoveryRequired)
    ));
    assert_eq!(server.calls("/submit"), 1);
    assert_eq!(server.calls("/lookup"), 0);
    assert!(f
        .journal
        .recover(&record.invocation, record.attempt)
        .unwrap()
        .result
        .is_none());
}
#[tokio::test]
async fn async_cancel_ack_does_not_settle_or_release_but_ready_original_result_is_recoverable() {
    for cancelled in [false, true] {
        let recipe = recipe(ProxyEndpoint::Chat, false);
        let sample = recipe.recipe.fixtures[0].clone();
        let server = server(sample.upstream_response, false, cancelled).await;
        server.pending.store(true, Ordering::SeqCst);
        let f = fixture(&server.base, recipe);
        let bytes = serde_json::to_vec(&sample.request).unwrap();
        let record = f.prepare(933, &bytes, ProxyRail::Tap);
        let cancel = Cancellation::default();
        let wait = async {
            while server.calls("/poll") == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            cancel.cancel();
        };
        let (result, _) = tokio::join!(
            f.executor.execute_json(&record.invocation, &bytes, &cancel),
            wait
        );
        assert!(result.is_err());
        // Ensure the explicit durable user cancellation, including when terminal remote
        // status won the race with local notification.
        let current = f.journal.get(&record.invocation).unwrap().unwrap();
        f.journal
            .advance(
                &record.invocation,
                current.generation,
                attempts::Event::CancelRequested,
                current.updated_at_ms + 1,
            )
            .unwrap();
        server.pending.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let result = f
            .executor
            .resume_job(&record.invocation, record.attempt)
            .await;
        if !cancelled {
            assert!(result.unwrap());
            assert_eq!(server.calls("/cancel"), 1);
        } else {
            assert!(!result.unwrap_or(false));
        }
        let retained = f
            .journal
            .recover(&record.invocation, record.attempt)
            .unwrap();
        assert_eq!(retained.result.is_some(), !cancelled);
        assert!(retained.record.closure.is_none());
        assert!(retained.record.resolution.is_none());
        assert_eq!(server.calls("/submit"), 1);
        if cancelled {
            server.cancelled.store(false, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1100)).await;
            assert!(f
                .executor
                .resume_job(&record.invocation, record.attempt)
                .await
                .unwrap());
            assert_eq!(server.calls("/cancel"), 1);
            assert!(f
                .journal
                .recover(&record.invocation, record.attempt)
                .unwrap()
                .record
                .closure
                .is_none());
        }
    }
}

#[tokio::test]
async fn async_setup_probe_uses_same_worker_and_consumes_one_explicit_allowance() {
    let recipe = recipe(ProxyEndpoint::Chat, false);
    let sample = recipe.recipe.fixtures[0].clone();
    let server = server(sample.upstream_response, false, false).await;
    let f = fixture(&server.base, recipe);
    let (authority, monitor) = probe_execution::setup(&f);
    let body = probe_execution::bounded(sample.request, ProxyEndpoint::Chat);
    let controller = probe_execution::controller(
        &f,
        authority.clone(),
        monitor,
        &body,
        false,
        Duration::from_secs(5),
    )
    .unwrap();
    controller.run().await.unwrap();
    assert_eq!(server.calls("/submit"), 1);
    assert_eq!(server.calls("/result"), 1);
    assert_eq!(authority.group_status(&d(10)).unwrap().occupied, 0);
    assert!(f
        .journal
        .recovery_page(None, 10)
        .unwrap()
        .records
        .is_empty());
}
#[tokio::test]
async fn async_invalid_result_fails_worker_without_financial_closure() {
    let recipe = recipe(ProxyEndpoint::Chat, false);
    let sample = recipe.recipe.fixtures[0].clone();
    let server = server(tool_response(&recipe, "{\"key\":1}"), false, false).await;
    let f = fixture(&server.base, recipe.clone());
    let bytes = serde_json::to_vec(&tool_request(&recipe)).unwrap();
    let record = f.prepare(935, &bytes, ProxyRail::Fiat);
    assert!(f
        .executor
        .execute_json(&record.invocation, &bytes, &Cancellation::default())
        .await
        .is_err());
    let retained = f
        .journal
        .recover(&record.invocation, record.attempt)
        .unwrap();
    assert!(retained.result.is_none());
    assert!(retained.record.resolution.is_none());
    assert_eq!(server.calls("/submit"), 1);
    drop(sample);
}

#[test]
fn async_capabilities_bounds_and_offline_export_are_explicit() {
    let signed = recipe(ProxyEndpoint::Chat, true);
    let raw = serde_json::to_value(&signed.recipe).unwrap();
    for (pointer, value) in [
        ("/abi_min", json!(1)),
        ("/job/submit_status", json!(204)),
        ("/job/poll_interval_ms", json!(0)),
        ("/job/control_timeout_ms", json!(60_001)),
        ("/job/job_id_path", json!([])),
        ("/job/poll/job_id_field", json!(["__proto__"])),
        ("/job/statuses", json!({"pending":"pending"})),
    ] {
        let mut bad = raw.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        let recipe: mayhem_proxy::recipe::Recipe = serde_json::from_value(bad).unwrap();
        assert!(recipe.signing_bytes().is_err());
    }
    let mut bad = raw;
    bad["job"]["url"] = json!("http://arbitrary.invalid");
    assert!(serde_json::from_value::<mayhem_proxy::recipe::Recipe>(bad).is_err());
    if let Ok(path) = std::env::var("PROXY_RECIPE_V2_FIXTURE_DIR") {
        std::fs::write(
            std::path::Path::new(&path).join("Chat-async.json"),
            signed.export().unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn async_lookup_binding_expansion_respects_original_request_byte_bound_before_dispatch() {
    let recipe = recipe(ProxyEndpoint::Chat, true);
    let sample = recipe.recipe.fixtures[0].clone();
    let server = server(sample.upstream_response, false, false).await;
    let mut f = fixture(&server.base, recipe);
    let bytes = serde_json::to_vec(&sample.request).unwrap();
    let mut snapshot = f.adapter.snapshot();
    snapshot.limits.request_bytes = bytes.len() + 20;
    f.adapter = Arc::new(Adapter::restore(snapshot).unwrap());
    f.executor = executor(&f, f.adapter.clone());
    let record = f.prepare(939, &bytes, ProxyRail::Fiat);
    assert!(matches!(
        f.executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await,
        Err(Error::Configuration)
    ));
    assert_eq!(server.calls("/submit"), 0);
    assert_eq!(
        f.journal.get(&record.invocation).unwrap().unwrap().phase,
        Phase::Prepared
    );
}
