#![cfg(unix)]

use mayhem_proto::proxy::{ProxyEndpoint, ProxyRail};
use mayhem_proxy::{
    attempts::*,
    connector::failure::{openai_error, Code},
};
use std::{
    path::Path,
    sync::{Arc, Barrier},
    time::SystemTime,
};

fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn digest(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: digest(100),
        subnet_bootstrap: digest(101),
        controller_pubkey: digest(102),
    }
}
fn limits() -> Limits {
    Limits {
        max_records: 500,
        max_unfinished: 200,
        closed_retention_ms: 1000,
        max_payload_bytes: 128 * 1024 * 1024,
    }
}
fn binding() -> Binding {
    Binding {
        request_hash: digest(1),
        endpoint: ProxyEndpoint::Chat,
        contract_version: 30,
        provider_pubkey: digest(2),
        market_id: digest(3),
        offer_digest: digest(4),
        endpoint_contract: digest(5),
        metering_policy: digest(6),
        accepted_terms: digest(7),
        reservation: digest(8),
        capacity_lease: digest(9),
        connection_digest: digest(10),
        connection_revision: 1,
        recipe_digest: digest(11),
        rail: ProxyRail::Tnk,
    }
}
fn open(path: &Path) -> Journal {
    Journal::open(path, identity(), limits()).unwrap()
}
fn event(j: &Journal, r: Record, e: Event) -> Record {
    j.advance(&r.invocation, r.generation, e, r.updated_at_ms + 1)
        .unwrap()
}
fn prepared(j: &Journal, n: u64) -> Record {
    j.prepare(digest(n), binding(), 100).unwrap()
}

#[test]
fn reserved_receipt_storage_is_bounded_idempotent_and_pruned_with_the_closed_attempt() {
    let dir = private_dir();
    let path = dir.path().join("journal");
    let mut l = limits();
    l.max_payload_bytes = 65536;
    let j = Journal::open(&path, identity(), l).unwrap();
    let r = prepared(&j, 1);
    j.reserve_outcome(&r.invocation, r.attempt).unwrap();
    j.reserve_outcome(&r.invocation, r.attempt).unwrap();
    assert_eq!(j.allocated_payload_bytes().unwrap(), 65536);
    let second = prepared(&j, 2);
    assert!(matches!(
        j.reserve_outcome(&second.invocation, second.attempt),
        Err(Error::Capacity)
    ));
    assert_eq!(
        j.get(&second.invocation).unwrap().unwrap().phase,
        Phase::Prepared
    );
    let r = event(&j, r, Event::CancelRequested);
    let closed = event(&j, r, Event::Close(digest(800)));
    assert_eq!(j.prune_closed(closed.expires_at_ms.unwrap(), 1).unwrap(), 1);
    assert_eq!(j.allocated_payload_bytes().unwrap(), 0);
    j.reserve_outcome(&second.invocation, second.attempt)
        .unwrap();
    drop(j);
    let j = Journal::open(&path, identity(), l).unwrap();
    assert_eq!(j.allocated_payload_bytes().unwrap(), 65536);
    assert!(j
        .completed_draft(&second.invocation, second.attempt)
        .unwrap()
        .is_none());
}
fn dispatch(j: &Journal, r: Record) -> Record {
    j.begin_dispatch(&r.invocation, r.generation, r.updated_at_ms + 1)
        .unwrap()
        .record()
        .clone()
}
fn completed() -> Resolution {
    Resolution::Completed {
        verified: VerifiedResult {
            result: digest(900),
            usage_evidence: digest(901),
        },
    }
}
fn close(j: &Journal, r: Record) -> Record {
    let r = event(j, r, Event::Resolve(completed()));
    event(j, r, Event::Close(digest(902)))
}

