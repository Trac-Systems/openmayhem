use super::*;
use mayhem_proxy::{
    attempts::Event,
    capacity,
    connector::failure::{Scope, Stage},
};

#[tokio::test]
async fn stopped_customer_execution_recovery_is_exact_exclusive_and_preserves_unknown_billing() {
    let backend = backend(200, json!({}), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let root = dir();
    let bridge = bridge::Bridge::start(d(70).as_str(), &public_key()).await;
    let mut value = config(
        root.path(),
        &backend.base,
        bridge.config(false).url.as_str(),
        ProxyEndpoint::Chat,
    );
    value["routes"][0]["adapter"] = json!(f.adapter.snapshot());
    private(&root.path().join("connection.json"), &serde_json::to_vec(&json!({"schema_version":1,"id":"fixture","revision":1,"base_url":backend.base,"network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},"paths":{"chat_completions":"chat/completions","completions":"completions","responses":"responses","decisions":"decisions"},"error_profile":"open_ai"})).unwrap());
    std::fs::create_dir(root.path().join("state")).unwrap();
    std::fs::set_permissions(
        root.path().join("state"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let path = save(root.path(), &value);
    let prepared = managed::Prepared::load_supervised(&path).unwrap();
    let identity = prepared.identity().clone();
    let signer = Authority::from_unlocked_wallet(key(), identity.clone()).unwrap();
    let limits = capacity::Limits {
        max_groups: 8,
        max_routes: 8,
        max_leases: 16,
        max_evidence_age: Duration::from_secs(60),
    };
    let capacity_path = root.path().join("state/capacity.redb");
    let a = capacity::Authority::open(&capacity_path, identity.clone(), limits.clone()).unwrap();
    a.configure_group(d(10), 2).unwrap();
    a.configure_route(capacity::Route {
        id: d(20),
        group: d(10),
        lane: capacity::Lane::Proxy,
        max_concurrency: 2,
    })
    .unwrap();
    for scope in [capacity::Scope::Group(d(10)), capacity::Scope::Route(d(20))] {
        capacity_ready(&a, scope);
    }
    let template = f.prepare(901, &chat(), ProxyRail::Tnk);
    let reservation = a
        .reserve(
            &d(20),
            capacity::Work {
                invocation: template.invocation.clone(),
                request_hash: template.binding.request_hash.clone(),
            },
        )
        .unwrap();
    let lease = reservation.lease().id.clone();
    let _dispatch = a.dispatch(&reservation).unwrap();
    let journal_path = root
        .path()
        .join(format!("state/route-{}.redb", d(20).as_str()));
    let journal_limits = serde_json::from_value(value["limits"]["journal"].clone()).unwrap();
    let j = Journal::open(&journal_path, identity.clone(), journal_limits).unwrap();
    let mut binding = template.binding;
    binding.capacity_lease = lease.clone();
    let r = j.prepare(template.invocation, binding, 100).unwrap();
    j.begin_dispatch(&r.invocation, r.generation, 101).unwrap();
    let current = j.get(&r.invocation).unwrap().unwrap();
    let failed = j
        .advance(
            &r.invocation,
            current.generation,
            Event::Failure(attempts::FailureSnapshot::from(
                &mayhem_proxy::connector::failure::Failure::new(
                    Code::UpstreamProtocol,
                    Scope::Model,
                    Stage::ResponseBody,
                    Execution::Unknown,
                ),
            )),
            102,
        )
        .unwrap();
    drop(j);
    let confirm = || managed::ExecutionRecoveryConfirmation {
        schema_version: 1,
        route: d(20),
        invocation: r.invocation.clone(),
        attempt: r.attempt,
        capacity_lease: lease.clone(),
        connection_digest: f.connection.fingerprint().clone(),
        evidence_digest: d(903),
        upstream_stopped: true,
    };
    let resolve = |input| {
        managed::Prepared::load_supervised(&path)
            .unwrap()
            .resolve_stopped_execution(&signer, input)
    };
    assert!(
        resolve(confirm()).is_err(),
        "must not fence a live controller"
    );
    drop(a);
    for field in [
        "route",
        "invocation",
        "capacity_lease",
        "connection_digest",
        "attempt",
        "upstream_stopped",
    ] {
        let mut input = serde_json::to_value(confirm()).unwrap();
        input[field] = match field {
            "attempt" => json!(r.attempt + 1),
            "upstream_stopped" => json!(false),
            _ => json!(d(999)),
        };
        assert!(
            resolve(serde_json::from_value(input).unwrap()).is_err(),
            "{field}"
        );
    }
    let resolved = resolve(confirm()).unwrap();
    assert!(resolved.released);
    assert!(!resolve(confirm()).unwrap().released);
    let j = Journal::open(
        &journal_path,
        identity.clone(),
        serde_json::from_value(value["limits"]["journal"].clone()).unwrap(),
    )
    .unwrap();
    let after = j.recover(&r.invocation, r.attempt).unwrap();
    assert_eq!(after.record.phase, Phase::Dispatched);
    assert_eq!(after.record.last_failure, failed.last_failure);
    assert_eq!(after.record.binding, failed.binding);
    assert_eq!(after.record.execution_ended, Some(resolved.evidence_digest));
    assert!(
        after.result.is_none()
            && after.record.resolution.is_none()
            && after.record.closure.is_none()
    );
    assert!(j
        .begin_dispatch(&r.invocation, after.record.generation, 103)
        .is_err());
    let a = capacity::Authority::open_existing(&capacity_path, identity, limits).unwrap();
    assert!(a.lease(&lease).unwrap().is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}
