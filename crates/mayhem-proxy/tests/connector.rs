use mayhem_proxy::connector::{
    config::{ConnectionConfig, NetworkPolicy, Operation},
    failure::{openai_error, parse_retry_after, Code, Execution, RetryAdvice, Scope, Stage},
    http::{HttpConnection, WireFormat},
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn configuration(base: &str) -> Value {
    json!({"schema_version":1,"id":"fixture","revision":1,"base_url":base,
        "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},
        "paths":{"models":"models","chat_completions":"chat/completions","decisions":"decisions"},
        "error_profile":"open_ai"})
}
fn configured(value: Value) -> ConnectionConfig {
    serde_json::from_value(value).unwrap()
}
fn connection(base: &str) -> HttpConnection {
    HttpConnection::new(configured(configuration(base))).unwrap()
}

#[test]
fn connection_commitment_binds_dispatch_authority_without_loading_credentials() {
    let config = configuration("http://127.0.0.1:18081/v1/");
    let reference = configured(config.clone()).fingerprint().unwrap();
    for (key, value) in [
        ("revision", serde_json::json!(2)),
        ("base_url", serde_json::json!("http://127.0.0.1:18082/v1/")),
        (
            "paths",
            serde_json::json!({"models":"models","chat_completions":"other"}),
        ),
        ("error_profile", serde_json::json!("http_status")),
        (
            "authentication",
            serde_json::json!({"type":"bearer","secret":{"source":"environment","name":"MAYHEM_PROXY_FIXTURE_NOT_LOADED"}}),
        ),
    ] {
        let mut changed = config.clone();
        changed[key] = value;
        assert_ne!(
            configured(changed).fingerprint().unwrap(),
            reference,
            "{key}"
        );
    }
    // Equivalent parsed configuration is stable, independently of object order.
    let reversed = serde_json::Value::Object(
        config
            .as_object()
            .unwrap()
            .iter()
            .rev()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    assert_eq!(configured(reversed).fingerprint().unwrap(), reference);
}

#[test]
fn upstream_errors_preserve_class_scope_and_uncertainty_without_vendor_text() {
    let now = UNIX_EPOCH + Duration::from_secs(1000);
    for (status, vendor, expected, scope, public) in [
        (
            400,
            "context_length_exceeded",
            Code::ContextTooLarge,
            Scope::Request,
            400,
        ),
        (
            400,
            "unsupported_parameter",
            Code::UnsupportedControl,
            Scope::Request,
            400,
        ),
        (
            401,
            "invalid_api_key",
            Code::UpstreamAuthentication,
            Scope::Connection,
            502,
        ),
        (
            402,
            "anything",
            Code::UpstreamPaymentRequired,
            Scope::Connection,
            502,
        ),
        (
            429,
            "insufficient_quota",
            Code::UpstreamPaymentRequired,
            Scope::Connection,
            502,
        ),
        (
            429,
            "rate_limit_exceeded",
            Code::UpstreamRateLimited,
            Scope::Connection,
            503,
        ),
        (
            404,
            "model_not_found",
            Code::UpstreamModelUnavailable,
            Scope::Model,
            503,
        ),
        (
            404,
            "unknown",
            Code::UpstreamEndpointUnavailable,
            Scope::Connection,
            502,
        ),
        (
            503,
            "overloaded_error",
            Code::UpstreamBusy,
            Scope::Model,
            503,
        ),
        (502, "unknown", Code::UpstreamUnavailable, Scope::Model, 503),
        (
            200,
            "invalid_request_error",
            Code::InvalidRequest,
            Scope::Request,
            400,
        ),
    ] {
        let body = serde_json::to_vec(&json!({"error":{"code":vendor,"param":"max_tokens",
            "message":"credential=secret-fixture at http://private.invalid/admin","trace":"private-path"}})).unwrap();
        let failure = openai_error(status, &body, Some("3"), now);
        assert_eq!(
            (failure.code, failure.scope, failure.public_status()),
            (expected, scope, public)
        );
        assert_eq!(
            failure.execution,
            Execution::Unknown,
            "HTTP status is not non-execution evidence"
        );
        assert_eq!(failure.retry_after_ms, Some(3000));
        assert_eq!(failure.parameter, Some("max_tokens"));
        assert_ne!(failure.retry_advice(), RetryAdvice::SafeBeforeDispatch);
        let exposed = format!(
            "{failure:?} {failure} {}",
            serde_json::to_string(&failure).unwrap()
        );
        for secret in ["secret-fixture", "private.invalid", "private-path"] {
            assert!(!exposed.contains(secret));
        }
    }
    let failure = openai_error(
        500,
        br#"{"error":{"code":"secret-fixture","param":"secret-fixture"}}"#,
        None,
        now,
    );
    assert!(failure.upstream_code.is_none() && failure.parameter.is_none());
    assert_eq!(failure.retry_advice(), RetryAdvice::RecoverSameAttempt);
    assert_eq!(parse_retry_after("9999999999999999999999999", now), None);
    assert_eq!(parse_retry_after("-1", now), None);
    assert_eq!(
        parse_retry_after("Thu, 01 Jan 1970 00:16:45 GMT", now),
        Some(5000)
    );
    assert_eq!(
        parse_retry_after("Thu, 01 Jan 1970 00:00:00 GMT", now),
        Some(0)
    );
}

#[test]
fn connection_policy_denies_escapes_control_endpoints_and_unprotected_remote_http() {
    let base = configuration("http://127.0.0.1:3333/v1/");
    let mut cases = Vec::new();
    for url in [
        "file:///etc/",
        "https://user:password@example.invalid/v1/",
        "https://example.invalid/v1/?key=x",
        "https://example.invalid/v1/#fragment",
        "http://169.254.169.254/",
        "http://100.100.100.200/",
        "http://127.0.0.1:11435/v1/",
        "http://127.0.0.1:11437/v1/",
        "http://127.0.0.1:2375/",
        "http://0.0.0.0/",
        "http://[::ffff:169.254.169.254]/",
        "http://127.0.0.1/v1/%2e%2e/",
        "http://8.8.8.8/v1/",
        "http://127.0.0.1/v1",
    ] {
        let mut v = base.clone();
        v["base_url"] = json!(url);
        cases.push(v);
    }
    for path in [
        "../admin",
        "/admin",
        "https://other.invalid/",
        "chat/%2e%2e/admin",
        "chat?secret=value",
        "chat#fragment",
        "a\\b",
        "a//b",
    ] {
        let mut v = base.clone();
        v["paths"]["chat_completions"] = json!(path);
        cases.push(v);
    }
    for name in [
        "Host",
        "cookie",
        "proxy-authorization",
        "content-length",
        "x-http-method-override",
        "x-forwarded-host",
    ] {
        let mut v = base.clone();
        v["headers"] = json!({name:"blocked"});
        cases.push(v);
    }
    let mut v = base.clone();
    v["network"] = json!({"mode":"public_https"});
    cases.push(v);
    let mut v = base.clone();
    v["network"] = json!({"mode":"pinned","networks":["0.0.0.0/0"],"allow_http":true});
    cases.push(v);
    for v in cases {
        assert!(
            HttpConnection::new(configured(v.clone())).is_err(),
            "accepted {v}"
        );
    }
    let mut media = base.clone();
    media["paths"]["images"] = json!("images/generations");
    assert!(serde_json::from_value::<ConnectionConfig>(media).is_err());
    assert!(HttpConnection::new(configured(base)).is_ok());
    let mut public = configuration("https://example.invalid/v1/");
    public["network"] = json!({"mode":"public_https"});
    for origin in [
        "https://api.openmayhem.ai/v1/",
        "https://openmayhem.ai/v1/",
        "https://MCP.OPENMAYHEM.AI./mcp/",
    ] {
        let mut recursive = public.clone();
        recursive["base_url"] = json!(origin);
        assert!(HttpConnection::new(configured(recursive)).is_err());
    }
    assert!(HttpConnection::new(configured(public)).is_ok());
}

#[test]
fn ipv4_ipv6_and_mapped_destinations_require_explicit_private_authority() {
    let public = NetworkPolicy::PublicHttps;
    for ip in [
        "127.0.0.1",
        "10.1.1.1",
        "172.16.2.2",
        "192.168.1.1",
        "169.254.169.254",
        "100.100.100.200",
        "100.64.0.1",
        "0.1.2.3",
        "224.0.0.1",
        "255.255.255.255",
        "192.0.2.1",
        "198.18.0.1",
        "203.0.113.1",
        "::1",
        "::",
        "fe80::1",
        "fc00::1",
        "ff00::1",
        "2001:db8::1",
        "2002:7f00:1::1",
        "64:ff9b::7f00:1",
        "::ffff:127.0.0.1",
    ] {
        assert!(!public.permits(ip.parse().unwrap()), "public allowed {ip}");
    }
    for ip in [
        "8.8.8.8",
        "1.1.1.1",
        "2606:4700:4700::1111",
        "::ffff:8.8.8.8",
    ] {
        assert!(public.permits(ip.parse().unwrap()), "public rejected {ip}");
    }
    let private: NetworkPolicy=serde_json::from_value(json!({"mode":"pinned","networks":["127.0.0.1/32","10.0.0.0/8","fc00::/7","169.254.0.0/16","100.100.100.200/32"],"allow_http":true})).unwrap();
    for ip in ["127.0.0.1", "::ffff:127.0.0.1", "10.1.2.3", "fd00::1"] {
        assert!(private.permits(ip.parse().unwrap()));
    }
    for ip in ["169.254.169.254", "100.100.100.200", "8.8.8.8", "::1"] {
        assert!(!private.permits(ip.parse().unwrap()));
    }
}

#[cfg(unix)]
fn private_write(path: &std::path::Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
#[test]
fn protected_configuration_checks_ownership_type_size_and_redacts_debug() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("connection.json");
    let secret_path = dir.path().join("credential");
    private_write(&secret_path, b"fixture-only-secret\n");
    let mut v = configuration("http://127.0.0.1:3333/v1/");
    v["authentication"] = json!({"type":"bearer","secret":{"source":"file","path":"credential"}});
    private_write(&config_path, &serde_json::to_vec(&v).unwrap());
    let config = ConnectionConfig::load(&config_path).unwrap();
    assert!(!format!("{config:?}").contains("127.0.0.1"));
    let connection = HttpConnection::new(config).unwrap();
    assert!(!format!("{connection:?}").contains("fixture-only-secret"));
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ConnectionConfig::load(&config_path).is_err());
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(HttpConnection::new(ConnectionConfig::load(&config_path).unwrap()).is_err());
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = dir.path().join("symlink");
    symlink(&config_path, &alias).unwrap();
    assert!(ConnectionConfig::load(&alias).is_err());
    let hard = dir.path().join("hardlink");
    std::fs::hard_link(&config_path, &hard).unwrap();
    assert!(ConnectionConfig::load(&hard).is_err());
    std::fs::remove_file(hard).unwrap();
    private_write(&config_path, &vec![b'x'; 32 * 1024 + 1]);
    assert!(ConnectionConfig::load(&config_path).is_err());
    assert!(ConnectionConfig::load(dir.path()).is_err());
}

struct Fixture {
    base: String,
    hits: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(response: Vec<u8>) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (h, r) = (hits.clone(), requests.clone());
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = stream.read(&mut buffer).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let len = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
                assert!(request.len() < 64 * 1024);
            }
            h.fetch_add(1, Ordering::SeqCst);
            r.lock().unwrap().push(request);
            let _ = stream.write_all(&response).await;
            let _ = stream.shutdown().await;
        }
    });
    Fixture {
        base,
        hits,
        requests,
        task,
    }
}
fn response(status: u16, headers: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Test\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    )
    .into_bytes()
}
async fn send(
    c: &HttpConnection,
) -> Result<
    mayhem_proxy::connector::http::UpstreamResponse,
    mayhem_proxy::connector::failure::Failure,