#[test]
fn idempotency_scopes_and_pinned_terms_survive_restart() {
    let dir = private_dir();
    let path = dir.path().join("attempts");
    let k = invocation_key(&digest(20), ProxyEndpoint::Chat, b"invocation-one").unwrap();
    assert_ne!(
        k,
        invocation_key(&digest(21), ProxyEndpoint::Chat, b"invocation-one").unwrap()
    );
    assert_ne!(
        k,
        invocation_key(&digest(20), ProxyEndpoint::Responses, b"invocation-one").unwrap()
    );
    assert_ne!(
        k,
        invocation_key(&digest(20), ProxyEndpoint::Chat, b"invocation-two").unwrap()
    );
    assert!(invocation_key(&digest(20), ProxyEndpoint::Chat, &[]).is_err());
    assert!(invocation_key(&digest(20), ProxyEndpoint::Chat, &[1; 257]).is_err());
    let expected = {
        let j = open(&path);
        let r = j.prepare(k.clone(), binding(), 100).unwrap();
        let mut new_offer = binding();
        new_offer.offer_digest = digest(555);
        new_offer.contract_version = 31;
        assert_eq!(r, j.prepare(k.clone(), new_offer, 200).unwrap());
        let mut different = binding();
        different.request_hash = digest(222);
        assert!(matches!(
            j.prepare(k.clone(), different, 200),
            Err(Error::Conflict)
        ));
        let mut different = binding();
        different.endpoint = ProxyEndpoint::Decisions;
        assert!(matches!(
            j.prepare(k.clone(), different, 200),
            Err(Error::Conflict)
        ));
        r
    };
    assert_eq!(open(&path).get(&k).unwrap(), Some(expected));
}

#[test]
fn dispatch_is_single_winner_across_concurrent_workers() {
    let dir = private_dir();
    let j = Arc::new(open(&dir.path().join("attempts")));
    let r = prepared(&j, 1);
    let barrier = Arc::new(Barrier::new(12));
    let joins = (0..12)
        .map(|_| {
            let j = j.clone();
            let r = r.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                j.begin_dispatch(&r.invocation, r.generation, 101)
            })
        })
        .collect::<Vec<_>>();
    let mut wins = 0;
    for t in joins {
        match t.join().unwrap() {
            Ok(_) => wins += 1,
            Err(Error::Stale) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
    assert_eq!(wins, 1);
    let r = j.get(&digest(1)).unwrap().unwrap();
    assert_eq!(r.phase, Phase::Dispatched);
    assert!(matches!(
        j.begin_dispatch(&r.invocation, r.generation, 200),
        Err(Error::Transition)
    ));
}

#[test]
fn actual_process_death_preserves_pre_dispatch_and_unknown_windows() {
    // This subprocess waits after a durable commit; parent SIGKILLs it so no
    // destructor/normal shutdown flush can make the proof pass accidentally.
    if let Ok(path) = std::env::var("MAYHEM_PROXY_ATTEMPT_CRASH_FIXTURE") {
        let path = std::path::PathBuf::from(path);
        let j = open(&path);
        let r = prepared(&j, 1);
        if std::env::var("MAYHEM_PROXY_ATTEMPT_CRASH_STAGE").unwrap() == "sent" {
            dispatch(&j, r);
        }
        std::fs::write(path.with_extension("ready"), b"committed").unwrap();
        loop {
            std::thread::park();
        }
    }
    for stage in ["prepared", "sent"] {
        let dir = private_dir();
        let path = dir.path().join("attempts");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "actual_process_death_preserves_pre_dispatch_and_unknown_windows",
                "--nocapture",
            ])
            .env("MAYHEM_PROXY_ATTEMPT_CRASH_FIXTURE", &path)
            .env("MAYHEM_PROXY_ATTEMPT_CRASH_STAGE", stage)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !path.with_extension("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = path.with_extension("ready").exists();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ready);
        let j = open(&path);
        let r = j.get(&digest(1)).unwrap().unwrap();
        assert_eq!(j.recovery_page(None, 10).unwrap().records.len(), 1);
        if stage == "sent" {
            assert_eq!(r.phase, Phase::Dispatched);
            assert!(matches!(
                j.begin_dispatch(&r.invocation, r.generation, 500),
                Err(Error::Transition)
            ));
        } else {
            assert_eq!(r.phase, Phase::Prepared);
            assert_eq!(dispatch(&j, r).phase, Phase::Dispatched);
        }
    }
}

