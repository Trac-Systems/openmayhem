use super::*;

#[tokio::test]
async fn proxy_capacity_refusal_retains_unsigned_evidence_and_503_replays_without_dispatch() {
    let mut f = Fixture::start().await;
    let first = f.harness.reserve_test_capacity(1701);
    let second = f.harness.reserve_test_capacity(1703);
    let body = f.body();
    let (status, headers, rejected) = f
        .post("capacity-refusal", body.clone(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{rejected}");
    let id = headers["x-mayhem-job-id"].to_str().unwrap();
    assert_eq!(rejected["status"], "failed");
    assert_eq!(rejected["error_info"]["code"], "capacity_unavailable");
    assert_eq!(rejected["error_info"]["retryable"], true);
    let (status, evidence) = f
        .get(
            &format!("/v1/jobs/{id}/proxy-evidence"),
            "owner-fixture-key",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(evidence["financial"], json!({"kind":"not_authorized"}));
    assert_eq!(evidence["admission_failure"], "capacity");
    assert_eq!(evidence["phase"], "not_authorized");
    assert_eq!(evidence["terms"], Value::Null);
    assert_eq!(evidence["acceptance"], Value::Null);
    assert_eq!(evidence["result_verified"], false);
    let identity: RequestIdentity = serde_json::from_value(evidence["identity"].clone()).unwrap();
    assert!(f
        .harness
        .negotiation
        .lookup(identity.billing_id.clone(), identity.billing_attempt)
        .await
        .unwrap()
        .is_none());
    assert!(f
        .state
        .access_control
        .pending_key_budgets(None, 64)
        .unwrap()
        .is_empty());
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    f.harness.release_test_capacity(first);
    f.harness.release_test_capacity(second);
    // Releasing capacity never reopens this terminal idempotency identity.
    let (status, _, replay) = f
        .post("capacity-refusal", body, "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(replay, rejected);
    let (status, result) = f
        .get(&format!("/v1/jobs/{id}/result"), "owner-fixture-key")
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
    assert_eq!(result["error_info"]["code"], "capacity_unavailable");
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    f.stop().await;
}