> {
    c.send(
        Operation::ChatCompletions,
        Some(br#"{"model":"fixture","messages":[]}"#.to_vec()),
    )
    .await
}

#[tokio::test]
async fn real_http_errors_are_sanitized_and_never_retried_implicitly() {
    let f = server(response(
        429,
        "Content-Type: application/json\r\nRetry-After: 7\r\n",
        r#"{"error":{"code":"rate_limit_exceeded","message":"do not print secret-fixture"}}"#,
    ))
    .await;
    let error = send(&connection(&f.base)).await.unwrap_err();
    assert_eq!(
        (error.code, error.retry_after_ms),
        (Code::UpstreamRateLimited, Some(7000))
    );
    assert_eq!(error.retry_advice(), RetryAdvice::RecoverSameAttempt);
    assert!(!format!("{error:?}").contains("secret-fixture"));
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
    let mut config = configuration(&f.base);
    config["error_profile"] = json!("http_status");
    let error = send(&HttpConnection::new(configured(config)).unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        error.upstream_code, None,
        "custom protocols do not accidentally use OpenAI body semantics"
    );
}

#[tokio::test]
async fn redirect_is_not_followed_and_unsupported_operations_never_dispatch() {
    let target = server(response(200, "", r#"{"wrong":true}"#)).await;
    let f = server(response(
        307,
        &format!("Location: {}chat/completions\r\n", target.base),
        "",
    ))
    .await;
    let c = connection(&f.base);
    let error = send(&c).await.unwrap_err();
    assert_eq!(error.code, Code::UpstreamProtocol);
    assert_eq!(target.hits.load(Ordering::SeqCst), 0);
    let error = c
        .send(Operation::Responses, Some(b"{}".to_vec()))
        .await
        .unwrap_err();
    assert_eq!(error.execution, Execution::NotDispatched);
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn private_credentials_are_only_injected_into_the_approved_request() {
    let f = server(response(
        200,
        "Content-Type: application/json\r\n",
        r#"{"ok":true}"#,
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key");
    private_write(&path, b"fixture-only-secret\n");
    let mut config = configuration(&f.base);
    config["authentication"] = json!({"type":"bearer","secret":{"source":"file","path":path}});
    config["headers"] = json!({"anthropic-version":"2023-06-01"});
    let c = HttpConnection::new(configured(config)).unwrap();
    assert_eq!(
        send(&c).await.unwrap().collect_json().await.unwrap(),
        json!({"ok":true})
    );
    let request = String::from_utf8(f.requests.lock().unwrap()[0].clone()).unwrap();
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(request.contains("authorization: Bearer fixture-only-secret\r\n"));
    assert!(request.contains("anthropic-version: 2023-06-01\r\n"));
    assert!(!format!("{c:?}").contains("fixture-only-secret"));
}

struct ScriptedDns {
    calls: AtomicUsize,
    answers: Vec<Vec<SocketAddr>>,
}
impl Resolve for ScriptedDns {
    fn resolve(&self, _: Name) -> Resolving {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answers[call.min(self.answers.len() - 1)].clone();
        Box::pin(async move { Ok(Box::new(answer.into_iter()) as Addrs) })
    }
}

#[tokio::test]
async fn dns_is_rechecked_on_new_connections_and_mixed_or_rebound_answers_fail_closed() {
    let f = server(response(
        200,
        "Content-Type: application/json\r\n",
        r#"{"ok":true}"#,
    ))
    .await;
    let base = f.base.replace("127.0.0.1", "fixture.invalid");
    let resolver = Arc::new(ScriptedDns {
        calls: AtomicUsize::new(0),
        answers: vec![
            vec!["127.0.0.1:1".parse().unwrap()],
            vec!["169.254.169.254:80".parse().unwrap()],
        ],
    });
    let c =
        HttpConnection::with_resolver(configured(configuration(&base)), resolver.clone()).unwrap();
    assert_eq!(
        send(&c).await.unwrap().collect_json().await.unwrap(),
        json!({"ok":true})
    );
    let error = send(&c).await.unwrap_err();
    assert_eq!(error.code, Code::DestinationRejected);
    assert_eq!(error.execution, Execution::NotDispatched);
    assert_eq!(
        f.hits.load(Ordering::SeqCst),
        1,
        "rebinding must not issue another request"
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    let resolver = Arc::new(ScriptedDns {
        calls: AtomicUsize::new(0),
        answers: vec![vec![
            "127.0.0.1:0".parse().unwrap(),
            "10.0.0.1:0".parse().unwrap(),
        ]],
    });
    let c = HttpConnection::with_resolver(configured(configuration(&base)), resolver).unwrap();
    assert_eq!(send(&c).await.unwrap_err().code, Code::DestinationRejected);
    assert_eq!(
        f.hits.load(Ordering::SeqCst),
        1,
        "mixed answers are rejected as a set"
    );
    let mut public = configuration(&base.replace("http:", "https:"));
    public["network"] = json!({"mode":"public_https"});
    let resolver = Arc::new(ScriptedDns {
        calls: AtomicUsize::new(0),
        answers: vec![vec!["127.0.0.1:0".parse().unwrap()]],
    });
    let c = HttpConnection::with_resolver(configured(public), resolver).unwrap();
    assert_eq!(send(&c).await.unwrap_err().code, Code::DestinationRejected);
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn local_io_concurrency_is_bounded_without_a_hidden_queue() {
    let f = server(response(
        200,
        "Content-Type: application/json\r\n",
        r#"{"ok":true}"#,
    ))
    .await;
    let mut config = configuration(&f.base);
    config["limits"] = json!({"max_in_flight":1});
    let c = HttpConnection::new(configured(config)).unwrap();
    let first = send(&c).await.unwrap();
    let busy = send(&c).await.unwrap_err();
    assert_eq!(busy.code, Code::LocalCapacity);
    assert_eq!(busy.retry_advice(), RetryAdvice::SafeBeforeDispatch);
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
    assert_eq!(first.collect_json().await.unwrap(), json!({"ok":true}));
    assert_eq!(
        send(&c).await.unwrap().collect_json().await.unwrap(),
        json!({"ok":true})
    );
    assert_eq!(f.hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn response_disconnect_is_latched_as_unknown_and_never_becomes_successful_eof() {
    let f=server(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 100\r\nConnection: close\r\n\r\ndata: partial\n\n".to_vec()).await;
    let c = connection(&f.base);
    let mut stream = send(&c).await.unwrap();
    assert_eq!(stream.format, WireFormat::Sse);
    let error = loop {
        match stream.next_chunk().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("truncated body reported EOF"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.execution, Execution::Unknown);
    assert_eq!(error.stage, Stage::ResponseBody);
    assert_eq!(error.retry_advice(), RetryAdvice::RecoverSameAttempt);
    assert_eq!(stream.next_chunk().await.unwrap_err().code, error.code);
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn request_response_and_decoder_chunk_bounds_are_enforced() {
    let f = server(response(
        200,
        "Content-Type: application/json\r\n",
        &format!("{{\"text\":\"{}\"}}", "x".repeat(200000)),
    ))
    .await;
    let mut config = configuration(&f.base);
    config["limits"] = json!({"max_request_bytes":1});
    let error = send(&HttpConnection::new(configured(config)).unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        (error.code, error.execution),
        (Code::RequestTooLarge, Execution::NotDispatched)
    );
    assert_eq!(f.hits.load(Ordering::SeqCst), 0);
    let mut config = configuration(&f.base);
    config["limits"] = json!({"max_response_bytes":100});
    assert_eq!(
        send(&HttpConnection::new(configured(config)).unwrap())
            .await
            .unwrap_err()
            .code,
        Code::ResponseTooLarge
    );
    let c = connection(&f.base);
    let mut stream = send(&c).await.unwrap();
    let mut bytes = 0;
    while let Some(chunk) = stream.next_chunk().await.unwrap() {
        assert!(chunk.len() <= 64 * 1024);
        bytes += chunk.len();
    }
    assert_eq!(bytes, 200011);
    let f=server(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\n123456\r\n6\r\n789012\r\n0\r\n\r\n".to_vec()).await;
    let mut config = configuration(&f.base);
    config["limits"] = json!({"max_response_bytes":10});
    let c = HttpConnection::new(configured(config)).unwrap();
    let mut stream = send(&c).await.unwrap();
    let error = loop {
        match stream.next_chunk().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("oversized chunks accepted"),
            Err(e) => break e,
        }
    };
    assert_eq!(error.code, Code::ResponseTooLarge);
}

#[tokio::test]
async fn progress_resets_inactivity_and_no_total_generation_deadline_is_added() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).await.unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        for _ in 0..8 {
            stream.write_all(b"9\r\ndata: x\n\n\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        stream.write_all(b"0\r\n\r\n").await.unwrap();
    });
    let mut config = configuration(&base);
    config["limits"] = json!({"read_idle_timeout_ms":150});
    let c = HttpConnection::new(configured(config)).unwrap();
    let mut stream = send(&c).await.unwrap();
    let mut bytes = 0;
    while let Some(chunk) = stream.next_chunk().await.unwrap() {
        bytes += chunk.len();
    }
    assert_eq!(bytes, 72);
    task.await.unwrap();
}

#[tokio::test]
async fn success_status_with_an_openai_error_body_does_not_become_an_answer() {
    let f = server(response(
        200,
        "Content-Type: application/json\r\n",
        r#"{"error":{"code":"invalid_api_key","message":"private-fixture"}}"#,
    ))
    .await;
    let c = connection(&f.base);
    let error = send(&c).await.unwrap().collect_json().await.unwrap_err();
    assert_eq!(
        (error.code, error.stage),
        (Code::UpstreamAuthentication, Stage::ResponseBody)
    );
    assert_eq!(error.execution, Execution::Unknown);
    assert!(!format!("{error:?}").contains("private-fixture"));
}

#[tokio::test]
async fn timeout_after_sending_is_not_reported_as_connect_failure_or_safe_replay() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let _ = wait.await;
    });
    let mut config = configuration(&base);
    config["limits"] = json!({"read_idle_timeout_ms":50});
    let c = HttpConnection::new(configured(config)).unwrap();
    let error = send(&c).await.unwrap_err();
    assert_eq!(
        (error.code, error.stage, error.scope, error.execution),
        (
            Code::UpstreamTimeout,
            Stage::Dispatch,
            Scope::Model,
            Execution::Unknown
        )
    );
    assert_eq!(error.retry_advice(), RetryAdvice::RecoverSameAttempt);
    let _ = release.send(());
    task.await.unwrap();
}
