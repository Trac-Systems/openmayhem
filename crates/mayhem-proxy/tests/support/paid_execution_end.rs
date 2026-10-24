use super::*;

#[tokio::test]
async fn failed_complete_responses_release_capacity_on_every_rail_without_billing_or_redispatch() {
    let invalid_stream = sse(
        &[json!({"choices":[{"index":9,"delta":{"content":"invalid"}}]})],
        true,
    );
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (status, response, mime, streaming) in [
            (
                400,
                br#"{"error":{"message":"invalid context","type":"BadRequestError","code":400}}"#
                    .to_vec(),
                "application/json",
                false,
            ),
            (
                400,
                br#"{"error":{"message":"invalid context","type":"BadRequestError","code":400}}"#
                    .to_vec(),
                "application/json",
                true,
            ),
            (200, b"invalid json".to_vec(), "application/json", false),
            (200, invalid_stream.clone(), "text/event-stream", true),
        ] {
            let b = backend_raw(status, response, mime, Duration::ZERO).await;
            let bytes = if streaming { stream_request() } else { chat() };
            let mut p = Paid::start(&b.base, ProxyEndpoint::Chat, rail, &bytes, streaming).await;
            let before = p.peer.command("status").await["publications"].clone();
            let result = if streaming {
                p.executor
                    .execute_stream(
                        &p.record.invocation,
                        &bytes,
                        &Cancellation::default(),
                        |_| async { panic!("invalid output delivered") },
                    )
                    .await
            } else {
                p.executor
                    .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                    .await
            };
            assert!(result.is_err());
            let saved = p
                .journal
                .recover(&p.record.invocation, p.record.attempt)
                .unwrap();
            assert!(
                saved.record.execution_ended.is_some(),
                "{rail:?} {status} {streaming}"
            );
            assert_eq!(saved.record.phase, Phase::Dispatched);
            assert!(
                saved.result.is_none()
                    && saved.record.resolution.is_none()
                    && saved.record.closure.is_none()
            );
            assert!(p.authority.lease(&p.lease).unwrap().is_none());
            assert_eq!(p.authority.status(&d(201)).unwrap().group_occupied, 0);
            assert!(p
                .executor
                .prepare_waiver(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            assert!(p
                .executor
                .prepare_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .is_err());
            assert!(!p
                .peer
                .client
                .observe(&p.authorization)
                .await
                .unwrap()
                .is_closed());
            assert_eq!(p.peer.command("status").await["publications"], before);
            let mut p = p.reopen();
            assert!(!p
                .executor
                .resume_saved_for_test(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert!(p.authority.lease(&p.lease).unwrap().is_none());
            assert!(p
                .executor
                .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
                .await
                .is_err());
            assert_eq!(b.calls.load(Ordering::SeqCst), 1);
            assert_eq!(p.peer.command("status").await["publications"], before);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn sse_eof_and_embedded_or_incomplete_done_text_do_not_prove_execution_ended() {
    for response in [
        sse(&[delta("[DONE]", Value::Null)], false),
        b"data: [DONE]\n".to_vec(),
        b"data: arbitrary\ndata: [DONE]\n\n".to_vec(),
    ] {
        let b = backend_raw(200, response, "text/event-stream", Duration::ZERO).await;
        let bytes = stream_request();
        let p = Paid::start(&b.base, ProxyEndpoint::Chat, ProxyRail::Tnk, &bytes, true).await;
        assert!(p
            .executor
            .execute_stream(
                &p.record.invocation,
                &bytes,
                &Cancellation::default(),
                |_| async { Ok(()) }
            )
            .await
            .is_err());
        let r = p.journal.get(&p.record.invocation).unwrap().unwrap();
        assert!(r.execution_ended.is_none());
        assert!(p.authority.lease(&p.lease).unwrap().is_some());
        assert!(p
            .executor
            .reconcile_capacity(&r.invocation, r.attempt)
            .await
            .is_err());
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn accepted_background_http_response_does_not_release_execution_capacity() {
    let b = backend(
        202,
        json!({"id":"background-job","status":"queued"}),
        Duration::ZERO,
    )
    .await;
    let bytes = chat();
    let p = Paid::start(&b.base, ProxyEndpoint::Chat, ProxyRail::Fiat, &bytes, false).await;
    assert!(p
        .executor
        .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
        .await
        .is_err());
    assert!(p
        .journal
        .get(&p.record.invocation)
        .unwrap()
        .unwrap()
        .execution_ended
        .is_none());
    assert!(p.authority.lease(&p.lease).unwrap().is_some());
    p.peer.stop().await;
}

#[tokio::test]
async fn execution_end_survives_crash_before_capacity_release_without_redispatch() {
    let b = backend_raw(
        200,
        sse(&[delta("partial", Value::Null)], false),
        "text/event-stream",
        Duration::ZERO,
    )
    .await;
    let bytes = stream_request();
    let p = Paid::start(&b.base, ProxyEndpoint::Chat, ProxyRail::Tap, &bytes, true).await;
    assert!(p
        .executor
        .execute_stream(
            &p.record.invocation,
            &bytes,
            &Cancellation::default(),
            |_| async { Ok(()) }
        )
        .await
        .is_err());
    let r = p.journal.get(&p.record.invocation).unwrap().unwrap();
    assert!(r.execution_ended.is_none());
    // Fault boundary: terminal evidence is durable, then the controller dies
    // before releasing its still-dispatched lease.
    p.journal
        .advance(
            &r.invocation,
            r.generation,
            attempts::Event::ExecutionEnded(d(987)),
            r.updated_at_ms + 1,
        )
        .unwrap();
    assert!(p.authority.lease(&p.lease).unwrap().is_some());
    let p = p.reopen();
    assert!(!p
        .executor
        .resume_saved_for_test(&r.invocation, r.attempt)
        .await
        .unwrap());
    assert!(p.authority.lease(&p.lease).unwrap().is_none());
    let saved = p.journal.recover(&r.invocation, r.attempt).unwrap();
    assert!(
        saved.result.is_none()
            && saved.record.resolution.is_none()
            && saved.record.closure.is_none()
    );
    assert_eq!(saved.record.execution_ended, Some(d(987)));
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert!(!p
        .peer
        .client
        .observe(&p.authorization)
        .await
        .unwrap()
        .is_closed());
    p.peer.stop().await;
}
