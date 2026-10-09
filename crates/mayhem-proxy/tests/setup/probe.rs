use super::*;
use mayhem_proxy::{
    capacity,
    setup::{ProbePlan, ProbeState},
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) struct Backend {
    pub(super) calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(super) fn backend(
    f: &mut Fixture,
    status: u16,
    body: Vec<u8>,
    streaming: bool,
    delay: Duration,
) -> Backend {
    // Synthetic credential-free loopback connection; no secret is ever created.
    f.connection
        .as_object_mut()
        .unwrap()
        .remove("authentication");
    f.connection["error_profile"] = json!("open_ai");
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    let listener = tokio::net::TcpListener::from_std(f.listener.try_clone().unwrap()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 1024];
            let boundary = loop {
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break None;
                }
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 64 * 1024);
                if let Some(i) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    break Some(i + 4);
                }
            };
            let Some(boundary) = boundary else { continue };
            let length = String::from_utf8_lossy(&bytes[..boundary])
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap();
            while bytes.len() < boundary + length {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let request: Value =
                serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap();
            assert_eq!(request["model"], "private-upstream-model");
            assert!(!String::from_utf8_lossy(&bytes[..boundary])
                .to_lowercase()
                .contains("authorization:"));
            count.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            let content = if streaming {
                "text/event-stream"
            } else {
                "application/json"
            };
            let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {content}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(&body).await;
            }
        }
    });
    Backend { calls, task }
}
fn chat() -> Value {
    json!({"model":"public-model","messages":[{"role":"user","content":"synthetic probe only"}],"max_tokens":16})
}
fn answer() -> Value {
    json!({"id":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]})
}
pub(super) fn plan(f: &Fixture, request: Value) -> ProbePlan {
    let work = f.dir.path().join("worker");
    if !work.exists() {
        std::fs::create_dir(&work).unwrap();
        std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    serde_json::from_value(json!({"schema_version":1,
        "scope":{"capacity_file":f.dir.path().join("capacity.redb"),"route":d(50),"connection_group":d(2),"connection_ceiling":2,"route_ceiling":2,"constraints":[{"id":d(51),"ceiling":2}]},
        "budget":{"max_attempts":2,"max_cost_microusd":20,"per_attempt_cost_microusd":10},
        "worker_program":env!("CARGO_BIN_EXE_mayhem-proxy-worker"),"worker_directory":work,
        "request":request,"streaming":false,"max_output_tokens":16,"timeout_ms":2000})).unwrap()
}
fn checked(f: &Fixture) -> u64 {
    let created = f.store().create(f.input.clone()).unwrap();
    f.store().check(created.revision).unwrap().revision
}
fn authority(f: &Fixture) -> capacity::Authority {
    capacity::Authority::open(
        f.dir.path().join("capacity.redb"),
        mayhem_proxy::attempts::Identity {
            network_id: f.input.network.network_id.clone(),
            msb_bootstrap: d(3),
            subnet_bootstrap: d(4),
            controller_pubkey: d(1),
        },
        capacity::Limits {
            max_groups: 1024,
            max_routes: 4096,
            max_leases: 65536,
            max_evidence_age: Duration::from_secs(60),
        },
    )
    .unwrap()
}
fn rebind_health(a: &capacity::Authority) {
    let monitor = mayhem_proxy::health::Monitor::new(serde_json::from_value(json!({
        "max_routes":1,"max_classes_per_route":8,"evidence_ttl_ms":60000,
        "successes_to_increase":2,"bad_samples_to_reduce":2,"latency_baseline_samples":3,
        "latency_multiplier":4,"latency_increase_ms":1000,"min_native_tok_s":5,
        "recovery":{"interval_ms":10000,"page_pause_ms":1,"retry_initial_ms":10,"retry_max_ms":100,"jitter_percent":0}
    })).unwrap(),2,1).unwrap();
    monitor.register(d(50), 2, true).unwrap();
    a.bind_live(capacity::Scope::Group(d(2)), monitor.connection_source())
        .unwrap();
    a.bind_live(
        capacity::Scope::Route(d(50)),
        monitor.route_source(&d(50)).unwrap(),
    )
    .unwrap();
}
fn redacted(review: &mayhem_proxy::setup::Review) {
    let s = serde_json::to_string(review).unwrap();
    for private in [
        "synthetic probe only",
        "private-upstream-model",
        "capacity.redb",
        "worker_program",
        "worker_directory",
        "connection_digest",
        "allocated_cost",
        "per_attempt_cost",
        "request_hash",
        "fixture-private-connection",
        "base_url",
        "configuration_binding",
    ] {
        assert!(!s.contains(private), "private field leaked: {private}");
    }
    assert_eq!(review.admission_status, "not_checked");
    assert_eq!(review.publication_status, "not_submitted");
    assert_eq!(review.serving_status, "not_started");
    assert_eq!(
        review.probe.as_ref().unwrap().native_throughput,
        "not_verified"
    );
}

#[tokio::test]
async fn four_families_run_real_bounded_decoder_probes_with_redacted_bound_reports() {
    let mut reviews = Vec::new();
    for (endpoint, request, reply) in [
        (ProxyEndpoint::Chat, chat(), answer()),
        (
            ProxyEndpoint::Completions,
            json!({"model":"public-model","prompt":"synthetic probe only","max_tokens":16}),
            json!({"id":"u","choices":[{"index":0,"text":"hello","finish_reason":"stop"}]}),
        ),
        (
            ProxyEndpoint::Responses,
            json!({"model":"public-model","input":"synthetic probe only","max_output_tokens":16}),
            json!({"id":"u","status":"completed","output":[{"id":"i","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}]}),
        ),
        (
            ProxyEndpoint::Decisions,
            json!({"model":"public-model","state":"synthetic probe only","questions":{"q":{"type":"noul","instructions":"hello?"}}}),
            json!({"id":"u","answers":{"q":{"type":"noul","noul":0.7}}}),
        ),
    ] {
        let mut f = Fixture::new(endpoint);
        let b = backend(
            &mut f,
            200,
            serde_json::to_vec(&reply).unwrap(),
            false,
            Duration::ZERO,
        );
        let rev = checked(&f);
        let p = plan(&f, request);
        let review = f.store().probe(rev, p).await.unwrap();
        assert_eq!(review.revision, rev + 2);
        assert_eq!(review.probe_status, "protocol_validated");
        let report = review.probe.as_ref().unwrap();
        assert!(
            report.for_current_configuration
                && report.probe_id.is_some()
                && report.evidence_hash.is_some()
        );
        redacted(&review);
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        let a = authority(&f);
        let budget = a.probe_budget(&d(2)).unwrap().unwrap();
        assert_eq!(
            (budget.used_attempts, budget.allocated_cost_microusd),
            (1, 10)
        );
        assert_eq!(
            budget.last_completed.unwrap().evidence,
            report.evidence_hash.clone().unwrap()
        );
        assert_eq!(a.group_status(&d(2)).unwrap().occupied, 0);
        assert_eq!(a.group_status(&d(51)).unwrap().occupied, 0);
        assert!(!f.dir.path().join("never-read-secret").exists());
        let mut commercial = f.input.clone();
        commercial.sequence += 1;
        commercial.membership.revision += 1;
        commercial.membership.accepted_rails = vec![ProxyRail::Fiat];
        commercial.settlement_policy.allow_checkpoints = true;
        for offer in &mut commercial.offers {
            offer.revision += 1;
            offer.membership_revision = commercial.membership.revision;
            offer.accepted_rails = vec![ProxyRail::Fiat];
            offer.per_request_au += 3;
            offer.min_session_au += 4;
            for rate in &mut offer.rates {
                rate.per_unit_au += 5;
            }
        }
        let updated = f.store().update(review.revision, commercial).unwrap();
        assert_eq!(updated.state, State::Unchecked);
        assert!(updated.admission_handoff.is_none());
        assert_eq!(updated.probe_status, "protocol_validated");
        assert_eq!(updated.probe.as_ref().unwrap().probe_id, report.probe_id);
        assert_eq!(
            updated.probe.as_ref().unwrap().evidence_hash,
            report.evidence_hash
        );
        let rechecked = f.store().check(updated.revision).unwrap();
        assert_eq!(rechecked.state, State::StructurallyValid);
        assert_eq!(rechecked.probe_status, "protocol_validated");
        assert_ne!(
            rechecked
                .admission_handoff
                .as_ref()
                .unwrap()
                .initial_operation_digest,
            review
                .admission_handoff
                .as_ref()
                .unwrap()
                .initial_operation_digest
        );
        let retained = a.probe_budget(&d(2)).unwrap().unwrap();
        assert_eq!(
            (retained.used_attempts, retained.allocated_cost_microusd),
            (1, 10)
        );
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        redacted(&updated);
        reviews.push(serde_json::to_value(&review).unwrap());
    }
    if let Some(path) = std::env::var_os("MAYHEM_TEST_PROXY_SETUP_PROBE_FIXTURE") {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(
            &serde_json::to_vec_pretty(
                &json!({"schema_version":1,"test_only":true,"reviews":reviews}),
            )
            .unwrap(),
        )
        .unwrap();
        file.sync_all().unwrap();
    }
}

#[tokio::test]
async fn allowance_is_cumulative_and_scope_cannot_be_replaced_or_recreated() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::ZERO,
    );
    let mut rev = checked(&f);
    let p = plan(&f, chat());
    for _ in 0..2 {
        rev = f.store().probe(rev, p.clone()).await.unwrap().revision;
    }
    let exhausted = f.store().probe(rev, p.clone()).await.unwrap();
    assert_eq!(exhausted.probe_status, "not_validated");
    assert_eq!(b.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        authority(&f)
            .probe_budget(&d(2))
            .unwrap()
            .unwrap()
            .used_attempts,
        2
    );
    let mut changed = p.clone();
    changed.scope.capacity_file = f.dir.path().join("replacement.redb");
    assert!(matches!(
        f.store().probe(exhausted.revision, changed).await,
        Err(Error::Invalid)
    ));
    assert!(!f.dir.path().join("replacement.redb").exists());
    std::fs::remove_file(&p.scope.capacity_file).unwrap();
    assert!(matches!(
        f.store().probe(exhausted.revision, p).await,
        Err(Error::ProbeRecovery)
    ));
    assert!(!f.dir.path().join("capacity.redb").exists());
}

#[tokio::test]
async fn timeout_and_restart_keep_original_physical_occupancy_and_never_redispatch() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::from_secs(2),
    );
    let rev = checked(&f);
    let mut p = plan(&f, chat());
    p.timeout_ms = 50;
    let review = f.store().probe(rev, p.clone()).await.unwrap();
    assert_eq!(review.probe_status, "recovery_required");
    assert_eq!(review.probe.as_ref().unwrap().state, ProbeState::Uncertain);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        f.store().probe(review.revision, p.clone()).await,
        Err(Error::ProbeRecovery)
    ));
    let recovered = f.store().recover_probe(review.revision).unwrap();
    assert_eq!(recovered.probe_status, "recovery_required");
    assert_eq!(
        recovered.probe.as_ref().unwrap().probe_id,
        review.probe.as_ref().unwrap().probe_id
    );
    let a = authority(&f);
    assert_eq!(a.group_status(&d(2)).unwrap().occupied, 1);
    assert_eq!(a.group_status(&d(51)).unwrap().occupied, 1);
    assert_eq!(
        a.probe_for_group(&d(2)).unwrap().unwrap().phase,
        capacity::probes::ProbePhase::Uncertain
    );
    assert_eq!(a.probe_budget(&d(2)).unwrap().unwrap().used_attempts, 1);
    drop(a);
    // A draft update keeps its original authority and interrupted probe.
    let path = f.store.join("draft.json");
    let mut legacy: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    legacy["probe"]
        .as_object_mut()
        .unwrap()
        .remove("configuration_binding");
    private(&path, &serde_json::to_vec(&legacy).unwrap());
    let mut commercial = f.input.clone();
    commercial.offers[0].revision += 1;
    commercial.offers[0].rates[0].per_unit_au += 1;
    let updated = f.store().update(recovered.revision, commercial).unwrap();
    assert_eq!(updated.probe_status, "recovery_required");
    let retained: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(retained["probe"].get("configuration_binding").is_none());
    let checked = f.store().check(updated.revision).unwrap();
    assert!(matches!(
        f.store().probe(checked.revision, p).await,
        Err(Error::ProbeRecovery)
    ));
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    let budget = authority(&f).probe_budget(&d(2)).unwrap().unwrap();
    assert_eq!(
        (budget.used_attempts, budget.allocated_cost_microusd),
        (1, 10)
    );
}

