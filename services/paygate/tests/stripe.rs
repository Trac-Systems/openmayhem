use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Query, State},
    http::{HeaderMap, Method, Request, StatusCode},
    routing::{get, post},
    Json, Router,
};
use mayhem_paygate::{
    paygate_router, run_stripe_backfill_once, stripe_signature_header, BoxFuture,
    ContractPostResult, ContractPoster, FiatChargebackFeature, FiatDepositFeature, OracleKeypair,
    PaygateConfig, PaygateState, RailConfig, StripeSettings, DEFAULT_STRIPE_API_BASE_URL,
};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};
use tokio::{net::TcpListener, sync::Mutex};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct StripeCapture {
    requests: Arc<Mutex<Vec<StripeRequest>>>,
    events: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone, Debug)]
struct StripeRequest {
    authorization: Option<String>,
    idempotency_key: Option<String>,
    body: String,
}

fn requested_currency(body: &str) -> &'static str {
    if body.contains("currency=eur") || body.contains("currency%5D=eur") {
        "eur"
    } else {
        "usd"
    }
}

#[derive(Clone, Default)]
struct RecordingContractPoster {
    deposits: Arc<Mutex<Vec<FiatDepositFeature>>>,
    chargebacks: Arc<Mutex<Vec<FiatChargebackFeature>>>,
    epoch_seconds: Arc<Mutex<Option<u64>>>,
}

impl ContractPoster for RecordingContractPoster {
    fn epoch_seconds_at<'a>(
        &'a self,
        _at: u64,
        fallback: u64,
    ) -> BoxFuture<'a, mayhem_paygate::Result<u64>> {
        Box::pin(async move { Ok((*self.epoch_seconds.lock().await).unwrap_or(fallback)) })
    }

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
                    "au": feature.au.to_string(),
                    "fiat_currency": feature.fiat_currency,
                    "fiat_amount_minor": feature.fiat_amount_minor,
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
                    "au": feature.au.to_string(),
                    "fiat_currency": feature.fiat_currency,
                    "fiat_amount_minor": feature.fiat_amount_minor,
                    "clawback_au": feature.au.to_string(),
                    "network_absorbed_au": "0",
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
    let currency = requested_currency(&body);
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
        "currency": currency,
        "client_secret": "pi_test_123_secret_abc",
        "status": "requires_payment_method"
    }))
}

async fn mock_create_checkout_session(
    State(capture): State<StripeCapture>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    let body = String::from_utf8(body.to_vec()).expect("form body utf8");
    let currency = requested_currency(&body);
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
        "id": "cs_test_123",
        "object": "checkout.session",
        "url": "https://checkout.stripe.com/c/pay/cs_test_123",
        "amount_total": 250,
        "currency": currency,
        "payment_intent": "pi_test_123",
        "payment_status": "unpaid",
        "status": "open",
        "expires_at": 1_900_000_000u64
    }))
}

