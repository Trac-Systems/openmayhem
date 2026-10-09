use super::*;
use crate::probe_execution::vllm_refusal;

async fn refused(p: &Paid, bytes: &[u8], streaming: bool) {
    let result = if streaming {
        p.executor
            .execute_stream(
                &p.record.invocation,
                bytes,
                &Cancellation::default(),
                |_| async { panic!("HTTP admission refusal cannot emit a model token") },
            )
            .await
    } else {
        p.executor
            .execute_json(&p.record.invocation, bytes, &Cancellation::default())
            .await
    };
    assert!(matches!(result, Err(Error::Upstream(ref f)) if f.execution == Execution::Rejected));
}

#[tokio::test]
async fn paid_verified_refusal_frees_capacity_but_only_signed_closure_releases_funds_on_all_rails()
{
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for streaming in [false, true] {
            let b = backend(503, vllm_refusal(streaming), Duration::ZERO).await;
            let bytes = if streaming { stream_request() } else { chat() };
            let mut p = Paid::start_scoped_profile(
                &b.base,
                ProxyEndpoint::Chat,
                rail,
                &bytes,
                streaming,
                None,
                true,
                false,
                "vllm_admission_v1",
            )
            .await;
            refused(&p, &bytes, streaming).await;
            let saved = p
                .journal
                .recover(&p.record.invocation, p.record.attempt)
                .unwrap();
            assert_eq!(saved.record.phase, Phase::Resolved);
            assert!(matches!(
                saved.record.resolution,
                Some(attempts::Resolution::NotExecuted { .. })
            ));
            assert!(saved.result.is_none() && saved.record.closure.is_none());
            assert!(p.authority.lease(&p.lease).unwrap().is_none());
            assert!(p
                .journal
                .waiver(&p.record.invocation, p.record.attempt)
                .unwrap()
                .is_none());
            assert_eq!(
                p.peer.command("status").await["publications"],
                1,
                "only the funded reservation was published"
            );
            let observed = p.peer.client.observe(&p.authorization).await.unwrap();
            assert!(!observed.is_closed() && !observed.has_receipt());
            assert!(!p
                .executor
                .reconcile_capacity(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            let closure = signed_waiver(&mut p).await;
            let draft = p
                .journal
                .waiver_draft(&p.record.invocation, p.record.attempt)
                .unwrap()
                .unwrap();
            assert!(draft.non_execution.is_some());
            assert_eq!(
                closure.body.outcome,
                mayhem_proto::proxy::finance::ProxyClosureOutcome::NotExecuted
            );
            p.executor
                .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
                .await
                .unwrap();
            p.peer.command("publish_pending").await;
            assert!(!p
                .executor
                .publish_waiver(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            p.peer.command("flush_publication").await;
            let mut p = p.reopen();
            assert!(p
                .executor
                .publish_waiver(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert!(p
                .executor
                .publish_waiver(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            let observed = p.peer.client.observe(&p.authorization).await.unwrap();
            assert!(observed.confirms_waiver(&closure).unwrap());
            assert_eq!(
                p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
                Phase::Closed
            );
            let stats = p.peer.command("status").await;
            assert_eq!(
                stats["publications"], 2,
                "reservation and one zero-charge closure"
            );
            assert_eq!(stats["submissions"], 2);
            assert!(p
                .executor
                .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                .await
                .is_err());
            assert_eq!(b.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn paid_generic_upstream_error_cannot_release_capacity_or_offer_nonexecution_waiver() {
    for (status, response, profile) in [
        (503, vllm_refusal(false), "open_ai"),
        (
            503,
            json!({"error":{"message":"busy","code":503}}),
            "vllm_admission_v1",
        ),
        (
            429,
            json!({"error":{"code":"rate_limit_exceeded"}}),
            "vllm_admission_v1",
        ),
    ] {
        for streaming in [false, true] {
            let b = backend(status, response.clone(), Duration::ZERO).await;
            let bytes = if streaming { stream_request() } else { chat() };
            let mut p = Paid::start_scoped_profile(
                &b.base,
                ProxyEndpoint::Chat,
                ProxyRail::Tnk,
                &bytes,
                streaming,
                None,
                true,
                false,
                profile,
            )
            .await;
            let result = if streaming {
                p.executor
                    .execute_stream(
                        &p.record.invocation,
                        &bytes,
                        &Cancellation::default(),
                        |_| async { Ok(()) },
                    )
                    .await
            } else {
                p.executor
                    .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                    .await
            };
            assert!(
                matches!(result, Err(Error::Upstream(ref f)) if f.execution == Execution::Unknown)
            );
            assert_eq!(
                p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
                Phase::Dispatched
            );
            assert!(p.authority.lease(&p.lease).unwrap().is_some());
            assert!(p
                .executor
                .prepare_waiver(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            assert!(p
                .executor
                .reconcile_capacity(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            assert_eq!(p.peer.command("status").await["publications"], 1);
            assert!(!p
                .peer
                .client
                .observe(&p.authorization)
                .await
                .unwrap()
                .is_closed());
            assert_eq!(b.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn paid_refusal_buyer_rejects_changed_assertion_binding_signature_and_received_output() {
    let b = backend(503, vllm_refusal(false), Duration::ZERO).await;
    let mut p = Paid::start_scoped_profile(
        &b.base,
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        &chat(),
        false,
        None,
        true,
        false,
        "vllm_admission_v1",
    )
    .await;
    refused(&p, &chat(), false).await;
    let closure = signed_waiver(&mut p).await;
    let original = p
        .journal
        .waiver_draft(&p.record.invocation, p.record.attempt)
        .unwrap()
        .unwrap();
    let received = p
        ._fixture
        .adapter
        .prepare_json(&chat())
        .unwrap()
        .decode_json(answer(), "proof", 1);
    // An independent successful response cannot be reconciled as never executed.
    for fault in [
        "attempt",
        "invocation",
        "failure",
        "missing",
        "outcome",
        "signature",
        "output",
    ] {
        let mut draft = original.clone();
        match fault {
            "attempt" => draft.attempt += 1,
            "invocation" => draft.invocation = d(999),
            "failure" => {
                draft.non_execution.as_mut().unwrap().failure.upstream_code =
                    Some("vllm_prefill_backlog".into())
            }
            "missing" => draft.non_execution = None,
            "outcome" => {
                draft.body.outcome =
                    mayhem_proto::proxy::finance::ProxyClosureOutcome::CompletedUnbilled
            }
            _ => (),
        }
        let signature = if fault == "signature" {
            "0".repeat(128)
        } else {
            closure.provider_sig.clone()
        };
        assert!(
            mayhem_proxy::receipts::approve_waiver(
                &draft,
                &signature,
                &p.authorization,
                &chat(),
                if fault == "output" {
                    Some(received.as_ref().unwrap())
                } else {
                    None
                },
                false
            )
            .is_err(),
            "{fault}"
        );
    }
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_saved_preintegration_refusal_resolves_one_attempt_after_restart_without_resend() {
    let b = backend(503, vllm_refusal(false), Duration::ZERO).await;
    let p = Paid::start_scoped_profile(
        &b.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tap,
        &chat(),
        false,
        None,
        true,
        false,
        "vllm_admission_v1",
    )
    .await;
    refused(&p, &chat(), false).await;
    let key = format!("{}:{:020}", p.record.invocation.as_str(), p.record.attempt);
    let mut p = p.reopen_edit(false, |tx| {
        use redb::ReadableTable;
        let mut table = tx
            .open_table(redb::TableDefinition::<&str, &[u8]>::new(
                "proxy_attempt_records_v1",
            ))
            .unwrap();
        let mut value: Value =
            serde_json::from_slice(table.get(key.as_str()).unwrap().unwrap().value()).unwrap();
        value["phase"] = json!("dispatched");
        value["resolution"] = Value::Null;
        table
            .insert(key.as_str(), serde_json::to_vec(&value).unwrap().as_slice())
            .unwrap();
    });
    assert_eq!(
        p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
        Phase::Dispatched
    );
    let closure = signed_waiver(&mut p).await;
    assert_eq!(
        p.journal.get(&p.record.invocation).unwrap().unwrap().phase,
        Phase::Resolved
    );
    p.executor
        .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
        .await
        .unwrap();
    assert!(p
        .executor
        .publish_waiver(&p.record.invocation, p.record.attempt)
        .await
        .unwrap());
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn paid_nonexecution_crash_between_journal_and_capacity_commit_recovers_without_payment_or_post(
) {
    use mayhem_proxy::connector::failure::{Failure, Scope, Stage};
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let b = backend(200, answer(), Duration::ZERO).await;
        let p = Paid::start(&b.base, ProxyEndpoint::Chat, rail, &chat(), false).await;
        p.authority
            .dispatch_accepted(
                &p.lease,
                &capacity::Work {
                    invocation: p.record.invocation.clone(),
                    request_hash: p.record.binding.request_hash.clone(),
                },
                &d(201),
            )
            .unwrap();
        let r = p
            .journal
            .begin_dispatch(
                &p.record.invocation,
                p.record.generation,
                p.record.updated_at_ms + 1,
            )
            .unwrap();
        let r = r.record();
        // Parent compilation rejected the schema before any HTTP request. Commit
        // that proof then simulate process loss before the capacity transaction.
        let f = Failure::new(
            Code::InvalidSchema,
            Scope::Request,
            Stage::BeforeDispatch,
            Execution::NotDispatched,
        );
        p.journal
            .advance(
                &r.invocation,
                r.generation,
                attempts::Event::Failure((&f).into()),
                r.updated_at_ms + 1,
            )
            .unwrap();
        assert!(p.authority.lease(&p.lease).unwrap().is_some());
        let mut p = p.reopen();
        assert_eq!(
            p.authority.lease(&p.lease).unwrap().unwrap().phase,
            capacity::Phase::Uncertain
        );
        assert!(p
            .executor
            .reconcile_capacity(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        assert!(!p
            .executor
            .reconcile_capacity(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        assert!(p.authority.lease(&p.lease).unwrap().is_none());
        assert!(!p
            .peer
            .client
            .observe(&p.authorization)
            .await
            .unwrap()
            .is_closed());
        let closure = signed_waiver(&mut p).await;
        p.executor
            .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
            .await
            .unwrap();
        assert!(p
            .executor
            .publish_waiver(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        assert_eq!(p.peer.command("status").await["publications"], 1);
        assert_eq!(b.calls.load(Ordering::SeqCst), 0);
        p.peer.stop().await;
    }
}
