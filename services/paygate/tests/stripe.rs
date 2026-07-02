use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode},
    routing::post,
    Json, Router,
};
use mayhem_paygate::{
    paygate_router, stripe_signature_header, BoxFuture, CoinbaseSettings, ContractPostResult,
    ContractPoster, FiatChargebackFeature, FiatDepositFeature, OracleKeypair, PaygateConfig,
    PaygateState, RailConfig, StripeSettings,
};
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::Mutex};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct StripeCapture {
    requests: Arc<Mutex<Vec<StripeRequest>>>,
}

#[derive(Clone, Debug)]
struct StripeRequest {
    authorization: Option<String>,
    idempotency_key: Option<String>,
    body: String,
}

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
        feature: FiatChargebackFeature,
    ) -> BoxFuture<'a, mayhem_paygate::Result<ContractPostResult>> {
        Box::pin(async move {
            self.chargebacks.lock().await.push(feature.clone());
            Ok(ContractPostResult {
                tx: "4".repeat(64),
                command_hash: Some("5".repeat(64)),
                result: json!({
                    "ok": true,
                    "op": "fiatChargeback",
                    "rail": feature.rail,
                    "who": feature.who,
                    "mu": feature.mu,
                    "clawback_mu": feature.mu,
                    "network_absorbed_mu": 0,
                    "deposit_root": "6".repeat(64),
                }),
            })
        })
    }
}

async fn mock_create_payment_intent(
    State(capture): State<StripeCapture>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    let body = String::from_utf8(body.to_vec()).expect("form body utf8");
    capture.requests.lock().await.push(StripeRequest {
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        idempotency_key: headers
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body,
    });
    Json(json!({
        "id": "pi_test_123",
        "object": "payment_intent",
        "amount": 250,
        "currency": "usd",
        "client_secret": "pi_test_123_secret_abc",
        "status": "requires_payment_method"
    }))
}

async fn start_mock_stripe() -> (String, StripeCapture) {
    let capture = StripeCapture::default();
    let app = Router::new()
        .route("/v1/payment_intents", post(mock_create_payment_intent))
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock stripe");
    let addr = listener.local_addr().expect("mock stripe local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock stripe server");
    });
    (format!("http://{addr}"), capture)
}

