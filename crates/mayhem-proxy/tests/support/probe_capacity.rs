use super::*;
use capacity::probes::{Budget, ProbePhase, Specification, VerifiedCompletion};

fn spec() -> Specification {
    Specification {
        route: d(20),
        budget_group: d(11),
        request_hash: d(100),
        connection_digest: d(101),
        connection_revision: 1,
        recipe_digest: d(102),
    }
}
fn budget() -> Budget {
    Budget {
        max_attempts: 2,
        max_cost_microusd: 1000,
        per_attempt_cost_microusd: 400,
    }
}
fn counts(a: &Authority, expected: u32) {
    for group in [10, 11] {
        assert_eq!(a.group_status(&d(group)).unwrap().occupied, expected);
    }
    assert_eq!(a.status(&d(20)).unwrap().route_occupied, expected);
}

#[test]
fn recovery_does_not_exceed_a_healthy_shared_allowance_or_bypass_an_unrelated_failed_group() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    a.configure_allocation_group(d(10), 4).unwrap();
    a.configure_group(d(11), 4).unwrap();
    a.configure_group(d(12), 4).unwrap();
    let m = monitor();
    for id in [20, 21] {
        a.configure_route_with_constraints(
            Route {
                id: d(id),
                group: d(10),
                lane: Lane::Proxy,
                max_concurrency: 4,
            },
            vec![d(11)],
        )
        .unwrap();
        a.bind_live(Scope::Route(d(id)), m.route_source(&d(id)).unwrap())
            .unwrap();
    }
    a.bind_live(Scope::Group(d(11)), m.connection_source())
        .unwrap();
    a.configure_probe_budget(&d(11), budget()).unwrap();
    m.observe_request(&d(20), class()).unwrap().success(None);
    let existing = a.reserve(&d(20), work(1)).unwrap();
    let mut probing = spec();
    probing.route = d(21);
    assert!(matches!(
        a.reserve_probe(probing.clone()),
        Err(capacity::Error::Busy)
    ));
    assert_eq!(a.probe_budget(&d(11)).unwrap().unwrap().used_attempts, 0);
    a.cancel_reserved(existing).unwrap();
    let probe = a.reserve_probe(probing.clone()).unwrap();
    a.cancel_prepared_probe(&probe.probe().id).unwrap();
    a.configure_route_with_constraints(
        Route {
            id: d(21),
            group: d(10),
            lane: Lane::Proxy,
            max_concurrency: 4,
        },
        vec![d(11), d(12)],
    )
    .unwrap();
    a.bind_live(Scope::Route(d(21)), m.route_source(&d(21)).unwrap())
        .unwrap();
    assert!(matches!(
        a.reserve_probe(probing),
        Err(capacity::Error::Checking)
    ));
    assert_eq!(a.probe_budget(&d(11)).unwrap().unwrap().used_attempts, 1);
}

#[test]
fn probes_require_explicit_budget_and_never_claim_readiness_or_paid_authority() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    let (first, _) = layered::layers(&a);
    fault(&first, Code::UpstreamBusy, FailureScope::Connection);
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::ProbeBudget)
    ));
    counts(&a, 0);
    a.configure_probe_budget(&d(11), budget()).unwrap();
    let lease = a.reserve_probe(spec()).unwrap();
    counts(&a, 1);
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Busy);
    assert!(matches!(
        a.reserve(&d(20), work(1)),
        Err(capacity::Error::Busy)
    ));
    assert!(a.lease(&lease.probe().id).unwrap().is_none());
    assert!(a.recover_group(&d(10), None, 64).unwrap().leases.is_empty());
    let dispatch = a.dispatch_probe(lease).unwrap();
    assert!(matches!(
        a.cancel_prepared_probe(&dispatch.probe().id),
        Err(capacity::Error::Invalid)
    ));
    assert!(a
        .complete_probe(VerifiedCompletion {
            probe: dispatch.probe().clone(),
            evidence: d(103)
        })
        .unwrap());
    counts(&a, 0);
    assert_eq!(a.status(&d(20)).unwrap().state, Readiness::Busy);
    assert!(!a
        .complete_probe(VerifiedCompletion {
            probe: dispatch.probe().clone(),
            evidence: d(103)
        })
        .unwrap());
    let saved = a.probe_budget(&d(11)).unwrap().unwrap();
    assert_eq!(saved.used_attempts, 1);
    assert_eq!(saved.allocated_cost_microusd, 400);
    assert_eq!(saved.last_completed.unwrap().probe, dispatch.probe().id);
}

