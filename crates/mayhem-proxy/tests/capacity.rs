#![cfg(unix)]
use mayhem_proxy::{
    attempts::{Digest, Identity},
    capacity::*,
};
use std::{
    path::Path,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};
fn d(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: d(1),
        subnet_bootstrap: d(2),
        controller_pubkey: d(3),
    }
}
fn limits() -> Limits {
    Limits {
        max_groups: 20,
        max_routes: 100,
        max_leases: 1000,
        max_evidence_age: Duration::from_secs(60),
    }
}
fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
fn open(path: &Path) -> Authority {
    Authority::open(path, identity(), limits()).unwrap()
}
fn route(id: u64, group: u64, lane: Lane, cap: u32) -> Route {
    Route {
        id: d(id),
        group: d(group),
        lane,
        max_concurrency: cap,
    }
}
fn ready(a: &Authority, scope: Scope, allowance: u32) {
    let t = a.begin_observation(scope).unwrap();
    a.observe(t, evidence(Readiness::Ready, allowance)).unwrap();
}
fn evidence(state: Readiness, allowance: u32) -> Evidence {
    Evidence {
        state,
        allowance,
        age: Duration::ZERO,
        valid_for: Duration::from_secs(60),
    }
}
fn setup(a: &Authority, group: u64, cap: u32, routes: &[(u64, Lane)]) {
    a.configure_group(d(group), cap).unwrap();
    ready(a, Scope::Group(d(group)), cap);
    for (id, lane) in routes {
        a.configure_route(route(*id, group, *lane, cap)).unwrap();
        ready(a, Scope::Route(d(*id)), cap);
    }
}
fn work(n: u64) -> Work {
    Work {
        invocation: d(n + 1000),
        request_hash: d(987),
    }
}
fn finish(a: &Authority, lease: Lease) -> bool {
    a.complete(VerifiedCompletion {
        lease,
        evidence: d(999),
    })
    .unwrap()
}

#[test]
fn old_capacity_schema_migrates_without_inventing_signing_or_releasing_native_work() {
    use redb::{ReadableTable, TableDefinition};
    let dir = private_dir();
    let path = dir.path().join("capacity");
    let a = open(&path);
    setup(&a, 10, 4, &[(20, Lane::Native), (21, Lane::Proxy)]);
    let native = a.reserve(&d(20), work(1)).unwrap().lease().clone();
    let legacy = a.reserve(&d(21), work(2)).unwrap().lease().clone();
    drop(a);
    let database = redb::Database::open(&path).unwrap();
    let tx = database.begin_write().unwrap();
    tx.delete_table(TableDefinition::<&str, &[u8]>::new(
        "capacity_signing_intents_v1",
    ))
    .unwrap();
    tx.delete_table(TableDefinition::<&str, &str>::new(
        "capacity_constraint_leases_v1",
    ))
    .unwrap();
    {
        let mut table = tx
            .open_table(TableDefinition::<&str, &[u8]>::new("capacity_meta_v1"))
            .unwrap();
        let mut m: serde_json::Value =
            serde_json::from_slice(table.get("state").unwrap().unwrap().value()).unwrap();
        m["schema"] = 1.into();
        table
            .insert("state", serde_json::to_vec(&m).unwrap().as_slice())
            .unwrap();
    }
    tx.commit().unwrap();
    drop(database);
    let a = open(&path);
    assert_eq!(a.status(&d(21)).unwrap().group_occupied, 2);
    for lease in [native, legacy] {
        assert_eq!(a.lease(&lease.id).unwrap().unwrap().phase, Phase::Uncertain);
        assert!(a.signing_intent(&lease.id).unwrap().is_none());
    }
    drop(a);
    // Missing required storage in the upgraded schema is corruption, not a new
    // empty table that could make protected signing obligations disappear.
    let database = redb::Database::open(&path).unwrap();
    let tx = database.begin_write().unwrap();
    tx.delete_table(TableDefinition::<&str, &[u8]>::new(
        "capacity_signing_intents_v1",
    ))
    .unwrap();
    tx.commit().unwrap();
    drop(database);
    assert!(Authority::open(&path, identity(), limits()).is_err());
}

