use super::*;
use crate::openai::{
    gateway_token_hash, GatewayAccessControl, GatewayTokenAttribution, GatewayTokenStore,
};
use std::sync::{Arc, Barrier};
fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}
fn token() -> GatewayTokenRecord {
    GatewayTokenRecord {
        name: "agent".into(),
        token_hash: gateway_token_hash("fixture-key"),
        token_id: "tok-fixture".into(),
        created_at: 1,
        expires_at: None,
        budget_au: Some(100),
        budget_period: Some(Period::Total),
        spent_total_au: 0,
        spent_period_au: 0,
        period_started_at: Some(1),
        max_rate_per_minute: None,
        models: vec![],
        last_used_at: None,
        revoked_at: None,
    }
}
fn limits() -> Limits {
    Limits {
        max_tokens: 8,
        max_reservations: 32,
    }
}
fn open(dir: &tempfile::TempDir, t: &GatewayTokenRecord) -> Authority {
    let path = dir.path().join("budget.redb");
    if path.exists() {
        Authority::open(&path, limits(), std::slice::from_ref(t))
    } else {
        Authority::create(&path, limits(), std::slice::from_ref(t))
    }
    .unwrap()
}

fn native<'a>(seq: u64, cum: u128, terminal: bool, proof: &'a str) -> NativeReceipt<'a> {
    NativeReceipt {
        buyer: "buyer",
        maximum: 100,
        billing_id: "billing",
        prior: 0,
        sequence: seq,
        cumulative: cum,
        terminal,
        proof,
    }
}
#[test]
fn key_budget_common_native_proxy_cap_is_atomic_and_survives_reopen() {
    let dir = private_dir();
    let t = token();
    let store = Arc::new(open(&dir, &t));
    let barrier = Arc::new(Barrier::new(3));
    let workers = [Lane::Native, Lane::Proxy]
        .into_iter()
        .enumerate()
        .map(|(n, lane)| {
            let a = store.clone();
            let b = barrier.clone();
            let t = t.clone();
            std::thread::spawn(move || {
                b.wait();
                a.reserve(&t, lane, &format!("request-{n}"), "buyer", "terms", 70, 2)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|v| v.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|v| v.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|v| **v == Err(Error::Cap)).count(), 1);
    drop(store);
    let store = open(&dir, &t);
    assert_eq!(
        store.reserve(&t, Lane::Proxy, "over", "f", "t", 31, 3),
        Err(Error::Cap)
    );
    store
        .reserve(&t, Lane::Proxy, "exact", "f", "t", 30, 3)
        .unwrap();
    assert_eq!(store.pending(None, 64).unwrap().len(), 2);
}

#[test]
fn key_budget_non_admission_fence_closes_only_exact_uncharged_intent_and_survives_restart() {
    let dir = private_dir();
    let t = token();
    let store = open(&dir, &t);
    store
        .reserve(&t, Lane::Proxy, "held", "f", "terms", 70, 2)
        .unwrap();
    assert_eq!(
        store.fence_proxy_non_admission(&t.token_id, "held", "other", "terms", 70, "proof", 3),
        Err(Error::Conflict)
    );
    assert_eq!(store.pending(None, 64).unwrap().len(), 1);
    store
        .fence_proxy_non_admission(&t.token_id, "held", "f", "terms", 70, "proof", 3)
        .unwrap();
    store
        .fence_proxy_non_admission(&t.token_id, "held", "f", "terms", 70, "proof", 3)
        .unwrap();
    // A rejected budget grant did not write a hold. Its fence must still prevent
    // a later cap increase or restart from allowing this original request.
    assert_eq!(
        store.reserve(&t, Lane::Proxy, "denied", "f", "terms", 101, 3),
        Err(Error::Cap)
    );
    store
        .fence_proxy_non_admission(&t.token_id, "denied", "f", "terms", 101, "denied-proof", 3)
        .unwrap();
    assert!(store.pending(None, 64).unwrap().is_empty());
    drop(store);
    let store = open(&dir, &t);
    store
        .fence_proxy_non_admission(&t.token_id, "held", "f", "terms", 70, "proof", 4)
        .unwrap();
    assert_eq!(
        store.reserve(&t, Lane::Proxy, "held", "f", "terms", 70, 4),
        Err(Error::Conflict)
    );
    let mut richer = t.clone();
    richer.budget_au = Some(200);
    assert_eq!(
        store.reserve(&richer, Lane::Proxy, "denied", "f", "terms", 101, 4),
        Err(Error::Conflict)
    );
    assert_eq!(
        store.fence_proxy_non_admission(&t.token_id, "held", "f", "terms", 70, "different", 4),
        Err(Error::Conflict)
    );
    store
        .reserve(&t, Lane::Proxy, "paid", "f", "terms", 100, 4)
        .unwrap();
    store
        .settle_proxy(&t.token_id, "paid", "f", "terms", 10, false, "receipt", 4)
        .unwrap();
    assert_eq!(
        store.fence_proxy_non_admission(&t.token_id, "paid", "f", "terms", 100, "proof", 5),
        Err(Error::Conflict)
    );
    let mut projected = t.clone();
    store.project(&mut projected, 5).unwrap();
    assert_eq!(projected.spent_total_au, 10);
    assert_eq!(store.pending(None, 64).unwrap()[0].1.charged, 10);
}

#[test]
fn key_budget_migrated_tokens_cannot_admit_with_volatile_authority() {
    let access = GatewayAccessControl::new(
        true,
        GatewayTokenStore {
            version: 2,
            tokens: vec![token()],
        },
        None,
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        "Bearer fixture-key".parse().unwrap(),
    );
    assert!(access.authorize(&headers, Some("model")).is_err());
    assert!(access.preauthorize_body_headers(&headers).is_err());
    let owner = access.authorize_existing(&headers, Some("model")).unwrap();
    assert!(owner.is_some());
    assert!(access
        .reserve_budget(&owner, "session", 1, "buyer")
        .is_err());
}
#[test]
fn key_budget_native_cumulative_receipts_deduplicate_across_restart_and_failover() {
    let dir = private_dir();
    let mut t = token();
    t.budget_au = Some(1000);
    let store = open(&dir, &t);
    store
        .reserve(&t, Lane::Native, "session-a", "buyer", "session-a", 100, 2)
        .unwrap();
    assert_eq!(
        store
            .settle_native(
                &t.token_id,
                "session-a",
                native(1, 30, false, "receipt-1"),
                3
            )
            .unwrap(),
        30
    );
    assert_eq!(
        store
            .settle_native(
                &t.token_id,
                "session-a",
                native(1, 30, false, "receipt-1"),
                3
            )
            .unwrap(),
        0
    );
    drop(store);
    let store = open(&dir, &t);
    assert_eq!(
        store
            .settle_native(
                &t.token_id,
                "session-a",
                native(1, 30, false, "receipt-1"),
                4
            )
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .settle_native(
                &t.token_id,
                "session-a",
                native(2, 50, true, "receipt-2"),
                5
            )
            .unwrap(),
        20
    );
    store
        .reserve(&t, Lane::Native, "session-b", "buyer", "session-b", 100, 6)
        .unwrap();
    let mut second = native(1, 75, true, "receipt-b");
    second.prior = 50;
    assert_eq!(
        store
            .settle_native(&t.token_id, "session-b", second, 7)
            .unwrap(),
        25
    );
    assert_eq!(
        store
            .settle_native(
                &t.token_id,
                "session-a",
                native(2, 50, true, "receipt-2"),
                8
            )
            .unwrap(),
        0
    );
    assert_eq!(
        store.settle_native(&t.token_id, "session-a", native(2, 51, true, "altered"), 8),
        Err(Error::Conflict)
    );
    store.project(&mut t, 8).unwrap();
    assert_eq!(t.spent_total_au, 75);
    assert!(store.pending(None, 64).unwrap().is_empty());
}
#[test]
fn key_budget_native_older_history_after_restart_preserves_charge_and_exposure() {
    let dir = private_dir();
    let mut t = token();
    t.budget_au = Some(100);
    let store = open(&dir, &t);
    store.reserve(&t, Lane::Native, "session", "buyer", "session", 100, 2).unwrap();
    assert_eq!(store.settle_native(&t.token_id, "session", native(387, 40, false, "newer"), 3).unwrap(), 40);
    drop(store);
    let store = open(&dir, &t);
    for _ in 0..2 {
        assert_eq!(store.settle_native(&t.token_id, "session", native(386, 39, false, "older"), 4).unwrap(), 0);
        let pending = store.pending(None, 64).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1.charged, 40);
        assert_eq!(pending[0].1.maximum, 100);
        assert_eq!(store.reserve(&t, Lane::Proxy, "other", "fp", "terms", 1, 4), Err(Error::Cap));
    }
    let mut wrong_identity = native(386, 39, false, "wrong");
    wrong_identity.buyer = "other";
    assert_eq!(store.settle_native(&t.token_id, "session", wrong_identity, 4), Err(Error::Conflict));
    let mut wrong_maximum = native(386, 39, false, "wrong");
    wrong_maximum.maximum = 99;
    assert_eq!(store.settle_native(&t.token_id, "session", wrong_maximum, 4), Err(Error::Conflict));
    let mut wrong_billing = native(386, 39, false, "wrong");
    wrong_billing.billing_id = "other";
    assert_eq!(store.settle_native(&t.token_id, "session", wrong_billing, 4), Err(Error::Conflict));
    for bad in [native(386, 41, false, "increased"), native(386, 39, true, "premature-final"), native(387, 40, false, "changed-proof")] {
        assert_eq!(store.settle_native(&t.token_id, "session", bad, 4), Err(Error::Conflict));
    }
    assert_eq!(store.settle_native(&t.token_id, "session", native(388, 45, true, "final"), 5).unwrap(), 5);
    assert!(store.pending(None, 64).unwrap().is_empty());
    assert_eq!(store.settle_native(&t.token_id, "session", native(386, 39, false, "older"), 6).unwrap(), 0);
    assert_eq!(store.settle_native(&t.token_id, "session", native(388, 45, true, "final"), 6).unwrap(), 0);
    store.project(&mut t, 6).unwrap();
    assert_eq!(t.spent_total_au, 45);
    store.reserve(&t, Lane::Proxy, "other", "fp", "terms", 55, 6).unwrap();
}

#[test]
fn key_budget_proxy_exact_bindings_retention_and_terminal_replay() {
    let dir = private_dir();
    let mut t = token();
    let store = open(&dir, &t);
    store
        .reserve(&t, Lane::Proxy, "job", "fp", "terms", 70, 2)
        .unwrap();
    store
        .reserve(&t, Lane::Proxy, "job", "fp", "terms", 70, 2)
        .unwrap();
    assert_eq!(
        store.reserve(&t, Lane::Proxy, "job", "other", "terms", 70, 2),
        Err(Error::Conflict)
    );
    assert_eq!(
        store.settle_proxy(&t.token_id, "job", "fp", "wrong", 30, false, "p1", 3),
        Err(Error::Conflict)
    );
    assert_eq!(
        store
            .settle_proxy(&t.token_id, "job", "fp", "terms", 30, false, "p1", 3)
            .unwrap(),
        30
    );
    drop(store);
    let store = open(&dir, &t);
    let pending = store.pending(None, 1).unwrap();
    assert_eq!(pending[0].1.charged, 30);
    assert_eq!(pending[0].1.maximum, 70);
    assert_eq!(
        store
            .settle_proxy(&t.token_id, "job", "fp", "terms", 30, true, "final", 4)
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .settle_proxy(&t.token_id, "job", "fp", "terms", 30, true, "final", 5)
            .unwrap(),
        0
    );
    assert_eq!(
        store.settle_proxy(&t.token_id, "job", "fp", "terms", 31, true, "different", 5),
        Err(Error::Conflict)
    );
    assert_eq!(
        store.reserve(&t, Lane::Proxy, "job", "fp", "terms", 70, 6),
        Err(Error::Conflict)
    );
    store.project(&mut t, 6).unwrap();
    assert_eq!(t.spent_total_au, 30);
    store
        .reserve(&t, Lane::Native, "remaining", "buyer", "remaining", 70, 6)
        .unwrap();
}
#[test]
fn key_budget_window_roll_keeps_earlier_uncertain_exposure_and_charges_actual_window() {
    for period in [Period::Day, Period::Month] {
        let dir = private_dir();
        let mut t = token();
        t.budget_period = Some(period);
        t.spent_total_au = 70;
        t.spent_period_au = 70;
        let store = open(&dir, &t);
        store
            .reserve(&t, Lane::Proxy, "old", "fp", "terms", 20, 2)
            .unwrap();
        let later = period.window_seconds().unwrap() + 2;
        let mut projection = t.clone();
        store.project(&mut projection, later).unwrap();
        assert_eq!(projection.spent_period_au, 0);
        store
            .reserve(&t, Lane::Native, "new", "buyer", "new", 80, later)
            .unwrap();
        assert_eq!(
            store.reserve(&t, Lane::Native, "over", "buyer", "over", 1, later),
            Err(Error::Cap)
        );
        store
            .settle_proxy(&t.token_id, "old", "fp", "terms", 10, true, "proof", later)
            .unwrap();
        store.project(&mut t, later).unwrap();
        assert_eq!((t.spent_total_au, t.spent_period_au), (80, 10));
        assert_eq!(
            store.reserve(&t, Lane::Native, "over2", "buyer", "over2", 11, later),
            Err(Error::Cap)
        );
    }
}
#[test]
fn key_budget_revocation_and_deletion_block_new_work_but_allow_original_settlement() {
    let dir = private_dir();
    let path = dir.path().join("tokens.json");
    let t = token();
    let config = GatewayTokenStore {
        version: 1,
        tokens: vec![t.clone()],
    };
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let access = GatewayAccessControl::new(true, config, Some(path.clone()))
        .initialize_durable_key_budget(dir.path().join("budget.redb"), limits())
        .unwrap();
    let attribution = GatewayTokenAttribution {
        name: t.name.clone(),
        token_id: t.token_id.clone(),
    };
    access
        .durable_budget
        .as_ref()
        .unwrap()
        .reserve(&t, Lane::Proxy, "job", "fp", "terms", 70, 2)
        .unwrap();
    let mut revoked = t.clone();
    revoked.revoked_at = Some(3);
    std::fs::write(
        &path,
        serde_json::to_vec(&GatewayTokenStore {
            version: 1,
            tokens: vec![revoked],
        })
        .unwrap(),
    )
    .unwrap();
    assert!(access
        .reserve_budget(&Some(attribution.clone()), "native", 10, "buyer")
        .is_err());
    access
        .settle_proxy_budget(&attribution, "job", "fp", "terms", 30, false, "partial")
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    access
        .settle_proxy_budget(&attribution, "job", "fp", "terms", 40, true, "final")
        .unwrap();
    access
        .settle_proxy_budget(&attribution, "job", "fp", "terms", 40, true, "final")
        .unwrap();
    assert!(access.pending_key_budgets(None, 64).unwrap().is_empty());
    // Reloading stale configuration cannot roll spending backwards or resurrect a hold.
    std::fs::write(
        &path,
        serde_json::to_vec(&GatewayTokenStore {
            version: 1,
            tokens: vec![t],
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(access.summary()["tokens"][0]["spent_total_au"], "40");
    assert!(access
        .reserve_budget(&Some(attribution), "over", 61, "buyer")
        .is_err());
}
#[test]
fn key_budget_existing_result_authorization_obeys_scope_rate_and_revocation_without_spend() {
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
    let dir = private_dir();
    let mut t = token();
    t.spent_total_au = 100;
    t.models = vec!["proxy/model".into()];
    t.max_rate_per_minute = Some(1);
    let access = GatewayAccessControl::new(
        true,
        GatewayTokenStore {
            version: 1,
            tokens: vec![t],
        },
        None,
    )
    .initialize_durable_key_budget(dir.path().join("budget.redb"), limits())
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-key"),
    );
    assert_eq!(
        access
            .authorize(&headers, Some("proxy/model"))
            .unwrap_err()
            .status,
        StatusCode::PAYMENT_REQUIRED
    );
    assert_eq!(
        access
            .authorize_existing(&headers, Some("other"))
            .unwrap_err()
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(access
        .authorize_existing(&headers, Some("proxy/model"))
        .unwrap()
        .is_some());
    assert_eq!(
        access
            .authorize_existing(&headers, Some("proxy/model"))
            .unwrap_err()
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    access.store.lock().unwrap().tokens[0].revoked_at = Some(1);
    assert_eq!(
        access
            .authorize_existing(&headers, Some("proxy/model"))
            .unwrap_err()
            .status,
        StatusCode::UNAUTHORIZED
    );
}
#[test]
fn key_budget_native_release_does_not_erase_later_signed_charge_and_identity_is_pinned() {
    let dir = private_dir();
    let mut t = token();
    let store = open(&dir, &t);
    store
        .reserve(&t, Lane::Native, "session", "buyer", "session", 100, 2)
        .unwrap();
    store.release_native(&t.token_id, "session", 3).unwrap();
    assert!(store.pending(None, 64).unwrap().is_empty());
    let mut wrong = native(1, 20, true, "proof");
    wrong.buyer = "foreign";
    assert_eq!(
        store.settle_native(&t.token_id, "session", wrong, 4),
        Err(Error::Conflict)
    );
    assert_eq!(
        store
            .settle_native(&t.token_id, "session", native(1, 20, true, "proof"), 4)
            .unwrap(),
        20
    );
    store.project(&mut t, 4).unwrap();
    assert_eq!(t.spent_total_au, 20);
}
#[test]
fn key_budget_closed_capacity_fences_new_work_without_preventing_recovery() {
    let dir = private_dir();
    let t = token();
    let store = Authority::create(
        &dir.path().join("budget.redb"),
        Limits {
            max_tokens: 1,
            max_reservations: 1,
        },
        std::slice::from_ref(&t),
    )
    .unwrap();
    store
        .reserve(&t, Lane::Proxy, "job", "fp", "terms", 100, 2)
        .unwrap();
    assert_eq!(
        store.reserve(&t, Lane::Native, "other", "buyer", "other", 1, 2),
        Err(Error::Full)
    );
    store
        .settle_proxy(&t.token_id, "job", "fp", "terms", 10, true, "proof", 3)
        .unwrap();
    assert_eq!(
        store.reserve(&t, Lane::Native, "other", "buyer", "other", 1, 3),
        Err(Error::Full)
    );
    assert!(store
        .settle_proxy(&t.token_id, "job", "fp", "terms", 10, true, "proof", 4)
        .is_ok());
}

#[test]
fn key_budget_gateway_startup_accepts_older_signed_dashboard_checkpoint() {
    use crate::openai::tests::{test_chat_output, test_chat_request, test_invocation, test_model};
    use crate::openai::{sign_hex, GatewayState};
    let dir = private_dir();
    let model = test_model();
    let mut invocation = test_invocation();
    invocation.spend_voucher.user_sig = sign_hex(
        &invocation.receipt_user_seed,
        &mayhem_proto::spend_voucher_signing_bytes(&invocation.spend_voucher.body).unwrap(),
    );
    let fixture = GatewayState::from_models(vec![model.clone()])
        .with_receipt_user_seed(invocation.receipt_user_seed);
    let mut receipt = fixture.meter_chat_session(
        &model, &test_chat_request(&model.id), &test_chat_output(), &invocation, None,
    ).unwrap();
    let mut t = token();
    t.budget_au = Some(2000);
    receipt.access_token = Some(GatewayTokenAttribution { name: t.name.clone(), token_id: t.token_id.clone() });
    let sign = |r: &mut crate::openai::StoredReceipt, seq, amount, terminal| {
        r.receipt.body.seq = seq;
        r.receipt.body.au_owed_cum = amount;
        r.receipt.body.final_receipt = terminal;
        let bytes = mayhem_proto::receipt_signing_bytes(&r.receipt.body).unwrap();
        r.receipt.user_sig = sign_hex(&invocation.receipt_user_seed, &bytes);
        r.receipt.enclave_sig = sign_hex(&fixture.receipt_config.enclave_seed, &bytes);
        r.receipt_ack.seq = seq;
        r.receipt_ack.user_sig = r.receipt.user_sig.clone();
    };
    sign(&mut receipt, 387, 200, false);
    let config = GatewayTokenStore { version: 1, tokens: vec![t] };
    let path = dir.path().join("startup.redb");
    let access = GatewayAccessControl::new(true, config.clone(), None)
        .initialize_durable_key_budget(path.clone(), limits()).unwrap();
    access.reserve_budget(&receipt.access_token, &receipt.receipt.body.session_id,
        receipt.voucher.body.max_spend_au, &receipt.receipt.body.user).unwrap();
    assert_eq!(access.reconcile_native_budget(&receipt).unwrap(), 200);
    drop(access); // durable accounting committed, dashboard checkpoint still older
    let mut older = receipt.clone();
    sign(&mut older, 386, 199, false);
    let history = dir.path().join("dashboard.json");
    std::fs::write(&history, serde_json::to_vec(&serde_json::json!({
        "version": 1, "receipts": [older], "paused_sessions": []
    })).unwrap()).unwrap();
    let state = GatewayState::from_models(vec![model])
        .with_dashboard_history_path(history)
        .with_access_control(GatewayAccessControl::new(true, config, None))
        .with_durable_key_budget(path, limits()).unwrap();
    state.restore_retained_native_key_budgets().unwrap();
    assert_eq!(state.access_summary()["tokens"][0]["spent_total_au"], "200");
    assert_eq!(state.access_control.pending_key_budgets(None, 64).unwrap().len(), 1);
    let mut tampered = older;
    tampered.receipt.body.au_owed_cum += 1;
    assert!(state.access_control.reconcile_native_budget(&tampered).is_err());
    sign(&mut receipt, 388, 250, true);
    assert_eq!(state.access_control.reconcile_native_budget(&receipt).unwrap(), 50);
    assert!(state.access_control.pending_key_budgets(None, 64).unwrap().is_empty());
    assert_eq!(state.access_summary()["tokens"][0]["spent_total_au"], "250");
}

#[test]
fn key_budget_real_native_receipt_recorder_and_reopen_reconcile_once() {
    use crate::openai::tests::{test_chat_output, test_chat_request, test_invocation, test_model};
    use crate::openai::{sign_hex, GatewayReceiptRecorder, GatewayState};
    let dir = private_dir();
    let mut t = token();
    t.budget_au = Some(2000);
    let config = GatewayTokenStore {
        version: 1,
        tokens: vec![t.clone()],
    };
    let path = dir.path().join("budget.redb");
    let access = GatewayAccessControl::new(true, config.clone(), None)
        .initialize_durable_key_budget(path.clone(), limits())
        .unwrap();
    let model = test_model();
    let request = test_chat_request(&model.id);
    let output = test_chat_output();
    let mut invocation = test_invocation();
    invocation.spend_voucher.user_sig = sign_hex(
        &invocation.receipt_user_seed,
        &mayhem_proto::spend_voucher_signing_bytes(&invocation.spend_voucher.body).unwrap(),
    );
    let state = GatewayState::from_models(vec![model.clone()])
        .with_receipt_user_seed(invocation.receipt_user_seed)
        .with_access_control(access);
    invocation.access_token = Some(GatewayTokenAttribution {
        name: t.name.clone(),
        token_id: t.token_id.clone(),
    });
    invocation.receipt_recorder = GatewayReceiptRecorder {
        receipts: state.receipts.clone(),
        spend_reservation: state
            .reserve_session_spend(
                &invocation.session_id,
                invocation.spend_voucher.body.max_spend_au,
                &invocation.access_token,
            )
            .unwrap(),
        history: None,
        settlement_publisher: Arc::new(None),
    };
    let receipt = state
        .meter_chat_session(&model, &request, &output, &invocation, None)
        .unwrap();
    let charged = receipt.receipt.body.au_owed_cum;
    assert_eq!(
        state.access_summary()["tokens"][0]["spent_total_au"],
        charged.to_string()
    );
    assert_eq!(
        state
            .access_control
            .reconcile_native_budget(&receipt)
            .unwrap(),
        0
    );
    drop(invocation);
    drop(state);
    let access = GatewayAccessControl::new(true, config, None)
        .with_durable_key_budget(path, limits())
        .unwrap();
    assert_eq!(access.reconcile_native_budget(&receipt).unwrap(), 0);
    assert_eq!(
        access.summary()["tokens"][0]["spent_total_au"],
        charged.to_string()
    );
    assert!(access.pending_key_budgets(None, 64).unwrap().is_empty());
    let mut altered = receipt.clone();
    altered.receipt.body.au_owed_cum += 1;
    assert!(access.reconcile_native_budget(&altered).is_err());
    assert_eq!(
        access.summary()["tokens"][0]["spent_total_au"],
        charged.to_string()
    );
    let restart_path = dir.path().join("native-restart.redb");
    let config = GatewayTokenStore {
        version: 1,
        tokens: vec![token()],
    };
    let mut config = config;
    config.tokens[0].budget_au = Some(2000);
    let admitted = GatewayAccessControl::new(true, config.clone(), None)
        .initialize_durable_key_budget(restart_path.clone(), limits())
        .unwrap();
    admitted
        .reserve_budget(
            &receipt.access_token,
            &receipt.receipt.body.session_id,
            receipt.voucher.body.max_spend_au,
            &receipt.receipt.body.user,
        )
        .unwrap();
    drop(admitted); // process death before the native receipt recorder ran
    let reopened = GatewayAccessControl::new(true, config, None)
        .with_durable_key_budget(restart_path, limits())
        .unwrap();
    let restored = GatewayState::from_models(vec![model.clone()]).with_access_control(reopened);
    restored.record_workbench_receipt(receipt).unwrap();
    restored.restore_retained_native_key_budgets().unwrap();
    restored.restore_retained_native_key_budgets().unwrap();
    assert_eq!(
        restored.access_summary()["tokens"][0]["spent_total_au"],
        charged.to_string()
    );
    assert!(restored
        .access_control
        .pending_key_budgets(None, 64)
        .unwrap()
        .is_empty());
    // Enabling common durable key budgets cannot disable anonymous native wallet charging.
    let anonymous = GatewayAccessControl::new(false, GatewayTokenStore::empty(), None)
        .initialize_durable_key_budget(dir.path().join("anonymous.redb"), limits())
        .unwrap();
    let mut invocation = test_invocation();
    invocation.spend_voucher.user_sig = sign_hex(
        &invocation.receipt_user_seed,
        &mayhem_proto::spend_voucher_signing_bytes(&invocation.spend_voucher.body).unwrap(),
    );
    let state = GatewayState::from_models(vec![model.clone()])
        .with_receipt_user_seed(invocation.receipt_user_seed)
        .with_access_control(anonymous);
    invocation.receipt_recorder = GatewayReceiptRecorder {
        receipts: state.receipts.clone(),
        spend_reservation: state
            .reserve_session_spend(
                &invocation.session_id,
                invocation.spend_voucher.body.max_spend_au,
                &None,
            )
            .unwrap(),
        history: None,
        settlement_publisher: Arc::new(None),
    };
    let before = state.ledger_balance_au();
    let receipt = state
        .meter_chat_session(&model, &request, &output, &invocation, None)
        .unwrap();
    assert_eq!(
        state.ledger_balance_au(),
        before - receipt.receipt.body.au_owed_cum
    );
}

#[test]
fn key_budget_reopen_rejects_missing_zero_or_uninitialized_database_instead_of_resetting_spend() {
    for failure in ["missing", "zero", "uninitialized"] {
        let dir = private_dir();
        let path = dir.path().join("budget.redb");
        let t = token();
        let store = Authority::create(&path, limits(), std::slice::from_ref(&t)).unwrap();
        store
            .reserve(&t, Lane::Proxy, "job", "fp", "terms", 100, 2)
            .unwrap();
        store
            .settle_proxy(&t.token_id, "job", "fp", "terms", 30, false, "partial", 3)
            .unwrap();
        drop(store);
        match failure {
            "missing" => std::fs::remove_file(&path).unwrap(),
            "zero" => std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap()
                .sync_all()
                .unwrap(),
            _ => {
                std::fs::remove_file(&path).unwrap();
                let file = private_file(&path, true).unwrap();
                drop(redb::Database::builder().create_file(file).unwrap());
            }
        }
        assert!(
            Authority::open(&path, limits(), std::slice::from_ref(&t)).is_err(),
            "{failure}"
        );
        let access = GatewayAccessControl::new(
            true,
            GatewayTokenStore {
                version: 1,
                tokens: vec![t],
            },
            None,
        );
        assert!(
            access
                .with_durable_key_budget(path.clone(), limits())
                .is_err(),
            "{failure}"
        );
        if failure != "missing" {
            assert!(
                Authority::create(&path, limits(), &[]).is_err(),
                "exclusive initialization cannot overwrite {failure}"
            );
        }
    }
}
