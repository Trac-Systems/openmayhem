use super::*;
use crate::discovery::{Context, Identity, Mode, Page, Proof, QueryBinding};
use std::sync::Mutex;

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}

fn commit(catalog: &Catalog, observed: u64) -> crate::catalog::Status {
    let prior = catalog.read().unwrap().status().committed;
    let n = prior.as_ref().map_or(1, |c| c.proof.signed_length + 1);
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
            epoch: 1,
        },
        proof: Proof {
            view_key: "d".repeat(64),
            fork: 0,
            signed_length: n,
            tree_hash: format!("{n:064x}"),
        },
        mode: if prior.is_some() {
            Mode::Changes
        } else {
            Mode::Snapshot
        },
        base_proof: prior.map(|c| c.proof),
        entries: vec![],
        truncated: false,
        next_cursor: None,
        checkpoint: Some(format!("pdc1.timing{n}.{}", "e".repeat(128))),
    };
    catalog
        .apply(&catalog.refresh_ticket().unwrap(), &page, observed)
        .unwrap()
}

// Exercise the actual supervisor loop with an in-process RPC substitute and
// real catalog commits. No sockets, wall-clock sleeps, or external services.
async fn refresh_twice(rpc_ms: u64, seed: u64) -> Vec<(u64, u64, crate::catalog::Status)> {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(dir.path().join("catalog.redb"), identity()).unwrap();
    let origin = Instant::now();
    let clock = || 1_000 + origin.elapsed().as_millis() as u64;
    let records = Mutex::new(Vec::new());
    let (stop, stopped) = watch::channel(false);
    let (updates, health) = watch::channel(Health::default());
    run_refresh(
        |observed| {
            let catalog = &catalog;
            let records = &records;
            let stop = &stop;
            async move {
                tokio::time::sleep(Duration::from_millis(rpc_ms)).await;
                let status = commit(catalog, observed);
                let mut records = records.lock().unwrap();
                records.push((observed, clock(), status.clone()));
                if records.len() == 2 {
                    stop.send_replace(true);
                }
                Ok(RefreshOutcome::Committed(status))
            }
        },
        clock,
        RefreshPolicy {
            interval_ms: 5_000,
            ..Default::default()
        },
        seed,
        stopped,
        updates,
    )
    .await
    .unwrap();
    assert_eq!(health.borrow().phase, Phase::Stopped);
    records.into_inner().unwrap()
}

#[tokio::test(start_paused = true)]
async fn successful_single_page_timing_keeps_evidence_fresh() {
    // Seed 158 gives a 5.9 s first jittered interval from a 5 s / 20% policy.
    let records = refresh_twice(4_900, 158).await;
    let [(first, first_done, old), (second, second_done, new)] = records.as_slice() else {
        panic!("expected exactly two refreshes");
    };
    assert_eq!(first_done - first, 4_900);
    assert_eq!(second - first, 5_900);
    assert_eq!(second_done - first, 10_800);
    assert!(old.discovery_is_fresh(*second_done, crate::presence::CATALOG_AGE_MS));
    assert_eq!(old.committed.as_ref().unwrap().observed_at_ms, Some(*first));
    assert_eq!(
        new.committed.as_ref().unwrap().observed_at_ms,
        Some(*second)
    );
    assert!(4_900 + 5_900 + 4_900 > crate::presence::CATALOG_AGE_MS);
}

#[tokio::test(start_paused = true)]
async fn slow_success_timing_yields_without_catch_up_or_stale_renewal() {
    let records = refresh_twice(20_000, 158).await;
    assert_eq!(records[1].0 - records[0].1, 1);
    for (observed, completed, status) in records {
        assert_eq!(completed - observed, 20_000);
        assert_eq!(
            status.committed.as_ref().unwrap().observed_at_ms,
            Some(observed)
        );
        assert!(!status.discovery_is_fresh(completed, crate::presence::CATALOG_AGE_MS));
    }
}
