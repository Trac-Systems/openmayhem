use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const REFRESH_PATH: &str = "v1/proxy/admission/invoice/refresh";

fn pending() -> Value {
    json!({"code":"proxy_admission_quote_reconciliation_pending", "retry_after_ms":1000,
        "message":"remote-private-detail-must-not-escape"})
}

fn quote() -> EnrollmentQuote {
    EnrollmentQuote {
        invoice_id: Digest::new("55".repeat(32)).unwrap(),
        invoice_commitment: Digest::new("66".repeat(32)).unwrap(),
    }
}

#[test]
fn error_diagnostics_require_known_status_and_code_and_discard_remote_text() {
    for (status, code) in [
        (503, "proxy_admission_canonical_unavailable"),
        (503, "proxy_admission_rate_unavailable"),
        (503, "proxy_admission_receiver_unavailable"),
        (503, "proxy_admission_collection_disabled"),
        (503, "proxy_admission_rail_unavailable"),
        (503, "proxy_admission_auth_disabled"),
        (503, "proxy_admission_auth_unavailable"),
        (401, "proxy_admission_auth_invalid"),
        (400, "proxy_admission_auth_invalid_request"),
        (400, "proxy_admission_auth_scope"),
        (409, "proxy_admission_auth_challenge_conflict"),
        (429, "proxy_admission_auth_rate_limited"),
        (409, "proxy_admission_quote_reconciliation_required"),
        (503, "proxy_admission_quote_reconciliation_required"),
        (404, "proxy_admission_invoice_not_found"),
        (503, "proxy_admission_quote_expired"),
        (400, "proxy_admission_identity_revoked"),
    ] {
        let body = serde_json::to_vec(&json!({"code":code,
            "message":"private-token-secret", "stack":"private-stack"}))
        .unwrap();
        let error = enrollment_http_error(status, REFRESH_PATH, &body);
        assert!(matches!(error, Error::EnrollmentService { code: actual, .. } if actual == code));
        assert!(error.to_string().contains(code));
        assert!(!format!("{error:?} {error}").contains("private-"));
        assert!(matches!(
            enrollment_http_error(418, REFRESH_PATH, &body),
            Error::EnrollmentHttp { status: 418 }
        ));
    }
    let pending = serde_json::to_vec(&pending()).unwrap();
    assert!(matches!(
        enrollment_http_error(503, REFRESH_PATH, &pending),
        Error::EnrollmentReconciliationPending
    ));
    for path in [
        "v1/proxy/admission/auth/challenge",
        "v1/proxy/admission/auth/verify",
        "v1/proxy/admission/invoice/create",
        "v1/proxy/admission/invoice/status",
        "v1/proxy/admission/invoice/checkout",
    ] {
        assert!(matches!(
            enrollment_http_error(503, path, &pending),
            Error::EnrollmentHttp { status: 503 }
        ));
    }
    for status in [200, 400, 401, 403, 409, 429, 500] {
        assert!(matches!(
            enrollment_http_error(status, REFRESH_PATH, &pending),
            Error::EnrollmentHttp { .. }
        ));
    }
    for body in [
        b"private-invalid-json".to_vec(),
        br#"{"code":"private-unknown-code","message":"private-secret"}"#.to_vec(),
        br#"{"code":"proxy_admission_quote_reconciliation_pending","code":"private-second-code"}"#
            .to_vec(),
        br#"{"code":7,"message":"private-secret"}"#.to_vec(),
    ] {
        let error = enrollment_http_error(503, REFRESH_PATH, &body);
        assert!(matches!(error, Error::EnrollmentHttp { status: 503 }));
        assert!(!format!("{error:?} {error}").contains("private-"));
    }
}

async fn send_response(socket: &mut TcpStream, status: u16, body: &[u8]) {
    let header = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    socket.write_all(header.as_bytes()).await.unwrap();
    socket.write_all(body).await.unwrap();
}

