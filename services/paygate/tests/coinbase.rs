use std::sync::Arc;

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode},
    routing::post,
    Json, Router,
};
use mayhem_paygate::{
    coinbase_signature_header, paygate_router, BoxFuture, CoinbaseSettings, ContractPostResult,
    ContractPoster, FiatChargebackFeature, FiatDepositFeature, OracleKeypair, PaygateConfig,
    PaygateState, RailConfig, StripeSettings,
};
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::Mutex};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct CoinbaseCapture {
    requests: Arc<Mutex<Vec<CoinbaseRequest>>>,
}

#[derive(Clone, Debug)]
struct CoinbaseRequest {
    api_key: Option<String>,
    body: String,
}

#[derive(Clone, Default)]
struct RecordingContractPoster {
    deposits: Arc<Mutex<Vec<FiatDepositFeature>>>,
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
                result: json!({
                    "ok": true,
                    "op": "fiatDeposit",
                    "rail": feature.rail,
                    "who": feature.who,
                    "mu": feature.mu,
                    "epoch": feature.epoch,
                    "deposit_root": "3".repeat(64),
                }),
            })
        })
    }

    fn post_fiat_chargeback<'a>(
        &'a self,
        _oracle: &'a OracleKeypair,
        _feature: FiatChargebackFeature,
    ) -> BoxFuture<'a, mayhem_paygate::Result<ContractPostResult>> {
        Box::pin(async move {
            Ok(ContractPostResult {
                tx: "4".repeat(64),
                command_hash: Some("5".repeat(64)),
                result: json!({ "ok": true }),
            })
        })
    }
}

async fn mock_create_charge(
    State(capture): State<CoinbaseCapture>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    let body = String::from_utf8(body.to_vec()).expect("json body utf8");
    capture.requests.lock().await.push(CoinbaseRequest {
        api_key: headers
            .get("x-cc-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body,
    });
    Json(json!({
        "data": {
            "id": "coinbase-charge-id",
            "code": "CB123",
            "hosted_url": "https://commerce.coinbase.com/charges/CB123",
            "pricing": {
                "local": {
                    "amount": "2.50",
                    "currency": "USD"
                }
            },
            "expires_at": "2026-07-02T12:00:00Z"
        }
    }))
}

async fn start_mock_coinbase() -> (String, CoinbaseCapture) {
    let capture = CoinbaseCapture::default();
    let app = Router::new()
        .route("/charges", post(mock_create_charge))
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock coinbase");
    let addr = listener.local_addr().expect("mock coinbase local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock coinbase server");
    });
    (format!("http://{addr}"), capture)
}

fn test_config(coinbase_base: String, temp: &std::path::Path) -> PaygateConfig {
    PaygateConfig {
        contract_simulate: true,
        rails: RailConfig {
            stripe: StripeSettings {
                event_store_path: temp.join("stripe-events.jsonl"),
                ..StripeSettings::default()
            },
            coinbase: CoinbaseSettings {
                enabled: true,
                api_key: Some("cc_test_local".to_owned()),
                webhook_secret: Some("ccwhsec_test".to_owned()),
                api_base_url: coinbase_base,
                event_store_path: temp.join("coinbase-events.jsonl"),
            },
        },
        ..PaygateConfig::default()
    }
}

async fn json_request(app: Router, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
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

#[tokio::test]
async fn coinbase_charge_route_posts_canonical_mu_metadata_to_commerce() {
    let (coinbase_base, capture) = start_mock_coinbase().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(coinbase_base, temp.path()),
        OracleKeypair::from_seed_hex(&"44".repeat(32)).expect("oracle"),
        poster,
    )
    .expect("state");
    let app = paygate_router(state);

    let (status, body) = json_request(
        app,
        Method::POST,
        "/v1/coinbase/charges",
        json!({
            "who": "d".repeat(64),
            "mu": 2_500_000u64,
            "redirect_url": "https://mayhem.local/pay/ok",
            "cancel_url": "https://mayhem.local/pay/cancel"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["denom"], "mu_usd");
    assert_eq!(body["rail"], "coinbase");
    assert_eq!(body["charge"]["id"], "coinbase-charge-id");
    assert_eq!(body["charge"]["code"], "CB123");
    assert_eq!(body["charge"]["amount"], "2.50");
    assert_eq!(body["charge"]["currency"], "USD");

    let requests = capture.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].api_key.as_deref(), Some("cc_test_local"));
    let request_body: Value = serde_json::from_str(&requests[0].body).expect("coinbase JSON");
    assert_eq!(request_body["pricing_type"], "fixed_price");
    assert_eq!(request_body["local_price"]["amount"], "2.50");
    assert_eq!(request_body["local_price"]["currency"], "USD");
    assert_eq!(request_body["metadata"]["mayhem_who"], "d".repeat(64));
    assert_eq!(request_body["metadata"]["mayhem_mu"], "2500000");
    assert_eq!(request_body["metadata"]["mayhem_denom"], "mu_usd");
    assert_eq!(request_body["redirect_url"], "https://mayhem.local/pay/ok");
    assert_eq!(
        request_body["cancel_url"],
        "https://mayhem.local/pay/cancel"
    );
}

#[tokio::test]
async fn coinbase_webhook_verifies_signature_posts_contract_once_and_dedups_replay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config("http://127.0.0.1:9".to_owned(), temp.path()),
        OracleKeypair::from_seed_hex(&"55".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");
    let app = paygate_router(state);
    let payload = json!({
        "event": {
            "id": "evt_coinbase_confirmed",
            "type": "charge:confirmed",
            "created": 7_200,
            "data": {
                "id": "coinbase-charge-id",
                "code": "CB123",
                "pricing": {
                    "local": {
                        "amount": "2.50",
                        "currency": "USD"
                    }
                },
                "metadata": {
                    "mayhem_who": "e".repeat(64),
                    "mayhem_mu": "2500000",
                    "mayhem_denom": "mu_usd"
                }
            }
        }
    })
    .to_string();
    let signature =
        coinbase_signature_header("ccwhsec_test", payload.as_bytes()).expect("signature");

    for expected_duplicate in [false, true] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/coinbase/webhook")
                    .header("x-cc-webhook-signature", &signature)
                    .body(Body::from(payload.clone()))
                    .expect("request builds"),
            )
            .await
            .expect("router response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response bytes");
        let body: Value = serde_json::from_slice(&bytes).expect("response JSON");
        assert_eq!(body["ok"], true);
        assert_eq!(body["duplicate"], expected_duplicate);
        assert_eq!(body["credited"], !expected_duplicate);
        if !expected_duplicate {
            assert_eq!(body["charge"], "coinbase-charge-id");
            assert_eq!(body["code"], "CB123");
            assert_eq!(body["mu"], 2_500_000);
        }
    }

    let deposits = poster.deposits.lock().await;
    assert_eq!(deposits.len(), 1);
    assert_eq!(deposits[0].op, "fiat_deposit");
    assert_eq!(deposits[0].rail, "coinbase");
    assert_eq!(deposits[0].who, "e".repeat(64));
    assert_eq!(deposits[0].mu, 2_500_000);
    assert_eq!(deposits[0].epoch, 3);
    assert_eq!(deposits[0].at, 7_200);
    assert_eq!(deposits[0].ext_ref_hash.len(), 64);

    let event_log =
        std::fs::read_to_string(temp.path().join("coinbase-events.jsonl")).expect("event log");
    assert_eq!(event_log.lines().count(), 1);
    assert!(event_log.contains("evt_coinbase_confirmed"));
}