#[test]
fn aliases_and_independent_keys_cannot_multiply_recovery_on_the_same_physical_pool() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    layered::layers(&a);
    a.configure_probe_budget(&d(11), budget()).unwrap();
    a.configure_probe_budget(&d(12), budget()).unwrap();
    let barrier = Arc::new(Barrier::new(32));
    let handles = (0..32)
        .map(|n| {
            let a = a.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut s = spec();
                s.route = d([20, 21, 22][n % 3]);
                if s.route == d(22) {
                    s.budget_group = d(12)
                }
                barrier.wait();
                a.reserve_probe(s)
            })
        })
        .collect::<Vec<_>>();
    let accepted = handles
        .into_iter()
        .filter_map(|h| match h.join().unwrap() {
            Ok(p) => Some(p),
            Err(capacity::Error::InUse) => None,
            Err(e) => panic!("{e}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(accepted.len(), 1);
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 1);
    let used: u64 = [11, 12]
        .iter()
        .map(|n| a.probe_budget(&d(*n)).unwrap().unwrap().used_attempts)
        .sum();
    assert_eq!(used, 1);
    let id = accepted[0].probe().id.clone();
    drop(accepted);
    assert_eq!(a.probe_for_group(&d(10)).unwrap().unwrap().id, id);
    assert!(a.cancel_prepared_probe(&id).unwrap());
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 0);
    // A separate replica has an independent recovery allocation.
    a.configure_allocation_group(d(13), 1).unwrap();
    a.configure_route(Route {
        id: d(40),
        group: d(13),
        lane: Lane::Proxy,
        max_concurrency: 1,
    })
    .unwrap();
    a.configure_probe_budget(&d(13), budget()).unwrap();
    let one = a.reserve_probe(spec()).unwrap();
    let mut separate = spec();
    separate.route = d(40);
    separate.budget_group = d(13);
    let other = a.reserve_probe(separate).unwrap();
    assert_ne!(one.probe().id, other.probe().id);
}

#[test]
fn uncertain_probe_survives_restart_and_ordinary_health_cannot_free_it() {
    let dir = directory();
    let path = dir.path().join("capacity");
    let a = open(&path);
    layered::layers(&a);
    a.configure_probe_budget(&d(11), budget()).unwrap();
    let dispatch = a.dispatch_probe(a.reserve_probe(spec()).unwrap()).unwrap();
    let before = dispatch.probe().clone();
    drop(a);
    let a = open(&path);
    let (_, _) = layered::layers(&a);
    let after = a.probe_for_group(&d(11)).unwrap().unwrap();
    assert_eq!(after.phase, ProbePhase::Uncertain);
    counts(&a, 1);
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::InUse)
    ));
    assert!(matches!(
        a.cancel_prepared_probe(&after.id),
        Err(capacity::Error::Invalid)
    ));
    assert!(matches!(
        a.complete_probe(VerifiedCompletion {
            probe: before,
            evidence: d(103)
        }),
        Err(capacity::Error::Invalid)
    ));
    assert_eq!(
        a.probe_budget(&d(11))
            .unwrap()
            .unwrap()
            .allocated_cost_microusd,
        400
    );
    a.complete_probe(VerifiedCompletion {
        probe: after,
        evidence: d(103),
    })
    .unwrap();
    counts(&a, 0);
}

#[test]
fn prepared_restart_can_cancel_without_resending_or_refunding_the_operator_allowance() {
    let dir = directory();
    let path = dir.path().join("capacity");
    let a = open(&path);
    layered::layers(&a);
    a.configure_probe_budget(&d(11), budget()).unwrap();
    let lease = a.reserve_probe(spec()).unwrap();
    let id = lease.probe().id.clone();
    drop(a);
    let a = open(&path);
    assert_eq!(
        a.probe_for_group(&d(11)).unwrap().unwrap().phase,
        ProbePhase::Prepared
    );
    assert!(matches!(
        a.dispatch_probe(lease),
        Err(capacity::Error::Stale)
    ));
    assert!(a.cancel_prepared_probe(&id).unwrap());
    assert!(!a.cancel_prepared_probe(&id).unwrap());
    counts(&a, 0);
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::Checking)
    ));
    layered::layers(&a); // Trusted startup must reattach the original live scopes.
    a.configure_probe_budget(&d(11), budget()).unwrap();
    let second = a.reserve_probe(spec()).unwrap();
    a.cancel_prepared_probe(&second.probe().id).unwrap();
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::ProbeBudget)
    ));
    a.configure_probe_budget(&d(11), budget()).unwrap();
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::ProbeBudget)
    ));
    let mut more = budget();
    more.max_attempts = 3;
    more.max_cost_microusd = 1200;
    a.configure_probe_budget(&d(11), more).unwrap();
    let third = a.reserve_probe(spec()).unwrap();
    a.cancel_prepared_probe(&third.probe().id).unwrap();
    assert_eq!(
        a.probe_budget(&d(11))
            .unwrap()
            .unwrap()
            .allocated_cost_microusd,
        1200
    );
}