#[test]
fn native_and_proxy_aliases_share_atomic_capacity_under_concurrency() {
    let dir = private_dir();
    let a = Arc::new(open(&dir.path().join("capacity")));
    setup(
        &a,
        10,
        2,
        &[(20, Lane::Native), (21, Lane::Proxy), (22, Lane::Proxy)],
    );
    let barrier = Arc::new(Barrier::new(24));
    let threads = (0..24)
        .map(|i| {
            let a = a.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                b.wait();
                a.reserve(&d(20 + i % 3), work(i))
            })
        })
        .collect::<Vec<_>>();
    let mut admitted = Vec::new();
    for t in threads {
        match t.join().unwrap() {
            Ok(r) => admitted.push(r),
            Err(Error::Busy) => (),
            _ => panic!("unexpected admission result"),
        }
    }
    assert_eq!(admitted.len(), 2);
    for id in [20, 21, 22] {
        let s = a.status(&d(id)).unwrap();
        assert_eq!(s.group_occupied, 2);
        assert_eq!(s.available, 0);
        assert_eq!(s.state, Readiness::Busy);
    }
    let first = admitted.pop().unwrap();
    a.cancel_reserved(first).unwrap();
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    a.cancel_reserved(admitted.pop().unwrap()).unwrap();
    assert_eq!(a.status(&d(20)).unwrap().available, 2);
}

#[test]
fn limit_and_observed_allowance_are_total_not_double_subtracted_free_slots() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 8, &[(20, Lane::Proxy), (21, Lane::Proxy)]);
    ready(&a, Scope::Group(d(10)), 4);
    let rs = (0..3)
        .map(|n| a.reserve(&d(20), work(n)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    assert_eq!(a.status(&d(21)).unwrap().available, 1);
    for r in rs {
        let active = a.dispatch(&r).unwrap();
        assert!(finish(&a, active.lease().clone()));
    }
    ready(&a, Scope::Group(d(10)), 999);
    assert_eq!(a.status(&d(20)).unwrap().available, 8);
    a.configure_route(route(21, 10, Lane::Proxy, 2)).unwrap();
    ready(&a, Scope::Route(d(21)), 99);
    assert_eq!(a.status(&d(21)).unwrap().available, 2);
}

#[test]
fn absence_busy_and_stale_evidence_stop_new_work_not_existing_execution() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 2, &[(20, Lane::Proxy), (21, Lane::Proxy)]);
    setup(&a, 11, 2, &[(22, Lane::Proxy)]);
    let r = a.reserve(&d(20), work(1)).unwrap();
    let sent = a.dispatch(&r).unwrap();
    let ticket = a.begin_observation(Scope::Route(d(20))).unwrap();
    a.observe(ticket, evidence(Readiness::Busy, 0)).unwrap();
    assert!(matches!(a.reserve(&d(20), work(2)), Err(Error::Busy)));
    assert_eq!(a.status(&d(21)).unwrap().available, 1);
    assert_eq!(a.status(&d(22)).unwrap().available, 2);
    let ticket = a.begin_observation(Scope::Group(d(10))).unwrap();
    a.observe(ticket, evidence(Readiness::Unavailable, 0))
        .unwrap();
    assert!(matches!(
        a.reserve(&d(21), work(2)),
        Err(Error::Unavailable)
    ));
    assert_eq!(
        a.lease(&sent.lease().id).unwrap().unwrap().phase,
        Phase::Dispatched
    );
    assert!(finish(&a, sent.lease().clone()));
    assert_eq!(a.status(&d(21)).unwrap().group_occupied, 0);
    // Removing all known local work cannot itself claim the upstream recovered.
    assert_eq!(a.status(&d(21)).unwrap().state, Readiness::Unavailable);
}

#[test]
fn health_age_expires_without_freeing_or_renewing_an_uncertain_slot() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 2, &[(20, Lane::Proxy)]);
    let r = a.reserve(&d(20), work(1)).unwrap();
    let sent = a.dispatch(&r).unwrap();
    let unknown = a.uncertain(sent).unwrap();
    let ticket = a.begin_observation(Scope::Group(d(10))).unwrap();
    a.observe(
        ticket,
        Evidence {
            state: Readiness::Ready,
            allowance: 2,
            age: Duration::from_secs(59),
            valid_for: Duration::from_millis(59_050),
        },
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(75));
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 1);
    assert!(matches!(a.reserve(&d(20), work(2)), Err(Error::Checking)));
    ready(&a, Scope::Group(d(10)), 2);
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    assert_eq!(a.lease(&unknown.id).unwrap(), Some(unknown.clone()));
    assert!(finish(&a, unknown));
}