#[tokio::test]
async fn error_body_limit_applies_to_content_length_and_streamed_bodies() {
    for chunked in [false, true] {
        for oversized in [false, true] {
            let mut body = serde_json::to_vec(&pending()).unwrap();
            body.resize(MAX_RESPONSE + usize::from(oversized), b' ');
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = socket.read(&mut request).await.unwrap();
                let wire = if chunked {
                    format!("HTTP/1.1 503 Test\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n", body.len(), String::from_utf8(body).unwrap())
                } else {
                    format!(
                        "HTTP/1.1 503 Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        String::from_utf8(body).unwrap()
                    )
                };
                // An oversized Content-Length can close the client before body I/O.
                let _ = socket.write_all(wire.as_bytes()).await;
            });
            let response = reqwest::Client::new().get(origin).send().await.unwrap();
            let error = enrollment_response(response, REFRESH_PATH)
                .await
                .unwrap_err();
            assert_eq!(
                matches!(error, Error::EnrollmentReconciliationPending),
                !oversized
            );
            assert!(!error.to_string().contains("remote-private-detail"));
            server.await.unwrap();
        }
    }
}

#[derive(Debug)]
struct Call {
    path: String,
    body: Value,
    authorization: Option<String>,
}

async fn receive(socket: &mut TcpStream) -> Call {
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    let end = loop {
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0 && bytes.len() + count <= 8192);
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let lower = headers.to_ascii_lowercase();
    let length: usize = lower
        .lines()
        .find_map(|line| line.strip_prefix("content-length: "))
        .unwrap()
        .parse()
        .unwrap();
    assert!(end + length <= 8192);
    while bytes.len() < end + length {
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0 && bytes.len() + count <= 8192);
        bytes.extend_from_slice(&buffer[..count]);
    }
    Call {
        path: headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_owned(),
        body: serde_json::from_slice(&bytes[end..end + length]).unwrap(),
        authorization: headers.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_owned())
        }),
    }
}

