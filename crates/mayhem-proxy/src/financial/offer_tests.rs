//! Local trusted-peer transport double; canonical signature verification remains
//! covered by the real signed RPC integration tests in tests/support/offer_state.rs.
use super::*;
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, Mutex},
};

enum Reply {
    Status(u16, Duration),
    Valid,
    Invalid(&'static str),
    DelayedBody(Duration),
    Disconnect,
}

struct Request {
    body: Value,
    at: Instant,
}

struct Fixture {
    client: Arc<Client>,
    query: Query,
    terms: ProxySpendTerms,
    policy: ProxySettlementPolicy,
    requests: Arc<Mutex<Vec<Request>>>,
    received: mpsc::UnboundedReceiver<()>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(replies: Vec<Reply>, capacity: usize) -> Self {
        let source: Value = serde_json::from_str(include_str!(
            "../../../mayhem-proto/tests/fixtures/proxy-finance-v1.json"
        ))
        .unwrap();
        let case = source["cases"][0].clone();
        let terms: ProxySpendTerms = serde_json::from_value(case["terms"].clone()).unwrap();
        let policy = serde_json::from_value(case["policy"].clone()).unwrap();
        let query = Query {
            offer: terms.offer.clone(),
            rail: terms.rail,
            settlement_policy_hash: terms.settlement_policy_hash.clone(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(
            Client::new(
                &format!("http://{}/v1", listener.local_addr().unwrap()),
                Identity {
                    network_id: terms.network_id.clone(),
                    msb_bootstrap: terms.msb_bootstrap.clone(),
                    subnet_bootstrap: terms.subnet_bootstrap.clone(),
                    contract_version: terms.contract_version,
                },
                terms.offer.provider_pubkey.clone(),
                capacity,
            )
            .unwrap(),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let (received_tx, received) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (mut socket, _) = connection.unwrap();
                        let seen = seen.clone();
                        let replies = replies.clone();
                        let case = case.clone();
                        let received_tx = received_tx.clone();
                        handlers.spawn(async move {
                            let mut bytes = Vec::new();
                            let request: Value = loop {
                                let mut buffer = [0; 4096];
                                let n = socket.read(&mut buffer).await.unwrap();
                                assert!(n > 0);
                                bytes.extend_from_slice(&buffer[..n]);
                                assert!(bytes.len() < MAX_BYTES);
                                if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                                    // Every attempt must remain an offer observation: no
                                    // publication, negotiation or model execution route.
                                    assert_eq!(headers.lines().next(), Some("post /v1/proxy/offer-state http/1.1"));
                                    let length: usize = headers.lines()
                                        .find_map(|line| line.strip_prefix("content-length: "))
                                        .unwrap().parse().unwrap();
                                    if bytes.len() >= end + 4 + length {
                                        break serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                                    }
                                }
                            };
                            seen.lock().await.push(Request { body: request.clone(), at: Instant::now() });
                            received_tx.send(()).unwrap();
                            let reply = replies.lock().await.pop_front().expect("unexpected extra offer read");
                            let mut value = wire(&case, &request);
                            let mut status = 200;
                            let mut body_delay = Duration::ZERO;
                            match reply {
                                Reply::Status(code, delay) => {
                                    tokio::time::sleep(delay).await;
                                    status = code;
                                    value = json!({"error":"Proxy offer state is unavailable; refresh the same observation.","private":"must not escape"});
                                }
                                Reply::Valid => {}
                                Reply::Invalid(field) => match field {
                                    "proof" => value["proof"]["signed_length"] = json!(0),
                                    "nonce" => value["request_nonce"] = json!("0".repeat(64)),
                                    "rail" => value["rail"] = json!("tnk"),
                                    "policy" => value["settlement_policy"]["allow_checkpoints"] = json!(false),
                                    _ => unreachable!(),
                                },
                                Reply::DelayedBody(delay) => body_delay = delay,
                                Reply::Disconnect => return,
                            }
                            let body = serde_json::to_vec(&value).unwrap();
                            let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            socket.write_all(header.as_bytes()).await.unwrap();
                            tokio::time::sleep(body_delay).await;
                            let _ = socket.write_all(&body).await;
                        });
                    }
                    Some(result) = handlers.join_next() => result.unwrap(),
                }
            }
        });
        Self {
            client,
            query,
            terms,
            policy,
            requests,
            received,
            server,
        }
    }

    async fn next_request(&mut self) {
        tokio::time::timeout(Duration::from_secs(2), self.received.recv())
            .await
            .unwrap()
            .unwrap();
    }
}