#[test]
fn classified_failure_is_durable_not_execution_or_payment_evidence() {
    let dir = private_dir();
    let path = dir.path().join("attempts");
    let saved = {
        let j = open(&path);
        let r = dispatch(&j, prepared(&j, 1));
        let f = openai_error(429, br#"{"error":{"code":"rate_limit_exceeded","message":"private upstream data","param":"privateField"}}"#, Some("4"), SystemTime::now());
        let r = event(&j, r, Event::Failure((&f).into()));
        let again = j
            .advance(
                &r.invocation,
                r.generation,
                Event::Failure((&f).into()),
                300,
            )
            .unwrap();
        assert_eq!(r, again, "identical diagnostic does not write again");
        assert_eq!(r.phase, Phase::Dispatched);
        assert!(matches!(
            j.advance(&r.invocation, r.generation, Event::Close(digest(2)), 400),
            Err(Error::Transition)
        ));
        assert!(matches!(
            j.replace_not_executed(&r.invocation, r.generation, binding(), 400),
            Err(Error::Transition)
        ));
        assert_eq!(j.prune_closed(u64::MAX, 64).unwrap(), 0);
        assert!(!serde_json::to_string(&r)
            .unwrap()
            .contains("private upstream data"));
        r
    };
    let actual = open(&path).get(&digest(1)).unwrap().unwrap();
    assert_eq!(actual, saved);
    let failure = actual.last_failure.unwrap().to_failure().unwrap();
    assert_eq!(failure.code, Code::UpstreamRateLimited);
    assert_eq!(failure.upstream_code, Some("rate_limit_exceeded"));
    assert_eq!(failure.retry_after_ms, Some(4000));
    assert_eq!(failure.parameter, None);
}

#[test]
fn partial_output_cancellation_and_resolution_keep_their_distinct_meanings() {
    let dir = private_dir();
    let j = open(&dir.path().join("attempts"));
    let r = dispatch(&j, prepared(&j, 1));
    let r = event(
        &j,
        r,
        Event::Accepted(RemoteId::new("private-job-identifier").unwrap()),
    );
    assert!(!format!("{r:?}").contains("private-job-identifier"));
    assert!(matches!(
        j.advance(
            &r.invocation,
            r.generation,
            Event::Accepted(RemoteId::new("other-job").unwrap()),
            120
        ),
        Err(Error::Conflict)
    ));
    let r = event(&j, r, Event::FirstOutput);
    let noop = j
        .advance(&r.invocation, r.generation, Event::FirstOutput, 125)
        .unwrap();
    assert_eq!(r, noop, "no write per additional output chunk");
    assert!(matches!(
        j.advance(
            &r.invocation,
            r.generation,
            Event::Resolve(Resolution::NotExecuted {
                evidence: digest(3)
            }),
            130
        ),
        Err(Error::Transition)
    ));
    let r = event(&j, r, Event::CancelRequested);
    assert_eq!(r.phase, Phase::Dispatched);
    assert!(matches!(
        j.advance(&r.invocation, r.generation, Event::FirstOutput, 140),
        Err(Error::Transition)
    ));
    assert_eq!(j.recovery_page(None, 64).unwrap().records.len(), 1);
    let r = event(
        &j,
        r,
        Event::Resolve(Resolution::Cancelled {
            evidence: digest(3),
            partial: Some(VerifiedResult {
                result: digest(4),
                usage_evidence: digest(5),
            }),
        }),
    );
    assert_eq!(
        j.recovery_page(None, 64).unwrap().records.len(),
        1,
        "confirmed upstream cancellation is not settlement closure"
    );
    let r = event(&j, r, Event::Close(digest(6)));
    assert!(j.recovery_page(None, 64).unwrap().records.is_empty());
    assert!(matches!(
        j.replace_not_executed(&r.invocation, r.generation, binding(), 200),
        Err(Error::Transition)
    ));
}

#[test]
fn cancel_before_dispatch_never_sends_and_completion_after_cancel_is_retained() {
    let dir = private_dir();
    let j = open(&dir.path().join("attempts"));
    let r = event(&j, prepared(&j, 1), Event::CancelRequested);
    assert!(matches!(r.resolution, Some(Resolution::NotExecuted { .. })));
    assert!(matches!(
        j.begin_dispatch(&r.invocation, r.generation, 120),
        Err(Error::Transition)
    ));
    let r = event(&j, r, Event::Close(digest(2)));
    assert!(matches!(
        j.replace_not_executed(&r.invocation, r.generation, binding(), 130),
        Err(Error::Transition)
    ));
    let r = event(&j, dispatch(&j, prepared(&j, 2)), Event::CancelRequested);
    let r = event(&j, r, Event::Resolve(completed()));
    assert!(r.cancellation_requested && matches!(r.resolution, Some(Resolution::Completed { .. })));
}

#[test]
fn replacement_requires_known_nonexecution_and_ack_preserves_history_and_fences_old_workers() {
    let dir = private_dir();
    let path = dir.path().join("attempts");
    let j = open(&path);
    let old = dispatch(&j, prepared(&j, 1));
    let r = event(
        &j,
        old.clone(),
        Event::Resolve(Resolution::NotExecuted {
            evidence: digest(3),
        }),
    );
    assert!(matches!(
        j.replace_not_executed(&r.invocation, r.generation, binding(), 200),
        Err(Error::Transition)
    ));
    let r = event(&j, r, Event::Close(digest(4)));
    let mut replacement = binding();
    replacement.provider_pubkey = digest(55);
    replacement.offer_digest = digest(56);
    replacement.accepted_terms = digest(57);
    replacement.contract_version = 31;
    let next = j
        .replace_not_executed(&r.invocation, r.generation, replacement.clone(), 200)
        .unwrap();
    assert!(next.attempt > old.attempt);
    assert_eq!(next.binding, replacement);
    assert!(next.generation > r.generation);
    assert!(matches!(
        j.advance(
            &old.invocation,
            old.generation,
            Event::Accepted(RemoteId::new("late").unwrap()),
            201
        ),
        Err(Error::Stale)
    ));
    assert_eq!(j.prune_closed(1500, 64).unwrap(), 1);
    assert_eq!(j.get(&next.invocation).unwrap(), Some(next.clone()));
    drop(j);
    assert_eq!(
        open(&path).recovery_page(None, 64).unwrap().records,
        vec![next]
    );
}

#[test]
fn recovery_and_retention_are_indexed_bounded_and_do_not_expire_unknown_work() {
    let dir = private_dir();
    let path = dir.path().join("attempts");
    let j = open(&path);
    for n in 1..=70 {
        let r = dispatch(&j, prepared(&j, n));
        if n % 2 == 0 {
            close(&j, r);
        }
    }
    assert!(j.recovery_page(None, 0).is_err());
    assert!(j.recovery_page(None, 65).is_err());
    let mut after = None;
    let mut seen = Vec::new();
    loop {
        let page = j.recovery_page(after.as_ref(), 7).unwrap();
        seen.extend(page.records.iter().map(|r| r.invocation.clone()));
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(
        seen,
        (1..=70)
            .filter(|n| n % 2 != 0)
            .map(digest)
            .collect::<Vec<_>>()
    );
    assert_eq!(j.prune_closed(1000, 64).unwrap(), 0);
    assert_eq!(j.prune_closed(2000, 3).unwrap(), 3);
    assert_eq!(j.prune_closed(2000, 64).unwrap(), 32);
    assert_eq!(j.prune_closed(u64::MAX, 64).unwrap(), 0);
    drop(j);
    assert_eq!(
        open(&path).recovery_page(None, 64).unwrap().records.len(),
        35
    );
}

#[test]
fn full_journal_refuses_new_work_but_allows_recovery_closure_and_replay() {
    let dir = private_dir();
    let path = dir.path().join("attempts");
    let tiny = Limits {
        max_records: 2,
        max_unfinished: 1,
        closed_retention_ms: 1000,
        max_payload_bytes: 128 * 1024 * 1024,
    };
    let j = Journal::open(&path, identity(), tiny).unwrap();
    let r = prepared(&j, 1);
    assert!(matches!(
        j.prepare(digest(2), binding(), 100),
        Err(Error::Capacity)
    ));
    assert_eq!(j.prepare(digest(1), binding(), 100).unwrap(), r);
    let closed = close(&j, dispatch(&j, r));
    let r = prepared(&j, 2);
    close(&j, dispatch(&j, r));
    assert!(matches!(
        j.prepare(digest(3), binding(), 200),
        Err(Error::Capacity)
    ));
    assert_eq!(j.prune_closed(3000, 1).unwrap(), 1);
    let new = j.prepare(digest(1), binding(), 3001).unwrap();
    assert!(
        new.generation > closed.generation,
        "reusing an expired key cannot resurrect a stale CAS token"
    );
    assert!(matches!(
        j.begin_dispatch(&new.invocation, closed.generation, 3002),
        Err(Error::Stale)
    ));
    // Reducing admission limits on restart must not strand existing recovery.
    drop(j);
    let j = Journal::open(
        &path,
        identity(),
        Limits {
            max_records: 1,
            ..tiny
        },
    )
    .unwrap();
    assert_eq!(j.recovery_page(None, 10).unwrap().records.len(), 1);
    close(&j, dispatch(&j, new));
}

#[test]
fn identity_corruption_permissions_and_second_writer_fail_closed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = private_dir();
    let path = dir.path().join("attempts");
    let j = open(&path);
    prepared(&j, 1);
    assert!(Journal::open(&path, identity(), limits()).is_err());
    drop(j);
    let mut wrong = identity();
    wrong.controller_pubkey = digest(999);
    assert!(matches!(
        Journal::open(&path, wrong, limits()),
        Err(Error::Identity)
    ));
    assert!(open(&path).get(&digest(1)).unwrap().is_some());
    let link = dir.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(matches!(
        Journal::open(&link, identity(), limits()),
        Err(Error::File)
    ));
    let hard = dir.path().join("hard");
    std::fs::hard_link(&path, &hard).unwrap();
    assert!(matches!(
        Journal::open(&path, identity(), limits()),
        Err(Error::File)
    ));
    std::fs::remove_file(hard).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Journal::open(&path, identity(), limits()),
        Err(Error::File)
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let corrupt = dir.path().join("corrupt");
    std::fs::write(&corrupt, b"broken").unwrap();
    std::fs::set_permissions(&corrupt, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Journal::open(&corrupt, identity(), limits()).is_err());
    assert_eq!(std::fs::read(&corrupt).unwrap(), b"broken");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        Journal::open(&path, identity(), limits()),
        Err(Error::File)
    ));
}

