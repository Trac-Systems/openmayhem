#![cfg(unix)]
use mayhem_proxy::{
    attempts::{Digest, Identity},
    capacity::{self, Authority, Evidence, Lane, Readiness, ReadinessSource, Route, Scope, Work},
    connector::failure::{Code, Execution, Failure, Scope as FailureScope, Stage},
    health::{Class, Monitor, Policy, Thinking},
    supervisor::RefreshPolicy,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
    time::Duration,
};

fn d(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
fn open(path: &std::path::Path) -> Arc<Authority> {
    Arc::new(
        Authority::open(
            path,
            Identity {
                network_id: "918".into(),
                msb_bootstrap: d(1),
                subnet_bootstrap: d(2),
                controller_pubkey: d(3),
            },
            capacity::Limits {
                max_groups: 4,
                max_routes: 8,
                max_leases: 64,
                max_evidence_age: Duration::from_secs(60),
            },
        )
        .unwrap(),
    )
}
fn monitor() -> Monitor {
    let m = Monitor::new(
        Policy {
            max_routes: 8,
            max_classes_per_route: 8,
            evidence_ttl_ms: 60_000,
            successes_to_increase: 1,
            bad_samples_to_reduce: 2,
            latency_baseline_samples: 3,
            latency_multiplier: 4,
            latency_increase_ms: 1000,
            min_native_tok_s: 5,
            recovery: RefreshPolicy {
                interval_ms: 10_000,
                page_pause_ms: 10,
                retry_initial_ms: 1000,
                retry_max_ms: 30_000,
                jitter_percent: 0,
            },
        },
        4,
        1,
    )
    .unwrap();
    for n in [20, 21] {
        m.register(d(n), 4, true).unwrap();
    }
    m
}
fn configure(a: &Authority) {
    a.configure_group(d(10), 4).unwrap();
    for n in [20, 21] {
        a.configure_route(Route {
            id: d(n),
            group: d(10),
            lane: Lane::Proxy,
            max_concurrency: 4,
        })
        .unwrap();
    }
}
fn bind(a: &Authority, m: &Monitor) {
    a.bind_live(Scope::Group(d(10)), m.connection_source())
        .unwrap();
    for n in [20, 21] {
        a.bind_live(Scope::Route(d(n)), m.route_source(&d(n)).unwrap())
            .unwrap();
    }
}
fn class() -> Class {
    Class::new(1024, Thinking::Disabled, false)
}
fn good(m: &Monitor) {
    for _ in 0..8 {
        for n in [20, 21] {
            m.observe_request(&d(n), class()).unwrap().success(None);
        }
    }
}
fn fault(m: &Monitor, code: Code, scope: FailureScope) {
    m.observe_request(&d(20), class())
        .unwrap()
        .failure(Failure::new(
            code,
            scope,
            Stage::ResponseHeaders,
            Execution::Unknown,
        ));
}
fn work(n: u64) -> Work {
    Work {
        invocation: d(100 + n),
        request_hash: d(999),
    }
}

#[tokio::test(start_paused = true)]
async fn live_admission_expires_without_poll_writes_and_failure_blocks_dispatch_without_releasing_work(
) {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let m = monitor();
    configure(&a);
    bind(&a, &m);
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    good(&m);
    let reserved = a.reserve(&d(20), work(1)).unwrap();
    assert_eq!(a.status(&d(21)).unwrap().available, 3);
    fault(&m, Code::UpstreamBusy, FailureScope::Connection);
    assert_eq!(a.status(&d(21)).unwrap().state, Readiness::Busy);
    assert!(matches!(a.dispatch(&reserved), Err(capacity::Error::Busy)));
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 1);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    assert!(a.reserve(&d(21), work(2)).is_err());
    assert!(a.lease(&reserved.lease().id).unwrap().is_some());
    m.observe_recovery(&d(20), class()).unwrap().success(None);
    // One fresh slot is consumed by the existing reservation, not counted twice.
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 1);
    a.cancel_reserved(reserved).unwrap();
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    for _ in 0..100 {
        assert_eq!(a.status(&d(20)).unwrap().available, 1);
    }
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
}

