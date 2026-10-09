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
use mayhem_proxy::discovery::{
    Context, Entry, Identity, Mode, Page, Proof, QueryBinding, CATALOG_PREFIX,
};
use serde_json::{json, Value};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, sync::Arc};
use tower::ServiceExt;

// These tests inspect the handler's shared global read budget. Serialize their
// local HTTP traffic so another test cannot consume a deliberately held permit.

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

async fn call(state: &GatewayState, body: Value, token: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1/proxy/families/lookup")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = openai_router(state.clone())
        .oneshot(
            request
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn exact_canonical_family_http_auth_missing_stale_and_wire() {
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    let state = state(Some(control.clone()));
    let input = json!({"schema_version":1,"family_ids":["other"]});
    assert_eq!(
        call(&state, input.clone(), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&state, input.clone(), Some("invalid")).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&state, input.clone(), Some(TOKEN)).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    apply(
        &control,
        vec![Entry {
            key: format!("{CATALOG_PREFIX}families/other"),
            value: json!({"enabled":true,"label":"Synthetic family"}),
        }],
    );
    let (status, body) = call(&state, input.clone(), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["network"], json!(identity()));
    assert_eq!(
        body["families"],
        json!([{"family_id":"other","enabled":true,"label":"Synthetic family"}])
    );
    assert_eq!(body.as_object().unwrap().len(), 7);
    for field in [
        "checkpoint",
        "token",
        "capacity",
        "controller",
        "private_url",
    ] {
        assert!(body.get(field).is_none());
    }
    if let Ok(path) = std::env::var("MAYHEM_PROXY_FAMILY_FIXTURE") {
        std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    }
    let snapshot = control.catalog().read().unwrap();
    assert!(read(
        &snapshot,
        serde_json::from_value(input.clone()).unwrap(),
        now_millis_u64() + CATALOG_AGE_MS + 1
    )
    .is_err());
    assert_eq!(
        call(
            &state,
            json!({"schema_version":1,"family_ids":["missing"]}),
            Some(TOKEN)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for body in [
        json!({"schema_version":1,"family_ids":[]}),
        json!({"schema_version":1,"family_ids":["other","other"]}),
        json!({"schema_version":1,"family_ids":["other"],"confirmed":true}),
        json!({"schema_version":1,"family_ids":["../other"]}),
        json!({"schema_version":1,"family_ids":(0..97).map(|i|format!("f{i:03}")).collect::<Vec<_>>()}),
    ] {
        assert_eq!(
            call(&state, body, Some(TOKEN)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    apply(
        &control,
        vec![Entry {
            key: format!("{CATALOG_PREFIX}families/other"),
            value: json!({"enabled":false,"label":"Retired family"}),
        }],
    );
    assert_eq!(
        call(&state, input, Some(TOKEN)).await.1["families"][0]["enabled"],
        false
    );
}