#[tokio::test]
async fn abort_during_dispatch_keeps_pending_intent_and_draft_lock_blocks_concurrent_mutation() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::from_secs(5),
    );
    let rev = checked(&f);
    let p = plan(&f, chat());
    let store = f.store();
    let task = tokio::spawn(async move { store.probe(rev, p).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while b.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(Store::open(&f.store), Err(Error::Busy)));
    task.abort();
    let _ = task.await;
    let review = f.store().inspect().unwrap();
    assert_eq!(review.revision, rev + 1);
    assert_eq!(review.probe.as_ref().unwrap().state, ProbeState::Pending);
    let recovered = f.store().recover_probe(review.revision).unwrap();
    assert_eq!(
        recovered.probe.as_ref().unwrap().state,
        ProbeState::Uncertain
    );
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(authority(&f).group_status(&d(2)).unwrap().occupied, 1);
}

#[tokio::test]
async fn exact_config_binding_and_bounded_private_plan_reject_before_http() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::ZERO,
    );
    let rev = checked(&f);
    let p = plan(&f, chat());
    let mut invalid = p.clone();
    invalid.timeout_ms = 30_001;
    assert!(matches!(
        f.store().probe(rev, invalid).await,
        Err(Error::Invalid)
    ));
    let mut invalid = p.clone();
    invalid.request["temperature"] = json!(0.75); // custom contract max is 0.5
    assert!(matches!(
        f.store().probe(rev, invalid).await,
        Err(Error::Invalid)
    ));
    let mut invalid = p.clone();
    invalid.scope.connection_group = d(90);
    assert!(matches!(
        f.store().probe(rev, invalid).await,
        Err(Error::Invalid)
    ));
    assert!(matches!(
        f.store().probe(rev - 1, p.clone()).await,
        Err(Error::Conflict)
    ));
    f.connection["revision"] = json!(2);
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    assert!(f.store().probe(rev, p).await.is_err());
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    assert!(!f.dir.path().join("capacity.redb").exists());
    let path = f.dir.path().join("plan.json");
    let p = plan(&f, chat());
    private(&path, &serde_json::to_vec(&p).unwrap());
    assert!(ProbePlan::load(&path).is_ok());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(ProbePlan::load(&path), Err(Error::Protection)));
}