#[test]
fn aliases_share_one_observed_concurrency_under_atomic_admission() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let m = monitor();
    configure(&a);
    bind(&a, &m);
    good(&m);
    let barrier = Arc::new(Barrier::new(20));
    let calls = (0..20)
        .map(|n| {
            let a = a.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                b.wait();
                a.reserve(&d(20 + n % 2), work(n))
            })
        })
        .collect::<Vec<_>>();
    let admitted = calls
        .into_iter()
        .filter_map(|t| match t.join().unwrap() {
            Ok(r) => Some(r),
            Err(capacity::Error::Busy) => None,
            Err(e) => panic!("{e}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(admitted.len(), 4);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 4);
    assert_eq!(a.status(&d(21)).unwrap().available, 0);
    fault(&m, Code::UpstreamModelUnavailable, FailureScope::Model);
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Unavailable);
    for r in admitted {
        a.cancel_reserved(r).unwrap();
    }
    assert_eq!(a.status(&d(21)).unwrap().available, 4);
}

#[test]
fn restart_keeps_live_requirement_and_unknown_leases_until_rebound_and_reconciled() {
    let dir = directory();
    let path = dir.path().join("capacity");
    let m = monitor();
    let a = open(&path);
    configure(&a);
    bind(&a, &m);
    good(&m);
    let reserved = a.reserve(&d(20), work(1)).unwrap();
    a.dispatch(&reserved).unwrap();
    let id = reserved.lease().id.clone();
    drop(a);
    let a = open(&path);
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    assert_eq!(
        a.lease(&id).unwrap().unwrap().phase,
        capacity::Phase::Uncertain
    );
    assert!(matches!(
        a.begin_observation(Scope::Route(d(20))),
        Err(capacity::Error::InUse)
    ));
    let fresh = monitor();
    bind(&a, &fresh);
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    good(&fresh);
    assert_eq!(a.status(&d(20)).unwrap().available, 3);
    assert_eq!(
        a.lease(&id).unwrap().unwrap().phase,
        capacity::Phase::Uncertain
    );
}

#[test]
fn stale_ticket_and_reconfiguration_cannot_replace_live_monitor_with_cached_ready() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let m = monitor();
    configure(&a);
    let old = a.begin_observation(Scope::Route(d(20))).unwrap();
    bind(&a, &m);
    good(&m);
    assert!(matches!(
        a.observe(
            old,
            Evidence {
                state: Readiness::Ready,
                allowance: 4,
                age: Duration::ZERO,
                valid_for: Duration::from_secs(60)
            }
        ),
        Err(capacity::Error::Stale)
    ));
    let reservation = a.reserve(&d(20), work(1)).unwrap();
    a.configure_route(Route {
        id: d(20),
        group: d(10),
        lane: Lane::Proxy,
        max_concurrency: 1,
    })
    .unwrap();
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    assert!(a.lease(&reservation.lease().id).unwrap().is_some());
    assert!(matches!(
        a.begin_observation(Scope::Route(d(20))),
        Err(capacity::Error::InUse)
    ));
    a.bind_live(Scope::Route(d(20)), m.route_source(&d(20)).unwrap())
        .unwrap();
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    a.cancel_reserved(reservation).unwrap();
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    a.remove_route(&d(20)).unwrap();
    a.configure_route(Route {
        id: d(20),
        group: d(10),
        lane: Lane::Proxy,
        max_concurrency: 4,
    })
    .unwrap();
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
}

struct Changing {
    revision: AtomicU64,
}
impl ReadinessSource for Changing {
    fn revision(&self) -> capacity::Result<u64> {
        Ok(self.revision.load(Ordering::SeqCst))
    }
    fn evidence(&self) -> capacity::Result<Evidence> {
        self.revision.fetch_add(1, Ordering::SeqCst);
        Ok(Evidence {
            state: Readiness::Ready,
            allowance: 4,
            age: Duration::ZERO,
            valid_for: Duration::from_secs(60),
        })
    }
}
#[test]
fn inconsistent_observation_pair_fails_closed_without_spinning_or_creating_a_lease() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let m = monitor();
    configure(&a);
    good(&m);
    a.bind_live(Scope::Group(d(10)), m.connection_source())
        .unwrap();
    let changed = Arc::new(Changing {
        revision: AtomicU64::new(0),
    });
    a.bind_live(Scope::Route(d(20)), changed.clone()).unwrap();
    assert!(matches!(
        a.reserve(&d(20), work(1)),
        Err(capacity::Error::Checking)
    ));
    assert_eq!(changed.revision.load(Ordering::SeqCst), 1);
    assert!(a.work_lease(&work(1)).unwrap().is_none());
}