struct Server {
    client: EnrollmentClient,
    key: SigningKey,
    calls: Arc<Mutex<Vec<Call>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(
        action: EnrollmentAction,
        request: Value,
        outcomes: Vec<(u16, Value)>,
        early: Option<(&'static str, u16, Value)>,
        stall_retry: bool,
        timeout_ms: u64,
    ) -> Self {
        let key = SigningKey::from_bytes(&[211; 32]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (http, base) = admission::peer(&origin, timeout_ms).unwrap();
        let network = Identity {
            network_id: "test".into(),
            msb_bootstrap: "11".repeat(32),
            subnet_bootstrap: "22".repeat(32),
            contract_version: 30,
        };
        let provider = Digest::new(hex(&key.verifying_key().to_bytes())).unwrap();
        let operation = Digest::new("33".repeat(32)).unwrap();
        let client = EnrollmentClient {
            client: http,
            base,
            network: network.clone(),
            provider: provider.clone(),
            operation: operation.clone(),
            timeout_ms,
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let task = tokio::spawn(async move {
            let operation_id = digest("mayhem/proxy/admission-operation/v1", &json!({
                "network":network,"public_origin":origin,"provider_pubkey":provider,"initial_operation_digest":operation})).unwrap();
            for (attempt, (status, outcome)) in outcomes.into_iter().enumerate() {
                let mut challenge = Value::Null;
                let token = format!("{:064x}", attempt + 1);
                for phase in ["challenge", "verify", "invoice"] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let call = receive(&mut socket).await;
                    let expected_path = if phase == "invoice" {
                        format!("/v1/proxy/admission/invoice/{}", action.path())
                    } else {
                        format!("/v1/proxy/admission/auth/{phase}")
                    };
                    assert_eq!(call.path, expected_path);
                    let body = call.body.clone();
                    if phase == "invoice" {
                        assert_eq!(body["request"], request);
                        assert_eq!(body["operation_id"], json!(operation_id));
                        assert_eq!(
                            call.authorization.as_deref(),
                            Some(format!("ProxyAdmission {token}").as_str())
                        );
                    } else {
                        assert!(call.authorization.is_none());
                    }
                    observed.lock().unwrap().push(call);
                    if let Some((stage, failure_status, failure)) = &early {
                        if phase == *stage {
                            // Zero means a transport closes after reading the request.
                            if *failure_status == 0 {
                                return;
                            }
                            send_response(
                                &mut socket,
                                *failure_status,
                                &serde_json::to_vec(failure).unwrap(),
                            )
                            .await;
                            return;
                        }
                    }
                    if stall_retry && attempt > 0 {
                        std::future::pending::<()>().await;
                    }
                    let response = match phase {
                        "challenge" => {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64;
                            challenge = body;
                            challenge["purpose"] = json!(PURPOSE);
                            challenge["network"] = json!(network);
                            challenge["public_origin"] = json!(origin);
                            challenge["operation_id"] = json!(operation_id);
                            challenge["challenge_id"] = json!(digest("mayhem/proxy/admission-challenge-id/v1", &json!({
                                "network":network,"public_origin":origin,"provider_pubkey":provider,"client_nonce":challenge["client_nonce"]})).unwrap());
                            challenge["nonce"] = json!(format!("{:064x}", attempt + 101));
                            challenge["issued_at_ms"] = json!(now);
                            challenge["expires_at_ms"] = json!(now + 60000);
                            json!({"schema_version":1,"purpose":PURPOSE,"signing_domain":AUTH,"challenge":challenge})
                        }
                        "verify" => {
                            assert_eq!(body["challenge_id"], challenge["challenge_id"]);
                            assert_eq!(body["provider_signature"].as_str().unwrap().len(), 128);
                            let mut claims = challenge.clone();
                            for name in ["client_nonce", "challenge_id", "nonce"] {
                                claims.as_object_mut().unwrap().remove(name);
                            }
                            json!({"schema_version":1,"purpose":PURPOSE,
                                "authorization":{"scheme":"ProxyAdmission","token":token,"claims":claims}})
                        }
                        _ => outcome.clone(),
                    };
                    send_response(
                        &mut socket,
                        if phase == "invoice" { status } else { 200 },
                        &serde_json::to_vec(&response).unwrap(),
                    )
                    .await;
                }
            }
        });
        Self {
            client,
            key,
            calls,
            task,
        }
    }

    async fn stop(self) -> Vec<Call> {
        self.task.abort();
        if let Err(error) = self.task.await {
            assert!(error.is_cancelled(), "mock service failed: {error}");
        }
        Arc::try_unwrap(self.calls).unwrap().into_inner().unwrap()
    }
}

#[tokio::test]
async fn refresh_pending_retries_same_quote_with_fresh_authorization() {
    let quote = quote();
    let server = Server::start(EnrollmentAction::Refresh, json!(quote), vec![(503, pending()),
        (200, json!({"schema_version":1,"purpose":PURPOSE,"state":"admitted","entitlement_id":"77".repeat(32)}))],
        None, false, 5000).await;
    let result = server
        .client
        .execute_with_quote(&server.key, EnrollmentAction::Refresh, None, Some(quote))
        .await
        .unwrap();
    assert_eq!(result.state, "admitted");
    assert!(!result.authorizes_publication);
    let calls = server.stop().await;
    assert_eq!(calls.len(), 6);
    assert_ne!(calls[0].body["client_nonce"], calls[3].body["client_nonce"]);
    assert_ne!(calls[1].body["challenge_id"], calls[4].body["challenge_id"]);
    assert_ne!(calls[2].authorization, calls[5].authorization);
    assert_eq!(calls[2].body, calls[5].body);
}

#[tokio::test]
async fn refresh_attempt_count_and_total_deadline_are_bounded() {
    for (timeout_ms, stall, expected_calls) in [(5000, false, 9), (200, false, 3), (1200, true, 4)]
    {
        let quote = quote();
        let server = Server::start(
            EnrollmentAction::Refresh,
            json!(quote),
            vec![(503, pending()); 4],
            None,
            stall,
            timeout_ms,
        )
        .await;
        let started = tokio::time::Instant::now();
        let error = server
            .client
            .execute_with_quote(&server.key, EnrollmentAction::Refresh, None, Some(quote))
            .await
            .err()
            .unwrap();
        assert!(matches!(error, Error::EnrollmentReconciliationPending));
        assert!(started.elapsed() < Duration::from_millis(timeout_ms + 500));
        assert_eq!(server.stop().await.len(), expected_calls);
    }
}

#[tokio::test]
async fn pending_never_retries_other_actions_or_authentication_endpoints() {
    for action in [
        EnrollmentAction::Create,
        EnrollmentAction::Status,
        EnrollmentAction::Checkout,
        EnrollmentAction::Returns,
    ] {
        let rail = if matches!(action, EnrollmentAction::Create) {
            Some(mayhem_proto::proxy::ProxyRail::Tnk)
        } else {
            None
        };
        let request = if rail.is_some() {
            json!({"rail":"tnk"})
        } else {
            json!({})
        };
        let server = Server::start(
            action,
            request,
            vec![(503, pending()); 3],
            None,
            false,
            5000,
        )
        .await;
        assert!(matches!(
            server.client.execute(&server.key, action, rail).await,
            Err(Error::EnrollmentHttp { status: 503 })
        ));
        assert_eq!(server.stop().await.len(), 3);
    }
    for phase in ["challenge", "verify"] {
        let quote = quote();
        let server = Server::start(
            EnrollmentAction::Refresh,
            json!(quote),
            vec![(503, pending()); 3],
            Some((phase, 503, pending())),
            false,
            5000,
        )
        .await;
        assert!(matches!(
            server
                .client
                .execute_with_quote(&server.key, EnrollmentAction::Refresh, None, Some(quote))
                .await,
            Err(Error::EnrollmentHttp { status: 503 })
        ));
        assert_eq!(
            server.stop().await.len(),
            if phase == "challenge" { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn refresh_does_not_retry_conflicts_rate_limits_or_other_service_errors() {
    for (status, code) in [
        (409, "proxy_admission_quote_reconciliation_required"),
        (429, "proxy_admission_auth_rate_limited"),
        (503, "proxy_admission_rate_unavailable"),
        (401, "proxy_admission_auth_invalid"),
        (500, "proxy_admission_quote_reconciliation_pending"),
    ] {
        let quote = quote();
        let server = Server::start(
            EnrollmentAction::Refresh,
            json!(quote),
            vec![
                (
                    status,
                    json!({"code":code,"message":"remote-private-detail"})
                );
                3
            ],
            None,
            false,
            5000,
        )
        .await;
        let error = server
            .client
            .execute_with_quote(&server.key, EnrollmentAction::Refresh, None, Some(quote))
            .await
            .err()
            .unwrap();
        assert!(!matches!(error, Error::EnrollmentReconciliationPending));
        assert!(!error.to_string().contains("remote-private-detail"));
        assert_eq!(server.stop().await.len(), 3);
    }
}

#[tokio::test]
async fn a_terminal_response_after_pending_stops_retrying() {
    let quote = quote();
    let server = Server::start(
        EnrollmentAction::Refresh,
        json!(quote),
        vec![
            (503, pending()),
            (
                409,
                json!({"code":"proxy_admission_quote_reconciliation_required"}),
            ),
            (503, pending()),
        ],
        None,
        false,
        5000,
    )
    .await;
    let error = server
        .client
        .execute_with_quote(&server.key, EnrollmentAction::Refresh, None, Some(quote))
        .await
        .err()
        .unwrap();
    assert!(matches!(
        error,
        Error::EnrollmentService {
            code: "proxy_admission_quote_reconciliation_required",
            ..
        }
    ));
    assert_eq!(server.stop().await.len(), 6);
}

#[tokio::test]
async fn transport_failure_is_unavailable_and_does_not_retry() {
    let quote = quote();
    let server = Server::start(
        EnrollmentAction::Refresh,
        json!(quote),
        vec![(503, pending()); 3],
        Some(("challenge", 0, Value::Null)),
        false,
        5000,
    )
    .await;
    assert!(matches!(
        server
            .client
            .execute_with_quote(&server.key, EnrollmentAction::Refresh, None, Some(quote))
            .await,
        Err(Error::EnrollmentUnavailable)
    ));
    assert_eq!(server.stop().await.len(), 1);
}