#[test]
fn stale_observations_cannot_reverse_newer_failures_or_reconfigured_routes() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 2, &[(20, Lane::Proxy)]);
    let old = a.begin_observation(Scope::Group(d(10))).unwrap();
    let newer = a.begin_observation(Scope::Group(d(10))).unwrap();
    a.observe(newer, evidence(Readiness::Busy, 0)).unwrap();
    assert!(matches!(
        a.observe(old, evidence(Readiness::Ready, 2)),
        Err(Error::Stale)
    ));
    let old = a.begin_observation(Scope::Route(d(20))).unwrap();
    a.remove_route(&d(20)).unwrap();
    a.configure_route(route(20, 10, Lane::Proxy, 2)).unwrap();
    assert!(matches!(
        a.observe(old, evidence(Readiness::Ready, 2)),
        Err(Error::Stale)
    ));
    let old = a.begin_observation(Scope::Group(d(10))).unwrap();
    a.configure_group(d(10), 3).unwrap();
    assert!(matches!(
        a.observe(old, evidence(Readiness::Ready, 3)),
        Err(Error::Stale)
    ));
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
}

#[test]
fn observations_are_bound_to_the_authority_even_with_identical_configuration() {
    let dir = private_dir();
    let a = open(&dir.path().join("a"));
    let b = open(&dir.path().join("b"));
    setup(&a, 10, 2, &[(20, Lane::Proxy)]);
    setup(&b, 10, 2, &[(20, Lane::Proxy)]);
    let ta = a.begin_observation(Scope::Group(d(10))).unwrap();
    let _tb = b.begin_observation(Scope::Group(d(10))).unwrap();
    assert!(matches!(
        b.observe(ta, evidence(Readiness::Ready, 2)),
        Err(Error::Stale)
    ));
    let ticket = a.begin_observation(Scope::Group(d(10))).unwrap();
    assert!(matches!(
        a.observe(
            ticket,
            Evidence {
                age: Duration::from_secs(60),
                ..evidence(Readiness::Ready, 2)
            }
        ),
        Err(Error::Invalid)
    ));
}

#[test]
fn capacity_is_rechecked_before_dispatch_and_failed_dispatch_can_cancel_reservation() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 2, &[(20, Lane::Proxy)]);
    let r = a.reserve(&d(20), work(1)).unwrap();
    let ticket = a.begin_observation(Scope::Group(d(10))).unwrap();
    a.observe(ticket, evidence(Readiness::Busy, 0)).unwrap();
    assert!(matches!(a.dispatch(&r), Err(Error::Busy)));
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 1);
    a.cancel_reserved(r).unwrap();
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 0);
}

#[test]
fn dropped_handles_do_not_free_slots_and_same_work_cannot_reserve_via_an_alias() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 2, &[(20, Lane::Proxy), (21, Lane::Native)]);
    let r = a.reserve(&d(20), work(1)).unwrap();
    let saved = r.lease().clone();
    drop(r);
    assert!(matches!(
        a.reserve(&d(21), work(1)),
        Err(Error::ExistingWork)
    ));
    let mut wrong = work(1);
    wrong.request_hash = d(222);
    assert!(matches!(a.work_lease(&wrong), Err(Error::Binding)));
    assert_eq!(a.work_lease(&work(1)).unwrap(), Some(saved.clone()));
    let r = a.reserve(&d(21), work(2)).unwrap();
    let sent = a.dispatch(&r).unwrap();
    assert!(matches!(a.dispatch(&r), Err(Error::Binding)));
    assert!(matches!(a.cancel_reserved(r), Err(Error::Binding)));
    let second = sent.lease().clone();
    drop(sent);
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    assert!(finish(&a, saved.clone()));
    assert!(!finish(&a, saved));
    assert!(finish(&a, second));
}