#[test]
fn cost_exhaustion_disable_and_zero_cost_are_distinct_from_customer_balances() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    layered::layers(&a);
    let mut policy = budget();
    policy.max_attempts = 5;
    policy.max_cost_microusd = 399;
    a.configure_probe_budget(&d(11), policy.clone()).unwrap();
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::ProbeBudget)
    ));
    assert_eq!(a.probe_budget(&d(11)).unwrap().unwrap().used_attempts, 0);
    policy.max_cost_microusd = 400;
    a.configure_probe_budget(&d(11), policy.clone()).unwrap();
    let one = a.reserve_probe(spec()).unwrap();
    let id = one.probe().id.clone();
    policy.max_attempts = 0;
    a.configure_probe_budget(&d(11), policy.clone()).unwrap();
    assert!(matches!(
        a.dispatch_probe(one),
        Err(capacity::Error::ProbeBudget)
    ));
    counts(&a, 1);
    a.cancel_prepared_probe(&id).unwrap();
    policy.max_attempts = 2;
    policy.per_attempt_cost_microusd = 0;
    a.configure_probe_budget(&d(11), policy).unwrap();
    let free = a.reserve_probe(spec()).unwrap();
    assert_eq!(free.probe().allocated_cost_microusd, 0);
    a.cancel_prepared_probe(&free.probe().id).unwrap();
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::ProbeBudget)
    ));
}

#[test]
fn full_shared_capacity_and_configuration_changes_cannot_be_bypassed_by_probes() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    layered::layers(&a);
    a.configure_probe_budget(&d(11), budget()).unwrap();
    let one = a.reserve(&d(20), work(1)).unwrap();
    let two = a.reserve(&d(21), work(2)).unwrap();
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::Busy)
    ));
    assert_eq!(a.probe_budget(&d(11)).unwrap().unwrap().used_attempts, 0);
    a.cancel_reserved(two).unwrap();
    let probe = a.reserve_probe(spec()).unwrap();
    assert!(matches!(
        a.reserve(&d(21), work(3)),
        Err(capacity::Error::Busy)
    ));
    assert!(matches!(
        a.remove_route(&d(20)),
        Err(capacity::Error::InUse)
    ));
    assert!(matches!(
        a.configure_route_with_constraints(
            Route {
                id: d(20),
                group: d(10),
                lane: Lane::Proxy,
                max_concurrency: 4
            },
            vec![d(12)]
        ),
        Err(capacity::Error::InUse)
    ));
    a.configure_allocation_group(d(10), 1).unwrap();
    let id = probe.probe().id.clone();
    assert!(matches!(
        a.dispatch_probe(probe),
        Err(capacity::Error::Busy)
    ));
    assert_eq!(a.group_status(&d(10)).unwrap().occupied, 2);
    a.cancel_prepared_probe(&id).unwrap();
    a.cancel_reserved(one).unwrap();
    counts(&a, 0);
}

#[test]
fn budgets_cannot_be_reset_by_removing_and_recreating_their_group() {
    let dir = directory();
    let a = open(&dir.path().join("capacity"));
    a.configure_allocation_group(d(10), 1).unwrap();
    a.configure_probe_budget(&d(10), budget()).unwrap();
    assert!(matches!(
        a.remove_group(&d(10)),
        Err(capacity::Error::InUse)
    ));
    let mut malformed = spec();
    malformed.budget_group = d(99);
    layered::layers(&a);
    assert!(matches!(
        a.reserve_probe(malformed),
        Err(capacity::Error::Invalid)
    ));
    let mut native = spec();
    native.route = d(30);
    native.budget_group = d(10);
    assert!(matches!(
        a.reserve_probe(native),
        Err(capacity::Error::Invalid)
    ));
}

#[test]
fn legacy_schema_four_preserves_layered_uncertain_work_and_grants_no_probe_budget() {
    use redb::{ReadableTable, TableDefinition};
    let dir = directory();
    let path = dir.path().join("capacity");
    let a = open(&path);
    layered::layers(&a);
    let paid = a.reserve(&d(20), work(1)).unwrap();
    a.dispatch(&paid).unwrap();
    drop(a);
    let db = redb::Database::open(&path).unwrap();
    let tx = db.begin_write().unwrap();
    for name in ["capacity_probe_budgets_v1", "capacity_probes_v1"] {
        tx.delete_table(TableDefinition::<&str, &[u8]>::new(name))
            .unwrap();
    }
    tx.delete_table(TableDefinition::<&str, &str>::new(
        "capacity_probe_groups_v1",
    ))
    .unwrap();
    {
        let mut table = tx
            .open_table(TableDefinition::<&str, &[u8]>::new("capacity_meta_v1"))
            .unwrap();
        let mut meta: serde_json::Value =
            serde_json::from_slice(table.get("state").unwrap().unwrap().value()).unwrap();
        meta["schema"] = 4.into();
        for field in ["probe_budgets", "probes", "probe_groups"] {
            meta.as_object_mut().unwrap().remove(field);
        }
        table
            .insert("state", serde_json::to_vec(&meta).unwrap().as_slice())
            .unwrap();
    }
    tx.commit().unwrap();
    drop(db);
    let a = open(&path);
    counts(&a, 1);
    assert_eq!(
        a.lease(&paid.lease().id).unwrap().unwrap().phase,
        capacity::Phase::Uncertain
    );
    assert!(a.probe_budget(&d(11)).unwrap().is_none());
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::Checking)
    ));
    layered::layers(&a);
    assert!(matches!(
        a.reserve_probe(spec()),
        Err(capacity::Error::ProbeBudget)
    ));
}
