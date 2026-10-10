use super::*;
use crate::openai::{
    gateway_token_hash, openai_router,
    proxy_control::{Prepared, ProxyControl},
    GatewayAccessControl, GatewayState, GatewayTokenRecord, GatewayTokenStore,
};
use axum::{
    body::Body,
    http::{header, Request},
};
use mayhem_proto::proxy::{ProxyMarketDescriptor, ProxyMembership, ProxyOffer};
use mayhem_proxy::discovery::{
    Context, Entry, Identity, Mode, Page, Proof, QueryBinding, CATALOG_PREFIX,
};
use serde_json::{json, Value};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, sync::Arc, time::Duration};
use tower::ServiceExt;

// These tests inspect the handler's shared global read budget. Serialize their
// local HTTP traffic so another test cannot consume a deliberately held permit.
static HTTP_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
mod availability;
mod batch;
const TOKEN: &str = "sk-mayhem-proxy-directory-fixture";

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn protected_control(dir: &Path) -> Arc<ProxyControl> {
    write(&dir.join("bridge-token"), b"fixture-bridge-token");
    let config = json!({"schema_version":1,"network":identity(),
        "peer_rpc_url":"http://127.0.0.1:1","state_dir":"state",
        "bridge":{"url":"ws://127.0.0.1:2","token_file":"bridge-token",
            "operation_timeout_ms":500,"frame_bytes":65536,"queue_events":8,"queue_bytes":131072},
        "max_markets":4,"max_presence_routes":16,"selected_markets":[]});
    let path = dir.join("gateway.json");
    write(&path, &serde_json::to_vec(&config).unwrap());
    let (control, lifecycle) = Prepared::load(&path, &identity()).unwrap().open().unwrap();
    // No lifecycle is run: HTTP fixtures use only direct local catalog commits,
    // and neither port above is contacted nor any inference performed.
    drop(lifecycle);
    control
}

fn state(control: Option<Arc<ProxyControl>>) -> GatewayState {
    let access = GatewayAccessControl::new(
        true,
        GatewayTokenStore {
            version: 1,
            tokens: vec![GatewayTokenRecord {
                name: "directory-test".into(),
                token_hash: gateway_token_hash(TOKEN),
                token_id: "tok_directory".into(),
                created_at: 1,
                expires_at: None,
                budget_au: None,
                budget_period: None,
                spent_total_au: 0,
                spent_period_au: 0,
                period_started_at: Some(1),
                max_rate_per_minute: None,
                models: vec![],
                last_used_at: None,
                revoked_at: None,
            }],
        },
        None,
    );
    let state = GatewayState::fixture().with_access_control(access);
    match control {
        Some(control) => state.with_proxy_control(control),
        None => state,
    }
}

fn rows(name: &str, family: &str, count: usize, decision: bool) -> Vec<Entry> {
    let source: Value = serde_json::from_str(include_str!(
        "../../../../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
    ))
    .unwrap();
    let fixture = &source["cases"][usize::from(decision)];
    let mut market: ProxyMarketDescriptor =
        serde_json::from_value(fixture["market"].clone()).unwrap();
    market.model.model_id = name.into();
    market.model.family_id = family.into();
    let id = market.id().unwrap();
    let mut rows = vec![Entry {
        key: format!("{CATALOG_PREFIX}markets/{id}"),
        value: json!(market),
    }];
    for n in 0..count {
        let mut member: ProxyMembership =
            serde_json::from_value(fixture["membership"].clone()).unwrap();
        let mut offer: ProxyOffer = serde_json::from_value(fixture["offer"].clone()).unwrap();
        member.market_id = id.clone();
        member.provider_pubkey = format!("{n:064x}");
        offer.market_id = id.clone();
        offer.provider_pubkey = member.provider_pubkey.clone();
        offer.validate_for_membership(&market, &member).unwrap();
        rows.push(Entry { key:format!("{CATALOG_PREFIX}memberships/{id}/{}",member.provider_pubkey), value:json!({"active":true,"revision":member.revision,"member":member,"offer_slots":1}) });
        rows.push(Entry { key:format!("{CATALOG_PREFIX}offers/{id}/{}/{}",offer.provider_pubkey,offer.slot_id().unwrap()), value:json!({"active":true,"revision":offer.revision,"digest":offer.digest().unwrap(),"offer":offer}) });
    }
    rows
}