#[test]
fn malformed_bindings_clock_reversal_and_unknown_fields_do_not_mutate_state() {
    let dir = private_dir();
    let j = open(&dir.path().join("attempts"));
    let mut bad = binding();
    bad.connection_revision = 0;
    assert!(j.prepare(digest(1), bad, 100).is_err());
    assert!(j.get(&digest(1)).unwrap().is_none());
    let r = prepared(&j, 1);
    assert!(j.begin_dispatch(&r.invocation, r.generation, 99).is_err());
    assert_eq!(j.get(&r.invocation).unwrap(), Some(r));
    let mut value = serde_json::to_value(binding()).unwrap();
    value["upstream_url"] = serde_json::json!("https://not-authorized.invalid/");
    assert!(serde_json::from_value::<Binding>(value).is_err());
    assert!(serde_json::from_str::<Digest>("\"garbage\"").is_err());
    assert!(RemoteId::new("a\nb").is_err());
    assert!(RemoteId::new("x".repeat(257)).is_err());
    assert!(j.prune_closed(100, 0).is_err());
    let r = dispatch(&j, j.get(&digest(1)).unwrap().unwrap());
    let f = openai_error(
        400,
        br#"{"error":{"code":"unsupported_parameter","param":"tools"}}"#,
        None,
        SystemTime::now(),
    );
    let safe = FailureSnapshot::from(&f);
    assert_eq!(safe.to_failure().unwrap().parameter, Some("tools"));
    for field in ["parameter", "upstream_code"] {
        let mut json = serde_json::to_value(&safe).unwrap();
        json[field] = serde_json::json!("untrusted provider text");
        let unsafe_value: FailureSnapshot = serde_json::from_value(json).unwrap();
        assert!(unsafe_value.to_failure().is_err());
        assert!(j
            .advance(
                &r.invocation,
                r.generation,
                Event::Failure(unsafe_value),
                200
            )
            .is_err());
        assert_eq!(j.get(&r.invocation).unwrap(), Some(r.clone()));
    }
}

