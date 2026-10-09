use super::*;
use crate::connector::failure::{Execution, Stage};
use serde_json::json;
fn d(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn policy() -> Policy {
    Policy {
        max_routes: 2,
        max_classes_per_route: 4,
        evidence_ttl_ms: 60_000,
        successes_to_increase: 2,
        bad_samples_to_reduce: 2,
        latency_baseline_samples: 3,
        latency_multiplier: 4,
        latency_increase_ms: 1000,
        min_native_tok_s: 5,
        recovery: RefreshPolicy {
            interval_ms: 10_000,
            page_pause_ms: 10,
            retry_initial_ms: 2000,
            retry_max_ms: 30_000,
            jitter_percent: 0,
        },
    }
}
fn monitor() -> Monitor {
    let m = Monitor::new(policy(), 4, 7).unwrap();
    m.register(d(1), 4, true).unwrap();
    m.register(d(2), 2, false).unwrap();
    m
}

#[tokio::test(start_paused = true)]
async fn native_evidence_age_does_not_restart_after_tokenization_or_durable_publication() {
    let m = monitor();
    let mut sample = m.observe_request(&d(1), class()).unwrap();
    sample.delta(&delta());
    sample.native_progress(d(9), 0).unwrap();
    advance(100).await;
    sample.delta(&delta());
    sample.native_progress(d(9), 20).unwrap();
    sample.finish_native().await;
    let success = sample.prepare_success(Some(999999));
    advance(60_001).await;
    success.publish();
    let view = m.snapshot(&d(1)).unwrap();
    assert_eq!(view.reason, Reason::Stale);
    assert_eq!(view.allowance, 0);
    assert!(!view.meets_native_floor(5));
    assert_eq!(view.last_measurement.unwrap().total_ms, 100);
}

#[tokio::test(start_paused = true)]
async fn delayed_measurements_cannot_replace_newer_route_or_connection_evidence() {
    let m = monitor();
    let mut old = m.observe_request(&d(1), class()).unwrap();
    old.delta(&delta());
    old.native_progress(d(9), 0).unwrap();
    advance(100).await;
    old.native_progress(d(9), 10).unwrap();
    old.finish_native().await;
    let old = old.prepare_success(None);
    advance(10).await;
    rate(m.observe_request(&d(1), class()).unwrap(), 0, 80, 100).await;
    let before = m.snapshot(&d(1)).unwrap().native_speed.unwrap();
    old.publish();
    let after = m.snapshot(&d(1)).unwrap().native_speed.unwrap();
    assert_eq!(before.tok_s, after.tok_s);
    assert_eq!(before.age_ms, after.age_ms);
    let mut old = m.observe_request(&d(2), class()).unwrap();
    old.delta(&delta());
    old.finish_native().await;
    let old = old.prepare_success(None);
    advance(20).await;
    rate(m.observe_request(&d(1), class()).unwrap(), 0, 20, 100).await;
    let connection = m.inner.data.lock().unwrap().connection.observed;
    old.publish();
    assert_eq!(m.inner.data.lock().unwrap().connection.observed, connection);
}

#[tokio::test(start_paused = true)]
async fn very_fast_native_progress_is_not_rejected_by_a_minimum_duration() {
    let m = monitor();
    let mut sample = m.observe_request(&d(1), class()).unwrap();
    sample.delta(&delta());
    sample.native_progress(d(9), 0).unwrap();
    tokio::time::advance(Duration::from_micros(1)).await;
    sample.native_progress(d(9), 10).unwrap();
    sample.success(None);
    let measurement = m.snapshot(&d(1)).unwrap().last_measurement.unwrap();
    assert_eq!(measurement.native_interval_tokens, Some(10));
    assert_eq!(measurement.native_interval_us, Some(1));
    assert_eq!(measurement.native_tok_s, Some(10_000_000.0));
}
fn class() -> Class {
    Class::new(1024, Thinking::Disabled, true)
}

fn measured_monitor() -> Monitor {
    let m = Monitor::new(policy(), 4, 7).unwrap();
    m.register_measured(d(1), 4, d(9)).unwrap();
    m
}
#[tokio::test(start_paused = true)]
async fn measured_admission_and_snapshot_share_unknown_fresh_stale_and_recovery_states() {
    let m = measured_monitor();
    let source = m.route_source(&d(1)).unwrap();
    good(&m, &d(1), class(), 1).await;
    assert_eq!(
        m.snapshot(&d(1)).unwrap().reason,
        Reason::UnverifiedThroughput
    );
    let evidence = source.evidence().unwrap();
    assert_eq!(evidence.state, crate::capacity::Readiness::Checking);
    assert_eq!(evidence.allowance, 0);
    // Ordinary health is fresh, but a speed-recovery observation is permitted.
    rate(m.observe_recovery(&d(1), class()).unwrap(), 0, 10, 100).await;
    assert!(m.snapshot(&d(1)).unwrap().meets_native_floor(5));
    assert!(!m.snapshot(&d(1)).unwrap().meets_native_floor(200));
    assert_eq!(
        source.evidence().unwrap().state,
        crate::capacity::Readiness::Ready
    );
    advance(59_000).await;
    good(&m, &d(1), class(), 1).await;
    // Successful short replies do not rejuvenate the last native measurement.
    assert!(source.evidence().unwrap().age >= Duration::from_millis(59_000));
    advance(1000).await;
    let view = m.snapshot(&d(1)).unwrap();
    assert_eq!(view.reason, Reason::StaleThroughput);
    assert_eq!(view.allowance, 0);
    assert_eq!(
        source.evidence().unwrap().state,
        crate::capacity::Readiness::Checking
    );
    let recovery = m.observe_recovery(&d(1), class()).unwrap();
    assert!(recovery.recovery_is_current().unwrap());
    rate(recovery, 0, 10, 100).await;
    assert!(m.snapshot(&d(1)).unwrap().meets_native_floor(5));
}

#[tokio::test(start_paused = true)]
async fn measured_registration_cannot_be_downgraded_or_satisfied_with_another_tokenizer() {
    let m = measured_monitor();
    assert!(m.register(d(1), 4, true).is_err());
    assert!(m.register_measured(d(1), 4, d(8)).is_err());
    let mut probe = m.observe_recovery(&d(1), class()).unwrap();
    probe.delta(&delta());
    probe.native_progress(d(8), 0).unwrap();
    advance(100).await;
    probe.native_progress(d(8), 100).unwrap();
    probe.success(None);
    let view = m.snapshot(&d(1)).unwrap();
    assert_eq!(view.reason, Reason::UnverifiedThroughput);
    assert_eq!(view.allowance, 0);
    assert!(!view.meets_native_floor(5));
    assert!(view.recovery_after_ms > 0);
    assert!(matches!(
        m.observe_recovery(&d(1), class()),
        Err(Error::RecoveryBusy)
    ));
}

#[tokio::test(start_paused = true)]
async fn inconclusive_probe_backs_off_without_renewing_speed_or_creating_ready_capacity() {
    let m = measured_monitor();
    let probe = m.observe_recovery(&d(1), class()).unwrap();
    probe.success(Some(999999));
    let view = m.snapshot(&d(1)).unwrap();
    assert_eq!(view.reason, Reason::UnverifiedThroughput);
    assert_eq!(view.allowance, 0);
    assert_eq!(view.recovery_after_ms, 2000);
    assert!(matches!(
        m.observe_recovery(&d(1), class()),
        Err(Error::RecoveryBusy)
    ));
    advance(2000).await;
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 0);
    rate(m.observe_recovery(&d(1), class()).unwrap(), 0, 10, 100).await;
    assert!(m.snapshot(&d(1)).unwrap().meets_native_floor(5));
}

