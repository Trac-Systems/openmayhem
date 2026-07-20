use std::{
    collections::HashMap,
    fs,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, Method, Request, StatusCode},
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use mayhem_paygate::{
    paygate_router, run_stripe_backfill_once, stripe_signature_header, BoxFuture,
    ContractPostResult, ContractPoster, FiatChargebackFeature, FiatDepositFeature, OracleKeypair,
    PaygateConfig, PaygateState, PeerRpcContractPoster, RailConfig, StripeConnectAccountType,
    StripeSettings, DEFAULT_STRIPE_API_BASE_URL,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::time::{sleep, Duration};
use tokio::{net::TcpListener, sync::Mutex};
use tower::ServiceExt;

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

const TEST_INTERNAL_AUTH_SECRET: &str =
    "mayhem-paygate-test-internal-auth-secret-not-for-production";
static TEST_INTERNAL_AUTH_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
struct StripeCapture {
    requests: Arc<Mutex<Vec<StripeRequest>>>,
    events: Arc<Mutex<Vec<Value>>>,
    connect_ready: Arc<Mutex<bool>>,
    connect_account_id: Arc<Mutex<Option<String>>>,
    connect_account_type: Arc<Mutex<Option<String>>>,
    connect_owner_provider: Arc<Mutex<Option<String>>>,
    connect_livemode: Arc<Mutex<bool>>,
    oauth_account_id: Arc<Mutex<Option<String>>>,
    oauth_livemode: Arc<Mutex<bool>>,
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

async fn mock_connect_account(capture: &StripeCapture, ready: bool) -> Value {
    let account_id = capture
        .connect_account_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| "acct_test_provider".to_owned());
    let account_type = capture
        .connect_account_type
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| "express".to_owned());
    let owner_provider = capture
        .connect_owner_provider
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| "a".repeat(64));
    let livemode = *capture.connect_livemode.lock().await;
    json!({
        "id": account_id,
        "object": "account",
        "type": account_type,
        "country": "DE",
        "default_currency": "eur",
        "livemode": livemode,
        "metadata": {
            "mayhem_provider": owner_provider,
            "mayhem_mode": if livemode { "live" } else { "test" }
        },
        "details_submitted": ready,
        "charges_enabled": false,
        "payouts_enabled": ready,
        "capabilities": {
            "transfers": if ready { "active" } else { "pending" }
        },
        "requirements": {
            "currently_due": if ready { json!([]) } else { json!(["business_profile.url"]) },
            "eventually_due": if ready { json!([]) } else { json!(["external_account"]) },
            "disabled_reason": if ready { Value::Null } else { json!("requirements.past_due") }
        }
    })
}

async fn mock_create_connect_account(
    State(capture): State<StripeCapture>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    let body = String::from_utf8(body.to_vec()).expect("form body utf8");
    if body.contains("type=standard") {
        *capture.connect_account_type.lock().await = Some("standard".to_owned());
    } else if body.contains("type=custom") {
        *capture.connect_account_type.lock().await = Some("custom".to_owned());
    } else if capture.connect_account_type.lock().await.is_none() {
        *capture.connect_account_type.lock().await = Some("express".to_owned());
    }
    if capture.connect_owner_provider.lock().await.is_none() {
        *capture.connect_owner_provider.lock().await = Some("a".repeat(64));
    }
    capture.requests.lock().await.push(StripeRequest {
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        idempotency_key: headers
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body: format!("connect-account:{body}"),
    });
    Json(mock_connect_account(&capture, *capture.connect_ready.lock().await).await)
}

async fn mock_retrieve_connect_account(
    State(capture): State<StripeCapture>,
    AxumPath(account_id): AxumPath<String>,
    headers: HeaderMap,
) -> Json<Value> {
    capture.requests.lock().await.push(StripeRequest {
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        idempotency_key: None,
        body: format!("connect-status:{account_id}"),
    });
    Json(mock_connect_account(&capture, *capture.connect_ready.lock().await).await)
}

async fn mock_create_connect_link(
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
        body: format!("connect-link:{body}"),
    });
    Json(json!({
        "object": "account_link",
        "url": "https://connect.stripe.com/setup/test-link",
        "expires_at": 1_900_000_000u64
    }))
}