#[tokio::test]
async fn streaming_uses_real_decoder_and_configuration_update_invalidates_success_claim() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let sse = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"id":"u","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}),
        json!({"id":"u","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
    );
    let b = backend(&mut f, 200, sse.into_bytes(), true, Duration::ZERO);
    let rev = checked(&f);
    let mut p = plan(&f, chat());
    p.streaming = true;
    p.request["stream"] = json!(true);
    let review = f.store().probe(rev, p).await.unwrap();
    assert_eq!(review.probe_status, "protocol_validated");
    let mut input = f.input.clone();
    input.membership.served_context += 1;
    let updated = f.store().update(review.revision, input).unwrap();
    assert_eq!(updated.probe_status, "recheck_required");
    assert!(!updated.probe.as_ref().unwrap().for_current_configuration);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    redacted(&updated);
}

#[tokio::test]
async fn recovery_never_adopts_or_cancels_a_later_identical_prepared_attempt() {
    for lost_id in [false, true] {
        let mut f = Fixture::new(ProxyEndpoint::Chat);
        let b = backend(
            &mut f,
            200,
            serde_json::to_vec(&answer()).unwrap(),
            false,
            Duration::ZERO,
        );
        let rev = checked(&f);
        let review = f.store().probe(rev, plan(&f, chat())).await.unwrap();
        let path = f.store.join("draft.json");
        let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let specification: capacity::probes::Specification =
            serde_json::from_value(record["probe"]["specification"].clone()).unwrap();
        let a = authority(&f);
        // A separate controller reserved another invocation with identical inputs.
        rebind_health(&a);
        let foreign = a.reserve_probe(specification).unwrap();
        let foreign_id = foreign.probe().id.clone();
        assert_ne!(
            Some(&foreign_id),
            review.probe.as_ref().unwrap().probe_id.as_ref()
        );
        drop((foreign, a));
        // Simulate loss of a completion write or an earlier intent with no ID.
        record["probe"]["state"] = json!("pending");
        record["probe"]["evidence_hash"] = Value::Null;
        if lost_id {
            record["probe"]["probe_id"] = Value::Null;
            record["probe"]
                .as_object_mut()
                .unwrap()
                .remove("reservation_intent");
        }
        private(&path, &serde_json::to_vec(&record).unwrap());
        let recovered = f.store().recover_probe(review.revision).unwrap();
        assert_eq!(
            recovered.probe.as_ref().unwrap().state,
            ProbeState::Uncertain
        );
        if lost_id {
            assert_eq!(
                recovered.probe.as_ref().unwrap().recovery_reason,
                Some("legacy_probe_identity_unavailable")
            );
        }
        assert_ne!(
            recovered.probe.as_ref().unwrap().probe_id.as_ref(),
            Some(&foreign_id)
        );
        let a = authority(&f);
        let retained = a.probe_for_group(&d(2)).unwrap().unwrap();
        assert_eq!(retained.id, foreign_id);
        assert_eq!(retained.phase, capacity::probes::ProbePhase::Prepared);
        assert_eq!(a.group_status(&d(51)).unwrap().occupied, 1);
        assert_eq!(a.probe_budget(&d(2)).unwrap().unwrap().used_attempts, 2);
        // Trusted test owner cancels its own prepared reservation. Recovery then
        // reports lack of validated result, never infers success from a digest.
        a.cancel_prepared_probe(&foreign_id).unwrap();
        drop(a);
        let closed = f.store().recover_probe(recovered.revision).unwrap();
        assert_eq!(closed.probe_status, "not_validated");
        assert!(closed.probe.as_ref().unwrap().evidence_hash.is_none());
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn existing_capacity_owner_and_overlapping_alias_probe_block_new_dispatch() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::ZERO,
    );
    let rev = checked(&f);
    let a = authority(&f);
    assert!(matches!(
        f.store().probe(rev, plan(&f, chat())).await,
        Err(Error::ProbeCapacity)
    ));
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    drop(a);
    let review = f.store().probe(rev, plan(&f, chat())).await.unwrap();
    let record: Value =
        serde_json::from_slice(&std::fs::read(f.store.join("draft.json")).unwrap()).unwrap();
    let mut specification: capacity::probes::Specification =
        serde_json::from_value(record["probe"]["specification"].clone()).unwrap();
    let a = authority(&f);
    a.configure_group(d(55), 2).unwrap();
    a.configure_route_with_constraints(
        capacity::Route {
            id: d(56),
            group: d(55),
            lane: capacity::Lane::Proxy,
            max_concurrency: 2,
        },
        vec![d(51)],
    )
    .unwrap();
    a.configure_probe_budget(
        &d(55),
        capacity::probes::Budget {
            max_attempts: 1,
            max_cost_microusd: 10,
            per_attempt_cost_microusd: 10,
        },
    )
    .unwrap();
    specification.route = d(56);
    specification.budget_group = d(55);
    let foreign = a.reserve_probe(specification).unwrap();
    let foreign_id = foreign.probe().id.clone();
    drop((foreign, a));
    assert!(matches!(
        f.store().probe(review.revision, plan(&f, chat())).await,
        Err(Error::ProbeRecovery)
    ));
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    let a = authority(&f);
    assert_eq!(a.probe_for_group(&d(51)).unwrap().unwrap().id, foreign_id);
    assert_eq!(a.probe_budget(&d(2)).unwrap().unwrap().used_attempts, 1);
}

