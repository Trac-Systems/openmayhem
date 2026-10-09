use super::*;
use ed25519_dalek::{Signer, SigningKey};

fn sign(key: &[u8; 32], bytes: &[u8]) -> String {
    SigningKey::from_bytes(key)
        .sign(bytes)
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// A valid provider signature for another retained purchase cannot become local
// approval merely because an authenticated provider wrapped it in this session.
#[tokio::test]
async fn exchange_rejects_signed_foreign_purchase_receipts_and_waivers_before_any_approval() {
    for waiver in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let mut p = paid(
            ProxyEndpoint::Chat,
            ProxyRail::Tnk,
            &backend.base,
            &chat(),
            false,
        )
        .await;
        let (provider_signer, _) = authorities(&mut p).await;
        let recovery = buyer_recovery(&p);
        recovery.refresh(&p.authorization, 1).await.unwrap();
        let original = if waiver {
            p.executor
                .cancel_unsent(&p.record.invocation, p.record.attempt)
                .await
                .unwrap();
            Message::Waiver {
                value: p
                    .executor
                    .sign_waiver(&provider_signer, &p.record.invocation, p.record.attempt)
                    .await
                    .unwrap(),
            }
        } else {
            p.executor
                .execute_json(&p.record.invocation, &chat(), &Cancellation::default())
                .await
                .unwrap();
            Message::Receipt {
                value: p
                    .executor
                    .sign_terminal_receipt(&provider_signer, &p.record.invocation, p.record.attempt)
                    .await
                    .unwrap(),
            }
        };
        let seeds = p.peer.command("ephemeral_test_wallet_seeds").await;
        let buyer_key: [u8; 32] = serde_json::from_value(seeds["buyer"].clone()).unwrap();
        let provider_key: [u8; 32] = serde_json::from_value(seeds["provider"].clone()).unwrap();
        let mut other = p.authorization.clone();
        other.terms.billing_id = d(910).as_str().into();
        other.terms.session_id = d(911).as_str().into();
        other.buyer_sig = sign(&buyer_key, &other.terms.buyer_signing_bytes().unwrap());
        other.provider_sig = sign(
            &provider_key,
            &other.terms.provider_signing_bytes().unwrap(),
        );
        other
            .verify(mayhem_proxy::receipts::verify_signature)
            .unwrap();
        let other_key = recovery
            .retain_reservation(
                other.clone(),
                p.peer
                    .buyer_client
                    .observe(&p.authorization)
                    .await
                    .unwrap()
                    .accepted()
                    .settlement_policy
                    .clone(),
                2,
            )
            .await
            .unwrap();
        for split in [false, true] {
            for field in ["purchase", "terms", "invocation"] {
                let auth = if field != "invocation" {
                    other.clone()
                } else {
                    p.authorization.clone()
                };
                let mut buyer_id = p.peer.identity.clone();
                buyer_id.controller_pubkey = Digest::new(&auth.terms.buyer_pubkey).unwrap();
                let buyer_session = Session::new(auth.clone(), &buyer_id, Role::Buyer).unwrap();
                let provider_session =
                    Session::new(auth.clone(), &p.peer.identity, Role::Provider).unwrap();
                let mut message = match &original {
                    Message::Receipt { value } => Message::Receipt {
                        value: value.clone(),
                    },
                    Message::Waiver { value } => Message::Waiver {
                        value: value.clone(),
                    },
                    _ => unreachable!(),
                };
                let (body, provider_sig) = match &mut message {
                    Message::Receipt { value } => {
                        if field != "purchase" {
                            value.draft.invocation = if field == "terms" {
                                buyer_session.invocation().clone()
                            } else {
                                d(912)
                            };
                        }
                        (
                            value.draft.body.provider_signing_bytes().unwrap(),
                            &value.provider_sig,
                        )
                    }
                    Message::Waiver { value } => {
                        if field != "purchase" {
                            value.draft.invocation = if field == "terms" {
                                buyer_session.invocation().clone()
                            } else {
                                d(912)
                            };
                        }
                        (
                            value.draft.body.provider_signing_bytes().unwrap(),
                            &value.provider_sig,
                        )
                    }
                    _ => unreachable!(),
                };
                assert!(mayhem_proxy::receipts::verify_signature(
                    provider_sig,
                    &body,
                    &auth.terms.offer.provider_pubkey
                ));
                let bridge =
                    Bridge::start(&auth.terms.buyer_pubkey, &auth.terms.offer.provider_pubkey)
                        .await;
                let mut provider = Channel::connect(
                    bridge.config(false),
                    provider_session,
                    Limits {
                        max_message_bytes: 1024 * 1024,
                    },
                )
                .await
                .unwrap();
                let mut buyer = Channel::connect(
                    bridge.config(true),
                    buyer_session,
                    Limits {
                        max_message_bytes: 1024 * 1024,
                    },
                )
                .await
                .unwrap();
                let received = if split {
                    let (mut sender, _provider_receiver) = provider.into_duplex().unwrap();
                    let (_buyer_sender, mut receiver) = buyer.into_duplex().unwrap();
                    let (sent, received) = tokio::join!(
                        sender.send(&message),
                        receiver.receive(Some(Duration::from_secs(5)))
                    );
                    sent.unwrap();
                    assert!(
                        matches!(&received, Err(exchange::Error::Identity)),
                        "waiver={waiver}, split={split}, field={field}"
                    );
                    assert!(matches!(
                        receiver.receive(None).await,
                        Err(exchange::Error::Interrupted)
                    ));
                    received
                } else {
                    let (sent, received) = tokio::join!(
                        provider.send(&message),
                        buyer.receive(Some(Duration::from_secs(5)))
                    );
                    sent.unwrap();
                    assert!(
                        matches!(&received, Err(exchange::Error::Identity)),
                        "waiver={waiver}, split={split}, field={field}"
                    );
                    assert!(matches!(
                        buyer.receive(None).await,
                        Err(exchange::Error::Interrupted)
                    ));
                    received
                };
                assert!(
                    matches!(received, Err(exchange::Error::Identity)),
                    "waiver={waiver}, split={split}, field={field}"
                );
                // No Received capability reached verification, the owner's output
                // hook or an approval store for either original purchase.
                for key in [key(&p), other_key.clone()] {
                    let status = recovery.recover(key).await.unwrap();
                    assert!(!status.outcome_approved && !status.outcome_signed);
                }
            }
        }
        assert_eq!(p.peer.command("status").await["publications"], 1);
        assert_eq!(backend.calls.load(Ordering::SeqCst), usize::from(!waiver));
        p.peer.stop().await;
    }
}
