use mayhem_proxy::{catalog::Catalog, discovery::*, Error};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: 30,
    }
}
fn context() -> Context {
    serde_json::from_value(json!({"network_id":"918", "msb_bootstrap":"a".repeat(64), "subnet_bootstrap":"b".repeat(64), "contract_version":30,"epoch":100})).unwrap()
}
fn proof(n: u64) -> Proof {
    Proof {
        view_key: "c".repeat(64),
        fork: 0,
        signed_length: n,
        tree_hash: format!("{n:064x}"),
    }
}
fn token(n: u64) -> String {
    format!("pdc1.test{n}.{}", "d".repeat(128))
}
fn row(n: u64) -> Entry {
    Entry {
        key: format!("{CATALOG_PREFIX}families/f{n:06}"),
        value: json!({"enabled":true,"label":format!("Family {n}")}),
    }
}
fn page(n: u64, entries: Vec<Entry>, base: Option<Proof>, next: Option<u64>) -> Page {
    Page {
        ok: true,
        lane: "proxy".into(),
        schema_version: 1,
        request_nonce: "e".repeat(64),
        query: QueryBinding::catalog(),
        context: context(),
        proof: proof(n),
        mode: if base.is_some() {
            Mode::Changes
        } else {
            Mode::Snapshot
        },
        base_proof: base,
        entries,
        truncated: next.is_some(),
        next_cursor: next.map(token),
        checkpoint: next.is_none().then(|| token(n + 1000)),
    }
}
fn apply(c: &Catalog, p: &Page) {
    c.apply(&c.refresh_ticket().unwrap(), p, 10000).unwrap();
}
fn get(c: &Catalog, n: u64) -> Option<Value> {
    c.read().unwrap().get(&row(n).key).unwrap()
}
fn open(path: &Path) -> Catalog {
    Catalog::open(path, identity()).unwrap()
}

#[test]
fn incomplete_snapshot_survives_restart_and_is_never_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.redb");
    {
        let catalog = open(&path);
        apply(&catalog, &page(10, vec![row(1), row(2)], None, Some(1)));
        assert!(get(&catalog, 1).is_none());
        assert_eq!(
            catalog.refresh_ticket().unwrap().query().cursor,
            Some(token(1))
        );
        assert!(catalog.read().unwrap().status().invalidated);
    }
    let catalog = open(&path);
    let old_read = catalog.read().unwrap();
    apply(&catalog, &page(10, vec![row(3)], None, None));
    assert!(
        old_read.get(&row(1).key).unwrap().is_none(),
        "readers retain their complete snapshot"
    );
    assert_eq!(get(&catalog, 1), Some(row(1).value));
    assert_eq!(get(&catalog, 3), Some(row(3).value));
    let status = catalog.read().unwrap().status();
    assert!(!status.refresh_in_progress);
    assert!(status.discovery_is_fresh(11000, 2000));
    assert!(!status.discovery_is_fresh(13000, 2000));
    assert!(
        !status.discovery_is_fresh(9000, 2000),
        "clock reversal fails closed"
    );
    assert_eq!(
        catalog.refresh_ticket().unwrap().query().since,
        Some(token(1010))
    );
}

#[test]
fn long_traversal_freshness_uses_first_observation_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    {
        let catalog = open(&path);
        catalog
            .apply(
                &catalog.refresh_ticket().unwrap(),
                &page(10, vec![row(1)], None, Some(1)),
                1000,
            )
            .unwrap();
    }
    let catalog = open(&path);
    let status = catalog
        .apply(
            &catalog.refresh_ticket().unwrap(),
            &page(10, vec![row(2)], None, None),
            100000,
        )
        .unwrap();
    assert_eq!(
        status.committed.as_ref().unwrap().observed_at_ms,
        Some(1000)
    );
    assert!(!status.discovery_is_fresh(100001, 2000));
    let status = catalog
        .apply(
            &catalog.refresh_ticket().unwrap(),
            &page(11, vec![], Some(proof(10)), None),
            100002,
        )
        .unwrap();
    assert!(
        status.discovery_is_fresh(100003, 2000),
        "a fresh complete delta restores freshness"
    );
    let mut legacy = serde_json::to_value(status.committed.as_ref().unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("observed_at_ms");
    let mut unknown_age = status.clone();
    unknown_age.committed = Some(serde_json::from_value(legacy).unwrap());
    assert!(
        !unknown_age.discovery_is_fresh(100003, 2000),
        "unknown old-cache age is not fresh evidence"
    );
}