async fn pending_record(f: &Fixture, revision: u64) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let bytes = std::fs::read(f.store.join("draft.json")).unwrap();
            let record: Value = serde_json::from_slice(&bytes).unwrap();
            if record["revision"] == json!(revision) && record["probe"]["state"] == json!("pending")
            {
                break bytes;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn unlocked(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while matches!(Store::open(&f.store), Err(Error::Busy)) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn crash_after_intent_before_reserve_has_original_identity_and_consumes_no_allowance() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::ZERO,
    );
    let rev = checked(&f);
    let p = plan(&f, chat());
    let store = f.store();
    let mut future = Box::pin(store.probe(rev, p.clone()));
    // Poll once to launch the actual blocking prepare, then never poll its
    // completion. Controller::run cannot begin until the caller resumes.
    {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(future.as_mut(), &mut context).is_pending());
    }
    let saved = pending_record(&f, rev + 1).await;
    drop(future);
    unlocked(&f).await;
    let record: Value = serde_json::from_slice(&saved).unwrap();
    let specification = serde_json::from_value(record["probe"]["specification"].clone()).unwrap();
    let intent = serde_json::from_value(record["probe"]["reservation_intent"].clone()).unwrap();
    let a = authority(&f);
    assert_eq!(
        serde_json::to_value(a.probe_intent_id(&specification, &intent).unwrap()).unwrap(),
        record["probe"]["probe_id"]
    );
    assert_eq!(a.probe_budget(&d(2)).unwrap().unwrap().used_attempts, 0);
    assert!(a.probe_for_group(&d(2)).unwrap().is_none());
    drop(a);
    let recovered = f.store().recover_probe(rev + 1).unwrap();
    assert_eq!(recovered.probe_status, "not_validated");
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    let next = f.store().probe(recovered.revision, p).await.unwrap();
    assert_eq!(next.probe_status, "protocol_validated");
    assert_ne!(
        next.probe.as_ref().unwrap().probe_id,
        recovered.probe.as_ref().unwrap().probe_id
    );
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn crash_after_real_reservation_before_dispatch_recovers_original_prepared_without_refund() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::ZERO,
    );
    let rev = checked(&f);
    let original = plan(&f, chat());
    let mut paused = original.clone();
    // A bounded ephemeral decoder fixture stalls only the decoder handshake.
    // exec replaces the shell; kill_on_drop terminates that exact child.
    paused.worker_program = f.dir.path().join("paused-decoder");
    private(
        &paused.worker_program,
        b"#!/bin/sh\nprintf ready > paused\nexec /bin/sleep 30\n",
    );
    std::fs::set_permissions(
        &paused.worker_program,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let marker = paused.worker_directory.join("paused");
    let store = f.store();
    let task = tokio::spawn(async move { store.probe(rev, paused).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let pending = pending_record(&f, rev + 1).await;
    task.abort();
    let _ = task.await;
    unlocked(&f).await;
    let record: Value = serde_json::from_slice(&pending).unwrap();
    let specification = serde_json::from_value(record["probe"]["specification"].clone()).unwrap();
    let intent = serde_json::from_value(record["probe"]["reservation_intent"].clone()).unwrap();
    let a = authority(&f);
    let id = a.probe_intent_id(&specification, &intent).unwrap();
    let retained = a.probe_for_group(&d(2)).unwrap().unwrap();
    assert_eq!(retained.id, id);
    assert_eq!(retained.phase, capacity::probes::ProbePhase::Prepared);
    assert_eq!(a.group_status(&d(51)).unwrap().occupied, 1);
    drop(a);
    let recovered = f.store().recover_probe(rev + 1).unwrap();
    assert_eq!(recovered.probe_status, "not_validated");
    assert_eq!(recovered.probe.as_ref().unwrap().probe_id, Some(id));
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    let a = authority(&f);
    assert_eq!(a.probe_budget(&d(2)).unwrap().unwrap().used_attempts, 1);
    assert_eq!(a.group_status(&d(51)).unwrap().occupied, 0);
    assert!(matches!(
        a.reserve_probe_once(specification, intent),
        Err(capacity::Error::Stale)
    ));
    drop(a);
    std::fs::remove_file(marker).unwrap();
    let next = f.store().probe(recovered.revision, original).await.unwrap();
    assert_eq!(next.probe_status, "protocol_validated");
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        authority(&f)
            .probe_budget(&d(2))
            .unwrap()
            .unwrap()
            .used_attempts,
        2
    );
}

#[tokio::test]
async fn lost_success_write_recovers_actual_saved_intent_without_inventing_validation_or_reexecution(
) {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::from_millis(200),
    );
    let rev = checked(&f);
    let p = plan(&f, chat());
    let store = f.store();
    let task = tokio::spawn(async move { store.probe(rev, p).await });
    let saved = pending_record(&f, rev + 1).await;
    let completed = task.await.unwrap().unwrap();
    assert_eq!(completed.probe_status, "protocol_validated");
    // Restore the byte-identical durable intent that preceded execution, as if
    // power loss prevented only the final setup report from becoming durable.
    private(&f.store.join("draft.json"), &saved);
    let original: Value = serde_json::from_slice(&saved).unwrap();
    let specification = serde_json::from_value(original["probe"]["specification"].clone()).unwrap();
    let intent = serde_json::from_value(original["probe"]["reservation_intent"].clone()).unwrap();
    let recovered = f.store().recover_probe(rev + 1).unwrap();
    assert_eq!(recovered.probe_status, "not_validated");
    assert!(recovered.probe.as_ref().unwrap().evidence_hash.is_none());
    assert_eq!(
        recovered.probe.as_ref().unwrap().probe_id,
        completed.probe.as_ref().unwrap().probe_id
    );
    let a = authority(&f);
    assert!(matches!(
        a.reserve_probe_once(specification, intent),
        Err(capacity::Error::Stale)
    ));
    assert_eq!(a.probe_budget(&d(2)).unwrap().unwrap().used_attempts, 1);
    assert_eq!(a.group_status(&d(51)).unwrap().occupied, 0);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

fn rebind_adapter(input: &mut Input) {
    let adapter = Adapter::restore(input.adapter.clone()).unwrap();
    input.market.family = adapter.endpoint().family();
    input.market.endpoints = vec![ProxyEndpointContract {
        endpoint: adapter.endpoint(),
        contract_hash: adapter.contract_hash().as_str().into(),
    }];
    input.market.metering = Policy::for_endpoint(adapter.endpoint()).contract();
    input.membership.endpoints = input.market.endpoints.clone();
    input.membership.market_id = input.market.id().unwrap();
    input.membership.recipe_hash = adapter.recipe_hash().as_str().into();
    for offer in &mut input.offers {
        offer.market_id = input.membership.market_id.clone();
        offer.endpoint = adapter.endpoint();
        offer.metering_policy_hash = input.market.metering.policy_hash.clone();
    }
    input.validate().unwrap();
}

#[tokio::test]
async fn protocol_and_resource_mutation_matrix_invalidates_reuse_without_spending_allowance() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::ZERO,
    );
    let review = f
        .store()
        .probe(checked(&f), plan(&f, chat()))
        .await
        .unwrap();
    let original_probe = review.probe.as_ref().unwrap();
    let mut revision = review.revision;
    let mut changes = Vec::new();
    for name in [
        "upstream",
        "request_bytes",
        "response_bytes",
        "choices",
        "tools",
        "questions",
        "decision_options",
        "contract",
        "endpoint",
        "context",
        "concurrency",
        "capacity_group",
        "offer_slot",
        "public_model",
    ] {
        let mut input = f.input.clone();
        match name {
            "upstream" => input.adapter.upstream_model = "another-private-model".into(),
            "request_bytes" => input.adapter.limits.request_bytes -= 1,
            "response_bytes" => input.adapter.limits.response_bytes -= 1,
            "choices" => input.adapter.limits.choices += 1,
            "tools" => input.adapter.limits.tools += 1,
            "questions" => input.adapter.limits.questions += 1,
            "decision_options" => input.adapter.limits.decision_options += 1,
            "contract" => {
                input
                    .adapter
                    .contract
                    .request_attribute_specs
                    .get_mut("temperature")
                    .unwrap()
                    .maximum = Some(0.4)
            }
            "endpoint" => {
                input.adapter.endpoint = ProxyEndpoint::Completions;
                input.adapter.contract = mayhem_proto::endpoint_family_contract_template(
                    mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
                )
                .unwrap();
            }
            "context" => input.membership.served_context += 1,
            "concurrency" => input.membership.max_concurrency += 1,
            "capacity_group" => input.membership.capacity_group = d(90).as_str().into(),
            "offer_slot" => input.offers[0].ctx_bracket = "ctx8k".into(),
            "public_model" => input.market.model.revision = "changed-claim".into(),
            _ => unreachable!(),
        }
        rebind_adapter(&mut input);
        changes.push((name, input));
    }
    let authority = authority(&f); // Updates must not need to reopen this store.
    for (name, input) in changes {
        let changed = f.store().update(revision, input).unwrap();
        revision = changed.revision;
        assert_eq!(changed.probe_status, "recheck_required", "{name}");
        let probe = changed.probe.as_ref().unwrap();
        assert!(!probe.for_current_configuration, "{name}");
        assert_eq!(probe.probe_id, original_probe.probe_id);
        assert_eq!(probe.evidence_hash, original_probe.evidence_hash);
        let checked = f.store().check(revision).unwrap();
        revision = checked.revision;
        assert_eq!(checked.probe_status, "recheck_required", "{name}");
    }
    let budget = authority.probe_budget(&d(2)).unwrap().unwrap();
    assert_eq!(
        (budget.used_attempts, budget.allocated_cost_microusd),
        (1, 10)
    );
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn connection_and_pinned_scope_mutations_cannot_reuse_the_old_observation() {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let b = backend(
        &mut f,
        200,
        serde_json::to_vec(&answer()).unwrap(),
        false,
        Duration::ZERO,
    );
    let review = f
        .store()
        .probe(checked(&f), plan(&f, chat()))
        .await
        .unwrap();
    let mut revision = review.revision;
    for name in [
        "revision",
        "origin",
        "path",
        "credentials",
        "headers",
        "request_bytes",
        "response_bytes",
        "concurrency",
        "timeout",
        "network",
    ] {
        let mut config = f.connection.clone();
        let mut input = f.input.clone();
        match name {
            "revision" => {
                config["revision"] = json!(2);
                input.membership.connection_revision = 2;
            }
            "origin" => config["base_url"] = json!("http://127.0.0.1:12345/v1/"),
            "path" => config["paths"]["chat_completions"] = json!("other-chat"),
            "credentials" => {
                config["authentication"] = json!({"type":"bearer","secret":{"source":"file","path":"still-never-read-secret"}})
            }
            "headers" => config["headers"] = json!({"x-fixture-config":"changed"}),
            "request_bytes" => {
                config["limits"] = json!({"max_request_bytes":8192,"max_response_bytes":65536,"max_in_flight":2,"connect_timeout_ms":10000})
            }
            "response_bytes" => {
                config["limits"] = json!({"max_request_bytes":65536,"max_response_bytes":8192,"max_in_flight":2,"connect_timeout_ms":10000})
            }
            "concurrency" => {
                config["limits"] = json!({"max_request_bytes":65536,"max_response_bytes":65536,"max_in_flight":1,"connect_timeout_ms":10000})
            }
            "timeout" => {
                config["limits"] = json!({"max_request_bytes":65536,"max_response_bytes":65536,"max_in_flight":2,"connect_timeout_ms":5000})
            }
            "network" => config["network"]["networks"] = json!(["127.0.0.0/8"]),
            _ => unreachable!(),
        }
        private(
            &f.input.connection_file,
            &serde_json::to_vec(&config).unwrap(),
        );
        let changed = f.store().update(revision, input).unwrap();
        revision = changed.revision;
        assert_eq!(changed.probe_status, "recheck_required", "{name}");
        assert!(
            !changed.probe.as_ref().unwrap().for_current_configuration,
            "{name}"
        );
    }
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    let original = f.store().update(revision, f.input.clone()).unwrap();
    assert_eq!(original.probe_status, "protocol_validated");
    let path = f.store.join("draft.json");
    let baseline: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for name in [
        "capacity_file",
        "route",
        "connection_group",
        "connection_ceiling",
        "route_ceiling",
        "constraints",
    ] {
        let mut record = baseline.clone();
        record["probe_scope"][name] = match name {
            "capacity_file" => json!(f.dir.path().join("another-capacity.redb")),
            "route" | "connection_group" => json!(d(90)),
            "connection_ceiling" => json!(3),
            "route_ceiling" => json!(1),
            "constraints" => json!([{"id":d(51),"ceiling":3}]),
            _ => unreachable!(),
        };
        private(&path, &serde_json::to_vec(&record).unwrap());
        assert_eq!(
            f.store().inspect().unwrap().probe_status,
            "recheck_required",
            "{name}"
        );
    }
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    let budget = authority(&f).probe_budget(&d(2)).unwrap().unwrap();
    assert_eq!(
        (budget.used_attempts, budget.allocated_cost_microusd),
        (1, 10)
    );
    assert!(!f.dir.path().join("still-never-read-secret").exists());
}