#[test]
fn old_controller_tickets_are_fenced_and_restart_does_not_refresh_health_or_release_work() {
    let dir = private_dir();
    let path = dir.path().join("capacity");
    let a = open(&path);
    setup(&a, 10, 2, &[(20, Lane::Proxy)]);
    let ticket = a.begin_observation(Scope::Group(d(10))).unwrap();
    let r = a.reserve(&d(20), work(1)).unwrap();
    let original = r.lease().clone();
    let r2 = a.reserve(&d(20), work(2)).unwrap();
    let sent = a.dispatch(&r2).unwrap();
    drop(a);
    let a = open(&path);
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 2);
    assert!(matches!(a.dispatch(&r), Err(Error::Stale)));
    assert!(matches!(a.cancel_reserved(r), Err(Error::Stale)));
    assert!(matches!(a.uncertain(sent), Err(Error::Stale)));
    assert!(matches!(
        a.observe(ticket, evidence(Readiness::Ready, 2)),
        Err(Error::Stale)
    ));
    ready(&a, Scope::Group(d(10)), 2);
    ready(&a, Scope::Route(d(20)), 2);
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    let recovered = a.lease(&original.id).unwrap().unwrap();
    assert_eq!(recovered.phase, Phase::Uncertain);
    assert!(matches!(
        a.complete(VerifiedCompletion {
            lease: original,
            evidence: d(999)
        }),
        Err(Error::Binding)
    ));
    assert!(finish(&a, recovered));
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
}

#[test]
fn shrinking_ceiling_preserves_inflight_work_and_occupied_alias_cannot_move() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 3, &[(20, Lane::Proxy)]);
    setup(&a, 11, 3, &[]);
    let leases = (0..3)
        .map(|n| {
            let r = a.reserve(&d(20), work(n)).unwrap();
            a.dispatch(&r).unwrap().lease().clone()
        })
        .collect::<Vec<_>>();
    a.configure_group(d(10), 1).unwrap();
    ready(&a, Scope::Group(d(10)), 1);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 3);
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    assert!(matches!(a.remove_route(&d(20)), Err(Error::InUse)));
    assert!(matches!(a.remove_group(&d(10)), Err(Error::InUse)));
    assert!(matches!(
        a.configure_route(route(20, 11, Lane::Proxy, 3)),
        Err(Error::InUse)
    ));
    assert!(matches!(
        a.configure_route(route(20, 10, Lane::Native, 3)),
        Err(Error::InUse)
    ));
    for l in leases {
        finish(&a, l);
    }
    a.configure_route(route(20, 11, Lane::Native, 3)).unwrap();
    a.remove_group(&d(10)).unwrap();
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    ready(&a, Scope::Route(d(20)), 3);
    assert_eq!(a.status(&d(20)).unwrap().available, 3);
}

#[test]
fn recovery_pages_are_group_scoped_bounded_and_indexed() {
    let dir = private_dir();
    let a = open(&dir.path().join("capacity"));
    setup(&a, 10, 200, &[(20, Lane::Proxy)]);
    setup(&a, 11, 200, &[(21, Lane::Proxy)]);
    for i in 0..145 {
        drop(
            a.reserve(&d(if i % 2 == 0 { 20 } else { 21 }), work(i))
                .unwrap(),
        );
    }
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let p = a.recover_group(&d(10), cursor.as_ref(), 16).unwrap();
        assert!(p.leases.len() <= 16);
        for l in &p.leases {
            assert_eq!(l.group, d(10));
            assert!(seen.insert(l.id.clone()));
        }
        cursor = p.next_after;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 73);
    assert!(matches!(
        a.recover_group(&d(10), None, 65),
        Err(Error::Invalid)
    ));
    for id in seen {
        finish(&a, a.lease(&id).unwrap().unwrap());
    }
    assert!(a.recover_group(&d(10), None, 64).unwrap().leases.is_empty());
    assert_eq!(a.status(&d(21)).unwrap().group_occupied, 72);
}