fn wire(case: &Value, request: &Value) -> Value {
    let terms = &case["terms"];
    let mut value = json!({
        "ok":true,"schema_version":1,"lane":"proxy", "requester":terms["offer"]["provider_pubkey"],
        "request_nonce":request["request_nonce"],"offer":request["offer"],
        "rail":request["rail"],"settlement_policy_hash":request["settlement_policy_hash"],
        "context":{"network_id":terms["network_id"],"msb_bootstrap":terms["msb_bootstrap"],
            "subnet_bootstrap":terms["subnet_bootstrap"],"contract_version":terms["contract_version"],
            "epoch":terms["billing_epoch"].as_u64().unwrap()-1},
        "proof":{"view_key":"a".repeat(64),"tree_hash":"b".repeat(64),"signed_length":10,"fork":0},
        "billing_epoch":terms["billing_epoch"],"market":case["market"],"membership":case["membership"],
        "settlement_policy":case["policy"],"payout_revision":terms["payout_revision"],
        "payment_terms_hash":terms["payment_terms_hash"],"rules_ver":terms["rules_ver"],
    });
    if request["follow_rates"] == true {
        value["follow_rates"] = json!(true);
        value["current_offer"] = request["offer"].clone();
    }
    value
}

fn assert_same_queries(requests: &[Request], query: &Query, follow_rates: bool) {
    let mut nonces = std::collections::HashSet::new();
    for request in requests {
        let mut body = request.body.clone();
        let nonce = body
            .as_object_mut()
            .unwrap()
            .remove("request_nonce")
            .unwrap();
        assert!(hex(nonce.as_str().unwrap()));
        assert!(nonces.insert(nonce.as_str().unwrap().to_owned()));
        if follow_rates {
            assert_eq!(
                body.as_object_mut().unwrap().remove("follow_rates"),
                Some(json!(true))
            );
        }
        assert_eq!(body, serde_json::to_value(query).unwrap());
    }
}

#[tokio::test]
async fn snapshot_race_retries_only_reads_with_fresh_challenges_and_original_age() {
    for follow_rates in [false, true] {
        let f = Fixture::new(vec![Reply::Status(500, Duration::ZERO), Reply::Valid], 1).await;
        let observation = f
            .client
            .observe_offer(&f.query, follow_rates)
            .await
            .unwrap();
        observation.check_terms(&f.terms, &f.policy).unwrap();
        assert_eq!(observation.offer().unwrap(), &f.query.offer);
        let requests = f.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_same_queries(&requests, &f.query, follow_rates);
        assert!(requests[1].at.duration_since(requests[0].at) >= Duration::from_millis(250));
        assert!(
            observation.started <= requests[0].at,
            "retry cannot renew the observation age"
        );
        assert_eq!(f.client.slots.available_permits(), 1);
    }
}

#[tokio::test]
async fn temporary_http_errors_stop_after_three_attempts_and_keep_safe_status() {
    let f = Fixture::new(
        vec![
            Reply::Status(429, Duration::ZERO),
            Reply::Status(503, Duration::ZERO),
            Reply::Status(500, Duration::ZERO),
            Reply::Valid,
        ],
        1,
    )
    .await;
    let error = f.client.offer_state(&f.query).await.err().unwrap();
    assert!(
        matches!(&error, Error::Http { status: 500, code } if code == "canonical_provider_offer_observation_unavailable")
    );
    assert!(!error.to_string().contains("must not escape"));
    let requests = f.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_same_queries(&requests, &f.query, false);
    assert!(requests[1].at.duration_since(requests[0].at) >= Duration::from_millis(250));
    assert!(requests[2].at.duration_since(requests[1].at) >= Duration::from_millis(500));
    assert_eq!(f.client.slots.available_permits(), 1);
}