#[tokio::test(start_paused = true)]
async fn recovery_backoff_starts_after_evaluation_but_evidence_keeps_network_age() {
    let m = measured_monitor();
    let mut probe = m.observe_recovery(&d(1), class()).unwrap();
    probe.delta(&delta());
    probe.network_complete(Instant::now());
    let done = probe.prepare_success(Some(5000));
    advance(20_000).await;
    done.publish();
    let view = m.snapshot(&d(1)).unwrap();
    assert_eq!(view.recovery_after_ms, 2000);
    assert!(view.evidence_age_ms.unwrap() >= 20_000);
    assert_eq!(view.allowance, 0);
    assert!(matches!(
        m.observe_recovery(&d(1), class()),
        Err(Error::RecoveryBusy)
    ));
    advance(2000).await;
    assert!(m.observe_recovery(&d(1), class()).is_ok());
}

#[test]
fn latency_classes_separate_effort_output_budget_and_schema_without_retaining_prompt_text() {
    let base = json!({"messages":[{"role":"user","content":"private prompt"}],"max_tokens":512,"reasoning_effort":"low"});
    let original = Class::request(&base, 1024, true);
    for (key, value) in [
        ("max_tokens", json!(8192)),
        ("reasoning_effort", json!("high")),
        ("thinking_mode", json!("enabled")),
        ("chat_template_kwargs", json!({"enable_thinking":true})),
        ("response_format", json!({"type":"json_object"})),
    ] {
        let mut changed = base.clone();
        changed[key] = value;
        assert_ne!(original, Class::request(&changed, 1024, true), "{key}");
    }
    let mut other = base.clone();
    other["messages"][0]["content"] = json!("different prompt");
    assert_eq!(original, Class::request(&other, 1024, true));
    assert_ne!(original, Class::request(&base, 8192, true));
    assert_ne!(original, Class::request(&base, 1024, false));
    assert!(!serde_json::to_string(&original)
        .unwrap()
        .contains("private prompt"));
}
fn fail(code: Code, scope: Scope) -> Failure {
    Failure::new(code, scope, Stage::ResponseHeaders, Execution::Unknown)
}
fn delta() -> serde_json::Value {
    json!({"choices":[{"delta":{"content":"test"}}]})
}
async fn advance(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
}
async fn good(m: &Monitor, id: &Digest, c: Class, latency: u64) {
    let mut sample = m.observe_request(id, c).unwrap();
    sample.headers();
    advance(latency).await;
    sample.delta(&delta());
    sample.success(None);
}
async fn rate(mut sample: Sample, first_ms: u64, tokens: u64, ms: u64) {
    sample.headers();
    advance(first_ms).await;
    sample.delta(&delta());
    sample.native_progress(d(9), 1).unwrap();
    advance(ms).await;
    sample.delta(&delta());
    sample.native_progress(d(9), tokens + 1).unwrap();
    sample.success(None);
}