#[tokio::test]
async fn only_exact_current_legacy_success_can_gain_a_configuration_binding() {
    for mode in ["valid", "stale_declaration", "changed_connection"] {
        let mut f = Fixture::new(ProxyEndpoint::Chat);
        let b = backend(
            &mut f,
            200,
            serde_json::to_vec(&answer()).unwrap(),
            false,
            Duration::ZERO,
        );
        let mut review = f
            .store()
            .probe(checked(&f), plan(&f, chat()))
            .await
            .unwrap();
        let mut input = f.input.clone();
        input.offers[0].rates[0].per_unit_au += 1;
        if mode == "stale_declaration" {
            // Remove the new binding only after changing the old declaration,
            // simulating a legacy observation whose exact original input is lost.
            review = f.store().update(review.revision, input.clone()).unwrap();
        }
        let path = f.store.join("draft.json");
        let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let original_attempt = record["probe"].clone();
        record["probe"]
            .as_object_mut()
            .unwrap()
            .remove("configuration_binding");
        private(&path, &serde_json::to_vec(&record).unwrap());
        if mode == "changed_connection" {
            f.connection["headers"] = json!({"x-fixture":"changed"});
            private(
                &f.input.connection_file,
                &serde_json::to_vec(&f.connection).unwrap(),
            );
        }
        input.offers[0].revision += 1;
        let updated = f.store().update(review.revision, input).unwrap();
        assert_eq!(
            updated.probe_status,
            if mode == "valid" {
                "protocol_validated"
            } else {
                "recheck_required"
            }
        );
        let stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored["probe"].get("configuration_binding").is_some(),
            mode == "valid"
        );
        for retained in [
            "binding",
            "specification",
            "probe_id",
            "evidence_hash",
            "reservation_intent",
        ] {
            assert_eq!(
                stored["probe"][retained], original_attempt[retained],
                "original {retained} remains unchanged"
            );
        }
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        let budget = authority(&f).probe_budget(&d(2)).unwrap().unwrap();
        assert_eq!(
            (budget.used_attempts, budget.allocated_cost_microusd),
            (1, 10)
        );
    }
}
