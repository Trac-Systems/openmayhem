use super::*;

fn route(id: u64, group: u64, lane: Lane, max: u32) -> Route {
    Route {
        id: d(id),
        group: d(group),
        lane,
        max_concurrency: max,
    }
}
fn native_ready(a: &Authority) {
    let t = a.begin_observation(Scope::Route(d(30))).unwrap();
    a.observe(
        t,
        Evidence {
            state: Readiness::Ready,
            allowance: 1,
            age: Duration::ZERO,
            valid_for: Duration::from_secs(60),
        },
    )
    .unwrap();
}
fn layers(a: &Authority) -> (Monitor, Monitor) {
    a.configure_allocation_group(d(10), 4).unwrap();
    a.configure_group(d(11), 2).unwrap();
    a.configure_group(d(12), 2).unwrap();
    let first = monitor();
    let other = monitor();
    other.register(d(22), 4, true).unwrap();
    for id in [20, 21] {
        a.configure_route_with_constraints(route(id, 10, Lane::Proxy, 4), vec![d(11)])
            .unwrap();
    }
    a.configure_route_with_constraints(route(22, 10, Lane::Proxy, 4), vec![d(12)])
        .unwrap();
    a.configure_route(route(30, 10, Lane::Native, 1)).unwrap();
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    assert_eq!(a.status(&d(30)).unwrap().state, Readiness::Checking);
    assert!(a.begin_observation(Scope::Group(d(10))).is_err());
    assert!(a
        .bind_live(Scope::Group(d(10)), first.connection_source())
        .is_err());
    a.bind_live(Scope::Group(d(11)), first.connection_source())
        .unwrap();
    a.bind_live(Scope::Group(d(12)), other.connection_source())
        .unwrap();
    for id in [20, 21] {
        a.bind_live(Scope::Route(d(id)), first.route_source(&d(id)).unwrap())
            .unwrap();
    }
    a.bind_live(Scope::Route(d(22)), other.route_source(&d(22)).unwrap())
        .unwrap();
    good(&first);
    good(&other);
    for _ in 0..4 {
        other
            .observe_request(&d(22), class())
            .unwrap()
            .success(None);
    }
    native_ready(a);
    (first, other)
}

#[test]
fn credential_failure_preserves_other_key_and_native_work_on_the_same_physical_backend() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let (first, _other) = layers(&a);
    let first_job = a.reserve(&d(20), work(1)).unwrap();
    a.dispatch(&first_job).unwrap();
    let other_job = a.reserve(&d(22), work(2)).unwrap();
    let native_job = a.reserve(&d(30), work(3)).unwrap();
    a.dispatch(&native_job).unwrap();
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 3);
    assert_eq!(a.group_status(&d(11)).unwrap().occupied, 1);
    assert_eq!(a.group_status(&d(12)).unwrap().occupied, 1);
    fault(
        &first,
        Code::UpstreamAuthentication,
        FailureScope::Connection,
    );
    for id in [20, 21] {
        assert_eq!(a.status(&d(id)).unwrap().state, Readiness::Unavailable);
    }
    assert_eq!(a.status(&d(22)).unwrap().available, 1);
    assert_eq!(a.status(&d(30)).unwrap().state, Readiness::Busy);
    let another = a.reserve(&d(22), work(4)).unwrap();
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 4);
    assert_eq!(a.group_status(&d(12)).unwrap().occupied, 2);
    assert!(matches!(
        a.reserve(&d(22), work(5)),
        Err(capacity::Error::Busy)
    ));
    assert_eq!(
        a.lease(&first_job.lease().id).unwrap().unwrap().phase,
        capacity::Phase::Dispatched
    );
    assert_eq!(
        a.lease(&native_job.lease().id).unwrap().unwrap().phase,
        capacity::Phase::Dispatched
    );
    a.cancel_reserved(another).unwrap();
    a.cancel_reserved(other_job).unwrap();
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 2);
    assert_eq!(a.group_status(&d(11)).unwrap().occupied, 1);
    assert_eq!(a.group_status(&d(12)).unwrap().occupied, 0);
}