#[tokio::test(start_paused = true)]
async fn fresh_allowance_ramps_then_stales_without_heartbeat_renewal_or_automatic_recovery() {
    let m = monitor();
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 0);
    for _ in 0..20 {
        good(&m, &d(1), class(), 1).await;
    }
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 4);
    for _ in 0..100 {
        assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 4);
    }
    advance(60_000).await;
    let snapshot = m.snapshot(&d(1)).unwrap();
    assert_eq!(snapshot.state, State::Checking);
    assert_eq!(snapshot.allowance, 0);
    let recovery = m.observe_recovery(&d(1), class()).unwrap();
    assert!(matches!(
        m.observe_recovery(&d(2), class()),
        Err(Error::RecoveryBusy)
    ));
    drop(recovery);
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 0);
    let recovery = m.observe_recovery(&d(1), class()).unwrap();
    recovery.success(None);
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 1);
}
#[tokio::test(start_paused = true)]
async fn retry_after_survives_evidence_expiry_and_is_not_capped_or_overflowed() {
    let m = monitor();
    good(&m, &d(1), class(), 1).await;
    good(&m, &d(2), class(), 1).await;
    let mut error = fail(Code::UpstreamRateLimited, Scope::Connection);
    error.retry_after_ms = Some(120_000);
    m.observe_request(&d(1), class()).unwrap().failure(error);
    assert_eq!(m.snapshot(&d(2)).unwrap().reason, Reason::RateLimited);
    advance(60_001).await;
    assert!(m.observe_recovery(&d(1), class()).is_err());
    assert_eq!(m.snapshot(&d(1)).unwrap().recovery_after_ms, 59_999);
    advance(59_999).await;
    m.observe_recovery(&d(1), class()).unwrap().success(None);
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 1);
    let mut error = fail(Code::UpstreamRateLimited, Scope::Connection);
    error.retry_after_ms = Some(u64::MAX);
    m.observe_request(&d(1), class()).unwrap().failure(error);
    assert_eq!(m.snapshot(&d(1)).unwrap().recovery_after_ms, u64::MAX);
}
#[tokio::test(start_paused = true)]
async fn failure_backoff_starts_at_completion_and_old_success_cannot_clear_new_failure() {
    let m = monitor();
    good(&m, &d(1), class(), 1).await;
    let old = m.observe_request(&d(1), class()).unwrap();
    m.observe_request(&d(1), class())
        .unwrap()
        .failure(fail(Code::UpstreamBusy, Scope::Model));
    old.success(None);
    assert_eq!(m.snapshot(&d(1)).unwrap().reason, Reason::Busy);
    advance(2000).await;
    for delay in [4000, 8000, 16000, 30000, 30000] {
        let recovery = m.observe_recovery(&d(1), class()).unwrap();
        advance(1200).await;
        recovery.failure(fail(Code::UpstreamBusy, Scope::Model));
        assert_eq!(m.snapshot(&d(1)).unwrap().recovery_after_ms, delay);
        advance(delay).await;
    }
    m.observe_recovery(&d(1), class()).unwrap().success(None);
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 1);
}
#[tokio::test(start_paused = true)]
async fn caller_errors_and_local_cancellation_do_not_poison_other_routes() {
    let m = monitor();
    good(&m, &d(1), class(), 1).await;
    good(&m, &d(2), class(), 1).await;
    for code in [
        Code::InvalidRequest,
        Code::InvalidSchema,
        Code::ContextTooLarge,
        Code::UnsupportedControl,
        Code::LocalCapacity,
        Code::AdmissionUnavailable,
        Code::RequestCancelled,
    ] {
        m.observe_request(&d(1), class())
            .unwrap()
            .failure(fail(code, Scope::Connection));
        assert!(m.snapshot(&d(1)).unwrap().allowance > 0);
    }
    drop(m.observe_request(&d(1), class()).unwrap());
    m.observe_request(&d(1), class())
        .unwrap()
        .failure(fail(Code::UpstreamModelUnavailable, Scope::Model));
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 0);
    assert!(m.snapshot(&d(2)).unwrap().allowance > 0);
}
#[tokio::test(start_paused = true)]
async fn slow_native_generation_needs_matching_fresh_rate_to_recover_not_reported_usage() {
    let m = monitor();
    rate(m.observe_request(&d(1), class()).unwrap(), 5, 100, 1000).await;
    assert!(m.snapshot(&d(1)).unwrap().meets_native_floor(5));
    for _ in 0..2 {
        rate(m.observe_request(&d(1), class()).unwrap(), 5, 10, 10_000).await;
    }
    assert_eq!(m.snapshot(&d(1)).unwrap().reason, Reason::SlowGeneration);
    advance(2000).await;
    m.observe_recovery(&d(1), class())
        .unwrap()
        .success(Some(1_000_000));
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 0);
    rate(
        m.observe_recovery(&d(1), Class::new(65536, Thinking::Enabled, true))
            .unwrap(),
        5,
        100,
        1000,
    )
    .await;
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 0);
    rate(m.observe_recovery(&d(1), class()).unwrap(), 5, 100, 1000).await;
    assert!(m.snapshot(&d(1)).unwrap().meets_native_floor(5));
}
#[tokio::test(start_paused = true)]
async fn prefill_and_reasoning_are_separate_from_generation_and_no_chunk_token_guess_is_made() {
    let m = monitor();
    let mut sample = m.observe_request(&d(1), class()).unwrap();
    advance(5000).await;
    sample.headers();
    sample.delta(&json!({"choices":[{"delta":{"role":"assistant"}}]}));
    advance(1000).await;
    sample.delta(&json!({"choices":[{"delta":{"reasoning_content":"test"}}]}));
    sample.native_progress(d(9), 1).unwrap();
    advance(1000).await;
    sample
        .delta(&json!({"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"test"}}]}}]}));
    sample.native_progress(d(9), 101).unwrap();
    sample.success(Some(9000));
    let s = m.snapshot(&d(1)).unwrap();
    let observed = s.last_measurement.unwrap();
    assert_eq!(observed.headers_ms, Some(5000));
    assert_eq!(observed.first_output_ms, Some(6000));
    assert_eq!(observed.total_ms, 7000);
    assert_eq!(observed.native_tok_s, Some(100.0));
    assert_eq!(observed.meaningful_updates, 2);
    assert_eq!(observed.reported_output_tokens, Some(9000));
}
#[tokio::test(start_paused = true)]
async fn buffered_short_and_json_outputs_do_not_claim_native_rate_and_old_speed_expires() {
    let m = monitor();
    let mut sample = m.observe_request(&d(1), class()).unwrap();
    sample.delta(&delta());
    sample.native_progress(d(9), 1000).unwrap();
    sample.native_progress(d(9), 2000).unwrap();
    sample.success(Some(5000));
    assert!(!m.snapshot(&d(1)).unwrap().meets_native_floor(5));
    rate(m.observe_request(&d(1), class()).unwrap(), 5, 100, 1).await;
    assert!(m.snapshot(&d(1)).unwrap().meets_native_floor(50000));
    advance(59_999).await;
    good(&m, &d(1), Class::new(1024, Thinking::Disabled, false), 10).await;
    let view = m.snapshot(&d(1)).unwrap();
    assert!(view.allowance > 0);
    assert!(!view.meets_native_floor(5));
    assert!(view.native_speed.unwrap().age_ms >= 60_000);
}
#[tokio::test(start_paused = true)]
async fn native_counter_rejects_non_streams_changed_tokenizers_and_decreasing_counts() {
    let m = monitor();
    let mut sample = m
        .observe_request(&d(1), Class::new(1, Thinking::Unknown, false))
        .unwrap();
    sample.delta(&delta());
    assert!(sample.native_progress(d(9), 10).is_err());
    drop(sample);
    let mut sample = m.observe_request(&d(1), class()).unwrap();
    sample.delta(&delta());
    sample.native_progress(d(9), 10).unwrap();
    assert!(sample.native_progress(d(8), 11).is_err());
    assert!(sample.native_progress(d(9), 9).is_err());
}
#[tokio::test(start_paused = true)]
async fn comparable_latency_reduces_concurrency_without_cancelling_or_misclassifying_larger_work() {
    let m = monitor();
    for _ in 0..12 {
        good(&m, &d(1), class(), 100).await;
    }
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 4);
    good(&m, &d(1), Class::new(65536, Thinking::Enabled, true), 5000).await;
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 4);
    for _ in 0..2 {
        good(&m, &d(1), class(), 5000).await;
    }
    let view = m.snapshot(&d(1)).unwrap();
    assert_eq!(view.state, State::Degraded);
    assert_eq!(view.allowance, 2);
    good(&m, &d(1), Class::new(65536, Thinking::Enabled, true), 100).await;
    assert_eq!(m.snapshot(&d(1)).unwrap().state, State::Degraded);
    for _ in 0..8 {
        good(&m, &d(1), class(), 100).await;
    }
    assert_eq!(m.snapshot(&d(1)).unwrap().allowance, 4);
}
#[tokio::test(start_paused = true)]
async fn route_and_class_memory_is_bounded_and_rates_do_not_apply_to_decisions() {
    let m = monitor();
    assert!(m.register(d(3), 1, true).is_err());
    assert!(m.register(d(1), 2, true).is_err());
    for n in 0..30 {
        good(
            &m,
            &d(1),
            Class::new(1usize << n, Thinking::Unknown, true),
            1,
        )
        .await;
    }
    assert_eq!(
        m.inner
            .data
            .lock()
            .unwrap()
            .routes
            .get(&d(1))
            .unwrap()
            .classes
            .len(),
        4
    );
    for _ in 0..2 {
        rate(m.observe_request(&d(2), class()).unwrap(), 5, 1, 10_000).await;
    }
    assert!(m.snapshot(&d(2)).unwrap().allowance > 0);
}
