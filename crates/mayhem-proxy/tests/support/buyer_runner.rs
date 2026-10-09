use super::*;
use ed25519_dalek::{Signer, SigningKey};
use mayhem_proxy::{
    financial::recovery::runner::{Health, Phase, Policy, Runner},
    signing::Authority,
};
use tokio::sync::watch;

async fn keys(f: &mut Fixture) -> (SigningKey, SigningKey) {
    let v = f.request("ephemeral_test_wallet_seeds").await;
    let b: [u8; 32] = serde_json::from_value(v["buyer"].clone()).unwrap();
    let p: [u8; 32] = serde_json::from_value(v["provider"].clone()).unwrap();
    (SigningKey::from_bytes(&b), SigningKey::from_bytes(&p))
}
async fn signer(f: &mut Fixture) -> Authority {
    Authority::from_unlocked_wallet(keys(f).await.0, identity(f)).unwrap()
}
fn runner(r: Arc<BuyerRecovery>, signer: Option<Authority>) -> Runner {
    Runner::new(r, signer, Policy::default(), 1).unwrap()
}

#[tokio::test]
async fn buyer_runner_all_rails_expire_only_original_opt_in_after_canonical_deadline() {
    for family in ["llm", "decisions"] {
        for rail in ["fiat", "tnk", "tap"] {
            let mut f = Fixture::new_with_expiry(rail, family, true).await;
            let d = dir();
            let r = Arc::new(recovery(&f, &d));
            r.refresh(&f.auth, 1).await.unwrap();
            let mut worker = runner(r.clone(), None);
            advance(&mut f, 0).await;
            let page = worker.page().await.unwrap();
            assert_eq!(
                (page.checked, page.resolved, page.awaiting_wallet),
                (1, 0, 0)
            );
            assert!(r.recover(key(&f)).await.unwrap().draft.is_none());
            advance(&mut f, 1).await;
            let page = worker.page().await.unwrap();
            assert_eq!(page.awaiting_wallet, 1);
            let saved = r.recover(key(&f)).await.unwrap();
            assert!(saved.draft.is_some() && saved.signed.is_none());
            assert_eq!(f.request("status").await["submissions"], 0);
            drop(worker);
            drop(r);
            let r = Arc::new(recovery(&f, &d));
            let mut worker = runner(r.clone(), Some(signer(&mut f).await));
            assert_eq!(worker.page().await.unwrap().resolved, 1);
            let saved = r.recover(key(&f)).await.unwrap();
            assert!(matches!(
                saved.confirmed,
                Some(FinancialOutcome::ExpiredUnknown { .. })
            ));
            assert_eq!(f.request("state").await["summary"]["reserved_au"], "50");
            assert_eq!(worker.page().await.unwrap().checked, 0);
            assert_eq!(f.request("status").await["publications"], 1);
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn buyer_runner_does_not_add_expiry_and_keeps_pending_publication_after_restart() {
    let mut f = Fixture::new_with_mode("tap", "llm", "unreserved").await;
    let d = dir();
    let r = Arc::new(recovery(&f, &d));
    r.retain_reservation(f.auth.clone(), f.policy.clone(), 1)
        .await
        .unwrap();
    f.command("publish_pending").await;
    let mut worker = runner(r.clone(), Some(signer(&mut f).await));
    let page = worker.page().await.unwrap();
    assert!(page.error_code.is_some());
    assert!(r.recover(key(&f)).await.unwrap().confirmed.is_none());
    drop(worker);
    drop(r);
    f.command("flush_publication").await;
    let r = Arc::new(recovery(&f, &d));
    let mut worker = runner(r.clone(), Some(signer(&mut f).await));
    assert!(worker.page().await.unwrap().error_code.is_none());
    advance(&mut f, 1).await;
    let page = worker.page().await.unwrap();
    assert_eq!(
        (page.checked, page.resolved, page.awaiting_wallet),
        (1, 0, 0)
    );
    let saved = r.recover(key(&f)).await.unwrap();
    assert!(saved.draft.is_none() && saved.confirmed.is_none());
    assert_eq!(f.request("status").await["publications"], 1);
    f.stop().await;
}

#[tokio::test]
async fn buyer_runner_bad_record_does_not_starve_following_good_hold() {
    let mut f = Fixture::new("tnk", "llm").await;
    let (buyer, provider) = keys(&mut f).await;
    let mut bad = f.auth.clone();
    bad.terms.capacity_lease = (0..1024u64)
        .map(|n| format!("{n:064x}"))
        .find(|lease| {
            let mut t = bad.terms.clone();
            t.capacity_lease = lease.clone();
            t.digest().unwrap() < f.auth.terms.digest().unwrap()
        })
        .unwrap();
    let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
    bad.buyer_sig = hex(&buyer
        .sign(&bad.terms.buyer_signing_bytes().unwrap())
        .to_bytes());
    bad.provider_sig = hex(&provider
        .sign(&bad.terms.provider_signing_bytes().unwrap())
        .to_bytes());
    let d = dir();
    let r = Arc::new(recovery(&f, &d));
    r.retain_reservation(bad, f.policy.clone(), 1)
        .await
        .unwrap();
    r.refresh(&f.auth, 2).await.unwrap();
    let mut worker = runner(r, None);
    let page = worker.page().await.unwrap();
    assert_eq!(page.checked, 1);
    assert!(page.error_code.is_some() && page.more);
    let page = worker.page().await.unwrap();
    assert_eq!(page.checked, 1);
    assert!(page.error_code.is_none() && !page.more);
    f.stop().await;
}

#[tokio::test]
async fn buyer_runner_shutdown_interrupts_backoff_without_dropping_pending_work() {
    let mut f = Fixture::new("fiat", "decisions").await;
    let d = dir();
    let r = Arc::new(recovery(&f, &d));
    r.refresh(&f.auth, 1).await.unwrap();
    f.command("nonce").await;
    let mut policy = Policy::default();
    policy.schedule.retry_initial_ms = 30_000;
    policy.schedule.retry_max_ms = 30_000;
    let worker = Runner::new(r.clone(), None, policy, 1).unwrap();
    let (stop, receiver) = watch::channel(false);
    let (updates, mut health) = watch::channel(Health::default());
    let task = tokio::spawn(worker.run(receiver, updates));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            health.changed().await.unwrap();
            if health.borrow().phase == Phase::Degraded {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(health.borrow().consecutive_failures, 1);
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(health.borrow().phase, Phase::Stopped);
    assert_eq!(r.pending(None, 64).await.unwrap(), vec![key(&f)]);
    assert_eq!(f.request("status").await["submissions"], 0);
    drop(r);
    f.command("reset").await;
    f.command("final").await;
    let mut worker = runner(Arc::new(recovery(&f, &d)), None);
    assert_eq!(worker.page().await.unwrap().resolved, 1);
    f.stop().await;
}

#[tokio::test]
async fn buyer_runner_locked_wallet_can_recover_already_signed_expiry() {
    let mut f = Fixture::new_with_expiry("tnk", "llm", true).await;
    let d = dir();
    let r = Arc::new(recovery(&f, &d));
    advance(&mut f, 1).await;
    r.prepare_expiry(&f.auth, 1).await.unwrap();
    r.sign_expiry(&signer(&mut f).await, key(&f)).await.unwrap();
    drop(r);
    let mut worker = runner(Arc::new(recovery(&f, &d)), None);
    assert_eq!(worker.page().await.unwrap().resolved, 1);
    f.stop().await;
}