fn test_config(stripe_base: String, event_store_path: std::path::PathBuf) -> PaygateConfig {
    let coinbase_event_store_path = event_store_path.with_file_name("coinbase-events.jsonl");
    PaygateConfig {
        contract_simulate: true,
        rails: RailConfig {
            stripe: StripeSettings {
                enabled: true,
                secret_key: Some("sk_test_local".to_owned()),
                webhook_secret: Some("whsec_test".to_owned()),
                api_base_url: stripe_base,
                event_store_path,
                ..StripeSettings::default()
            },
            coinbase: CoinbaseSettings {
                event_store_path: coinbase_event_store_path,
                ..CoinbaseSettings::default()
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

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs()
}

#[tokio::test]
async fn stripe_payment_intent_route_posts_canonical_mu_metadata_to_stripe() {
    let (stripe_base, capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(stripe_base, temp.path().join("stripe-events.jsonl")),
        OracleKeypair::from_seed_hex(&"11".repeat(32)).expect("oracle"),
        poster,
    )
    .expect("state");
    let app = paygate_router(state);

    let (status, body) = json_request(
        app,
        Method::POST,
        "/v1/stripe/payment-intents",
        json!({
            "who": "a".repeat(64),
            "mu": 2_500_000u64,
            "idempotency_key": "stripe-route-test-1"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["denom"], "mu_usd");
    assert_eq!(body["payment_intent"]["id"], "pi_test_123");
    assert_eq!(body["payment_intent"]["amount"], 250);

    let requests = capture.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .authorization
        .as_deref()
        .unwrap_or_default()
        .starts_with("Basic "));
    assert_eq!(
        requests[0].idempotency_key.as_deref(),
        Some("stripe-route-test-1")
    );
    assert!(requests[0].body.contains("amount=250"));
    assert!(requests[0].body.contains("currency=usd"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_who%5D=aaaaaaaa"));
    assert!(requests[0].body.contains("metadata%5Bmayhem_mu%5D=2500000"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_denom%5D=mu_usd"));
}

#[tokio::test]
async fn stripe_webhook_verifies_signature_posts_contract_once_and_dedups_replay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(
            "http://127.0.0.1:9".to_owned(),
            temp.path().join("stripe-events.jsonl"),
        ),
        OracleKeypair::from_seed_hex(&"22".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");
    let app = paygate_router(state);
    let payload = json!({
        "id": "evt_test_replay",
        "object": "event",
        "type": "payment_intent.succeeded",
        "created": 3_600,
        "data": {
            "object": {
                "id": "pi_test_replay",
                "object": "payment_intent",
                "latest_charge": "ch_test_replay",
                "amount_received": 250,
                "currency": "usd",
                "metadata": {
                    "mayhem_who": "b".repeat(64),
                    "mayhem_mu": "2500000",
                    "mayhem_denom": "mu_usd"
                }
            }
        }
    })
    .to_string();
    let signature =
        stripe_signature_header("whsec_test", payload.as_bytes(), now_seconds()).expect("sig");

    for expected_duplicate in [false, true] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/stripe/webhook")
                    .header("stripe-signature", &signature)
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
    }

    let deposits = poster.deposits.lock().await;
    assert_eq!(deposits.len(), 1);
    assert_eq!(deposits[0].op, "fiat_deposit");
    assert_eq!(deposits[0].rail, "stripe");
    assert_eq!(deposits[0].who, "b".repeat(64));
    assert_eq!(deposits[0].mu, 2_500_000);
    assert_eq!(deposits[0].epoch, 2);
    assert_eq!(deposits[0].at, 3_600);
    assert_eq!(deposits[0].ext_ref_hash.len(), 64);

    let event_log =
        std::fs::read_to_string(temp.path().join("stripe-events.jsonl")).expect("event log");
    assert_eq!(event_log.lines().count(), 1);
    assert!(event_log.contains("evt_test_replay"));
}

#[tokio::test]
async fn stripe_dispute_webhook_claws_back_once_and_dedups_replay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(
            "http://127.0.0.1:9".to_owned(),
            temp.path().join("stripe-events.jsonl"),
        ),
        OracleKeypair::from_seed_hex(&"33".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");
    let app = paygate_router(state);
    let deposit_payload = json!({
        "id": "evt_test_deposit_before_dispute",
        "object": "event",
        "type": "payment_intent.succeeded",
        "created": 3_600,
        "data": {
            "object": {
                "id": "pi_test_dispute",
                "object": "payment_intent",
                "latest_charge": "ch_test_dispute",
                "amount_received": 250,
                "currency": "usd",
                "metadata": {
                    "mayhem_who": "c".repeat(64),
                    "mayhem_mu": "2500000",
                    "mayhem_denom": "mu_usd"
                }
            }
        }
    })
    .to_string();
    let deposit_signature =
        stripe_signature_header("whsec_test", deposit_payload.as_bytes(), now_seconds())
            .expect("deposit sig");
    let deposit_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/stripe/webhook")
                .header("stripe-signature", &deposit_signature)
                .body(Body::from(deposit_payload.clone()))
                .expect("deposit request builds"),
        )
        .await
        .expect("deposit response");
    assert_eq!(deposit_response.status(), StatusCode::OK);

    let dispute_payload = json!({
        "id": "evt_test_dispute_replay",
        "object": "event",
        "type": "charge.dispute.created",
        "created": 7_200,
        "data": {
            "object": {
                "id": "dp_test_replay",
                "object": "dispute",
                "amount": 250,
                "currency": "usd",
                "charge": "ch_test_dispute",
                "payment_intent": "pi_test_dispute",
                "reason": "fraudulent",
                "status": "needs_response"
            }
        }
    })
    .to_string();
    let dispute_signature =
        stripe_signature_header("whsec_test", dispute_payload.as_bytes(), now_seconds())
            .expect("dispute sig");

    for expected_duplicate in [false, true] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/stripe/webhook")
                    .header("stripe-signature", &dispute_signature)
                    .body(Body::from(dispute_payload.clone()))
                    .expect("dispute request builds"),
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
        if !expected_duplicate {
            assert_eq!(body["clawed_back"], true);
            assert_eq!(body["dispute"], "dp_test_replay");
            assert_eq!(body["charge"], "ch_test_dispute");
        }
    }

    let deposits = poster.deposits.lock().await;
    let chargebacks = poster.chargebacks.lock().await;
    assert_eq!(deposits.len(), 1);
    assert_eq!(chargebacks.len(), 1);
    assert_eq!(chargebacks[0].op, "fiat_chargeback");
    assert_eq!(chargebacks[0].rail, "stripe");
    assert_eq!(chargebacks[0].who, "c".repeat(64));
    assert_eq!(chargebacks[0].mu, 2_500_000);
    assert_eq!(chargebacks[0].ext_ref_hash, deposits[0].ext_ref_hash);
    assert_eq!(chargebacks[0].dispute_ref_hash.len(), 64);
    assert_eq!(chargebacks[0].epoch, 3);
    assert_eq!(chargebacks[0].at, 7_200);

    let event_log =
        std::fs::read_to_string(temp.path().join("stripe-events.jsonl")).expect("event log");
    assert_eq!(event_log.lines().count(), 2);
    assert!(event_log.contains("evt_test_deposit_before_dispute"));
    assert!(event_log.contains("evt_test_dispute_replay"));
}
