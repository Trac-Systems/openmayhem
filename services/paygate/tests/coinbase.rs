use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use mayhem_paygate::{
    paygate_router, BoxFuture, CoinbaseSettings, ContractPostResult, ContractPoster,
    FiatChargebackFeature, FiatDepositFeature, OracleKeypair, PaygateConfig, PaygateState,
    RailConfig, StripeSettings, COINBASE_RETIRED_MESSAGE,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tower::ServiceExt;

#[derive(Clone, Default)]
struct RecordingContractPoster {
    deposits: Arc<Mutex<Vec<FiatDepositFeature>>>,
    chargebacks: Arc<Mutex<Vec<FiatChargebackFeature>>>,
}

impl ContractPoster for RecordingContractPoster {
    fn post_fiat_deposit<'a>(
        &'a self,
        _oracle: &'a OracleKeypair,
        feature: FiatDepositFeature,
    ) -> BoxFuture<'a, mayhem_paygate::Result<ContractPostResult>> {
        Box::pin(async move {
            self.deposits.lock().await.push(feature.clone());
            Ok(ContractPostResult {
                tx: "1".repeat(64),
                command_hash: Some("2".repeat(64)),
                result: json!({ "ok": true }),
            })
        })
    }

    fn post_fiat_chargeback<'a>(
        &'a self,
        _oracle: &'a OracleKeypair,
        feature: FiatChargebackFeature,
    ) -> BoxFuture<'a, mayhem_paygate::Result<ContractPostResult>> {
        Box::pin(async move {
            self.chargebacks.lock().await.push(feature.clone());
            Ok(ContractPostResult {
                tx: "3".repeat(64),
                command_hash: Some("4".repeat(64)),
                result: json!({ "ok": true }),
            })
        })
    }
}

fn test_config(temp: &std::path::Path) -> PaygateConfig {
    PaygateConfig {
        contract_dry_run: true,
        rails: RailConfig {
            stripe: StripeSettings {
                event_store_path: temp.join("stripe-events.jsonl"),
                ..StripeSettings::default()
            },
            coinbase: CoinbaseSettings {
                event_store_path: temp.join("coinbase-events.jsonl"),
                ..CoinbaseSettings::default()
            },
        },
        ..PaygateConfig::default()
    }
}

async fn request(app: axum::Router, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request builds"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    let body = serde_json::from_slice(&bytes).expect("response JSON");
    (status, body)
}

#[test]
fn coinbase_enabled_config_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.rails.coinbase.enabled = true;
    config.rails.coinbase.api_key = Some("cc_test_local".to_owned());
    config.rails.coinbase.webhook_secret = Some("ccwhsec_test".to_owned());

    let err = match PaygateState::try_new_with_contract_poster(
        config,
        OracleKeypair::from_seed_hex(&"44".repeat(32)).expect("oracle"),
        Arc::new(RecordingContractPoster::default()),
    ) {
        Ok(_) => panic!("coinbase-enabled config must fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains(COINBASE_RETIRED_MESSAGE));
}

#[tokio::test]
async fn coinbase_routes_are_retired_and_do_not_credit_contract() {
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(temp.path()),
        OracleKeypair::from_seed_hex(&"55".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");
    let app = paygate_router(state);

    for (method, uri) in [
        (Method::POST, "/v1/coinbase/charges"),
        (Method::POST, "/v1/coinbase/webhook"),
        (Method::GET, "/v1/coinbase/return"),
        (Method::GET, "/v1/coinbase/cancel"),
    ] {
        let (status, body) = request(
            app.clone(),
            method,
            uri,
            json!({
                "who": "d".repeat(64),
                "mu": 2_500_000u64
            }),
        )
        .await;
        assert_eq!(status, StatusCode::GONE, "{uri}");
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], COINBASE_RETIRED_MESSAGE);
    }

    assert!(poster.deposits.lock().await.is_empty());
    assert!(poster.chargebacks.lock().await.is_empty());
    assert!(!temp.path().join("coinbase-events.jsonl").exists());
}
