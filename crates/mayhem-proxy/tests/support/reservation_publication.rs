use super::*;

#[tokio::test]
async fn buyer_reservation_publication_all_rails_and_families_is_durable_and_confirmed() {
    for family in ["llm", "decisions"] {
        for rail in ["fiat", "tnk", "tap"] {
            let mut f = Fixture::new_with_mode(rail, family, "unreserved").await;
            let d = dir();
            let r = recovery(&f, &d);
            let before = f.request("state").await;
            assert_eq!(before["summary"]["reserved_au"], "50");
            let id = r
                .retain_reservation(f.auth.clone(), f.policy.clone(), 1)
                .await
                .unwrap();
            assert_eq!(id, key(&f));
            assert!(r
                .recover(id.clone())
                .await
                .unwrap()
                .reservation
                .is_some_and(|s| s.at == 1 && !s.confirmed));
            assert_eq!(f.request("status").await["submissions"], 0);
            // Retaining a retry cannot change even the unsigned envelope time.
            r.retain_reservation(f.auth.clone(), f.policy.clone(), 999)
                .await
                .unwrap();
            drop(r);
            let r = recovery(&f, &d);
            assert_eq!(r.pending(None, 64).await.unwrap(), vec![id.clone()]);
            let o = r.publish_reservation(id.clone(), 2).await.unwrap();
            assert_eq!(o.initial_binding().unwrap().accepted_terms, id);
            assert!(r
                .recover(id.clone())
                .await
                .unwrap()
                .reservation
                .is_some_and(|s| s.at == 1 && s.confirmed));
            let after = f.request("state").await;
            assert_eq!(
                after["summary"]["reserved_au"],
                (50 + f.auth.terms.max_spend_au).to_string()
            );
            assert_eq!(
                after["balance"], before["balance"],
                "reserving does not debit"
            );
            assert_eq!(f.request("status").await["publications"], 1);
            drop(r);
            let r = recovery(&f, &d);
            r.publish_reservation(id, 3).await.unwrap();
            assert_eq!(
                f.request("status").await["submissions"],
                1,
                "confirmed reservation is not re-appended"
            );
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn buyer_reservation_pending_or_lost_ack_never_invents_confirmation_or_second_hold() {
    for mode in ["publish_pending", "publish_lost_ack", "nonce"] {
        let mut f = Fixture::new_with_mode("tap", "llm", "unreserved").await;
        let d = dir();
        let r = recovery(&f, &d);
        let id = r
            .retain_reservation(f.auth.clone(), f.policy.clone(), 7)
            .await
            .unwrap();
        f.command(mode).await;
        let first = r.publish_reservation(id.clone(), 8).await;
        assert_eq!(first.is_ok(), mode == "publish_lost_ack");
        assert_eq!(
            r.recover(id.clone())
                .await
                .unwrap()
                .reservation
                .unwrap()
                .confirmed,
            mode == "publish_lost_ack"
        );
        assert_eq!(
            r.prune(u64::MAX, 64).await.unwrap(),
            0,
            "uncertain publication cannot be aged away"
        );
        drop(r);
        if mode == "publish_pending" {
            f.command("flush_publication").await;
        }
        if mode == "nonce" {
            f.command("reset").await;
        }
        let r = recovery(&f, &d);
        let a = r.publish_reservation(id.clone(), 9);
        let b = r.publish_reservation(id.clone(), 10);
        let (a, b) = tokio::join!(a, b);
        a.unwrap();
        b.unwrap();
        assert!(r.recover(id).await.unwrap().reservation.unwrap().confirmed);
        assert_eq!(f.request("status").await["publications"], 1);
        assert_eq!(
            f.request("state").await["summary"]["reserved_au"],
            (50 + f.auth.terms.max_spend_au).to_string()
        );
        f.stop().await;
    }
}

#[tokio::test]
async fn buyer_reservation_rejects_bad_authority_policy_and_time_before_publication() {
    let mut f = Fixture::new_with_mode("tnk", "llm", "unreserved").await;
    let d = dir();
    let r = recovery(&f, &d);
    let mut auth = f.auth.clone();
    auth.provider_sig = "0".repeat(128);
    assert!(r
        .retain_reservation(auth, f.policy.clone(), 1)
        .await
        .is_err());
    let mut policy = f.policy.clone();
    policy.allow_checkpoints = !policy.allow_checkpoints;
    assert!(r
        .retain_reservation(f.auth.clone(), policy, 1)
        .await
        .is_err());
    assert!(r
        .retain_reservation(f.auth.clone(), f.policy.clone(), u64::MAX)
        .await
        .is_err());
    let stranger = Fixture::new_with_mode("tnk", "llm", "unreserved").await;
    assert!(r
        .retain_reservation(stranger.auth.clone(), stranger.policy.clone(), 1)
        .await
        .is_err());
    assert!(r.pending(None, 64).await.unwrap().is_empty());
    assert_eq!(f.request("status").await["submissions"], 0);
    stranger.stop().await;
    f.stop().await;
}

#[tokio::test]
async fn buyer_reservation_replay_of_old_closed_recovery_cannot_create_another_hold() {
    let mut f = Fixture::new("fiat", "llm").await;
    let d = dir();
    let r = recovery(&f, &d);
    r.refresh(&f.auth, 1).await.unwrap();
    f.command("final").await;
    r.refresh(&f.auth, 2).await.unwrap();
    let before = f.request("state").await;
    let id = r
        .retain_reservation(f.auth.clone(), f.policy.clone(), 3)
        .await
        .unwrap();
    assert!(r.recover(id.clone()).await.unwrap().reservation.is_none());
    assert!(
        r.publish_reservation(id, 4)
            .await
            .unwrap()
            .receipt_head()
            .unwrap()
            .unwrap()
            .body
            .final_receipt
    );
    assert_eq!(f.request("status").await["submissions"], 0);
    assert_eq!(
        f.request("state").await,
        before,
        "replay changes neither native hold nor unsettled proxy liability"
    );
    f.stop().await;
}
