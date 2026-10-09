use super::*;
use mayhem_proto::proxy::finance::{ProxyExpiryBody, ProxyReservationExpiry};
use mayhem_proxy::{
    attempts,
    financial::recovery::{BuyerRecovery, FinancialOutcome, Limits, Store},
};
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, sync::Arc};
#[path = "reservation_publication.rs"]
mod reservation_publication;
fn identity(f: &Fixture) -> attempts::Identity {
    let t = &f.auth.terms;
    attempts::Identity {
        network_id: t.network_id.clone(),
        msb_bootstrap: attempts::Digest::new(&t.msb_bootstrap).unwrap(),
        subnet_bootstrap: attempts::Digest::new(&t.subnet_bootstrap).unwrap(),
        controller_pubkey: attempts::Digest::new(&t.buyer_pubkey).unwrap(),
    }
}
fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn recovery(f: &Fixture, d: &tempfile::TempDir) -> BuyerRecovery {
    let store = Arc::new(
        Store::open(
            d.path().join("buyer"),
            identity(f),
            Limits {
                max_records: 8,
                closed_retention_ms: 1000,
            },
        )
        .unwrap(),
    );
    BuyerRecovery::new(store, f.buyer_client.clone(), 4).unwrap()
}
fn key(f: &Fixture) -> attempts::Digest {
    attempts::Digest::new(f.auth.terms.digest().unwrap()).unwrap()
}
async fn advance(f: &mut Fixture, extra: u64) {
    let epoch = f.auth.terms.reservation_expires_after_epoch
        + f.auth.terms.reservation_receipt_grace_epochs
        + extra;
    f.request(&json!({"epoch":epoch}).to_string()).await;
}
async fn sign(f: &mut Fixture, body: ProxyExpiryBody) -> ProxyReservationExpiry {
    let v = f.request(&json!({"sign_expiry":body}).to_string()).await;
    ProxyReservationExpiry {
        body,
        buyer_sig: v["buyer_sig"].as_str().unwrap().into(),
    }
}
#[tokio::test]
async fn buyer_expiry_all_rails_and_families_requires_canonical_deadline_and_keeps_native_holds() {
    for family in ["llm", "decisions"] {
        for rail in ["fiat", "tnk", "tap"] {
            let mut f = Fixture::new_with_expiry(rail, family, true).await;
            let d = dir();
            let r = recovery(&f, &d);
            assert!(r.refresh(&f.auth, 1).await.unwrap().is_none());
            assert_eq!(r.pending(None, 64).await.unwrap(), vec![key(&f)]);
            assert!(r.prepare_expiry(&f.auth, 2).await.is_err());
            advance(&mut f, 0).await;
            assert!(r.prepare_expiry(&f.auth, 3).await.is_err());
            advance(&mut f, 1).await;
            let body = r.prepare_expiry(&f.auth, 4).await.unwrap();
            assert_eq!(r.prepare_expiry(&f.auth, 500).await.unwrap(), body);
            let signed = sign(&mut f, body).await;
            r.retain_expiry(key(&f), signed.clone()).await.unwrap();
            let before = f.request("state").await;
            assert_eq!(
                r.publish_expiry(key(&f), 10).await.unwrap(),
                Some(FinancialOutcome::ExpiredUnknown {
                    expiry: signed.clone()
                })
            );
            let after = f.request("state").await;
            assert_eq!(
                after["summary"]["reserved_au"], "50",
                "native reservation untouched"
            );
            assert_ne!(
                before["summary"]["reserved_au"],
                after["summary"]["reserved_au"]
            );
            assert_eq!(
                before["balance"], after["balance"],
                "expiry is not a payout or debit"
            );
            assert_eq!(after["billing"]["retry_blocked"], true);
            assert_eq!(after["billing"]["reserved_au"], "0");
            assert!(r.pending(None, 64).await.unwrap().is_empty());
            assert_eq!(
                r.publish_expiry(key(&f), 20).await.unwrap(),
                Some(FinancialOutcome::ExpiredUnknown { expiry: signed })
            );
            assert_eq!(f.request("status").await["publications"], 1);
            assert_eq!(r.prune(1009, 64).await.unwrap(), 0);
            assert_eq!(r.prune(1010, 64).await.unwrap(), 1);
            f.stop().await;
        }
    }
}
#[tokio::test]
async fn buyer_expiry_restart_preserves_unsigned_and_signed_intent_and_lost_ack_is_not_double_release(
) {
    let mut f = Fixture::new_with_expiry("tap", "llm", true).await;
    let d = dir();
    let r = recovery(&f, &d);
    advance(&mut f, 1).await;
    let body = r.prepare_expiry(&f.auth, 5).await.unwrap();
    drop(r);
    let r = recovery(&f, &d);
    let pending = r.pending(None, 64).await.unwrap();
    let saved = r.recover(pending[0].clone()).await.unwrap();
    assert_eq!(saved.authorization, f.auth);
    assert_eq!(saved.draft, Some(body.clone()));
    assert!(saved.signed.is_none() && saved.confirmed.is_none());
    assert_eq!(
        r.prepare_expiry(&saved.authorization, 55).await.unwrap(),
        body
    );
    let signed = sign(&mut f, body).await;
    r.retain_expiry(key(&f), signed.clone()).await.unwrap();
    drop(r);
    let r = recovery(&f, &d);
    f.command("publish_lost_ack").await;
    let (a, b) = tokio::join!(r.publish_expiry(key(&f), 60), r.publish_expiry(key(&f), 61));
    assert!(matches!(
        a.unwrap(),
        Some(FinancialOutcome::ExpiredUnknown { .. })
    ));
    assert!(matches!(
        b.unwrap(),
        Some(FinancialOutcome::ExpiredUnknown { .. })
    ));
    drop(r);
    let r = recovery(&f, &d);
    assert!(r.pending(None, 64).await.unwrap().is_empty());
    assert_eq!(
        r.publish_expiry(key(&f), 70).await.unwrap(),
        Some(FinancialOutcome::ExpiredUnknown { expiry: signed })
    );
    assert_eq!(f.request("status").await["publications"], 1);
    f.stop().await;
}
#[tokio::test]
async fn buyer_expiry_pending_ack_does_not_confirm_and_another_buyer_intent_can_win() {
    let mut f = Fixture::new_with_expiry("fiat", "decisions", true).await;
    let d = dir();
    let r = recovery(&f, &d);
    advance(&mut f, 1).await;
    let body = r.prepare_expiry(&f.auth, 5).await.unwrap();
    let signed = sign(&mut f, body).await;
    r.retain_expiry(key(&f), signed).await.unwrap();
    f.command("publish_pending").await;
    assert!(r.publish_expiry(key(&f), 6).await.unwrap().is_none());
    assert_eq!(r.pending(None, 64).await.unwrap(), vec![key(&f)]);
    let other_dir = dir();
    let other = recovery(&f, &other_dir);
    let body = other.prepare_expiry(&f.auth, 99).await.unwrap();
    let signed_other = sign(&mut f, body).await;
    other.retain_expiry(key(&f), signed_other).await.unwrap();
    f.command("flush_publication").await;
    let a = r.publish_expiry(key(&f), 100).await.unwrap();
    let b = other.publish_expiry(key(&f), 100).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(f.request("status").await["publications"], 1);
    f.stop().await;
}
#[tokio::test]
async fn buyer_expiry_loses_to_final_receipt_or_waiver_without_second_publication() {
    for winner in ["final", "close"] {
        let mut f = Fixture::new_with_expiry("tnk", "llm", true).await;
        let d = dir();
        let r = recovery(&f, &d);
        advance(&mut f, 1).await;
        let body = r.prepare_expiry(&f.auth, 5).await.unwrap();
        let signed = sign(&mut f, body).await;
        r.retain_expiry(key(&f), signed).await.unwrap();
        f.command(winner).await;
        let result = r.publish_expiry(key(&f), 10).await.unwrap().unwrap();
        assert!(matches!(
            (winner, result),
            ("final", FinancialOutcome::Paid { .. }) | ("close", FinancialOutcome::Waived { .. })
        ));
        assert_eq!(f.request("status").await["submissions"], 0);
        assert!(r.pending(None, 64).await.unwrap().is_empty());
        f.stop().await;
    }
}
#[tokio::test]
async fn buyer_expiry_later_waiver_updates_financial_outcome_without_second_release() {
    let mut f = Fixture::new_with_expiry("tap", "llm", true).await;
    let d = dir();
    let r = recovery(&f, &d);
    advance(&mut f, 1).await;
    let body = r.prepare_expiry(&f.auth, 5).await.unwrap();
    let signed = sign(&mut f, body).await;
    r.retain_expiry(key(&f), signed).await.unwrap();
    r.publish_expiry(key(&f), 10).await.unwrap();
    let before = f.request("state").await;
    f.command("close").await;
    assert!(matches!(
        r.refresh(&f.auth, 20).await.unwrap(),
        Some(FinancialOutcome::Waived { .. })
    ));
    let after = f.request("state").await;
    assert_eq!(before["summary"], after["summary"]);
    assert_eq!(after["billing"]["retry_blocked"], false);
    assert_eq!(r.prune(1010, 64).await.unwrap(), 1);
    f.stop().await;
}
#[tokio::test]
async fn buyer_expiry_opt_in_signature_role_and_original_body_are_mandatory() {
    let mut f = Fixture::new("tnk", "llm").await;
    let d = dir();
    let r = recovery(&f, &d);
    advance(&mut f, 1).await;
    assert!(r.prepare_expiry(&f.auth, 1).await.is_err());
    f.stop().await;
    let mut f = Fixture::new_with_expiry("tnk", "llm", true).await;
    let d = dir();
    let r = recovery(&f, &d);
    advance(&mut f, 1).await;
    let body = r.prepare_expiry(&f.auth, 5).await.unwrap();
    let signed = sign(&mut f, body.clone()).await;
    let observation = f.client.observe(&f.auth).await.unwrap();
    assert!(
        f.client.submit_expiry(&observation, &signed).await.is_err(),
        "provider cannot submit buyer expiry"
    );
    let mut bad = signed.clone();
    bad.buyer_sig = "0".repeat(128);
    assert!(r.retain_expiry(key(&f), bad).await.is_err());
    let mut changed = body;
    changed.at_ms += 1;
    let changed = sign(&mut f, changed).await;
    assert!(
        r.retain_expiry(key(&f), changed).await.is_err(),
        "even valid signature cannot replace intent"
    );
    r.retain_expiry(key(&f), signed).await.unwrap();
    assert_eq!(f.request("status").await["submissions"], 0);
    assert!(r.pending(None, 65).await.is_err());
    assert!(r.prune(u64::MAX, 65).await.is_err());
    assert_eq!(
        r.prune(u64::MAX, 64).await.unwrap(),
        0,
        "pending holds never pruned"
    );
    f.stop().await;
}
#[tokio::test]
async fn buyer_expiry_store_is_private_exclusive_and_bound_to_original_buyer_network() {
    let f = Fixture::new_with_expiry("tnk", "llm", true).await;
    let d = dir();
    let r = recovery(&f, &d);
    assert!(Store::open(
        d.path().join("buyer"),
        identity(&f),
        Limits {
            max_records: 1,
            closed_retention_ms: 1
        }
    )
    .is_err());
    r.refresh(&f.auth, 0).await.unwrap();
    drop(r);
    let mut changed = identity(&f);
    changed.network_id = "other".into();
    assert!(Store::open(
        d.path().join("buyer"),
        changed,
        Limits {
            max_records: 1,
            closed_retention_ms: 1
        }
    )
    .is_err());
    let meta = std::fs::metadata(d.path().join("buyer")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o077, 0);
    std::fs::set_permissions(
        d.path().join("buyer"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(Store::open(
        d.path().join("buyer"),
        identity(&f),
        Limits {
            max_records: 1,
            closed_retention_ms: 1
        }
    )
    .is_err());
    f.stop().await;
}

#[tokio::test]
async fn buyer_expiry_retention_changes_do_not_rewrite_old_prune_deadlines() {
    let mut f = Fixture::new_with_expiry("tnk", "llm", true).await;
    let d = dir();
    let r = recovery(&f, &d);
    advance(&mut f, 1).await;
    let body = r.prepare_expiry(&f.auth, 5).await.unwrap();
    let signed = sign(&mut f, body).await;
    r.retain_expiry(key(&f), signed).await.unwrap();
    r.publish_expiry(key(&f), 10).await.unwrap();
    drop(r);
    let store = Arc::new(
        Store::open(
            d.path().join("buyer"),
            identity(&f),
            Limits {
                max_records: 8,
                closed_retention_ms: 999999,
            },
        )
        .unwrap(),
    );
    let r = BuyerRecovery::new(store, f.buyer_client.clone(), 2).unwrap();
    assert_eq!(r.prune(1009, 64).await.unwrap(), 0);
    assert_eq!(r.prune(1010, 64).await.unwrap(), 1);
    f.stop().await;
}
