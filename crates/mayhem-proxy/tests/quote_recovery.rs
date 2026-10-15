//! Read-only quote recovery never republishes a purchase or accepts bad evidence.
use mayhem_proxy::{
    discovery::Identity,
    financial::{quote::Query, Client},
    Error,
};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

enum Reply {
    Status(u16, Duration),
    Valid,
    Invalid(&'static str, Value),
    InvalidJson,
    Truncated,
}

struct Fixture {
    client: Client,
    query: Query,
    requests: Arc<Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(replies: Vec<Reply>) -> Self {
        let source: Value = serde_json::from_str(include_str!(
            "../../mayhem-proto/tests/fixtures/proxy-finance-v1.json"
        ))
        .unwrap();
        let case = &source["cases"][0];
        let terms = &case["terms"];
        let query = Query {
            offer: serde_json::from_value(terms["offer"].clone()).unwrap(),
            rail: serde_json::from_value(terms["rail"].clone()).unwrap(),
            settlement_policy_hash: terms["settlement_policy_hash"].as_str().unwrap().into(),
            billing_id: terms["billing_id"].as_str().unwrap().into(),
        };
        let identity = Identity {
            network_id: terms["network_id"].as_str().unwrap().into(),
            msb_bootstrap: terms["msb_bootstrap"].as_str().unwrap().into(),
            subnet_bootstrap: terms["subnet_bootstrap"].as_str().unwrap().into(),
            contract_version: terms["contract_version"].as_u64().unwrap() as u32,
        };
        let mut context = serde_json::to_value(&identity).unwrap();
        context["epoch"] = json!(50);
        let valid = json!({
            "ok": true, "schema_version": 1, "lane": "proxy",
            "requester": terms["buyer_pubkey"], "request_nonce": "",
            "offer": query.offer, "rail": query.rail,
            "settlement_policy_hash": query.settlement_policy_hash, "billing_id": query.billing_id,
            "context": context,
            "proof": {"view_key": "a".repeat(64), "fork": 0, "signed_length": 10, "tree_hash": "b".repeat(64)},
            "billing_epoch": 51, "market": case["market"], "membership": case["membership"],
            "settlement_policy": case["policy"], "payout_revision": terms["payout_revision"],
            "payment_terms_hash": terms["payment_terms_hash"], "rules_ver": 1,
            "funding": {"balance_au": "10000", "reserved_au": "0", "available_au": "10000", "chain_id": null},
            "billing": null
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::new(
            &format!("http://{}/v1", listener.local_addr().unwrap()),
            identity,
            terms["buyer_pubkey"].as_str().unwrap().into(),
            2,
        )
        .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let server = tokio::spawn(async move {
            let mut replies = replies.into_iter();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 4096];
                let request = loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    assert!(bytes.len() < 32768);
                    if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        assert!(headers.starts_with("post /v1/proxy/quote-state "));
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break serde_json::from_slice::<Value>(
                                &bytes[end + 4..end + 4 + length],
                            )
                            .unwrap();
                        }
                    }
                };
                seen.lock().await.push(request.clone());
                let mut response = valid.clone();
                response["request_nonce"] = request["request_nonce"].clone();
                let mut status = 200;
                let mut malformed = false;
                match replies.next().unwrap_or(Reply::Valid) {
                    Reply::Status(code, delay) => {
                        status = code;
                        response = json!({"error": "private upstream detail"});
                        tokio::time::sleep(delay).await;
                    }
                    Reply::Valid => (),
                    Reply::Invalid(pointer, value) => {
                        *response.pointer_mut(pointer).unwrap() = value
                    }
                    Reply::InvalidJson => malformed = true,
                    Reply::Truncated => {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{\"ok\":").await.unwrap();
                        socket.shutdown().await.unwrap();
                        continue;
                    }
                }
                let body = if malformed {
                    b"{invalid-json".to_vec()
                } else {
                    serde_json::to_vec(&response).unwrap()
                };
                let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                if socket.write_all(header.as_bytes()).await.is_ok() {
                    let _ = socket.write_all(&body).await;
                }
                let _ = socket.shutdown().await;
            }
        });
        Self {
            client,
            query,
            requests,
            server,
        }
    }

    async fn assert_reads(&self, count: usize) {
        let requests = self.requests.lock().await;
        assert_eq!(requests.len(), count);
        let mut nonces = HashSet::new();
        for request in requests.iter() {
            let mut body = request.clone();
            let nonce = body
                .as_object_mut()
                .unwrap()
                .remove("request_nonce")
                .unwrap();
            let nonce = nonce.as_str().unwrap();
            assert_eq!(nonce.len(), 64);
            assert!(nonce.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                nonces.insert(nonce.to_owned()),
                "every attempt needs a fresh challenge"
            );
            assert_eq!(
                body,
                serde_json::to_value(&self.query).unwrap(),
                "the authorized query must not change"
            );
        }
        assert!(
            !self.server.is_finished(),
            "mock must not fail its quote-only path checks"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn transient_http_failures_retry_exact_quote_with_fresh_nonces() {
    let f = Fixture::new(vec![
        Reply::Status(429, Duration::ZERO),
        Reply::Status(503, Duration::ZERO),
        Reply::Valid,
    ])
    .await;
    let observation = f.client.quote(&f.query).await.unwrap();
    assert_eq!(observation.funding().unwrap().available_au, 10000);
    assert_eq!(observation.epoch().unwrap(), 51);
    assert!(observation.billing().unwrap().is_none());
    f.assert_reads(3).await;
}

#[tokio::test]
async fn interrupted_response_body_can_retry_without_accepting_partial_evidence() {
    let f = Fixture::new(vec![Reply::Truncated, Reply::Valid]).await;
    let result = f.client.quote(&f.query).await;
    assert!(result.is_ok(), "{:?}", result.err());
    f.assert_reads(2).await;
}

#[tokio::test]
async fn permanent_http_errors_and_redirects_are_not_retried() {
    for status in [302, 400, 401, 403, 404, 409, 422] {
        let f = Fixture::new(vec![Reply::Status(status, Duration::ZERO)]).await;
        let error = f.client.quote(&f.query).await.err().expect("must reject");
        assert!(matches!(&error, Error::Http { status: actual, .. } if *actual == status));
        assert!(!error.to_string().contains("private upstream detail"));
        f.assert_reads(1).await;
    }
}

#[tokio::test]
async fn invalid_successful_evidence_never_retries_or_replaces_original_query() {
    for reply in [
        Reply::Invalid("/request_nonce", json!("f".repeat(64))),
        Reply::Invalid("/context/network_id", json!("other-network")),
        Reply::Invalid("/offer/revision", json!(2)),
        Reply::Invalid("/funding/available_au", json!("9999")),
        Reply::Invalid("/proof/tree_hash", json!("invalid")),
        Reply::Invalid("/settlement_policy/allow_checkpoints", json!(false)),
        Reply::InvalidJson,
    ] {
        let f = Fixture::new(vec![reply, Reply::Valid]).await;
        assert!(f.client.quote(&f.query).await.is_err());
        f.assert_reads(1).await;
    }
}

#[tokio::test]
async fn transient_retry_count_is_finite_and_backed_off() {
    let f = Fixture::new((0..4).map(|_| Reply::Status(500, Duration::ZERO)).collect()).await;
    let started = std::time::Instant::now();
    assert!(matches!(
        f.client.quote(&f.query).await,
        Err(Error::Http { status: 500, .. })
    ));
    assert!(started.elapsed() >= Duration::from_millis(700));
    f.assert_reads(3).await;
}

#[tokio::test]
async fn stalled_retry_cannot_restart_the_fifteen_second_budget() {
    let f = Fixture::new(vec![
        Reply::Status(503, Duration::from_secs(2)),
        Reply::Status(503, Duration::from_secs(60)),
    ])
    .await;
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_millis(16500), f.client.quote(&f.query))
        .await
        .expect("all attempts must share the original fifteen-second deadline");
    assert!(result.is_err());
    assert!(started.elapsed() >= Duration::from_secs(14));
    f.assert_reads(2).await;
}