#[test]
fn delta_delete_update_and_insertion_commit_together_and_fence_stale_responses() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = open(&dir.path().join("catalog"));
    apply(&catalog, &page(10, vec![row(1), row(2)], None, None));
    let old_read = catalog.read().unwrap();
    let stale = catalog.refresh_ticket().unwrap();
    let removal = Entry {
        key: row(1).key,
        value: Value::Null,
    };
    let first = page(20, vec![removal], Some(proof(10)), Some(4));
    catalog.apply(&stale, &first, 20000).unwrap();
    assert!(matches!(
        catalog.apply(&stale, &first, 20000),
        Err(Error::StaleRefresh)
    ));
    assert!(
        get(&catalog, 1).is_some(),
        "incomplete deltas cannot leak deletion"
    );
    let mut updated = row(2);
    updated.value["enabled"] = json!(false);
    apply(
        &catalog,
        &page(20, vec![updated.clone(), row(3)], Some(proof(10)), None),
    );
    assert!(get(&catalog, 1).is_none());
    assert_eq!(get(&catalog, 2), Some(updated.value));
    assert_eq!(get(&catalog, 3), Some(row(3).value));
    assert_eq!(old_read.get(&row(1).key).unwrap(), Some(row(1).value));
    apply(&catalog, &page(21, vec![], Some(proof(20)), None));
    assert_eq!(
        get(&catalog, 3),
        Some(row(3).value),
        "empty changes advance only the checkpoint"
    );
}

#[test]
fn invalid_pages_never_modify_checkpoint_or_visible_rows() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = open(&dir.path().join("catalog"));
    apply(&catalog, &page(10, vec![row(1)], None, None));
    let good = page(20, vec![row(2)], Some(proof(10)), None);
    let before = catalog.refresh_ticket().unwrap();
    let mut invalid = Vec::new();
    let mut p = good.clone();
    p.base_proof = Some(proof(9));
    invalid.push(p);
    let mut p = good.clone();
    p.proof.fork = 1;
    invalid.push(p);
    let mut p = good.clone();
    p.proof.signed_length = 5;
    invalid.push(p);
    let mut p = good.clone();
    p.proof = proof(10);
    p.proof.tree_hash = "f".repeat(64);
    invalid.push(p);
    let mut p = good.clone();
    p.context.contract_version += 1;
    invalid.push(p);
    let mut p = good.clone();
    p.query.kind = "markets".into();
    invalid.push(p);
    let mut p = good.clone();
    p.entries.push(row(2));
    invalid.push(p);
    let mut p = good.clone();
    p.entries[0].key = "native/receipts/anything".into();
    invalid.push(p);
    let mut p = good.clone();
    p.entries[0].value["credential"] = json!("never-cache-unknown-fields");
    invalid.push(p);
    let mut p = good.clone();
    p.entries[0] = Entry {
        key: format!("{CATALOG_PREFIX}endpoints/{}", "a".repeat(64)),
        value: json!({"enabled":true}),
    };
    invalid.push(p);
    let mut p = good.clone();
    p.entries[0] = Entry {
        key: format!("{CATALOG_PREFIX}network/current"),
        value: json!({"enabled":true,"network_id":"another","msb_bootstrap":"a".repeat(64),"subnet_bootstrap":"b".repeat(64),"contract_version":30}),
    };
    invalid.push(p);
    let mut p = good.clone();
    p.entries[0].value["label"] = json!("x".repeat(20000));
    invalid.push(p);
    let mut p = good.clone();
    p.truncated = true;
    invalid.push(p);
    let mut p = good.clone();
    p.entries.clear();
    p.truncated = true;
    p.next_cursor = Some(token(4));
    p.checkpoint = None;
    invalid.push(p);
    for p in invalid {
        assert!(catalog.apply(&before, &p, 20000).is_err());
        assert_eq!(catalog.refresh_ticket().unwrap(), before);
        assert!(get(&catalog, 1).is_some());
        assert!(get(&catalog, 2).is_none());
    }
    apply(&catalog, &good);
}

#[test]
fn continuation_rejects_changed_snapshot_repeated_cursor_and_nonadvancing_keys() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = open(&dir.path().join("catalog"));
    apply(&catalog, &page(10, vec![row(1)], None, Some(1)));
    let ticket = catalog.refresh_ticket().unwrap();
    for p in [
        page(11, vec![row(2)], None, None),
        page(10, vec![row(1)], None, None),
        page(10, vec![row(2)], None, Some(1)),
    ] {
        assert!(catalog.apply(&ticket, &p, 20000).is_err());
        assert_eq!(catalog.refresh_ticket().unwrap(), ticket);
    }
    apply(&catalog, &page(10, vec![row(2)], None, None));
}