async fn mock_list_events(
    State(capture): State<StripeCapture>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<Value> {
    capture.requests.lock().await.push(StripeRequest {
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        idempotency_key: None,
        body: format!("events:{query:?}"),
    });
    let event_type = query.get("type").map(String::as_str);
    let created_gte = query
        .get("created[gte]")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let mut data = capture.events.lock().await.clone();
    data.retain(|event| {
        let type_ok = event_type
            .map(|expected| event.get("type").and_then(Value::as_str) == Some(expected))
            .unwrap_or(true);
        let created_ok = event.get("created").and_then(Value::as_u64).unwrap_or(0) >= created_gte;
        type_ok && created_ok
    });
    data.sort_by(|left, right| {
        (
            right.get("created").and_then(Value::as_u64).unwrap_or(0),
            right.get("id").and_then(Value::as_str).unwrap_or(""),
        )
            .cmp(&(
                left.get("created").and_then(Value::as_u64).unwrap_or(0),
                left.get("id").and_then(Value::as_str).unwrap_or(""),
            ))
    });
    Json(json!({
        "object": "list",
        "data": data,
        "has_more": false
    }))
}

async fn start_mock_stripe() -> (String, StripeCapture) {
    let capture = StripeCapture::default();
    let app = Router::new()
        .route("/v1/payment_intents", post(mock_create_payment_intent))
        .route("/v1/checkout/sessions", post(mock_create_checkout_session))
        .route("/v1/events", get(mock_list_events))
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
    let backfill_cursor_path = event_store_path.with_file_name("stripe-backfill-cursor.json");
    PaygateConfig {
        contract_dry_run: true,
        rails: RailConfig {
            stripe: StripeSettings {
                enabled: true,
                secret_key: Some("sk_test_local".to_owned()),
                webhook_secret: Some("whsec_test".to_owned()),
                api_base_url: stripe_base,
                event_store_path,
                backfill_cursor_path,
                ..StripeSettings::default()
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
async fn stripe_payment_intent_route_posts_canonical_au_metadata_to_stripe() {
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
            "au": "2500000000000000000",
            "currency": "eur",
            "idempotency_key": "stripe-route-test-1"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["denom"], "au_usd");
    assert_eq!(body["payment_intent"]["id"], "pi_test_123");
    assert_eq!(body["payment_intent"]["amount"], 250);
    assert_eq!(body["payment_intent"]["currency"], "eur");

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
    assert!(requests[0].body.contains("currency=eur"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_who%5D=aaaaaaaa"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_au%5D=2500000000000000000"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_denom%5D=au_usd"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_fiat_currency%5D=eur"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_fiat_amount_minor%5D=250"));
}

#[tokio::test]
async fn stripe_checkout_session_route_returns_hosted_url_and_binds_payment_intent_metadata() {
    let (stripe_base, capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(stripe_base, temp.path().join("stripe-events.jsonl")),
        OracleKeypair::from_seed_hex(&"12".repeat(32)).expect("oracle"),
        poster,
    )
    .expect("state");
    let app = paygate_router(state);

    let (status, body) = json_request(
        app,
        Method::POST,
        "/v1/stripe/checkout-sessions",
        json!({
            "who": "a".repeat(64),
            "au": "2500000000000000000",
            "success_url": "http://127.0.0.1:11436/v1/stripe/return?session_id={CHECKOUT_SESSION_ID}",
            "cancel_url": "http://127.0.0.1:11436/v1/stripe/cancel",
            "currency": "usd",
            "locale": "en",
            "idempotency_key": "stripe-checkout-test-1"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["denom"], "au_usd");
    assert_eq!(body["checkout_session"]["id"], "cs_test_123");
    assert_eq!(
        body["checkout_session"]["url"],
        "https://checkout.stripe.com/c/pay/cs_test_123"
    );
    assert_eq!(
        body["copy_paste"]["checkout_url"],
        "https://checkout.stripe.com/c/pay/cs_test_123"
    );
    assert_eq!(body["checkout_session"]["amount_total"], 250);

    let requests = capture.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .authorization
        .as_deref()
        .unwrap_or_default()
        .starts_with("Basic "));
    assert_eq!(
        requests[0].idempotency_key.as_deref(),
        Some("stripe-checkout-test-1")
    );
    assert!(requests[0].body.contains("mode=payment"));
    assert!(requests[0].body.contains("locale=en"));
    assert!(requests[0]
        .body
        .contains("line_items%5B0%5D%5Bprice_data%5D%5Bunit_amount%5D=250"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_denom%5D=au_usd"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_fiat_currency%5D=usd"));
    assert!(requests[0]
        .body
        .contains("metadata%5Bmayhem_fiat_amount_minor%5D=250"));
    assert!(requests[0]
        .body
        .contains("payment_intent_data%5Bmetadata%5D%5Bmayhem_who%5D=aaaaaaaa"));
    assert!(requests[0]
        .body
        .contains("payment_intent_data%5Bmetadata%5D%5Bmayhem_au%5D=2500000000000000000"));
    assert!(requests[0]
        .body
        .contains("payment_intent_data%5Bmetadata%5D%5Bmayhem_denom%5D=au_usd"));
    assert!(requests[0]
        .body
        .contains("payment_intent_data%5Bmetadata%5D%5Bmayhem_fiat_currency%5D=usd"));
    assert!(requests[0]
        .body
        .contains("payment_intent_data%5Bmetadata%5D%5Bmayhem_fiat_amount_minor%5D=250"));
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
                    "mayhem_au": "2500000000000000000",
                    "mayhem_denom": "au_usd",
                    "mayhem_fiat_currency": "usd",
                    "mayhem_fiat_amount_minor": "250"
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
    assert_eq!(deposits[0].au, 2_500_000_000_000_000_000u128);
    assert_eq!(deposits[0].fiat_currency, "usd");
    assert_eq!(deposits[0].fiat_amount_minor, 250);
    assert_eq!(deposits[0].epoch, 2);
    assert_eq!(deposits[0].at, 3_600);
    assert_eq!(deposits[0].ext_ref_hash.len(), 64);

    let event_log =
        std::fs::read_to_string(temp.path().join("stripe-events.jsonl")).expect("event log");
    assert_eq!(event_log.lines().count(), 1);
    assert!(event_log.contains("evt_test_replay"));
}

#[tokio::test]
async fn stripe_backfill_pulls_events_api_from_cursor_and_dedups_replay() {
    let (stripe_base, capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(stripe_base, temp.path().join("stripe-events.jsonl")),
        OracleKeypair::from_seed_hex(&"55".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");

    *capture.events.lock().await = vec![
        json!({
            "id": "evt_backfill_unrelated_payment",
            "object": "event",
            "type": "payment_intent.succeeded",
            "created": 1_800,
            "data": {
                "object": {
                    "id": "pi_unrelated",
                    "object": "payment_intent",
                    "latest_charge": "ch_unrelated",
                    "amount_received": 500,
                    "currency": "usd",
                    "metadata": {
                        "other_product": "shared-stripe-account"
                    }
                }
            }
        }),
        json!({
            "id": "evt_backfill_dispute",
            "object": "event",
            "type": "charge.dispute.created",
            "created": 7_200,
            "data": {
                "object": {
                    "id": "dp_backfill",
                    "object": "dispute",
                    "amount": 250,
                    "currency": "usd",
                    "charge": "ch_backfill",
                    "payment_intent": "pi_backfill",
                    "reason": "fraudulent",
                    "status": "needs_response"
                }
            }
        }),
        json!({
            "id": "evt_backfill_unrelated_dispute",
            "object": "event",
            "type": "charge.dispute.created",
            "created": 5_400,
            "data": {
                "object": {
                    "id": "dp_unrelated",
                    "object": "dispute",
                    "amount": 500,
                    "currency": "usd",
                    "charge": "ch_unrelated",
                    "payment_intent": "pi_unrelated",
                    "reason": "fraudulent",
                    "status": "needs_response"
                }
            }
        }),
        json!({
            "id": "evt_backfill_deposit",
            "object": "event",
            "type": "payment_intent.succeeded",
            "created": 3_600,
            "data": {
                "object": {
                    "id": "pi_backfill",
                    "object": "payment_intent",
                    "latest_charge": "ch_backfill",
                    "amount_received": 250,
                    "currency": "usd",
                    "metadata": {
                        "mayhem_who": "f".repeat(64),
                        "mayhem_au": "2500000000000000000",
                        "mayhem_denom": "au_usd",
                        "mayhem_fiat_currency": "usd",
                        "mayhem_fiat_amount_minor": "250"
                    }
                }
            }
        }),
    ];

    let first = run_stripe_backfill_once(&state)
        .await
        .expect("first backfill");
    assert_eq!(first.ok, true);
    assert_eq!(first.fetched, 4);
    assert_eq!(first.processed, 4);
    assert_eq!(first.duplicates, 0);
    assert_eq!(first.credited, 1);
    assert_eq!(first.clawed_back, 1);
    assert_eq!(first.ignored, 2);
    assert_eq!(first.previous_last_created, 0);
    assert_eq!(first.last_created, 7_200);

    let deposits = poster.deposits.lock().await;
    let chargebacks = poster.chargebacks.lock().await;
    assert_eq!(deposits.len(), 1);
    assert_eq!(chargebacks.len(), 1);
    assert_eq!(deposits[0].ext_ref_hash, chargebacks[0].ext_ref_hash);
    assert_eq!(deposits[0].ext_ref_hash.len(), 64);
    assert_eq!(chargebacks[0].dispute_ref_hash.len(), 64);
    drop(deposits);
    drop(chargebacks);

    let cursor_text =
        std::fs::read_to_string(temp.path().join("stripe-backfill-cursor.json")).expect("cursor");
    let cursor: Value = serde_json::from_str(&cursor_text).expect("cursor json");
    assert_eq!(cursor["schema_version"], 1);
    assert_eq!(cursor["last_created"], 7_200);

    let second = run_stripe_backfill_once(&state)
        .await
        .expect("second backfill");
    assert_eq!(second.previous_last_created, 7_200);
    assert_eq!(second.last_created, 7_200);
    assert_eq!(second.fetched, 1);
    assert_eq!(second.processed, 1);
    assert_eq!(second.duplicates, 1);
    assert_eq!(second.credited, 0);
    assert_eq!(second.clawed_back, 0);
    assert_eq!(poster.deposits.lock().await.len(), 1);
    assert_eq!(poster.chargebacks.lock().await.len(), 1);

    let event_log =
        std::fs::read_to_string(temp.path().join("stripe-events.jsonl")).expect("event log");
    assert_eq!(event_log.lines().count(), 2);
    assert!(event_log.contains("evt_backfill_deposit"));
    assert!(event_log.contains("evt_backfill_dispute"));
}

#[tokio::test]
async fn stripe_backfill_rejects_partial_mayhem_metadata_without_advancing_cursor() {
    let (stripe_base, capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let event_store_path = temp.path().join("stripe-events.jsonl");
    let cursor_path = event_store_path.with_file_name("stripe-backfill-cursor.json");
    let state = PaygateState::try_new_with_contract_poster(
        test_config(stripe_base, event_store_path),
        OracleKeypair::from_seed_hex(&"57".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");

    *capture.events.lock().await = vec![json!({
        "id": "evt_backfill_partial_mayhem",
        "object": "event",
        "type": "payment_intent.succeeded",
        "created": 3_600,
        "data": {
            "object": {
                "id": "pi_partial_mayhem",
                "object": "payment_intent",
                "latest_charge": "ch_partial_mayhem",
                "amount_received": 100,
                "currency": "usd",
                "metadata": {
                    "mayhem_au": "1000000000000000000"
                }
            }
        }
    })];

    let error = run_stripe_backfill_once(&state)
        .await
        .expect_err("partial Mayhem metadata must fail closed");
    assert!(error
        .to_string()
        .contains("PaymentIntent missing mayhem_who metadata"));
    assert!(!cursor_path.exists());
    assert!(poster.deposits.lock().await.is_empty());
}

#[tokio::test]
#[ignore = "requires a real Stripe test key and MAYHEM_STRIPE_REAL_BACKFILL=1"]
async fn stripe_real_events_api_backfill_credits_created_test_payment_intent() {
    if std::env::var("MAYHEM_STRIPE_REAL_BACKFILL").as_deref() != Ok("1") {
        panic!("set MAYHEM_STRIPE_REAL_BACKFILL=1 to run the real Stripe backfill proof");
    }
    let secret_key = std::env::var("MAYHEM_STRIPE_SECRET_KEY")
        .or_else(|_| std::env::var("STRIPE_SECRET_KEY"))
        .expect("MAYHEM_STRIPE_SECRET_KEY or STRIPE_SECRET_KEY must be set");
    assert!(
        secret_key.starts_with("sk_test_"),
        "real Stripe backfill proof must use a Stripe test-mode key"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let mut config = test_config(
        DEFAULT_STRIPE_API_BASE_URL.to_owned(),
        temp.path().join("stripe-events.jsonl"),
    );
    config.rails.stripe.secret_key = Some(secret_key.clone());
    let cursor_path = config.rails.stripe.backfill_cursor_path.clone();
    let state = PaygateState::try_new_with_contract_poster(
        config,
        OracleKeypair::from_seed_hex(&"66".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");

    let who = "9".repeat(64);
    let request_marker = format!("mayhem-a16-backfill-{}", now_seconds());
    let params = vec![
        ("amount", "100".to_owned()),
        ("currency", "usd".to_owned()),
        ("payment_method", "pm_card_visa".to_owned()),
        ("confirm", "true".to_owned()),
        ("automatic_payment_methods[enabled]", "true".to_owned()),
        (
            "automatic_payment_methods[allow_redirects]",
            "never".to_owned(),
        ),
        ("metadata[mayhem_who]", who.clone()),
        ("metadata[mayhem_au]", "1000000000000000000".to_owned()),
        ("metadata[mayhem_denom]", "au_usd".to_owned()),
        ("metadata[mayhem_fiat_currency]", "usd".to_owned()),
        ("metadata[mayhem_fiat_amount_minor]", "100".to_owned()),
        ("metadata[mayhem_test_marker]", request_marker),
    ];
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "{}/v1/payment_intents",
            DEFAULT_STRIPE_API_BASE_URL.trim_end_matches('/')
        ))
        .basic_auth(&secret_key, Some(""))
        .form(&params)
        .send()
        .await
        .expect("create Stripe test PaymentIntent");
    let status = response.status();
    let body = response.text().await.expect("Stripe response body");
    assert!(
        status.is_success(),
        "Stripe test PaymentIntent create failed {status}: {body}"
    );
    let payment_intent: Value = serde_json::from_str(&body).expect("PaymentIntent JSON");
    let payment_intent_id = payment_intent
        .get("id")
        .and_then(Value::as_str)
        .expect("PaymentIntent id");
    assert_eq!(
        payment_intent.get("status").and_then(Value::as_str),
        Some("succeeded")
    );
    let created = payment_intent
        .get("created")
        .and_then(Value::as_u64)
        .unwrap_or_else(now_seconds);
    std::fs::write(
        &cursor_path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "last_created": created
        }))
        .expect("cursor JSON"),
    )
    .expect("write cursor");

    let mut saw_credit_report = false;
    let mut total_fetched = 0_usize;
    for _ in 0..10 {
        let report = run_stripe_backfill_once(&state)
            .await
            .expect("real Stripe events backfill");
        total_fetched += report.fetched;
        saw_credit_report |= report.credited > 0;
        let deposits = poster.deposits.lock().await;
        if deposits
            .iter()
            .any(|deposit| deposit.who == who && deposit.au == 1_000_000_000_000_000_000u128)
        {
            break;
        }
        drop(deposits);
        sleep(Duration::from_secs(2)).await;
    }

    let deposits = poster.deposits.lock().await;
    let deposit = deposits
        .iter()
        .find(|deposit| deposit.who == who && deposit.au == 1_000_000_000_000_000_000u128)
        .expect("backfill credited the created Stripe test PaymentIntent");
    assert_eq!(deposit.rail, "stripe");
    assert_eq!(deposit.fiat_currency, "usd");
    assert_eq!(deposit.fiat_amount_minor, 100);
    assert_eq!(deposit.ext_ref_hash.len(), 64);
    assert!(total_fetched >= 1);
    assert!(saw_credit_report);
    let event_log =
        std::fs::read_to_string(temp.path().join("stripe-events.jsonl")).expect("event log");
    assert!(event_log.contains(payment_intent_id));
}

#[tokio::test]
async fn stripe_webhook_uses_contract_epoch_seconds_for_deposit_epoch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    *poster.epoch_seconds.lock().await = Some(7_200);
    let state = PaygateState::try_new_with_contract_poster(
        test_config(
            "http://127.0.0.1:9".to_owned(),
            temp.path().join("stripe-events.jsonl"),
        ),
        OracleKeypair::from_seed_hex(&"44".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");
    let app = paygate_router(state);
    let payload = json!({
        "id": "evt_test_admin_epoch_seconds",
        "object": "event",
        "type": "payment_intent.succeeded",
        "created": 3_600,
        "data": {
            "object": {
                "id": "pi_test_admin_epoch_seconds",
                "object": "payment_intent",
                "latest_charge": "ch_test_admin_epoch_seconds",
                "amount_received": 250,
                "currency": "usd",
                "metadata": {
                    "mayhem_who": "e".repeat(64),
                    "mayhem_au": "2500000000000000000",
                    "mayhem_denom": "au_usd",
                    "mayhem_fiat_currency": "usd",
                    "mayhem_fiat_amount_minor": "250"
                }
            }
        }
    })
    .to_string();
    let signature =
        stripe_signature_header("whsec_test", payload.as_bytes(), now_seconds()).expect("sig");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/stripe/webhook")
                .header("stripe-signature", &signature)
                .body(Body::from(payload))
                .expect("request builds"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);

    let deposits = poster.deposits.lock().await;
    assert_eq!(deposits.len(), 1);
    assert_eq!(deposits[0].epoch, 1);
    assert_eq!(deposits[0].at, 3_600);
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
                    "mayhem_au": "2500000000000000000",
                    "mayhem_denom": "au_usd",
                    "mayhem_fiat_currency": "usd",
                    "mayhem_fiat_amount_minor": "250"
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
    assert_eq!(chargebacks[0].au, 2_500_000_000_000_000_000u128);
    assert_eq!(chargebacks[0].fiat_currency, "usd");
    assert_eq!(chargebacks[0].fiat_amount_minor, 250);
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

#[tokio::test]
async fn stripe_dispute_cannot_claw_back_more_than_original_deposit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        test_config(
            "http://127.0.0.1:9".to_owned(),
            temp.path().join("stripe-events.jsonl"),
        ),
        OracleKeypair::from_seed_hex(&"34".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");
    let app = paygate_router(state);
    let deposit_payload = json!({
        "id": "evt_test_deposit_before_oversized_dispute",
        "object": "event",
        "type": "payment_intent.succeeded",
        "created": 3_600,
        "data": {
            "object": {
                "id": "pi_test_oversized_dispute",
                "object": "payment_intent",
                "latest_charge": "ch_test_oversized_dispute",
                "amount_received": 250,
                "currency": "usd",
                "metadata": {
                    "mayhem_who": "d".repeat(64),
                    "mayhem_au": "2500000000000000000",
                    "mayhem_denom": "au_usd",
                    "mayhem_fiat_currency": "usd",
                    "mayhem_fiat_amount_minor": "250"
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
                .body(Body::from(deposit_payload))
                .expect("deposit request builds"),
        )
        .await
        .expect("deposit response");
    assert_eq!(deposit_response.status(), StatusCode::OK);

    let dispute_payload = json!({
        "id": "evt_test_oversized_dispute",
        "object": "event",
        "type": "charge.dispute.created",
        "created": 7_200,
        "data": {
            "object": {
                "id": "dp_test_oversized",
                "object": "dispute",
                "amount": 300,
                "currency": "usd",
                "charge": "ch_test_oversized_dispute",
                "payment_intent": "pi_test_oversized_dispute",
                "reason": "fraudulent",
                "status": "needs_response"
            }
        }
    })
    .to_string();
    let dispute_signature =
        stripe_signature_header("whsec_test", dispute_payload.as_bytes(), now_seconds())
            .expect("dispute sig");
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/stripe/webhook")
                .header("stripe-signature", &dispute_signature)
                .body(Body::from(dispute_payload))
                .expect("dispute request builds"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    let body: Value = serde_json::from_slice(&bytes).expect("response JSON");
    assert_eq!(body["ok"], false);
    assert!(body["error"]
        .as_str()
        .expect("error")
        .contains("Dispute amount exceeds original deposit"));

    let deposits = poster.deposits.lock().await;
    let chargebacks = poster.chargebacks.lock().await;
    assert_eq!(deposits.len(), 1);
    assert_eq!(chargebacks.len(), 0);

    let event_log =
        std::fs::read_to_string(temp.path().join("stripe-events.jsonl")).expect("event log");
    assert_eq!(event_log.lines().count(), 1);
    assert!(event_log.contains("evt_test_deposit_before_oversized_dispute"));
    assert!(!event_log.contains("evt_test_oversized_dispute"));
}