fn apply(control: &ProxyControl, mut rows: Vec<Entry>) {
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    let catalog = control.catalog();
    let prior = catalog.read().unwrap().status().committed;
    let base = prior.map(|value| value.proof);
    let n = base.as_ref().map_or(1, |proof| proof.signed_length + 1);
    let network = identity();
    let page = Page {
        ok: true,
        lane: "proxy".into(),
        schema_version: 1,
        request_nonce: "c".repeat(64),
        query: QueryBinding::catalog(),
        context: Context {
            network_id: network.network_id,
            msb_bootstrap: network.msb_bootstrap,
            subnet_bootstrap: network.subnet_bootstrap,
            contract_version: network.contract_version,
            epoch: n,
        },
        proof: Proof {
            view_key: "d".repeat(64),
            fork: 0,
            signed_length: n,
            tree_hash: format!("{n:064x}"),
        },
        mode: if base.is_some() {
            Mode::Changes
        } else {
            Mode::Snapshot
        },
        base_proof: base,
        entries: rows,
        truncated: false,
        next_cursor: None,
        checkpoint: Some(format!("pdc1.http{n}.{}", "e".repeat(128))),
    };
    catalog
        .apply(&catalog.refresh_ticket().unwrap(), &page, now_millis_u64())
        .unwrap();
}

async fn request(
    state: &GatewayState,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder().uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = openai_router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    if uri.starts_with("/v1/proxy/offers") {
        assert_eq!(
            headers.get(header::CACHE_CONTROL).unwrap(),
            "no-store",
            "proxy response {status} for {uri} must not be cached"
        );
    }
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, headers, serde_json::from_slice(&body).unwrap())
}

fn uri(query: &[(&str, &str)]) -> String {
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(query.iter().copied())
        .finish();
    format!("/v1/proxy/offers?{encoded}")
}

fn error_code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

fn native_catalog(mut body: Value) -> Value {
    // Native availability observations are stamped for each HTTP response.
    // Compare every other field so proxy browsing cannot rewrite native data.
    for model in body["data"].as_array_mut().unwrap() {
        assert!(model["mayhem"]
            .as_object_mut()
            .unwrap()
            .remove("availability_observed_at_millis")
            .unwrap()
            .is_u64());
    }
    body
}

#[tokio::test]
async fn router_auth_and_disabled_or_unhydrated_control_do_not_hide_native_models() {
    let _serial = HTTP_TESTS.lock().await;
    let disabled = state(None);
    for token in [None, Some("wrong")] {
        assert_eq!(
            request(&disabled, "/v1/proxy/offers", token).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, _, body) = request(&disabled, "/v1/proxy/offers", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&body), "proxy_directory_disabled");
    let native = request(&disabled, "/v1/models", Some(TOKEN)).await;
    assert_eq!(native.0, StatusCode::OK);
    assert!(!native.2["data"].as_array().unwrap().is_empty());
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    let enabled = disabled.with_proxy_control(control.clone());
    let (status, headers, body) = request(&enabled, "/v1/proxy/offers", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&body), "proxy_directory_unavailable");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(
        native_catalog(request(&enabled, "/v1/models", Some(TOKEN)).await.2),
        native_catalog(native.2)
    );
    assert!(!control.health().unwrap().running);
}

