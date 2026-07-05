use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use mayhem_paygate::{
    paygate_router, OracleKeypair, PaygateConfig, PaygateState, CREDIT_DENOM, SERVICE_NAME,
};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn health_reports_oracle_public_key_and_redacts_seed() {
    let seed_hex = "11".repeat(32);
    let oracle = OracleKeypair::from_seed_hex(&seed_hex).expect("oracle seed");
    let oracle_pubkey = oracle.public_key_hex();
    let mut config = PaygateConfig::default();
    config.rails.stripe.enabled = true;

    let app = paygate_router(PaygateState::new(config, oracle));
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("health body");
    let body: Value = serde_json::from_slice(&bytes).expect("health JSON");

    assert_eq!(body["ok"], true);
    assert_eq!(body["service"], SERVICE_NAME);
    assert_eq!(body["denom"], CREDIT_DENOM);
    assert_eq!(body["oracle_pubkey"], oracle_pubkey);
    assert_eq!(body["rails"]["stripe"]["enabled"], true);
    assert_eq!(body["rails"]["stripe"]["mode"], "test");
    assert_eq!(body["rails"]["coinbase"]["enabled"], false);
    assert_eq!(body["controls"]["admin_controls_economy"], true);
    assert_eq!(body["controls"]["admin_sets_terms"], true);
    assert_eq!(body["controls"]["admin_sets_prices"], true);
    assert_eq!(body["controls"]["admin_sets_rules"], true);
    assert_eq!(body["controls"]["admin_sets_params"], true);
    assert_eq!(body["controls"]["admin_sets_provider_payout_targets"], true);
    assert_eq!(body["controls"]["admin_can_ban_providers"], true);
    assert_eq!(body["controls"]["providers_set_prices"], false);
    assert_eq!(body["controls"]["providers_set_rules"], false);
    assert_eq!(body["controls"]["providers_set_params"], false);
    assert_eq!(body["controls"]["providers_set_payout_terms"], false);
    assert_eq!(body["controls"]["providers_submit_models"], false);
    assert_eq!(body["controls"]["providers_create_canonical_rooms"], false);
    assert_eq!(body["controls"]["providers_only_join_admin_rooms"], true);
    assert_eq!(
        body["controls"]["provider_payout_targets_admin_verified"],
        true
    );
    assert!(!String::from_utf8_lossy(&bytes).contains(&seed_hex));
}

#[tokio::test]
async fn versioned_health_route_is_available() {
    let oracle = OracleKeypair::from_seed_hex(&"22".repeat(32)).expect("oracle seed");
    let app = paygate_router(PaygateState::new(PaygateConfig::default(), oracle));
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);
}
