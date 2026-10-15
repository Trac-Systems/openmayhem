//! Local HTTP faults against the real signed reservation publication boundary.
use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

enum Reply {
    Status(u16, Duration),
    ErrorCode(u16, &'static str),
    Financial,
    Admitted,
    BadProof,
    Ack,
    Json(Value),
    InvalidJson,
    Truncated,
    LostAck,
}
type Requests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn financial_wire(
    auth: &ProxySpendAuthorization,
    policy: &ProxySettlementPolicy,
    request: &Value,
) -> Value {
    let t = &auth.terms;
    let digest = t.digest().unwrap();
    let mut session = json!({
        "billing_id":t.billing_id,"billing_attempt":t.billing_attempt,"billing_epoch":t.billing_epoch,
        "session_id":t.session_id,"reservation_id":t.reservation_id,"user":t.buyer_pubkey,
        "provider":t.offer.provider_pubkey,"payout_revision":t.payout_revision,"rail":t.rail,
        "lane":"proxy","accepted_terms":digest,
    });
    let mut reservation = session.clone();
    reservation["type"] = json!("receipt_reservation_identity");
    reservation["status"] = json!("active");
    session["type"] = json!("targeted_spend_session");
    session["settlement_ready"] = json!(false);
    session["max_spend_au"] = json!(t.max_spend_au.to_string());
    session["closed_at"] = Value::Null;
    session["authorization"] = json!(auth);
    session["settlement_policy"] = json!(policy);
    json!({
        "ok":true,"schema_version":1,"lane":"proxy","requester":t.buyer_pubkey,
        "request_nonce":request["request_nonce"],"accepted_terms":digest,
        "context":{"network_id":t.network_id,"msb_bootstrap":t.msb_bootstrap,"subnet_bootstrap":t.subnet_bootstrap,"contract_version":t.contract_version,"epoch":t.billing_epoch-1},
        "proof":{"view_key":"a".repeat(64),"tree_hash":"b".repeat(64),"signed_length":10,"fork":0},
        "accepted":{"type":"proxy_accepted_spend","accepted_terms":digest,"authorization":auth,
            "settlement_policy":policy,"max_checkpoints":0,"recorded_at":format!("proxy/spend/{digest}"),
            "result":{"ok":true,"op":"proxySpendReserve","accepted_terms":digest}},
        "session":session,"reservation":reservation,"receipt_head":null,"resolution":null,"expiry":null,
        "billing":{"type":"proxy_billing_anchor","lane":"proxy","billing_id":t.billing_id,
            "user":t.buyer_pubkey,"rail":t.rail,"request_hash":t.request_hash,"endpoint_contract":t.endpoint_contract,
            "latest_attempt":t.billing_attempt,"latest_accepted_terms":digest,"active_reservation_id":t.reservation_id,
            "retry_blocked":false,"spent_au":t.prior_spend_au.to_string(),"reserved_au":t.max_spend_au.to_string(),
            "max_total_spend_au":t.max_total_spend_au.to_string()},
    })
}

struct Fixture {
    client: Arc<Client>,
    authorization: ProxySpendAuthorization,
    policy: ProxySettlementPolicy,
    requests: Requests,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(replies: Vec<Reply>, request_timeout: Option<Duration>) -> Self {
        Self::with_observations(replies, request_timeout, vec![]).await
    }
    async fn with_observations(
        replies: Vec<Reply>,
        request_timeout: Option<Duration>,
        observations: Vec<Reply>,
    ) -> Self {
        let source: Value = serde_json::from_str(include_str!(
            "../../../mayhem-proto/tests/fixtures/proxy-finance-v1.json"
        ))
        .unwrap();
        let case = &source["cases"][0];
        let mut terms: mayhem_proto::proxy::finance::ProxySpendTerms =
            serde_json::from_value(case["terms"].clone()).unwrap();
        let buyer = SigningKey::from_bytes(&[17; 32]);
        let provider = SigningKey::from_bytes(&[29; 32]);
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        terms.buyer_pubkey = hex(buyer.verifying_key().as_bytes());
        terms.offer.provider_pubkey = hex(provider.verifying_key().as_bytes());
        let authorization = ProxySpendAuthorization {
            buyer_sig: hex(&buyer.sign(&terms.buyer_signing_bytes().unwrap()).to_bytes()),
            provider_sig: hex(&provider
                .sign(&terms.provider_signing_bytes().unwrap())
                .to_bytes()),
            terms,
        };
        authorization
            .verify(crate::receipts::verify_signature)
            .unwrap();
        let t = &authorization.terms;
        let identity = Identity {
            network_id: t.network_id.clone(),
            msb_bootstrap: t.msb_bootstrap.clone(),
            subnet_bootstrap: t.subnet_bootstrap.clone(),
            contract_version: t.contract_version,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = Client::new(
            &format!("http://{}/v1", listener.local_addr().unwrap()),
            identity,
            t.buyer_pubkey.clone(),
            2,
        )
        .unwrap();
        if let Some(timeout) = request_timeout {
            client.http = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .build()
                .unwrap();
        }
        let requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let observations = Arc::new(Mutex::new(VecDeque::from(observations)));
        let observed_auth = authorization.clone();
        let policy: ProxySettlementPolicy = serde_json::from_value(case["policy"].clone()).unwrap();
        let observed_policy = policy.clone();
        let server = tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (mut socket, _) = connection.unwrap();
                        let seen = seen.clone();
                        let replies = replies.clone();
                        let observations = observations.clone();
                        let authorization = observed_auth.clone();
                        let policy = observed_policy.clone();
                        handlers.spawn(async move {
                            let mut bytes = Vec::new();
                            let mut buffer = [0; 4096];
                            let (path, body) = loop {
                                let n = socket.read(&mut buffer).await.unwrap();
                                assert!(n > 0);
                                bytes.extend_from_slice(&buffer[..n]);
                                assert!(bytes.len() < 131072);
                                if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                                    let mut line = headers.lines().next().unwrap().split_whitespace();
                                    assert_eq!(line.next(), Some("post"));
                                    let path = line.next().unwrap().to_owned();
                                    assert!(["/v1/contract/feature", "/v1/proxy/financial-state", "/v1/proxy/intent-state"].contains(&path.as_str()));
                                    let length: usize = headers.lines().find_map(|l| l.strip_prefix("content-length: "))
                                        .unwrap().parse().unwrap();
                                    if bytes.len() >= end + 4 + length {
                                        break (path, bytes[end + 4..end + 4 + length].to_vec());
                                    }
                                }
                            };
                            let request: Value = serde_json::from_slice(&body).unwrap();
                            seen.lock().await.push((path.clone(), body));
                            let reply = if path == "/v1/contract/feature" {
                                replies.lock().await.pop_front().unwrap_or(Reply::Ack)
                            } else {
                                // An HTTP success is deliberately not a canonical proof.
                                observations.lock().await.pop_front().unwrap_or(Reply::Json(json!({"ok": true})))
                            };
                            let mut status = 200;
                            let body = match reply {
                                Reply::Status(code, delay) => {
                                    status = code;
                                    tokio::time::sleep(delay).await;
                                    serde_json::to_vec(&json!({"error":"private upstream detail"})).unwrap()
                                }
                                Reply::ErrorCode(code, reason) => {
                                    status = code;
                                    serde_json::to_vec(&json!({"code":reason,"error":"private exception must not escape"})).unwrap()
                                }
                                Reply::Financial | Reply::BadProof => {
                                    let mut value = financial_wire(&authorization, &policy, &request);
                                    if matches!(reply, Reply::BadProof) { value["proof"]["signed_length"] = json!(0); }
                                    serde_json::to_vec(&value).unwrap()
                                }
                                Reply::Admitted => {
                                    let t = &authorization.terms;
                                    serde_json::to_vec(&json!({
                                        "ok":true,"schema_version":1,"lane":"proxy","requester":t.buyer_pubkey,
                                        "request_nonce":request["request_nonce"],"intent":request["intent"],
                                        "accepted_terms":t.digest().unwrap(),"status":"admitted","authorization":authorization,
                                        "context":{"network_id":t.network_id,"msb_bootstrap":t.msb_bootstrap,"subnet_bootstrap":t.subnet_bootstrap,"contract_version":t.contract_version,"epoch":t.billing_epoch-1},
                                        "proof":{"view_key":"a".repeat(64),"tree_hash":"b".repeat(64),"signed_length":10,"fork":0},
                                    })).unwrap()
                                }
                                Reply::Ack => serde_json::to_vec(&json!({"accepted":true,"status":"pending"})).unwrap(),
                                Reply::Json(value) => serde_json::to_vec(&value).unwrap(),
                                Reply::InvalidJson => b"{invalid-json".to_vec(),
                                Reply::Truncated => {
                                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{\"accepted\":").await.unwrap();
                                    socket.shutdown().await.unwrap();
                                    return;
                                }
                                Reply::LostAck => {
                                    // The complete signed body was received, but no ACK arrives
                                    // before the client's transport timeout.
                                    tokio::time::sleep(Duration::from_secs(2)).await;
                                    return;
                                }
                            };
                            let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                            if socket.write_all(header.as_bytes()).await.is_ok() {
                                let _ = socket.write_all(&body).await;
                            }
                            let _ = socket.shutdown().await;
                        });
                    }
                    completed = handlers.join_next(), if !handlers.is_empty() => { completed.unwrap().unwrap(); }
                }
            }
        });
        Self {
            client: Arc::new(client),
            authorization,
            policy,
            requests,
            server,
        }
    }

    async fn submit(&self) -> Result<()> {
        self.client
            .submit_reservation(&self.authorization, 1234)
            .await
    }

    async fn assert_publications(&self, count: usize) {
        let requests = self.requests.lock().await;
        let bodies: Vec<_> = requests
            .iter()
            .filter(|(path, _)| path == "/v1/contract/feature")
            .map(|(_, bytes)| bytes)
            .collect();
        assert_eq!(bodies.len(), count);
        let expected = serde_json::to_vec(&json!({"feature":"mayhem",
            "key":format!("proxy/spend/{}",self.authorization.terms.digest().unwrap()),
            "value":{"op":"proxy_spend_reserve","authorization":self.authorization,"at":1234}}))
        .unwrap();
        for body in bodies {
            assert_eq!(
                body, &expected,
                "every byte, signature and original timestamp must be retained"
            );
        }
        assert!(
            !self.server.is_finished(),
            "mock must preserve allowed publication/read-only paths"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn statuses_retry_identical_signed_bytes_and_accept_only_acknowledgment() {
    let f = Fixture::new(
        vec![
            Reply::Status(500, Duration::ZERO),
            Reply::Status(429, Duration::ZERO),
            Reply::Ack,
        ],
        None,
    )
    .await;
    f.submit().await.unwrap();
    f.assert_publications(3).await;
}

#[tokio::test]
async fn lost_ack_and_truncated_reply_retry_original_signed_envelope() {
    for (failure, timeout) in [
        (Reply::LostAck, Some(Duration::from_millis(100))),
        (Reply::Truncated, None),
    ] {
        let f = Fixture::new(vec![failure, Reply::Ack], timeout).await;
        f.submit().await.unwrap();
        f.assert_publications(2).await;
    }
}

#[tokio::test]
async fn permanent_status_denial_and_invalid_json_are_never_retried() {
    for status in [302, 400, 401, 403, 404, 409, 422] {
        let f = Fixture::new(
            vec![Reply::Status(status, Duration::ZERO), Reply::Ack],
            None,
        )
        .await;
        let error = f.submit().await.unwrap_err();
        assert!(matches!(&error, Error::Http { status: actual, .. } if *actual == status));
        assert!(!error.to_string().contains("private upstream detail"));
        f.assert_publications(1).await;
    }
    for reply in [
        Reply::InvalidJson,
        Reply::Json(
            json!({"ok":false,"accepted":false,"status":"rejected","message":"invalid signature"}),
        ),
        Reply::Json(json!({"ok":true,"accepted":true,"result":{"ok":false}})),
        Reply::Json(
            json!({"ok":false,"accepted":false,"status":"rejected","message":"insufficient funds"}),
        ),
    ] {
        let f = Fixture::new(vec![reply, Reply::Ack], None).await;
        assert!(f.submit().await.is_err());
        f.assert_publications(1).await;
    }
}

#[tokio::test]
async fn only_existing_exact_semantic_backpressure_is_retried() {
    let message =
        "Proxy publication journal: admission dependencies changed; retry the same operation.";
    for (message, expected) in [(message.to_owned(), 2), (format!("{message} extra"), 1)] {
        let f = Fixture::new(
            vec![
                Reply::Json(
                    json!({"ok":false,"accepted":false,"status":"rejected","message":message}),
                ),
                Reply::Ack,
            ],
            None,
        )
        .await;
        assert_eq!(f.submit().await.is_ok(), expected == 2);
        f.assert_publications(expected).await;
    }
}

#[tokio::test]
async fn publication_attempts_remain_bounded_and_backed_off() {
    let f = Fixture::new(
        (0..11)
            .map(|_| Reply::Status(503, Duration::ZERO))
            .collect(),
        None,
    )
    .await;
    let started = Instant::now();
    assert!(f.submit().await.is_err());
    assert!(started.elapsed() >= Duration::from_millis(7500));
    f.assert_publications(10).await;
}

#[tokio::test]
async fn publication_deadline_includes_prior_attempt_and_backoff() {
    let f = Fixture::new(
        vec![
            Reply::Status(500, Duration::from_secs(2)),
            Reply::Status(500, Duration::from_secs(30)),
        ],
        None,
    )
    .await;
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_millis(11500), f.submit())
        .await
        .expect("retries cannot renew the original ten-second publication budget");
    assert!(result.is_err());
    assert!(started.elapsed() >= Duration::from_secs(9));
    f.assert_publications(2).await;
}

#[tokio::test]
async fn recovered_publication_ack_cannot_confirm_reservation_without_canonical_proof() {
    use recovery::{BuyerRecovery, Limits, Store};
    let f = Fixture::new(vec![Reply::Status(500, Duration::ZERO), Reply::Ack], None).await;
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let t = &f.authorization.terms;
    let identity = crate::attempts::Identity {
        network_id: t.network_id.clone(),
        msb_bootstrap: Digest::new(&t.msb_bootstrap).unwrap(),
        subnet_bootstrap: Digest::new(&t.subnet_bootstrap).unwrap(),
        controller_pubkey: Digest::new(&t.buyer_pubkey).unwrap(),
    };
    let store = Arc::new(
        Store::open(
            root.path().join("recovery.redb"),
            identity,
            Limits {
                max_records: 2,
                closed_retention_ms: 1000,
            },
        )
        .unwrap(),
    );
    let recovery = BuyerRecovery::new(store, f.client.clone(), 1).unwrap();
    let key = recovery
        .retain_reservation(f.authorization.clone(), f.policy.clone(), 1234)
        .await
        .unwrap();
    assert!(recovery
        .publish_reservation_wait(key.clone(), 1235)
        .await
        .is_err());
    let status = recovery.recover(key).await.unwrap();
    assert_eq!(status.authorization, f.authorization);
    let reservation = status.reservation.unwrap();
    assert_eq!(reservation.at, 1234);
    assert!(!reservation.confirmed);
    assert!(!reservation.expired_unadmitted);
    f.assert_publications(2).await;
    let requests = f.requests.lock().await;
    assert_eq!(
        requests
            .iter()
            .filter(|(path, _)| path == "/v1/proxy/financial-state")
            .count(),
        1,
        "publication ACK must be followed by an independently validated canonical read"
    );
}

#[tokio::test]
async fn financial_error_codes_retry_only_transients_and_never_expose_peer_messages() {
    for code in [
        "proxy_financial_pending",
        "proxy_financial_busy",
        "proxy_financial_unavailable",
        "proxy_financial_snapshot_changed",
        "proxy_financial_unknown",
        "proxy_financial_invalid",
        "proxy_financial_rejected",
        "private_unsupported_code",
    ] {
        let transient = matches!(
            code,
            "proxy_financial_pending"
                | "proxy_financial_busy"
                | "proxy_financial_unavailable"
                | "proxy_financial_snapshot_changed"
                | "proxy_financial_unknown"
        );
        // Even a permanent failure incorrectly delivered as 503 must fail once.
        let f = Fixture::with_observations(
            vec![],
            None,
            (0..3).map(|_| Reply::ErrorCode(503, code)).collect(),
        )
        .await;
        let error = f.client.observe(&f.authorization).await.err().unwrap();
        assert_eq!(
            diagnostic_code(&error),
            financial_code(code).unwrap_or("proxy_financial_invalid")
        );
        assert!(!format!("{error:?}").contains("private"));
        let requests = f.requests.lock().await;
        assert_eq!(requests.len(), if transient { 3 } else { 1 });
        let nonces: std::collections::BTreeSet<_> = requests
            .iter()
            .map(|(_, bytes)| {
                serde_json::from_slice::<Value>(bytes).unwrap()["request_nonce"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(nonces.len(), requests.len());
    }
}

#[tokio::test]
async fn legacy_financial_http_failures_keep_permanent_statuses_permanent() {
    for (status, transient) in [(400, false), (503, true)] {
        let f = Fixture::with_observations(
            vec![],
            None,
            (0..3)
                .map(|_| Reply::Status(status, Duration::ZERO))
                .collect(),
        )
        .await;
        let error = f.client.observe(&f.authorization).await.err().unwrap();
        assert_eq!(observation_retryable(&error), transient);
        assert_eq!(
            diagnostic_code(&error),
            if transient {
                "proxy_financial_unknown"
            } else {
                "proxy_financial_invalid"
            }
        );
        assert!(!format!("{error:?}").contains("private"));
        assert_eq!(f.requests.lock().await.len(), if transient { 3 } else { 1 });
    }
}

#[tokio::test]
async fn bounded_local_financial_capacity_is_transient_and_reuses_original_purchase() {
    let f = Fixture::with_observations(vec![Reply::Ack], None, vec![Reply::Financial]).await;
    let occupied = f.client.slots.acquire_many(2).await.unwrap();
    let offer = negotiation::BuyerOffer {
        terms: f.authorization.terms.clone(),
        buyer_sig: f.authorization.buyer_sig.clone(),
    };
    for error in [
        f.submit().await.err().unwrap(),
        f.client.observe(&f.authorization).await.err().unwrap(),
        f.client.intent_state(&offer).await.err().unwrap(),
    ] {
        assert!(matches!(error, Error::FinancialAdmissionUnavailable));
        assert!(observation_retryable(&error));
        assert_eq!(diagnostic_code(&error), "proxy_financial_unavailable");
    }
    assert!(
        f.requests.lock().await.is_empty(),
        "capacity cannot spawn extra reads"
    );
    drop(occupied);
    f.submit().await.unwrap();
    let observed = f.client.observe(&f.authorization).await.unwrap();
    assert_eq!(observed.accepted().authorization, f.authorization);
    assert_eq!(
        observed.initial_binding().unwrap().accepted_terms.as_str(),
        f.authorization.terms.digest().unwrap()
    );
    f.assert_publications(1).await;
}

async fn recovery_fixture(f: &Fixture, root: &tempfile::TempDir) -> recovery::BuyerRecovery {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let t = &f.authorization.terms;
    let identity = crate::attempts::Identity {
        network_id: t.network_id.clone(),
        msb_bootstrap: Digest::new(&t.msb_bootstrap).unwrap(),
        subnet_bootstrap: Digest::new(&t.subnet_bootstrap).unwrap(),
        controller_pubkey: Digest::new(&t.buyer_pubkey).unwrap(),
    };
    recovery::BuyerRecovery::new(
        Arc::new(
            recovery::Store::open(
                root.path().join("recovery.redb"),
                identity,
                recovery::Limits {
                    max_records: 2,
                    closed_retention_ms: 1000,
                },
            )
            .unwrap(),
        ),
        f.client.clone(),
        1,
    )
    .unwrap()
}

#[tokio::test]
async fn foreground_admitted_intent_requires_another_full_fresh_observation() {
    for malformed in [false, true] {
        let f = Fixture::with_observations(
            vec![Reply::Ack],
            None,
            vec![
                Reply::ErrorCode(503, "proxy_financial_pending"),
                Reply::ErrorCode(503, "proxy_financial_pending"),
                Reply::ErrorCode(503, "proxy_financial_pending"),
                Reply::Admitted,
                if malformed {
                    Reply::BadProof
                } else {
                    Reply::Financial
                },
            ],
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let recovery = recovery_fixture(&f, &root).await;
        let key = recovery
            .retain_reservation(f.authorization.clone(), f.policy.clone(), 1)
            .await
            .unwrap();
        let result = recovery
            .publish_reservation_until(
                key.clone(),
                2,
                tokio::time::Instant::now() + Duration::from_secs(3),
                &AtomicBool::new(false),
            )
            .await;
        if malformed {
            assert_eq!(
                diagnostic_code(&result.err().unwrap()),
                "proxy_financial_invalid"
            );
        } else {
            assert_eq!(
                result.unwrap().initial_binding().unwrap().accepted_terms,
                key
            );
        }
        assert_eq!(
            recovery
                .recover(key)
                .await
                .unwrap()
                .reservation
                .unwrap()
                .confirmed,
            !malformed
        );
        let requests = f.requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            [
                "/v1/contract/feature",
                "/v1/proxy/financial-state",
                "/v1/proxy/financial-state",
                "/v1/proxy/financial-state",
                "/v1/proxy/intent-state",
                "/v1/proxy/financial-state",
            ]
        );
        assert_ne!(
            serde_json::from_slice::<Value>(&requests[3].1).unwrap()["request_nonce"],
            serde_json::from_slice::<Value>(&requests[5].1).unwrap()["request_nonce"]
        );
    }
}

#[tokio::test]
async fn foreground_deadline_includes_publication_and_does_not_renew_for_observation() {
    let f = Fixture::with_observations(
        vec![Reply::Status(503, Duration::from_millis(150)), Reply::Ack],
        None,
        vec![Reply::Financial],
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let recovery = recovery_fixture(&f, &root).await;
    let key = recovery
        .retain_reservation(f.authorization.clone(), f.policy.clone(), 1)
        .await
        .unwrap();
    let started = Instant::now();
    let error = recovery
        .publish_reservation_until(
            key.clone(),
            2,
            tokio::time::Instant::now() + Duration::from_millis(200),
            &AtomicBool::new(false),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(diagnostic_code(&error), "proxy_financial_unknown");
    assert!(started.elapsed() < Duration::from_secs(1));
    let saved = recovery.recover(key).await.unwrap();
    assert_eq!(saved.authorization, f.authorization);
    assert!(!saved.reservation.unwrap().confirmed);
    assert_eq!(f.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn foreground_deadline_keeps_legacy_financial_and_intent_failures_unknown() {
    for (observations, wait, expected) in [
        (
            vec![Reply::ErrorCode(503, "proxy_financial_pending")],
            200,
            "proxy_financial_unavailable",
        ),
        (
            vec![Reply::Status(503, Duration::ZERO)],
            200,
            "proxy_financial_unknown",
        ),
        (
            vec![Reply::ErrorCode(503, "proxy_financial_unknown")],
            200,
            "proxy_financial_unknown",
        ),
        (
            vec![
                Reply::ErrorCode(503, "proxy_financial_pending"),
                Reply::ErrorCode(503, "proxy_financial_pending"),
                Reply::ErrorCode(503, "proxy_financial_pending"),
                Reply::Status(503, Duration::ZERO),
            ],
            1200,
            "proxy_financial_unknown",
        ),
    ] {
        let f = Fixture::with_observations(vec![Reply::Ack], None, observations).await;
        let root = tempfile::tempdir().unwrap();
        let recovery = recovery_fixture(&f, &root).await;
        let key = recovery
            .retain_reservation(f.authorization.clone(), f.policy.clone(), 1234)
            .await
            .unwrap();
        let unknown = AtomicBool::new(false);
        let error = recovery
            .publish_reservation_until(
                key.clone(),
                1235,
                tokio::time::Instant::now() + Duration::from_millis(wait),
                &unknown,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(diagnostic_code(&error), expected);
        assert_eq!(
            diagnostic_code(&admission_timeout(Some(&unknown))),
            expected
        );
        assert!(
            !recovery
                .recover(key)
                .await
                .unwrap()
                .reservation
                .unwrap()
                .confirmed
        );
        f.assert_publications(1).await;
    }
}