#[tokio::test]
async fn router_filters_and_pages_public_offers_without_inference_or_rate_projection() {
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    let wanted = rows("Vendor/Model", "qwen", 5, false);
    let mut all = wanted.clone();
    all.extend(rows("Vendor/Other", "qwen", 2, false));
    all.extend(rows("Vendor/Model", "other", 1, false));
    all.extend(rows("Vendor/Model", "qwen", 1, true));
    apply(&control, all);
    let state = state(Some(control.clone()));
    let native_before = native_catalog(request(&state, "/v1/models", Some(TOKEN)).await.2);
    let mut cursor = None;
    let mut found = Vec::new();
    loop {
        let mut query = vec![
            ("kind", "llm"),
            ("family_id", "qwen"),
            ("name_prefix", "VENDOR/MO"),
            ("endpoint", "openai_chat_completions"),
            ("rail", "fiat"),
            ("minimum_context", "262144"),
            ("limit", "2"),
        ];
        if let Some(cursor) = cursor.as_deref() {
            query.push(("cursor", cursor));
        }
        let (status, headers, body) = request(&state, &uri(&query), Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["cache-control"], "no-store");
        let entries = body["entries"].as_array().unwrap();
        assert!(entries.len() <= 2);
        assert!(body["scanned_candidates"].as_u64().unwrap() <= directory::MAX_CANDIDATES as u64);
        for entry in entries {
            assert_eq!(entry["lane"], "proxy");
            assert_eq!(entry["operator_verification"], "unknown");
            assert_eq!(entry["catalog_eligible"], false);
            assert_eq!(entry["availability"]["status"], "catalog_unavailable");
            assert!(entry["availability"]["observed_at_ms"].is_u64());
            assert!(entry["availability"]["expires_at_ms"].is_null());
            found.push(entry["id"].as_str().unwrap().to_owned());
        }
        cursor = body["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
        assert!(found.len() <= 5);
    }
    let mut expected: Vec<_> = wanted
        .iter()
        .filter_map(|row| {
            row.key
                .strip_prefix(&format!("{CATALOG_PREFIX}offers/"))
                .map(str::to_owned)
        })
        .collect();
    expected.sort();
    assert_eq!(found, expected);
    let id = expected.last().unwrap();
    let original = wanted
        .iter()
        .find(|row| row.key == format!("{CATALOG_PREFIX}offers/{id}"))
        .unwrap();
    let detail = format!("/v1/proxy/offers/{id}");
    assert_eq!(
        request(&state, &detail, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _, body) = request(&state, &detail, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], id.as_str());
    assert_eq!(body["offer"], original.value["offer"]);
    assert_eq!(body["digest"], original.value["digest"]);
    assert_eq!(body["availability"]["status"], "catalog_unavailable");
    assert!(body["offer"]["rates"].as_array().unwrap().len() > 1);
    let missing = format!(
        "/v1/proxy/offers/{}/{}/{}",
        "f".repeat(64),
        "f".repeat(64),
        "f".repeat(64)
    );
    assert_eq!(
        request(&state, &missing, Some(TOKEN)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        native_catalog(request(&state, "/v1/models", Some(TOKEN)).await.2),
        native_before
    );
    assert_eq!(control.health().unwrap().presence.attempts, 0);
}

#[tokio::test]
async fn no_op_refresh_keeps_http_cursor_and_repricing_returns_expiry_conflict() {
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    let rows = rows("Model", "other", 3, false);
    apply(&control, rows.clone());
    let state = state(Some(control.clone()));
    let first = request(&state, &uri(&[("limit", "1")]), Some(TOKEN))
        .await
        .2;
    let cursor = first["next_cursor"].as_str().unwrap();
    apply(&control, vec![]);
    let continuation = uri(&[("limit", "1"), ("cursor", cursor)]);
    let (status, _, next) = request(&state, &continuation, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["snapshot"], first["snapshot"]);
    let mut row = rows
        .into_iter()
        .find(|row| row.key.contains("/offers/"))
        .unwrap();
    let mut offer: ProxyOffer = serde_json::from_value(row.value["offer"].clone()).unwrap();
    offer.revision += 1;
    offer.rates[0].per_unit_au += 1;
    row.value = json!({"active":true,"revision":offer.revision,"digest":offer.digest().unwrap(),"offer":offer});
    apply(&control, vec![row]);
    let (status, headers, body) = request(&state, &continuation, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "proxy_directory_cursor_expired");
    assert_eq!(headers["cache-control"], "no-store");
}

#[tokio::test]
async fn malformed_http_queries_cursors_and_ids_are_bounded_bad_requests() {
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    apply(&control, rows("Model", "other", 3, false));
    let state = state(Some(control));
    for query in [
        vec![("unexpected", "true")],
        vec![("limit", "0")],
        vec![("limit", "101")],
        vec![("minimum_context", "-1")],
        vec![("kind", "native")],
        vec![("cursor", "!!!")],
        vec![("kind", "llm"), ("endpoint", "mayhem_decisions")],
    ] {
        let (status, _, body) = request(&state, &uri(&query), Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "query {query:?}: {body}");
        assert_eq!(error_code(&body), "invalid_request_error");
    }
    let first = request(&state, &uri(&[("limit", "1")]), Some(TOKEN))
        .await
        .2;
    let cursor = first["next_cursor"].as_str().unwrap();
    assert_eq!(
        request(
            &state,
            &uri(&[("name_prefix", "other"), ("cursor", cursor)]),
            Some(TOKEN)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&state, &uri(&[("cursor", &"a".repeat(8193))]), Some(TOKEN))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &state,
            "/v1/proxy/offers/not-a-market/provider/slot",
            Some(TOKEN)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn cancelled_http_read_keeps_worker_permit_until_blocking_work_finishes() {
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    apply(&control, rows("Model", "other", 1, false));
    let state = state(Some(control));
    let held = READS.try_acquire_many(7).unwrap();
    let (started, active) = tokio::sync::oneshot::channel();
    let (finish, finishing) = std::sync::mpsc::channel();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {TOKEN}").parse().unwrap(),
    );
    let task = tokio::spawn(read(Arc::new(state.clone()), headers, move |_, _| {
        started.send(()).unwrap();
        let _ = finishing.recv_timeout(Duration::from_secs(3));
        Ok(Some(json!({"fixture":"completed"})))
    }));
    tokio::time::timeout(Duration::from_secs(1), active)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(READS.available_permits(), 0);
    let (status, _, body) = request(&state, "/v1/proxy/offers", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&body), "proxy_directory_busy");
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while READS.available_permits() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(held);
    assert_eq!(READS.available_permits(), 8);
    assert_eq!(
        request(&state, "/v1/proxy/offers", Some(TOKEN)).await.0,
        StatusCode::OK
    );
}