#[test]
fn invalidation_retains_old_data_but_requires_a_complete_fresh_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    {
        let catalog = open(&path);
        apply(&catalog, &page(10, vec![row(1)], None, None));
        apply(&catalog, &page(20, vec![row(2)], Some(proof(10)), Some(2)));
        let in_flight = catalog.refresh_ticket().unwrap();
        catalog.invalidate(&in_flight).unwrap();
        assert!(matches!(
            catalog.apply(
                &in_flight,
                &page(20, vec![row(3)], Some(proof(10)), None),
                10000
            ),
            Err(Error::StaleRefresh)
        ));
        assert!(get(&catalog, 1).is_some());
        assert!(get(&catalog, 2).is_none());
        assert!(!catalog
            .read()
            .unwrap()
            .status()
            .discovery_is_fresh(10001, 10000));
    }
    let catalog = open(&path);
    assert_eq!(*catalog.refresh_ticket().unwrap().query(), Query::catalog());
    let mut replacement = page(3, vec![row(9)], None, None);
    replacement.proof.view_key = "f".repeat(64);
    replacement.proof.fork = 1;
    apply(&catalog, &replacement);
    assert!(get(&catalog, 1).is_none());
    assert!(get(&catalog, 9).is_some());
}

#[test]
fn wrong_identity_corruption_and_second_writer_do_not_reset_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let catalog = open(&path);
    apply(&catalog, &page(10, vec![row(1)], None, None));
    assert!(Catalog::open(&path, identity()).is_err());
    drop(catalog);
    let mut wrong = identity();
    wrong.contract_version += 1;
    assert!(matches!(Catalog::open(&path, wrong), Err(Error::Identity)));
    assert!(get(&open(&path), 1).is_some());
    let corrupt = dir.path().join("corrupt");
    std::fs::write(&corrupt, b"not a database").unwrap();
    assert!(Catalog::open(&corrupt, identity()).is_err());
    assert_eq!(std::fs::read(&corrupt).unwrap(), b"not a database");
}

#[test]
fn indexed_paging_and_reopening_a_large_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    {
        let catalog = open(&path);
        for n in 0..1000 {
            let next = (n != 999).then_some(n + 1);
            apply(
                &catalog,
                &page(
                    100001,
                    (n * 100..(n + 1) * 100).map(row).collect(),
                    None,
                    next,
                ),
            );
        }
    }
    let catalog = open(&path);
    assert_eq!(get(&catalog, 99999), Some(row(99999).value));
    let read = catalog.read().unwrap();
    let result = read
        .page(&format!("{CATALOG_PREFIX}families/f099"), None, 100)
        .unwrap();
    assert_eq!(result.entries.len(), 100);
    assert_eq!(result.entries[0].key, row(99000).key);
    let next = read
        .page(
            &format!("{CATALOG_PREFIX}families/f099"),
            result.next_after.as_deref(),
            100,
        )
        .unwrap();
    assert_eq!(next.entries[0].key, row(99100).key);
    assert!(read.page("native/receipts", None, 1).is_err());
    assert!(read.page(CATALOG_PREFIX, None, 101).is_err());
}

#[test]
fn concurrent_refreshers_only_accept_one_response() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(open(&dir.path().join("catalog")));
    let ticket = catalog.refresh_ticket().unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let catalog = catalog.clone();
            let ticket = ticket.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                catalog.apply(&ticket, &page(10, vec![row(1)], None, None), 10000)
            })
        })
        .collect();
    let mut accepted = 0;
    let mut stale = 0;
    for t in threads {
        match t.join().unwrap() {
            Ok(_) => accepted += 1,
            Err(Error::StaleRefresh) => stale += 1,
            other => panic!("unexpected: {other:?}"),
        }
    }
    assert_eq!((accepted, stale), (1, 1));
}

#[test]
fn crash_child() {
    let Some(path) = std::env::var_os("MAYHEM_PROXY_CACHE_CRASH_TEST") else {
        return;
    };
    let catalog = open(Path::new(&path));
    apply(&catalog, &page(10, vec![row(1)], None, Some(1)));
    std::process::exit(73); // Deliberately bypass database and process destructors.
}

#[test]
fn restart_after_abrupt_process_exit_resumes_the_durable_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let exit = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child"])
        .env("MAYHEM_PROXY_CACHE_CRASH_TEST", &path)
        .output()
        .unwrap();
    assert_eq!(exit.status.code(), Some(73));
    let catalog = open(&path);
    assert_eq!(
        catalog.refresh_ticket().unwrap().query().cursor,
        Some(token(1))
    );
    assert!(get(&catalog, 1).is_none());
    apply(&catalog, &page(10, vec![row(2)], None, None));
    assert!(get(&catalog, 1).is_some());
    assert!(get(&catalog, 2).is_some());
}