#[test]
fn simultaneous_native_proxy_and_aliases_obey_all_overlapping_limits_atomically() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    layers(&a);
    let barrier = Arc::new(Barrier::new(32));
    let jobs = (0..32)
        .map(|n| {
            let a = a.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                b.wait();
                a.reserve(&d([20, 21, 22, 30][n as usize % 4]), work(n))
            })
        })
        .collect::<Vec<_>>();
    let accepted = jobs
        .into_iter()
        .filter_map(|job| match job.join().unwrap() {
            Ok(v) => Some(v),
            Err(capacity::Error::Busy) => None,
            Err(e) => panic!("{e}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(accepted.len(), 4);
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 4);
    assert!(a.group_status(&d(11)).unwrap().occupied <= 2);
    assert!(a.group_status(&d(12)).unwrap().occupied <= 2);
    assert!(a.status(&d(30)).unwrap().route_occupied <= 1);
    for job in accepted {
        a.cancel_reserved(job).unwrap();
    }
    for group in [10, 11, 12] {
        assert_eq!(a.group_status(&d(group)).unwrap().occupied, 0);
        assert!(a
            .recover_group(&d(group), None, 64)
            .unwrap()
            .leases
            .is_empty());
    }
}

#[test]
fn constraints_cannot_disappear_from_busy_routes_or_legacy_ceiling_updates() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let (first, _other) = layers(&a);
    for invalid in [
        vec![d(11), d(11)],
        vec![d(10)],
        (1000..1017).map(d).collect(),
    ] {
        assert!(matches!(
            a.configure_route_with_constraints(route(20, 10, Lane::Proxy, 4), invalid),
            Err(capacity::Error::Invalid)
        ));
    }
    assert!(matches!(
        a.configure_route_with_constraints(route(20, 10, Lane::Proxy, 4), vec![d(99)]),
        Err(capacity::Error::NotFound)
    ));
    assert_eq!(a.group_status(&d(11)).unwrap().routes, 2);
    let held = a.reserve(&d(20), work(1)).unwrap();
    assert!(matches!(
        a.configure_route_with_constraints(route(20, 10, Lane::Proxy, 4), vec![d(12)]),
        Err(capacity::Error::InUse)
    ));
    assert!(matches!(
        a.remove_group(&d(11)),
        Err(capacity::Error::InUse)
    ));
    assert!(matches!(
        a.configure_allocation_group(d(11), 2),
        Err(capacity::Error::InUse)
    ));
    a.configure_route(route(20, 10, Lane::Proxy, 3)).unwrap();
    a.bind_live(Scope::Route(d(20)), first.route_source(&d(20)).unwrap())
        .unwrap();
    assert_eq!(a.group_status(&d(11)).unwrap().occupied, 1);
    let second = a.reserve(&d(21), work(2)).unwrap();
    assert!(matches!(
        a.reserve(&d(20), work(3)),
        Err(capacity::Error::Busy)
    ));
    a.cancel_reserved(second).unwrap();
    a.cancel_reserved(held).unwrap();
    a.remove_route(&d(21)).unwrap();
    a.remove_route(&d(20)).unwrap();
    assert_eq!(a.group_status(&d(11)).unwrap().routes, 0);
    a.remove_group(&d(11)).unwrap();
}