#[tokio::test]
async fn permanent_http_errors_invalid_successes_and_transport_failures_never_retry() {
    for status in [400, 401, 403, 404, 409, 422] {
        let f = Fixture::new(vec![Reply::Status(status, Duration::ZERO), Reply::Valid], 1).await;
        assert!(
            matches!(f.client.offer_state(&f.query).await, Err(Error::Http { status: got, .. }) if got == status)
        );
        assert_eq!(f.requests.lock().await.len(), 1);
    }
    for field in ["proof", "nonce", "rail", "policy"] {
        let f = Fixture::new(vec![Reply::Invalid(field), Reply::Valid], 1).await;
        assert!(
            matches!(f.client.offer_state(&f.query).await, Err(Error::Invalid(_))),
            "{field}"
        );
        assert_eq!(f.requests.lock().await.len(), 1, "{field}");
    }
    let f = Fixture::new(vec![Reply::Disconnect, Reply::Valid], 1).await;
    assert!(matches!(
        f.client.offer_state(&f.query).await,
        Err(Error::Transport(_))
    ));
    assert_eq!(f.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn retry_holds_one_permit_through_backoff_and_releases_on_cancellation() {
    let mut f = Fixture::new(vec![Reply::Status(500, Duration::ZERO), Reply::Valid], 1).await;
    let client = f.client.clone();
    let query = f.query.clone();
    let operation = tokio::spawn(async move { client.offer_state(&query).await });
    f.next_request().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(f.client.slots.available_permits(), 0);
    assert!(matches!(
        f.client.offer_state(&f.query).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(f.requests.lock().await.len(), 1);
    operation.await.unwrap().unwrap();
    assert_eq!(f.requests.lock().await.len(), 2);
    assert_eq!(f.client.slots.available_permits(), 1);

    let mut f = Fixture::new(vec![Reply::Status(500, Duration::from_secs(5))], 1).await;
    let client = f.client.clone();
    let query = f.query.clone();
    let operation = tokio::spawn(async move { client.offer_state(&query).await });
    f.next_request().await;
    operation.abort();
    assert!(matches!(operation.await, Err(error) if error.is_cancelled()));
    assert_eq!(f.client.slots.available_permits(), 1);
}

#[tokio::test]
async fn two_concurrent_offer_reads_recover_without_exceeding_shared_capacity() {
    let f = Fixture::new(
        vec![
            Reply::Status(500, Duration::ZERO),
            Reply::Status(500, Duration::ZERO),
            Reply::Valid,
            Reply::Valid,
        ],
        2,
    )
    .await;
    let (a, b) = tokio::join!(
        f.client.offer_state(&f.query),
        f.client.offer_state(&f.query)
    );
    for observation in [a.unwrap(), b.unwrap()] {
        observation.check_terms(&f.terms, &f.policy).unwrap();
    }
    let requests = f.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert_same_queries(&requests, &f.query, false);
    assert_eq!(f.client.slots.available_permits(), 2);
}

#[tokio::test]
async fn retries_and_response_body_share_the_original_fifteen_second_deadline() {
    let f = Fixture::new(
        vec![
            Reply::Status(500, Duration::from_secs(6)),
            Reply::DelayedBody(Duration::from_secs(10)),
            Reply::Valid,
        ],
        1,
    )
    .await;
    let started = Instant::now();
    assert!(matches!(f.client.offer_state(&f.query).await,
        Err(Error::Http { status: 504, code }) if code == "canonical_provider_offer_observation_timeout"));
    assert!(started.elapsed() >= FRESHNESS);
    assert!(started.elapsed() < FRESHNESS + Duration::from_secs(1));
    assert_eq!(f.requests.lock().await.len(), 2);
    assert_eq!(f.client.slots.available_permits(), 1);
}