#[test]
fn shortening_retention_cannot_overwrite_an_older_attempt_when_a_key_is_reused() {
    let dir = private_dir();
    let path = dir.path().join("attempts");
    let j = open(&path);
    let old = dispatch(&j, prepared(&j, 1));
    let old = event(
        &j,
        old,
        Event::Resolve(Resolution::NotExecuted {
            evidence: digest(800),
        }),
    );
    let old = event(&j, old, Event::Close(digest(801)));
    drop(j);
    let j = Journal::open(
        &path,
        identity(),
        Limits {
            closed_retention_ms: 1,
            max_payload_bytes: 128 * 1024 * 1024,
            ..limits()
        },
    )
    .unwrap();
    let next = j
        .replace_not_executed(&old.invocation, old.generation, binding(), 200)
        .unwrap();
    let next = close(&j, dispatch(&j, next));
    assert_eq!(j.prune_closed(300, 64).unwrap(), 1);
    assert!(j.get(&old.invocation).unwrap().is_none());
    let reused = j.prepare(old.invocation.clone(), binding(), 301).unwrap();
    assert_ne!(reused.attempt, old.attempt);
    assert_ne!(reused.attempt, next.attempt);
    assert_eq!(
        j.get_attempt(&old.invocation, old.attempt).unwrap(),
        Some(old.clone())
    );
    assert!(matches!(
        j.begin_dispatch(&reused.invocation, next.generation, 302),
        Err(Error::Stale)
    ));
    drop(j);
    let j = open(&path);
    assert_eq!(j.get(&old.invocation).unwrap(), Some(reused.clone()));
    assert_eq!(j.prune_closed(2000, 64).unwrap(), 1);
    assert_eq!(j.get(&old.invocation).unwrap(), Some(reused));
}

#[test]
fn all_llm_and_decision_endpoints_and_all_rails_retain_exact_response_commitments() {
    let dir = private_dir();
    let j = open(&dir.path().join("attempts"));
    let mut n = 1;
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
            let mut binding = binding();
            binding.endpoint = endpoint;
            binding.rail = rail;
            let r = j.prepare(digest(n), binding.clone(), 100).unwrap();
            let r = close(&j, dispatch(&j, r));
            let replay = j.prepare(digest(n), binding, 200).unwrap();
            assert_eq!(replay, r);
            assert_eq!(replay.binding.rail, rail);
            assert_eq!(replay.binding.endpoint, endpoint);
            assert_eq!(replay.resolution, Some(completed()));
            assert!(matches!(
                j.replace_not_executed(&r.invocation, r.generation, r.binding.clone(), 201),
                Err(Error::Transition)
            ));
            n += 1;
        }
    }
}