#[test]
fn unrelated_credentials_and_replicas_are_not_withdrawn_by_an_authentication_fault() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let first = monitor();
    let other = monitor();
    configure(&a);
    bind(&a, &first);
    good(&first);
    good(&other);
    other.register(d(22), 4, true).unwrap();
    for _ in 0..4 {
        other
            .observe_request(&d(22), class())
            .unwrap()
            .success(None);
    }
    a.configure_group(d(11), 4).unwrap();
    a.configure_route(Route {
        id: d(22),
        group: d(11),
        lane: Lane::Proxy,
        max_concurrency: 4,
    })
    .unwrap();
    a.bind_live(Scope::Group(d(11)), other.connection_source())
        .unwrap();
    a.bind_live(Scope::Route(d(22)), other.route_source(&d(22)).unwrap())
        .unwrap();
    fault(
        &first,
        Code::UpstreamAuthentication,
        FailureScope::Connection,
    );
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Unavailable);
    assert_eq!(a.status(&d(21)).unwrap().state, Readiness::Unavailable);
    assert_eq!(a.status(&d(22)).unwrap().available, 4);
}

#[test]
fn legacy_capacity_schema_two_without_live_fields_preserves_inflight_work_during_upgrade() {
    use redb::{ReadableTable, TableDefinition};
    let dir = directory();
    let path = dir.path().join("capacity");
    let a = open(&path);
    configure(&a);
    for scope in [
        Scope::Group(d(10)),
        Scope::Route(d(20)),
        Scope::Route(d(21)),
    ] {
        let ticket = a.begin_observation(scope).unwrap();
        a.observe(
            ticket,
            Evidence {
                state: Readiness::Ready,
                allowance: 4,
                age: Duration::ZERO,
                valid_for: Duration::from_secs(60),
            },
        )
        .unwrap();
    }
    let reserved = a.reserve(&d(20), work(1)).unwrap();
    a.dispatch(&reserved).unwrap();
    let id = reserved.lease().id.clone();
    drop(a);
    let db = redb::Database::open(&path).unwrap();
    let tx = db.begin_write().unwrap();
    {
        let mut meta = tx
            .open_table(TableDefinition::<&str, &[u8]>::new("capacity_meta_v1"))
            .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(meta.get("state").unwrap().unwrap().value()).unwrap();
        value["schema"] = 2.into();
        meta.insert("state", serde_json::to_vec(&value).unwrap().as_slice())
            .unwrap();
    }
    for table in ["capacity_groups_v1", "capacity_routes_v1"] {
        let mut rows = tx
            .open_table(TableDefinition::<&str, &[u8]>::new(table))
            .unwrap();
        let old = rows
            .iter()
            .unwrap()
            .map(|row| {
                let (k, v) = row.unwrap();
                (
                    k.value().to_owned(),
                    serde_json::from_slice::<serde_json::Value>(v.value()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        for (id, mut value) in old {
            value["gate"].as_object_mut().unwrap().remove("live");
            rows.insert(id.as_str(), serde_json::to_vec(&value).unwrap().as_slice())
                .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(db);
    let a = open(&path);
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    assert_eq!(a.status(&d(20)).unwrap().group_occupied, 1);
    assert_eq!(
        a.lease(&id).unwrap().unwrap().phase,
        capacity::Phase::Uncertain
    );
    let m = monitor();
    bind(&a, &m);
    good(&m);
    assert_eq!(a.status(&d(20)).unwrap().available, 3);
    assert!(matches!(a.dispatch(&reserved), Err(capacity::Error::Stale)));
}