#[test]
fn storage_quotas_and_lowered_configuration_do_not_drop_accepted_work() {
    let dir = private_dir();
    let path = dir.path().join("capacity");
    let a = Authority::open(
        &path,
        identity(),
        Limits {
            max_groups: 1,
            max_routes: 2,
            max_leases: 2,
            ..limits()
        },
    )
    .unwrap();
    setup(&a, 10, 100, &[(20, Lane::Proxy), (21, Lane::Native)]);
    assert!(matches!(a.configure_group(d(11), 1), Err(Error::Quota)));
    assert!(matches!(
        a.configure_route(route(22, 10, Lane::Proxy, 1)),
        Err(Error::Quota)
    ));
    let r1 = a.reserve(&d(20), work(1)).unwrap();
    let l1 = r1.lease().clone();
    drop(r1);
    let r2 = a.reserve(&d(21), work(2)).unwrap();
    let l2 = r2.lease().clone();
    drop(r2);
    assert!(matches!(a.reserve(&d(20), work(3)), Err(Error::Quota)));
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    drop(a);
    let a = Authority::open(
        &path,
        identity(),
        Limits {
            max_groups: 1,
            max_routes: 1,
            max_leases: 1,
            ..limits()
        },
    )
    .unwrap();
    ready(&a, Scope::Group(d(10)), 100);
    ready(&a, Scope::Route(d(20)), 100);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 2);
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    finish(&a, a.lease(&l1.id).unwrap().unwrap());
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    finish(&a, a.lease(&l2.id).unwrap().unwrap());
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    a.remove_route(&d(21)).unwrap();
    a.remove_route(&d(20)).unwrap();
    a.remove_group(&d(10)).unwrap();
}

#[test]
fn completion_binding_and_identity_mismatch_cannot_free_someone_elses_slot() {
    let dir = private_dir();
    let path = dir.path().join("capacity");
    let a = open(&path);
    setup(&a, 10, 2, &[(20, Lane::Proxy)]);
    let r = a.reserve(&d(20), work(1)).unwrap();
    let actual = a.dispatch(&r).unwrap().lease().clone();
    for kind in 0..5 {
        let mut bad = actual.clone();
        match kind {
            0 => bad.route = d(22),
            1 => bad.work.invocation = d(222),
            2 => bad.work.request_hash = d(222),
            3 => bad.group = d(222),
            _ => bad.controller_fence += 1,
        };
        assert!(matches!(
            a.complete(VerifiedCompletion {
                lease: bad,
                evidence: d(999)
            }),
            Err(Error::Binding)
        ));
    }
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 1);
    drop(a);
    let mut wrong = identity();
    wrong.subnet_bootstrap = d(222);
    assert!(matches!(
        Authority::open(&path, wrong, limits()),
        Err(Error::Identity)
    ));
    assert_eq!(open(&path).status(&d(20)).unwrap().group_occupied, 1);
}

#[test]
fn real_process_crash_keeps_unknown_work_and_excludes_a_second_live_owner() {
    const ENV: &str = "MAYHEM_PROXY_CAPACITY_CRASH_FIXTURE";
    if let Ok(path) = std::env::var(ENV) {
        let path = std::path::PathBuf::from(path);
        let a = open(&path);
        setup(&a, 10, 2, &[(20, Lane::Proxy)]);
        drop(a.reserve(&d(20), work(1)).unwrap());
        let r = a.reserve(&d(20), work(2)).unwrap();
        drop(a.dispatch(&r).unwrap());
        std::fs::write(path.with_extension("ready"), b"committed").unwrap();
        loop {
            std::thread::park();
        }
    }
    let dir = private_dir();
    let path = dir.path().join("capacity");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "real_process_crash_keeps_unknown_work_and_excludes_a_second_live_owner",
            "--nocapture",
        ])
        .env(ENV, &path)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.with_extension("ready").exists() {
        if child.try_wait().unwrap().is_some() {
            panic!("crash fixture exited before commit");
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("crash fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        Authority::open(&path, identity(), limits()),
        Err(Error::Storage)
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    let a = open(&path);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 2);
    let p = a.recover_group(&d(10), None, 64).unwrap();
    assert_eq!(p.leases.len(), 2);
    assert!(p.leases.iter().all(|l| l.phase == Phase::Uncertain));
    ready(&a, Scope::Group(d(10)), 2);
    ready(&a, Scope::Route(d(20)), 2);
    assert!(matches!(a.reserve(&d(20), work(3)), Err(Error::Busy)));
    for l in p.leases {
        finish(&a, l);
    }
    assert_eq!(a.status(&d(20)).unwrap().available, 2);
}

#[test]
fn unsafe_storage_permissions_and_symlinks_are_refused_without_reset() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = private_dir();
    let path = dir.path().join("capacity");
    drop(open(&path));
    symlink(&path, dir.path().join("link")).unwrap();
    assert!(matches!(
        Authority::open(dir.path().join("link"), identity(), limits()),
        Err(Error::File)
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Authority::open(&path, identity(), limits()),
        Err(Error::File)
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    drop(open(&path));
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        Authority::open(&path, identity(), limits()),
        Err(Error::File)
    ));
}
