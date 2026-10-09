use super::*;
use ed25519_dalek::SigningKey;
use mayhem_proxy::{
    attempts::AcceptanceSnapshot,
    financial::recovery::{BuyerRecovery, FinancialOutcome, Limits, SignedAcknowledgment, Store},
    signing::Authority,
};

fn buyer_identity(p: &Paid) -> Identity {
    let mut id = p.peer.identity.clone();
    id.controller_pubkey = Digest::new(&p.authorization.terms.buyer_pubkey).unwrap();
    id
}
fn buyer(p: &Paid) -> BuyerRecovery {
    BuyerRecovery::new(
        Arc::new(
            Store::open(
                p._fixture._store.path().join("buyer-signing"),
                buyer_identity(p),
                Limits {
                    max_records: 8,
                    closed_retention_ms: 1000,
                },
            )
            .unwrap(),
        ),
        p.peer.buyer_client.clone(),
        4,
    )
    .unwrap()
}
fn key(p: &Paid) -> Digest {
    Digest::new(p.authorization.terms.digest().unwrap()).unwrap()
}
async fn authorities(p: &mut Paid) -> (Authority, Authority) {
    let value = p.peer.command("ephemeral_test_wallet_seeds").await;
    let provider: [u8; 32] = serde_json::from_value(value["provider"].clone()).unwrap();
    let buyer_key: [u8; 32] = serde_json::from_value(value["buyer"].clone()).unwrap();
    (
        Authority::from_unlocked_wallet(SigningKey::from_bytes(&provider), p.peer.identity.clone())
            .unwrap(),
        Authority::from_unlocked_wallet(SigningKey::from_bytes(&buyer_key), buyer_identity(p))
            .unwrap(),
    )
}
fn snapshot(p: &Paid) -> AcceptanceSnapshot {
    AcceptanceSnapshot {
        adapter: p._fixture.adapter.snapshot(),
        offer: p.authorization.terms.offer.clone(),
    }
}
fn copy_reply(v: &mayhem_proxy::endpoint::ProtocolReply) -> mayhem_proxy::endpoint::ProtocolReply {
    serde_json::from_value(serde_json::to_value(v).unwrap()).unwrap()
}
#[tokio::test]
async fn protected_streamed_receipts_are_independently_verified_before_buyer_signing() {
    let completion =
        serde_json::to_vec(&json!({"model":"public-model","prompt":"hi","stream":true})).unwrap();
    let fixtures = [
        (
            ProxyEndpoint::Chat,
            stream_request(),
            sse(
                &[delta("hello", json!(null)), delta(" world", json!("stop"))],
                true,
            ),
        ),
        (
            ProxyEndpoint::Completions,
            completion,
            sse(
                &[
                    json!({"id":"c","choices":[{"index":0,"text":"hello","finish_reason":null}]}),
                    json!({"id":"c","choices":[{"index":0,"text":" world","finish_reason":"stop"}]}),
                ],
                true,
            ),
        ),
        (
            ProxyEndpoint::Responses,
            response_fixture::body(),
            sse(&response_fixture::flow(r#"{"city":"Paris"}"#), false),
        ),
    ];
    for (endpoint, bytes, body) in fixtures {
        let backend = backend_raw(200, body, "text/event-stream", Duration::ZERO).await;
        let mut p = Paid::start(&backend.base, endpoint, ProxyRail::Tap, &bytes, true).await;
        let (provider, signer) = authorities(&mut p).await;
        let b = buyer(&p);
        b.refresh(&p.authorization, 1).await.unwrap();
        let mut chunks = 0;
        let result = p
            .executor
            .execute_stream(
                &p.record.invocation,
                &bytes,
                &Cancellation::default(),
                |_| {
                    chunks += 1;
                    async { Ok(()) }
                },
            )
            .await
            .unwrap();
        assert!(chunks > 0);
        let offer = p
            .executor
            .sign_terminal_receipt(&provider, &p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        b.approve_receipt(
            &p.verifier(),
            offer,
            snapshot(&p),
            bytes,
            result.reply,
            false,
            2,
        )
        .await
        .unwrap();
        let SignedAcknowledgment::Receipt { receipt } =
            b.sign_approved(&signer, key(&p)).await.unwrap()
        else {
            panic!("receipt expected")
        };
        p.executor
            .retain_terminal_receipt(&p.record.invocation, p.record.attempt, &receipt)
            .await
            .unwrap();
        assert!(p
            .executor
            .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
            .await
            .unwrap());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        p.peer.stop().await;
    }
}
#[tokio::test]
async fn protected_signatures_recount_persist_and_settle_all_endpoints_and_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, answer) in cases() {
            let backend = backend(200, answer, Duration::ZERO).await;
            let mut p = Paid::start(&backend.base, endpoint, rail, &bytes, false).await;
            let (provider, signer) = authorities(&mut p).await;
            let b = buyer(&p);
            b.refresh(&p.authorization, 1).await.unwrap();
            let output = p
                .executor
                .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
                .await
                .unwrap();
            let offer = p
                .executor
                .sign_terminal_receipt(&provider, &p.record.invocation, p.record.attempt)
                .await
                .unwrap();
            let js = p
                .peer
                .command(&json!({"sign_receipt":offer.draft.body}).to_string())
                .await;
            assert_eq!(
                offer.provider_sig, js["provider_sig"],
                "native signature agrees with actual Trac wallet"
            );
            b.approve_receipt(
                &p.verifier(),
                offer,
                snapshot(&p),
                bytes,
                output.reply,
                false,
                5,
            )
            .await
            .unwrap();
            let saved = b.recover(key(&p)).await.unwrap();
            assert!(saved.outcome_approved && !saved.outcome_signed);
            drop(b);
            let b = buyer(&p);
            let signed = b.sign_approved(&signer, key(&p)).await.unwrap();
            let SignedAcknowledgment::Receipt { receipt } = &signed else {
                panic!("receipt expected")
            };
            assert_eq!(receipt.buyer_sig, js["buyer_sig"]);
            p.executor
                .retain_terminal_receipt(&p.record.invocation, p.record.attempt, receipt)
                .await
                .unwrap();
            drop(b);
            let b = buyer(&p);
            assert!(
                b.acknowledgment(key(&p)).await.unwrap() == Some(signed.clone()),
                "restart recovery needs no signing call"
            );
            assert!(p
                .executor
                .publish_terminal_receipt(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert!(matches!(
                b.refresh(&p.authorization, 10).await.unwrap(),
                Some(FinancialOutcome::Paid { .. })
            ));
            assert!(b.sign_approved(&signer, key(&p)).await.unwrap() == signed);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}
#[tokio::test]
async fn protected_waiver_approvals_recover_without_charging_on_all_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for unsent in [false, true] {
            let backend = backend(200, terminal_response("refused"), Duration::ZERO).await;
            let mut p = Paid::start(&backend.base, ProxyEndpoint::Chat, rail, &chat(), false).await;
            let (provider, signer) = authorities(&mut p).await;
            let b = buyer(&p);
            b.refresh(&p.authorization, 1).await.unwrap();
            let reply = if unsent {
                p.executor
                    .cancel_unsent(&p.record.invocation, p.record.attempt)
                    .await
                    .unwrap();
                None
            } else {
                Some(
                    p.executor
                        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
                        .await
                        .unwrap()
                        .reply,
                )
            };
            let offer = p
                .executor
                .sign_waiver(&provider, &p.record.invocation, p.record.attempt)
                .await
                .unwrap();
            let js = p
                .peer
                .command(&json!({"sign_waiver":offer.draft.body}).to_string())
                .await;
            assert_eq!(offer.provider_sig, js["provider_sig"]);
            b.approve_waiver(offer, chat(), reply, unsent, 5)
                .await
                .unwrap();
            drop(b);
            let b = buyer(&p);
            let signed = b.sign_approved(&signer, key(&p)).await.unwrap();
            let SignedAcknowledgment::Waiver { closure } = signed else {
                panic!("waiver expected")
            };
            assert_eq!(closure.buyer_sig, js["buyer_sig"]);
            p.executor
                .retain_waiver(&p.record.invocation, p.record.attempt, &closure)
                .await
                .unwrap();
            assert!(p
                .executor
                .publish_waiver(&p.record.invocation, p.record.attempt)
                .await
                .unwrap());
            assert!(matches!(
                b.refresh(&p.authorization, 10).await.unwrap(),
                Some(FinancialOutcome::Waived { .. })
            ));
            assert_eq!(backend.calls.load(Ordering::SeqCst), usize::from(!unsent));
            p.peer.stop().await;
        }
    }
}
#[tokio::test]
async fn protected_buyer_rejects_bad_evidence_foreign_signer_and_conflicting_approvals() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let mut p = Paid::start(
        &backend.base,
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &chat(),
        false,
    )
    .await;
    let (provider, signer) = authorities(&mut p).await;
    let b = buyer(&p);
    b.refresh(&p.authorization, 1).await.unwrap();
    let output = p
        .executor
        .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
        .await
        .unwrap();
    assert!(
        p.executor
            .sign_terminal_receipt(&signer, &p.record.invocation, p.record.attempt)
            .await
            .is_err(),
        "buyer cannot sign provider role"
    );
    assert!(
        p.journal
            .terminal_draft(&p.record.invocation, p.record.attempt)
            .unwrap()
            .is_none(),
        "wrong wallet must not freeze a provider outcome before rejecting"
    );
    let offer = p
        .executor
        .sign_terminal_receipt(&provider, &p.record.invocation, p.record.attempt)
        .await
        .unwrap();
    let mut wrong = copy_reply(&output.reply);
    wrong.body["choices"][0]["message"]["content"] = json!("not what I received");
    assert!(b
        .approve_receipt(
            &p.verifier(),
            offer.clone(),
            snapshot(&p),
            chat(),
            wrong,
            false,
            2
        )
        .await
        .is_err());
    assert!(!b.recover(key(&p)).await.unwrap().outcome_approved);
    let mut alternative = offer.clone();
    alternative.draft.body.at_ms += 1;
    let js = p
        .peer
        .command(&json!({"sign_receipt":alternative.draft.body}).to_string())
        .await;
    alternative.provider_sig = js["provider_sig"].as_str().unwrap().into();
    let verifier = p.verifier();
    let (first, second) = tokio::join!(
        b.approve_receipt(
            &verifier,
            offer,
            snapshot(&p),
            chat(),
            copy_reply(&output.reply),
            false,
            3
        ),
        b.approve_receipt(
            &verifier,
            alternative,
            snapshot(&p),
            chat(),
            copy_reply(&output.reply),
            false,
            4
        )
    );
    assert_ne!(
        first.is_ok(),
        second.is_ok(),
        "one immutable buyer intent must win"
    );
    assert!(
        b.sign_approved(&provider, key(&p)).await.is_err(),
        "provider cannot sign buyer role"
    );
    let seeds = p.peer.command("ephemeral_test_wallet_seeds").await;
    let seed: [u8; 32] = serde_json::from_value(seeds["buyer"].clone()).unwrap();
    let mut wrong_network = buyer_identity(&p);
    wrong_network.network_id = "999".into();
    let foreign =
        Authority::from_unlocked_wallet(SigningKey::from_bytes(&seed), wrong_network).unwrap();
    assert!(
        b.sign_approved(&foreign, key(&p)).await.is_err(),
        "same key on another network must not sign"
    );
    let signed = b.sign_approved(&signer, key(&p)).await.unwrap();
    assert!(b.acknowledgment(key(&p)).await.unwrap() == Some(signed));
    assert_eq!(p.peer.command("status").await["publications"], 0);
    p.peer.stop().await;
}

// A malicious provider owns its signing key and can recompute every public
// commitment. Correct signatures/counts must not substitute for buyer validation.
async fn sign_claim(
    p: &mut Paid,
    mut value: mayhem_proxy::signing::ProviderReceipt,
    request: &[u8],
    reply: &mut mayhem_proxy::endpoint::ProtocolReply,
) -> mayhem_proxy::signing::ProviderReceipt {
    use mayhem_proxy::metering::{Policy, VerifiedSubtotal};
    let binding = p.record.binding.clone();
    let observation = Policy::for_endpoint(binding.endpoint)
        .prepare(binding.endpoint, &serde_json::from_slice(request).unwrap())
        .unwrap()
        .observe(&reply.body)
        .unwrap();
    reply.observed_usage = Some(observation.clone());
    let mut h = blake3::Hasher::new_derive_key("mayhem/proxy/owned-result/v1");
    let record_key = format!("{}:{:020}", p.record.invocation.as_str(), p.record.attempt);
    for part in [
        record_key.into_bytes(),
        serde_json::to_vec(&binding).unwrap(),
        serde_json::to_vec(reply).unwrap(),
    ] {
        h.update(&(part.len() as u64).to_le_bytes());
        h.update(&part);
    }
    let result_digest = Digest::new(h.finalize().to_hex().to_string()).unwrap();
    let amount = p
        .authorization
        .terms
        .offer
        .cost(&observation.units)
        .unwrap();
    let subtotal = VerifiedSubtotal {
        invocation: p.record.invocation.clone(),
        attempt: p.record.attempt,
        binding,
        result_digest: result_digest.clone(),
        observation,
        subtotal_au: amount,
    };
    value.draft.body.result_hash = result_digest.as_str().into();
    value.draft.body.observation_hash = subtotal.digest().unwrap().as_str().into();
    value.draft.body.usage = subtotal.observation.units;
    value.draft.body.au_owed_cum = amount;
    value.draft.body.billing_au_owed_cum = p.authorization.terms.prior_spend_au + amount;
    let signed = p
        .peer
        .command(&json!({"sign_receipt":value.draft.body}).to_string())
        .await;
    value.provider_sig = signed["provider_sig"].as_str().unwrap().into();
    assert!(mayhem_proxy::receipts::verify_signature(
        &value.provider_sig,
        &value.draft.body.provider_signing_bytes().unwrap(),
        &p.authorization.terms.offer.provider_pubkey
    ));
    value
}

#[tokio::test]
async fn protected_buyer_rejects_correctly_signed_and_counted_contract_violations() {
    for kind in ["unknown_tool", "invalid_arguments", "invalid_json_schema"] {
        let mut request: Value = serde_json::from_slice(&chat()).unwrap();
        let mut response = answer();
        if kind == "invalid_json_schema" {
            request["response_format"] = json!({"type":"json_schema","json_schema":{"name":"result","strict":true,
                "schema":{"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}},"additionalProperties":false}}});
            response["choices"][0]["message"]["content"] = json!(r#"{"ok":true}"#);
        } else {
            request["tools"] = json!([{"type":"function","function":{"name":"read_file",
                "parameters":{"type":"object","required":["path"],"properties":{"path":{"type":"string"}},"additionalProperties":false}}}]);
            response["choices"][0]["finish_reason"] = json!("tool_calls");
            response["choices"][0]["message"]["tool_calls"] = json!([{"id":"call_1","type":"function",
                "function":{"name":"read_file","arguments":r#"{"path":"readme.md"}"#}}]);
        }
        let bytes = serde_json::to_vec(&request).unwrap();
        let backend = backend(200, response, Duration::ZERO).await;
        let mut p = Paid::start(
            &backend.base,
            ProxyEndpoint::Chat,
            ProxyRail::Fiat,
            &bytes,
            false,
        )
        .await;
        let (provider, buyer_signer) = authorities(&mut p).await;
        let b = buyer(&p);
        b.refresh(&p.authorization, 1).await.unwrap();
        let original = p
            .executor
            .execute_json(&p.record.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap();
        let claim = p
            .executor
            .sign_terminal_receipt(&provider, &p.record.invocation, p.record.attempt)
            .await
            .unwrap();
        let mut forged = copy_reply(&original.reply);
        match kind {
            "unknown_tool" => {
                forged.body["choices"][0]["message"]["tool_calls"][0]["function"]["name"] =
                    json!("delete_all")
            }
            "invalid_arguments" => {
                forged.body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] =
                    json!(r#"{"path":3}"#)
            }
            _ => forged.body["choices"][0]["message"]["content"] = json!(r#"{"ok":"yes"}"#),
        }
        let forged_claim = sign_claim(&mut p, claim.clone(), &bytes, &mut forged).await;
        let error = b
            .approve_receipt(
                &p.verifier(),
                forged_claim,
                snapshot(&p),
                bytes.clone(),
                forged,
                false,
                2,
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("endpoint contract"),
            "{kind}: {error}"
        );
        assert!(!b.recover(key(&p)).await.unwrap().outcome_approved);
        assert!(b.sign_approved(&buyer_signer, key(&p)).await.is_err());
        // The adversarial commitment constructor also matches a real, valid
        // receipt. This prevents an unrelated broken hash from passing the test.
        let mut valid = copy_reply(&original.reply);
        let valid_claim = sign_claim(&mut p, claim.clone(), &bytes, &mut valid).await;
        assert!(valid_claim == claim);
        b.approve_receipt(
            &p.verifier(),
            valid_claim,
            snapshot(&p),
            bytes,
            valid,
            false,
            3,
        )
        .await
        .unwrap();
        b.sign_approved(&buyer_signer, key(&p)).await.unwrap();
        assert_eq!(p.peer.command("status").await["publications"], 0);
        p.peer.stop().await;
    }
}
#[tokio::test]
async fn protected_expiry_signature_uses_original_saved_policy_and_survives_restart() {
    use mayhem_proto::proxy::finance::ProxyReservationExpiry;
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let policy = json!({"schema_version":1,"lane":"proxy","payable_outcomes":["complete"],"allow_checkpoints":false,
            "hold_expiry":"release_unfinalized_and_block_retry"});
        let mut p = Paid::start_with_policy(
            &backend.base,
            ProxyEndpoint::Chat,
            rail,
            &chat(),
            false,
            Some(policy),
        )
        .await;
        let (provider, signer) = authorities(&mut p).await;
        let b = buyer(&p);
        b.refresh(&p.authorization, 1).await.unwrap();
        let epoch = p.authorization.terms.reservation_expires_after_epoch
            + p.authorization.terms.reservation_receipt_grace_epochs
            + 1;
        p.peer.command(&json!({"epoch":epoch}).to_string()).await;
        let body = b.prepare_expiry(&p.authorization, 2).await.unwrap();
        drop(b);
        let b = buyer(&p);
        assert!(b.sign_expiry(&provider, key(&p)).await.is_err());
        let expiry = b.sign_expiry(&signer, key(&p)).await.unwrap();
        let js = p
            .peer
            .command(&json!({"sign_expiry":body}).to_string())
            .await;
        assert_eq!(expiry.buyer_sig, js["buyer_sig"]);
        drop(b);
        let b = buyer(&p);
        let saved: ProxyReservationExpiry = b.recover(key(&p)).await.unwrap().signed.unwrap();
        assert_eq!(saved, expiry);
        assert!(matches!(
            b.publish_expiry(key(&p), 3).await.unwrap(),
            Some(FinancialOutcome::ExpiredUnknown { .. })
        ));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        p.peer.stop().await;
    }
}