async fn mock_exchange_connect_oauth(
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
        idempotency_key: None,
        body: format!("connect-oauth:{body}"),
    });
    Json(json!({
        "token_type": "bearer",
        "scope": "read_write",
        "livemode": *capture.oauth_livemode.lock().await,
        "stripe_user_id": capture
            .oauth_account_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "acct_test_provider".to_owned())
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
        .route("/v1/accounts", post(mock_create_connect_account))
        .route(
            "/v1/accounts/{account_id}",
            get(mock_retrieve_connect_account),
        )
        .route("/v1/account_links", post(mock_create_connect_link))
        .route("/oauth/token", post(mock_exchange_connect_oauth))
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContractFeatureMode {
    Applied,
    Rejected,
    MissingState,
    MismatchedState,
}

#[derive(Clone)]
struct ContractRpcCapture {
    mode: ContractFeatureMode,
    feature_requests: Arc<Mutex<Vec<Value>>>,
    states: Arc<Mutex<HashMap<String, Value>>>,
}

async fn mock_contract_feature(
    State(capture): State<ContractRpcCapture>,
    Json(body): Json<Value>,
) -> Json<Value> {
    capture.feature_requests.lock().await.push(body.clone());
    let key = body["key"].as_str().expect("feature key").to_owned();
    let hash = "9".repeat(64);
    if capture.mode == ContractFeatureMode::Rejected {
        return Json(json!({
            "ok": false,
            "accepted": true,
            "status": "rejected",
            "feature": "mayhem",
            "key": key,
            "hash": hash,
            "message": "Admin required.",
            "result": {
                "type": "feature_result",
                "feature_key": format!("mayhem_{key}"),
                "hash": hash,
                "status": "rejected",
                "ok": false,
                "result": null,
                "error": {"name": "Error", "message": "Admin required."}
            }
        }));
    }

    let value = body["value"].clone();
    let op = value["op"].as_str().expect("feature op");
    if capture.mode != ContractFeatureMode::MissingState {
        let mut state = json!({
            "rail": "fiat",
            "processor_rail": value["rail"],
            "who": value["who"],
            "au": value["au"],
            "ext_ref_hash": value["ext_ref_hash"],
            "fiat_currency": value["fiat_currency"],
            "fiat_amount_minor": value["fiat_amount_minor"],
            "epoch": value["epoch"],
            "at": value["at"],
            "credited_at": key,
            "credited_by": "a".repeat(64),
            "credited_by_role": "admin"
        });
        if op == "fiat_chargeback" {
            state["dispute_ref_hash"] = value["dispute_ref_hash"].clone();
        }
        if capture.mode == ContractFeatureMode::MismatchedState {
            state["au"] = json!("1");
        }
        capture.states.lock().await.insert(key.clone(), state);
    }

    let contract_op = match op {
        "fiat_deposit" => "fiatDeposit",
        "fiat_chargeback" => "fiatChargeback",
        _ => "unknown",
    };
    Json(json!({
        "ok": true,
        "accepted": true,
        "status": "applied",
        "feature": "mayhem",
        "key": key,
        "hash": hash,
        "message": "Feature applied.",
        "result": {
            "type": "feature_result",
            "feature_key": format!("mayhem_{key}"),
            "hash": hash,
            "status": "applied",
            "ok": true,
            "result": {"ok": true, "op": contract_op},
            "error": null
        }
    }))
}

async fn mock_contract_state(
    State(capture): State<ContractRpcCapture>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let key = query.get("key").cloned().unwrap_or_default();
    let value = capture.states.lock().await.get(&key).cloned();
    Json(json!({
        "key": key,
        "confirmed": false,
        "value": value
    }))
}

async fn start_mock_contract(mode: ContractFeatureMode) -> (String, ContractRpcCapture) {
    let capture = ContractRpcCapture {
        mode,
        feature_requests: Arc::new(Mutex::new(Vec::new())),
        states: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/contract/feature", post(mock_contract_feature))
        .route("/state", get(mock_contract_state))
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock contract");
    let addr = listener.local_addr().expect("mock contract local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock contract server");
    });
    (format!("http://{addr}"), capture)
}

fn test_config(stripe_base: String, event_store_path: std::path::PathBuf) -> PaygateConfig {
    let backfill_cursor_path = event_store_path.with_file_name("stripe-backfill-cursor.json");
    let connect_accounts_path = event_store_path.with_file_name("stripe-connect-accounts.jsonl");
    let connect_consents_path = event_store_path.with_file_name("stripe-connect-consents.jsonl");
    let internal_auth_secret_path = event_store_path.with_file_name("paygate-internal-auth.secret");
    fs::write(&internal_auth_secret_path, TEST_INTERNAL_AUTH_SECRET).expect("write auth secret");
    #[cfg(unix)]
    fs::set_permissions(
        &internal_auth_secret_path,
        fs::Permissions::from_mode(0o600),
    )
    .expect("secure auth secret");
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
                connect_accounts_path,
                connect_consents_path,
                internal_auth_secret_path,
                ..StripeSettings::default()
            },
        },
        ..PaygateConfig::default()
    }
}

