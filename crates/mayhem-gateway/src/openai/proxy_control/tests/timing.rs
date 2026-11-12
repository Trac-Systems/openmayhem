use super::*;

#[test]
fn refresh_timing_counts_both_slow_rpc_budgets_and_local_headroom() {
    assert!(validate_refresh_timing(&refresh_policy(), rpc_timeout_ms()).is_ok());
    let policy = RefreshPolicy {
        interval_ms: 1_000,
        jitter_percent: 0,
        ..refresh_policy()
    };
    // Old interval + RPC validation accepted both of these. Two 8 s calls
    // cannot fit; even two 6.5 s calls leave no configured local headroom.
    assert!(validate_refresh_timing(&policy, 8_000).is_err());
    assert!(validate_refresh_timing(&policy, 6_500).is_err());
    assert!(validate_refresh_timing(&policy, 6_499).is_ok());

    let policy = RefreshPolicy {
        interval_ms: 10_000,
        jitter_percent: 20,
        ..refresh_policy()
    };
    assert!(validate_refresh_timing(&policy, 2_000).is_err());
    assert!(validate_refresh_timing(&policy, 1_999).is_ok());
    assert!(validate_refresh_timing(&policy, u64::MAX).is_err());
    assert!(validate_refresh_timing(&policy, 0).is_err());
}

#[test]
fn protected_config_enforces_full_refresh_timing_before_opening_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = config(
        dir.path(),
        "http://127.0.0.1:1".into(),
        "ws://127.0.0.1:2".into(),
    );
    let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["refresh"]["interval_ms"] = json!(1_000);
    document["rpc_timeout_ms"] = json!(8_000);
    write(&path, &serde_json::to_vec(&document).unwrap());
    assert!(matches!(
        Prepared::load(&path, &identity()),
        Err(Error::Configuration)
    ));
    assert!(!dir.path().join("proxy-state").exists());
}