#[test]
fn indexed_recovery_merges_primary_and_additional_memberships_and_survives_restart() {
    let dir = directory();
    let path = dir.path().join("capacity");
    let a = open(&path);
    let (first, _) = layers(&a);
    first.register(d(40), 4, true).unwrap();
    for _ in 0..4 {
        first
            .observe_request(&d(40), class())
            .unwrap()
            .success(None);
    }
    a.configure_route_with_constraints(route(40, 11, Lane::Proxy, 4), vec![d(10)])
        .unwrap();
    a.bind_live(Scope::Route(d(40)), first.route_source(&d(40)).unwrap())
        .unwrap();
    let one = a.reserve(&d(20), work(1)).unwrap();
    a.dispatch(&one).unwrap();
    let two = a.reserve(&d(40), work(2)).unwrap();
    a.dispatch(&two).unwrap();
    for group in [10, 11] {
        let p1 = a.recover_group(&d(group), None, 1).unwrap();
        let p2 = a
            .recover_group(&d(group), p1.next_after.as_ref(), 1)
            .unwrap();
        assert_eq!(p1.leases.len(), 1);
        assert_eq!(p2.leases.len(), 1);
        assert_ne!(p1.leases[0].id, p2.leases[0].id);
        assert!(p2.next_after.is_none());
    }
    drop(a);
    let a = open(&path);
    for group in [10, 11] {
        assert_eq!(a.group_status(&d(group)).unwrap().occupied, 2);
        let p = a.recover_group(&d(group), None, 64).unwrap();
        assert_eq!(p.leases.len(), 2);
        assert!(p
            .leases
            .iter()
            .all(|l| l.phase == capacity::Phase::Uncertain));
    }
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Checking);
    for lease in [one.lease(), two.lease()] {
        let effective = a.lease(&lease.id).unwrap().unwrap();
        assert!(a
            .complete(capacity::VerifiedCompletion {
                lease: effective,
                evidence: d(999)
            })
            .unwrap());
    }
    for group in [10, 11] {
        assert_eq!(a.group_status(&d(group)).unwrap().occupied, 0);
        assert!(a
            .recover_group(&d(group), None, 64)
            .unwrap()
            .leases
            .is_empty());
    }
}

#[test]
fn a_smaller_physical_ceiling_never_cancels_or_unaccounts_existing_work() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    layers(&a);
    let one = a.reserve(&d(20), work(1)).unwrap();
    let two = a.reserve(&d(22), work(2)).unwrap();
    a.configure_allocation_group(d(10), 1).unwrap();
    assert!(matches!(a.dispatch(&one), Err(capacity::Error::Busy)));
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 2);
    assert_eq!(a.group_status(&d(11)).unwrap().occupied, 1);
    assert_eq!(a.group_status(&d(12)).unwrap().occupied, 1);
    a.cancel_reserved(two).unwrap();
    a.dispatch(&one).unwrap();
    assert_eq!(a.status(&d(22)).unwrap().available, 0);
    let lease = a.lease(&one.lease().id).unwrap().unwrap();
    a.complete(capacity::VerifiedCompletion {
        lease,
        evidence: d(999),
    })
    .unwrap();
    assert_eq!(a.status(&d(22)).unwrap().available, 1);
}

#[test]
fn corrupted_constraint_reference_cannot_release_work_or_advance_a_recovery_cursor() {
    use redb::TableDefinition;
    let dir = directory();
    let path = dir.path().join("capacity");
    let a = open(&path);
    layers(&a);
    let one = a.reserve(&d(20), work(1)).unwrap();
    let two = a.reserve(&d(21), work(2)).unwrap();
    a.dispatch(&one).unwrap();
    a.dispatch(&two).unwrap();
    let id = one.lease().id.clone();
    drop(a);
    let db = redb::Database::open(&path).unwrap();
    let tx = db.begin_write().unwrap();
    {
        let mut index = tx
            .open_table(TableDefinition::<&str, &str>::new(
                "capacity_constraint_leases_v1",
            ))
            .unwrap();
        index
            .insert(
                format!("{}/{}", d(11).as_str(), id.as_str()).as_str(),
                two.lease().id.as_str(),
            )
            .unwrap();
    }
    tx.commit().unwrap();
    drop(db);
    let a = open(&path);
    assert!(matches!(
        a.recover_group(&d(11), None, 64),
        Err(capacity::Error::Invalid)
    ));
    let retained = a.lease(&id).unwrap().unwrap();
    assert!(matches!(
        a.complete(capacity::VerifiedCompletion {
            lease: retained.clone(),
            evidence: d(1000),
        }),
        Err(capacity::Error::Invalid)
    ));
    assert_eq!(a.lease(&id).unwrap().unwrap(), retained);
    for group in [10, 11] {
        assert_eq!(a.group_status(&d(group)).unwrap().occupied, 2);
    }
    assert_eq!(a.status(&d(20)).unwrap().route_occupied, 1);
    assert_eq!(a.status(&d(21)).unwrap().route_occupied, 1);
}