fn test_internal_auth_headers(
    method: &Method,
    uri: &str,
    body: &str,
    timestamp: u64,
    nonce: &str,
    secret: &str,
) -> Vec<(&'static str, String)> {
    let path = uri.split('?').next().unwrap_or(uri);
    let body_digest = hex::encode(Sha256::digest(body.as_bytes()));
    let message = format!(
        "mayhem-paygate-internal-request-v1\n{timestamp}\n{nonce}\n{}\n{path}\n{body_digest}",
        method.as_str()
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("test HMAC key");
    mac.update(message.as_bytes());
    vec![
        ("x-mayhem-paygate-timestamp", timestamp.to_string()),
        ("x-mayhem-paygate-nonce", nonce.to_owned()),
        (
            "x-mayhem-paygate-signature",
            hex::encode(mac.finalize().into_bytes()),
        ),
    ]
}

async fn raw_json_request(
    app: Router,
    method: Method,
    uri: &str,
    body: Value,
    headers: Vec<(&'static str, String)>,
) -> (StatusCode, Value) {
    let body = body.to_string();
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = app
        .oneshot(request.body(Body::from(body)).expect("request builds"))
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    let body =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    (status, body)
}

async fn json_request(app: Router, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let nonce = format!(
        "{:064x}",
        TEST_INTERNAL_AUTH_NONCE.fetch_add(1, Ordering::Relaxed)
    );
    let serialized = body.to_string();
    raw_json_request(
        app,
        method.clone(),
        uri,
        body,
        test_internal_auth_headers(
            &method,
            uri,
            &serialized,
            timestamp,
            &nonce,
            TEST_INTERNAL_AUTH_SECRET,
        ),
    )
    .await
}

async fn get_request(app: Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    (
        status,
        String::from_utf8(bytes.to_vec()).expect("utf8 body"),
    )
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs()
}

fn stripe_relink_request(provider: &str, source_provider: &str, request_nonce: &str) -> Value {
    json!({
        "provider": provider,
        "source_provider": source_provider,
        "account_id": "acct_test_provider",
        "context_revision": "7".repeat(64),
        "country": "DE",
        "request_nonce": request_nonce,
        "consent_expires_at": now_seconds() + 300,
        "source_consent_signature": "8".repeat(128),
        "target_service_signature": "9".repeat(128),
    })
}

fn succeeded_payment_payload(event_id: &str, payment_intent: &str) -> String {
    json!({
        "id": event_id,
        "object": "event",
        "type": "payment_intent.succeeded",
        "created": 3_600,
        "data": {
            "object": {
                "id": payment_intent,
                "object": "payment_intent",
                "latest_charge": format!("ch_{payment_intent}"),
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
    .to_string()
}

async fn post_signed_webhook(app: Router, payload: &str) -> axum::response::Response {
    let signature =
        stripe_signature_header("whsec_test", payload.as_bytes(), now_seconds()).expect("sig");
    app.oneshot(
        Request::builder()
            .method(Method::POST)
            .uri("/v1/stripe/webhook")
            .header("stripe-signature", signature)
            .body(Body::from(payload.to_owned()))
            .expect("request builds"),
    )
    .await
    .expect("router response")
}

#[tokio::test]
async fn stripe_event_is_not_persisted_until_admin_feature_and_state_are_applied() {
    for mode in [
        ContractFeatureMode::Rejected,
        ContractFeatureMode::MissingState,
        ContractFeatureMode::MismatchedState,
    ] {
        let (contract_base, capture) = start_mock_contract(mode).await;
        let temp = tempfile::tempdir().expect("tempdir");
        let event_store_path = temp.path().join("stripe-events.jsonl");
        let mut config = test_config("http://127.0.0.1:9".to_owned(), event_store_path.clone());
        config.contract_rpc_url = contract_base;
        config.contract_dry_run = false;
        let state = PaygateState::try_new(
            config,
            OracleKeypair::from_seed_hex(&"61".repeat(32)).expect("oracle"),
        )
        .expect("state");
        let payload = succeeded_payment_payload(
            &format!("evt_contract_{mode:?}"),
            &format!("pi_contract_{mode:?}"),
        );

        let response = post_signed_webhook(paygate_router(state), &payload).await;

        assert_ne!(response.status(), StatusCode::OK, "mode {mode:?}");
        assert!(
            std::fs::read_to_string(&event_store_path)
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "mode {mode:?} must not persist the Stripe event"
        );
        let requests = capture.feature_requests.lock().await;
        assert_eq!(requests.len(), 1, "mode {mode:?}");
        assert_eq!(requests[0]["feature"], "mayhem");
        assert_eq!(requests[0]["value"]["op"], "fiat_deposit");
    }
}

#[tokio::test]
async fn stripe_event_uses_admin_feature_and_persists_after_exact_state_proof() {
    let (contract_base, capture) = start_mock_contract(ContractFeatureMode::Applied).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let event_store_path = temp.path().join("stripe-events.jsonl");
    let mut config = test_config("http://127.0.0.1:9".to_owned(), event_store_path.clone());
    config.contract_rpc_url = contract_base;
    config.contract_dry_run = false;
    let state = PaygateState::try_new(
        config,
        OracleKeypair::from_seed_hex(&"62".repeat(32)).expect("oracle"),
    )
    .expect("state");
    let payload = succeeded_payment_payload("evt_contract_applied", "pi_contract_applied");

    let response = post_signed_webhook(paygate_router(state), &payload).await;

    assert_eq!(response.status(), StatusCode::OK);
    let event_log = std::fs::read_to_string(&event_store_path).expect("event log");
    assert_eq!(event_log.lines().count(), 1);
    assert!(event_log.contains("evt_contract_applied"));
    let requests = capture.feature_requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["feature"], "mayhem");
    assert_eq!(requests[0]["value"]["op"], "fiat_deposit");
    assert!(requests[0]["key"]
        .as_str()
        .expect("feature key")
        .starts_with("dep/fiat/"));
}

#[tokio::test]
async fn stripe_chargeback_uses_admin_feature_and_exact_chargeback_state() {
    let (contract_base, capture) = start_mock_contract(ContractFeatureMode::Applied).await;
    let poster = PeerRpcContractPoster::new(contract_base, false, reqwest::Client::new());
    let oracle = OracleKeypair::from_seed_hex(&"63".repeat(32)).expect("oracle");
    let feature = FiatChargebackFeature {
        op: "fiat_chargeback",
        rail: "stripe",
        who: "b".repeat(64),
        au: 2_500_000_000_000_000_000u128,
        ext_ref_hash: "c".repeat(64),
        dispute_ref_hash: "d".repeat(64),
        fiat_currency: "usd".to_owned(),
        fiat_amount_minor: 250,
        epoch: 2,
        at: 3_600,
    };

    let result = poster
        .post_fiat_chargeback(&oracle, feature)
        .await
        .expect("chargeback feature");

    assert_eq!(result.result["ok"], true);
    assert_eq!(result.result["op"], "fiatChargeback");
    let requests = capture.feature_requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["value"]["op"], "fiat_chargeback");
    assert_eq!(
        requests[0]["key"],
        format!("dep/fiat/{}/chargeback/{}", "c".repeat(64), "d".repeat(64))
    );
}

#[tokio::test]
async fn internal_stripe_posts_require_fresh_unreplayed_request_auth() {
    let (stripe_base, _capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(stripe_base, temp.path().join("stripe-events.jsonl"));
    let poster = Arc::new(RecordingContractPoster::default());
    let state = PaygateState::try_new_with_contract_poster(
        config.clone(),
        OracleKeypair::from_seed_hex(&"10".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");
    let app = paygate_router(state);
    let uri = "/v1/stripe/payment-intents";
    let method = Method::POST;
    let body = json!({
        "who": "a".repeat(64),
        "au": "2500000000000000000",
        "currency": "eur",
        "idempotency_key": "stripe-internal-auth-test"
    });
    let serialized = body.to_string();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();

    let (status, _) =
        raw_json_request(app.clone(), method.clone(), uri, body.clone(), Vec::new()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let forged = test_internal_auth_headers(
        &method,
        uri,
        &serialized,
        now,
        &"1".repeat(64),
        "forged-paygate-internal-auth-secret-material",
    );
    let (status, _) =
        raw_json_request(app.clone(), method.clone(), uri, body.clone(), forged).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let stale = test_internal_auth_headers(
        &method,
        uri,
        &serialized,
        now.saturating_sub(31),
        &"2".repeat(64),
        TEST_INTERNAL_AUTH_SECRET,
    );
    let (status, _) = raw_json_request(app.clone(), method.clone(), uri, body.clone(), stale).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let valid = test_internal_auth_headers(
        &method,
        uri,
        &serialized,
        now,
        &"3".repeat(64),
        TEST_INTERNAL_AUTH_SECRET,
    );
    let (status, _) = raw_json_request(
        app.clone(),
        method.clone(),
        uri,
        body.clone(),
        valid.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = raw_json_request(app, method.clone(), uri, body.clone(), valid.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let restarted = PaygateState::try_new_with_contract_poster(
        config,
        OracleKeypair::from_seed_hex(&"10".repeat(32)).expect("oracle"),
        poster,
    )
    .expect("restarted state");
    let (status, _) = raw_json_request(paygate_router(restarted), method, uri, body, valid).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[cfg(unix)]
#[test]
fn stripe_internal_auth_secret_rejects_symlink_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(
        "http://127.0.0.1:9".to_owned(),
        temp.path().join("stripe-events.jsonl"),
    );
    let configured = config.rails.stripe.internal_auth_secret_path.clone();
    fs::remove_file(&configured).expect("remove generated secret");
    let target = temp.path().join("other.secret");
    fs::write(&target, TEST_INTERNAL_AUTH_SECRET).expect("write target secret");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("secure target secret");
    symlink(&target, &configured).expect("link configured secret");
    let error = PaygateState::try_new(
        config,
        OracleKeypair::from_seed_hex(&"09".repeat(32)).expect("oracle"),
    )
    .err()
    .expect("symlink must fail");
    assert!(error.to_string().contains("non-symlink"));
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
async fn stripe_connect_onboarding_reuses_account_and_reports_ready_status() {
    let (stripe_base, capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let event_store_path = temp.path().join("stripe-events.jsonl");
    let poster = Arc::new(RecordingContractPoster::default());
    let config = test_config(stripe_base, event_store_path.clone());
    let state = PaygateState::try_new_with_contract_poster(
        config.clone(),
        OracleKeypair::from_seed_hex(&"13".repeat(32)).expect("oracle"),
        poster.clone(),
    )
    .expect("state");

    let (status, body) = json_request(
        paygate_router(state),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": "a".repeat(64),
            "country": "DE",
            "request_nonce": "1".repeat(64)
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rail"], "fiat");
    assert_eq!(body["processor_rail"], "stripe");
    assert_eq!(body["account"]["id"], "acct_test_provider");
    assert_eq!(body["account"]["default_currency"], "eur");
    assert_eq!(body["account"]["ready"], false);
    assert_eq!(
        body["copy_paste"]["onboarding_url"],
        "https://connect.stripe.com/setup/test-link"
    );

    *capture.connect_ready.lock().await = true;
    let reloaded = PaygateState::try_new_with_contract_poster(
        config,
        OracleKeypair::from_seed_hex(&"13".repeat(32)).expect("oracle"),
        poster,
    )
    .expect("reloaded state");
    let app = paygate_router(reloaded);
    let (status, ready_onboard) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": "a".repeat(64),
            "country": "DE",
            "request_nonce": "2".repeat(64)
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ready_onboard["account"]["ready"], true);
    assert_eq!(
        ready_onboard["copy_paste"]["onboarding_url"],
        "https://connect.stripe.com/setup/test-link"
    );

    let (status, body) = json_request(
        app,
        Method::POST,
        "/v1/stripe/connect/status",
        json!({
            "provider": "a".repeat(64),
            "request_nonce": "3".repeat(64)
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["account"]["ready"], true);
    assert_eq!(body["account"]["payouts_enabled"], true);
    assert_eq!(body["account"]["transfers_enabled"], true);
    assert!(body["onboarding"].is_null());
    let requests = capture.requests.lock().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.body.starts_with("connect-account:"))
            .count(),
        1
    );
    assert!(requests.iter().any(|request| {
        request.body.starts_with("connect-account:")
            && request.body.contains("type=express")
            && request.body.contains("country=DE")
            && request
                .body
                .contains("metadata%5Bmayhem_provider%5D=aaaaaaaa")
    }));
    assert!(requests.iter().any(|request| {
        request.body.starts_with("connect-link:")
            && request.body.contains("type=account_onboarding")
            && request.body.contains("account=acct_test_provider")
    }));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.body.starts_with("connect-link:"))
            .count(),
        2
    );
}

#[tokio::test]
async fn stripe_connect_rotation_is_restart_idempotent_and_rejects_cas_substitution() {
    let (stripe_base, capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(stripe_base, temp.path().join("stripe-events.jsonl"));
    let state = PaygateState::try_new_with_contract_poster(
        config.clone(),
        OracleKeypair::from_seed_hex(&"19".repeat(32)).expect("oracle"),
        Arc::new(RecordingContractPoster::default()),
    )
    .expect("state");
    let app = paygate_router(state);
    let provider = "a".repeat(64);

    let (status, first) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "1".repeat(64),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["account"]["id"], "acct_test_provider");

    let (status, missing_previous) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "5".repeat(64),
            "rotate": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(missing_previous["error"]
        .as_str()
        .unwrap_or_default()
        .contains("requires previous_account_id"));

    let (status, unexpected_previous) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "6".repeat(64),
            "previous_account_id": "acct_test_provider",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(unexpected_previous["error"]
        .as_str()
        .unwrap_or_default()
        .contains("valid only for Stripe Connect rotation"));

    *capture.connect_account_id.lock().await = Some("acct_test_rotated".to_owned());
    let (status, rotated) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "2".repeat(64),
            "rotate": true,
            "previous_account_id": "acct_test_provider",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rotated["account"]["id"], "acct_test_rotated");

    let (status, replayed) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "2".repeat(64),
            "rotate": true,
            "previous_account_id": "acct_test_provider",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["account"]["id"], "acct_test_rotated");

    let (status, nonce_substituted) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "2".repeat(64),
            "rotate": true,
            "previous_account_id": "acct_test_rotated",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(nonce_substituted["error"]
        .as_str()
        .unwrap_or_default()
        .contains("nonce already consumed"));

    let (status, stale) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "3".repeat(64),
            "rotate": true,
            "previous_account_id": "acct_test_provider",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(stale["error"]
        .as_str()
        .unwrap_or_default()
        .contains("previous account is stale"));

    let reloaded = PaygateState::try_new_with_contract_poster(
        config,
        OracleKeypair::from_seed_hex(&"19".repeat(32)).expect("oracle"),
        Arc::new(RecordingContractPoster::default()),
    )
    .expect("reloaded state");
    let reloaded_app = paygate_router(reloaded);
    let (status, restarted_replay) = json_request(
        reloaded_app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "2".repeat(64),
            "rotate": true,
            "previous_account_id": "acct_test_provider",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restarted_replay["account"]["id"], "acct_test_rotated");

    let (status, current) = json_request(
        reloaded_app.clone(),
        Method::POST,
        "/v1/stripe/connect/status",
        json!({
            "provider": provider,
            "request_nonce": "4".repeat(64),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["account"]["id"], "acct_test_rotated");

    *capture.connect_account_id.lock().await = Some("acct_test_provider".to_owned());
    let (status, substituted) = json_request(
        reloaded_app,
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "7".repeat(64),
            "rotate": true,
            "previous_account_id": "acct_test_rotated",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(substituted["error"]
        .as_str()
        .unwrap_or_default()
        .contains("already bound"));

    let account_log =
        fs::read_to_string(temp.path().join("stripe-connect-accounts.jsonl")).expect("account log");
    let records = account_log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("account record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema_version"], 1);
    assert_eq!(records[0]["account_id"], "acct_test_provider");
    assert_eq!(records[0]["previous_account_id"], Value::Null);
    assert_eq!(records[1]["schema_version"], 1);
    assert_eq!(records[1]["account_id"], "acct_test_rotated");
    assert_eq!(records[1]["previous_account_id"], "acct_test_provider");
    assert_eq!(records[1]["request_nonce"], "2".repeat(64));

    let requests = capture.requests.lock().await;
    let creations = requests
        .iter()
        .filter(|request| request.body.starts_with("connect-account:"))
        .collect::<Vec<_>>();
    assert_eq!(creations.len(), 3);
    assert_ne!(creations[0].idempotency_key, creations[1].idempotency_key);
    assert!(creations[1]
        .idempotency_key
        .as_deref()
        .is_some_and(|key| key.starts_with("mayhem-connect-account-rotation-")));
}

#[tokio::test]
async fn stripe_connect_store_keeps_existing_account_and_rotates_without_rewrite() {
    let (stripe_base, capture) = start_mock_stripe().await;
    *capture.connect_account_id.lock().await = Some("acct_test_rotated".to_owned());
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(stripe_base, temp.path().join("stripe-events.jsonl"));
    let provider = "a".repeat(64);
    fs::write(
        &config.rails.stripe.connect_accounts_path,
        format!(
            "{}\n",
            json!({
                "schema_version": 1,
                "mode": "test",
                "provider": provider,
                "account_id": "acct_test_provider",
                "account_type": "express",
                "country": "DE",
                "created_at": 1,
            })
        ),
    )
    .expect("existing account log");
    let state = PaygateState::try_new_with_contract_poster(
        config.clone(),
        OracleKeypair::from_seed_hex(&"20".repeat(32)).expect("oracle"),
        Arc::new(RecordingContractPoster::default()),
    )
    .expect("existing account state");
    let app = paygate_router(state);
    let (status, rotated) = json_request(
        app,
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": provider,
            "country": "DE",
            "request_nonce": "8".repeat(64),
            "rotate": true,
            "previous_account_id": "acct_test_provider",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rotated["account"]["id"], "acct_test_rotated");

    let records =
        fs::read_to_string(&config.rails.stripe.connect_accounts_path).expect("account log");
    let records = records
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("account record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema_version"], 1);
    assert!(records[0].get("request_nonce").is_none());
    assert_eq!(records[1]["schema_version"], 1);
    assert_eq!(records[1]["request_nonce"], "8".repeat(64));
    assert_eq!(records[1]["previous_account_id"], "acct_test_provider");
}

#[tokio::test]
async fn stripe_connect_relink_requires_and_replays_verified_standard_oauth_consent() {
    let (stripe_base, capture) = start_mock_stripe().await;
    *capture.connect_ready.lock().await = true;
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(stripe_base.clone(), temp.path().join("stripe-events.jsonl"));
    config.rails.stripe.connect_account_type = StripeConnectAccountType::Standard;
    config.rails.stripe.connect_oauth_client_id = Some("ca_test_mayhem".to_owned());
    config.rails.stripe.connect_oauth_redirect_url =
        Some("https://paygate.example/v1/stripe/connect/relink/return".to_owned());
    config.rails.stripe.connect_oauth_token_url = format!("{stripe_base}/oauth/token");
    let state = PaygateState::try_new_with_contract_poster(
        config.clone(),
        OracleKeypair::from_seed_hex(&"14".repeat(32)).expect("oracle"),
        Arc::new(RecordingContractPoster::default()),
    )
    .expect("state");
    let app = paygate_router(state);

    let (status, source) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/onboard",
        json!({
            "provider": "a".repeat(64),
            "country": "DE",
            "request_nonce": "1".repeat(64)
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(source["account"]["account_type"], "standard");
    assert_eq!(source["account"]["ready"], true);

    let relink_request = stripe_relink_request(&"b".repeat(64), &"a".repeat(64), &"2".repeat(64));
    let (status, relink) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/relink",
        relink_request.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(relink["status"], "consent_required");
    assert!(relink["account"].is_null());
    let consent_url = relink["onboarding"]["url"].as_str().expect("consent URL");
    assert!(!consent_url.contains("acct_"));
    assert_eq!(
        relink["copy_paste"]["onboarding_url"].as_str(),
        Some(consent_url)
    );
    let parsed = reqwest::Url::parse(consent_url).expect("valid consent URL");
    assert_eq!(parsed.host_str(), Some("connect.stripe.com"));
    assert_eq!(
        parsed
            .query_pairs()
            .find(|(key, _)| key == "scope")
            .map(|(_, value)| value.into_owned())
            .as_deref(),
        Some("read_write")
    );
    let oauth_state = parsed
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("OAuth state");

    let (status, replay) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/relink",
        relink_request,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["onboarding"]["url"], consent_url);

    let (status, _) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/status",
        json!({
            "provider": "b".repeat(64),
            "request_nonce": "3".repeat(64)
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    *capture.oauth_account_id.lock().await = Some("acct_substituted".to_owned());
    let substituted_callback = format!(
        "/v1/stripe/connect/relink/return?state={oauth_state}&code=ac_wrong_account&scope=read_write"
    );
    let (status, _) = get_request(app.clone(), &substituted_callback).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    *capture.oauth_account_id.lock().await = None;
    *capture.oauth_livemode.lock().await = true;
    let crossover_callback = format!(
        "/v1/stripe/connect/relink/return?state={oauth_state}&code=ac_wrong_mode&scope=read_write"
    );
    let (status, _) = get_request(app.clone(), &crossover_callback).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    *capture.oauth_livemode.lock().await = false;
    let (status, _) = json_request(
        app.clone(),
        Method::POST,
        "/v1/stripe/connect/status",
        json!({
            "provider": "b".repeat(64),
            "request_nonce": "4".repeat(64)
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let callback = format!(
        "/v1/stripe/connect/relink/return?state={oauth_state}&code=ac_test_consent&scope=read_write"
    );
    let (status, body) = get_request(app.clone(), &callback).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("consent verified"));
    let (status, _) = get_request(app.clone(), &callback).await;
    assert_eq!(status, StatusCode::OK);

    let (status, target) = json_request(
        app,
        Method::POST,
        "/v1/stripe/connect/status",
        json!({
            "provider": "b".repeat(64),
            "request_nonce": "5".repeat(64)
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(target["account"]["id"], "acct_test_provider");
    assert_eq!(target["account"]["ready"], true);

    let requests = capture.requests.lock().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.body.starts_with("connect-oauth:"))
            .count(),
        3
    );
    assert!(requests.iter().any(|request| {
        request.body.starts_with("connect-oauth:")
            && request.body.contains("code=ac_test_consent")
            && request.body.contains("grant_type=authorization_code")
    }));
    drop(requests);
    let consent_log = std::fs::read_to_string(temp.path().join("stripe-connect-consents.jsonl"))
        .expect("consent log");
    assert_eq!(consent_log.lines().count(), 2);

    let reloaded = PaygateState::try_new_with_contract_poster(
        config,
        OracleKeypair::from_seed_hex(&"14".repeat(32)).expect("oracle"),
        Arc::new(RecordingContractPoster::default()),
    )
    .expect("reloaded state");
    let (status, target) = json_request(
        paygate_router(reloaded),
        Method::POST,
        "/v1/stripe/connect/status",
        json!({
            "provider": "b".repeat(64),
            "request_nonce": "6".repeat(64)
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(target["account"]["ready"], true);
}

#[tokio::test]
async fn stripe_connect_relink_dual_consent_reuses_express_and_custom_accounts() {
    for (account_type, expected_type) in [
        (StripeConnectAccountType::Express, "express"),
        (StripeConnectAccountType::Custom, "custom"),
    ] {
        let (stripe_base, capture) = start_mock_stripe().await;
        *capture.connect_ready.lock().await = true;
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(stripe_base, temp.path().join("stripe-events.jsonl"));
        config.rails.stripe.connect_account_type = account_type;
        let state = PaygateState::try_new_with_contract_poster(
            config.clone(),
            OracleKeypair::from_seed_hex(&"15".repeat(32)).expect("oracle"),
            Arc::new(RecordingContractPoster::default()),
        )
        .expect("state");
        let app = paygate_router(state);

        let (status, source) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/onboard",
            json!({
                "provider": "a".repeat(64),
                "country": "DE",
                "request_nonce": "1".repeat(64)
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(source["account"]["account_type"], expected_type);
        assert_eq!(source["account"]["ready"], true);

        let relink_request =
            stripe_relink_request(&"b".repeat(64), &"a".repeat(64), &"2".repeat(64));
        let (status, relinked) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/relink",
            relink_request.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(relinked["status"], "linked");
        assert_eq!(relinked["account"]["id"], "acct_test_provider");
        assert_eq!(relinked["account"]["account_type"], expected_type);
        assert!(relinked["onboarding"].is_null());

        let (status, replayed) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/relink",
            relink_request.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(replayed["account"]["id"], "acct_test_provider");

        let mut substituted = relink_request.clone();
        substituted["account_id"] = json!("acct_substituted");
        let (status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/relink",
            substituted,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let mut context_substituted = relink_request.clone();
        context_substituted["context_revision"] = json!("9".repeat(64));
        let (status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/relink",
            context_substituted,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let mut signature_substituted = relink_request.clone();
        signature_substituted["source_consent_signature"] = json!("a".repeat(128));
        let (status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/relink",
            signature_substituted,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let mut target_signature_substituted = relink_request.clone();
        target_signature_substituted["target_service_signature"] = json!("b".repeat(128));
        let (status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/relink",
            target_signature_substituted,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let mut stale = stripe_relink_request(&"c".repeat(64), &"a".repeat(64), &"3".repeat(64));
        stale["consent_expires_at"] = json!(now_seconds() - 1);
        let (status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/relink",
            stale,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/stripe/connect/onboard",
            json!({
                "provider": "c".repeat(64),
                "country": "DE",
                "request_nonce": "4".repeat(64),
                "account_id": "acct_test_provider"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        let consent_log = fs::read_to_string(temp.path().join("stripe-connect-consents.jsonl"))
            .expect("consent log");
        assert_eq!(consent_log.lines().count(), 2);

        let reloaded = PaygateState::try_new_with_contract_poster(
            config,
            OracleKeypair::from_seed_hex(&"15".repeat(32)).expect("oracle"),
            Arc::new(RecordingContractPoster::default()),
        )
        .expect("reloaded state");
        let (status, restarted_replay) = json_request(
            paygate_router(reloaded),
            Method::POST,
            "/v1/stripe/connect/relink",
            relink_request,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(restarted_replay["status"], "linked");
        assert_eq!(restarted_replay["account"]["id"], "acct_test_provider");
    }
}

#[tokio::test]
async fn stripe_connect_rejects_mixed_case_identities_before_account_work() {
    let (stripe_base, capture) = start_mock_stripe().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let state = PaygateState::try_new_with_contract_poster(
        test_config(stripe_base, temp.path().join("stripe-events.jsonl")),
        OracleKeypair::from_seed_hex(&"16".repeat(32)).expect("oracle"),
        Arc::new(RecordingContractPoster::default()),
    )
    .expect("state");
    let app = paygate_router(state);
    let mut mixed_source = stripe_relink_request(&"b".repeat(64), &"a".repeat(64), &"2".repeat(64));
    mixed_source["source_provider"] = json!("A".repeat(64));

    for (uri, body) in [
        (
            "/v1/stripe/connect/onboard",
            json!({
                "provider": "A".repeat(64),
                "country": "DE",
                "request_nonce": "1".repeat(64)
            }),
        ),
        (
            "/v1/stripe/connect/onboard",
            json!({
                "provider": "a".repeat(64),
                "country": "DE",
                "request_nonce": "A".repeat(64)
            }),
        ),
        ("/v1/stripe/connect/relink", mixed_source),
    ] {
        let (status, body) = json_request(app.clone(), Method::POST, uri, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("lower-case"));
    }

    assert!(
        capture.requests.lock().await.is_empty(),
        "non-canonical identities must fail before Stripe account work"
    );
}

#[tokio::test]
async fn stripe_connect_rejects_account_ownership_and_mode_substitution() {
    for live_account in [false, true] {
        let (stripe_base, capture) = start_mock_stripe().await;
        *capture.connect_owner_provider.lock().await = if live_account {
            Some("a".repeat(64))
        } else {
            Some("b".repeat(64))
        };
        *capture.connect_livemode.lock().await = live_account;
        let temp = tempfile::tempdir().expect("tempdir");
        let state = PaygateState::try_new_with_contract_poster(
            test_config(stripe_base, temp.path().join("stripe-events.jsonl")),
            OracleKeypair::from_seed_hex(&"16".repeat(32)).expect("oracle"),
            Arc::new(RecordingContractPoster::default()),
        )
        .expect("state");
        let (status, _) = json_request(
            paygate_router(state),
            Method::POST,
            "/v1/stripe/connect/onboard",
            json!({
                "provider": "a".repeat(64),
                "country": "DE",
                "request_nonce": "1".repeat(64)
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
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
