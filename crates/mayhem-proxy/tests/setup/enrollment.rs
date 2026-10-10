use super::*;
use ed25519_dalek::SigningKey;
use mayhem_proxy::setup::{EnrollmentAction, EnrollmentResult};

fn owned(seed: u8) -> Fixture {
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    let provider = SigningKey::from_bytes(&[seed; 32])
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    f.input.provider_pubkey = Digest::new(provider.clone()).unwrap();
    f.input.market.creator_pubkey = provider.clone();
    let market = f.input.market.id().unwrap();
    f.input.membership.provider_pubkey = provider.clone();
    f.input.membership.market_id = market.clone();
    for offer in &mut f.input.offers {
        offer.provider_pubkey = provider.clone();
        offer.market_id = market.clone();
    }
    f
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires isolated native Windows private NTFS fixture parent"
)]
fn enrollment_requires_checked_exact_draft_and_trusted_origin_without_upstream_credentials() {
    let f = owned(210);
    f.store().create(f.input.clone()).unwrap();
    assert!(f
        .store()
        .enrollment_client(1, "https://admission.invalid", 5000)
        .is_err());
    f.store().check(1).unwrap();
    for origin in [
        "http://remote.invalid",
        "https://admission.invalid/api",
        "https://user:pass@admission.invalid",
        "https://admission.invalid?key=secret",
        "https://admission.invalid#fragment",
    ] {
        assert!(f.store().enrollment_client(2, origin, 5000).is_err());
    }
    assert!(f
        .store()
        .enrollment_client(1, "https://admission.invalid", 5000)
        .is_err());
    assert!(f
        .store()
        .enrollment_client(2, "https://admission.invalid", 15001)
        .is_err());
    assert!(f
        .store()
        .enrollment_client(2, "https://admission.invalid", 5000)
        .is_ok());
    f.no_network_or_secret();
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "requires isolated native Windows private NTFS fixture parent"
)]
async fn enrollment_never_signs_a_substituted_or_extended_service_challenge() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for mutation in [
        "origin",
        "network",
        "provider",
        "operation",
        "request",
        "nonce",
        "action",
        "operation_id",
        "challenge_id",
        "domain",
        "expired",
        "future",
        "extension",
    ] {
        let f = owned(210);
        f.store().create(f.input.clone()).unwrap();
        f.store().check(1).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let client = f.store().enrollment_client(2, &origin, 3000).unwrap();
        let network = f.input.network.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 1024];
            let end = loop {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 8192);
                if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let header = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            assert!(header.starts_with("post /v1/proxy/admission/auth/challenge "));
            assert!(!header.contains("authorization:"));
            let len: usize = header
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            while bytes.len() < end + len {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let query: Value = serde_json::from_slice(&bytes[end..end + len]).unwrap();
            let digest = |domain: &str, value: Value| {
                let mut b = domain.as_bytes().to_vec();
                b.push(0);
                b.extend(mayhem_proto::stable_json_bytes(&value).unwrap());
                blake3::hash(&b).to_hex().to_string()
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let mut challenge = query;
            let c = challenge.as_object_mut().unwrap();
            c.insert("purpose".into(), json!("proxy_admission_fee"));
            c.insert("network".into(), json!(network));
            c.insert("public_origin".into(), json!(origin));
            c.insert("operation_id".into(),json!(digest("mayhem/proxy/admission-operation/v1",json!({"network":network,"public_origin":origin,
                "provider_pubkey":c["provider_pubkey"],"initial_operation_digest":c["initial_operation_digest"]}))));
            c.insert(
                "challenge_id".into(),
                json!(digest(
                    "mayhem/proxy/admission-challenge-id/v1",
                    json!({"network":network,"public_origin":origin,
                "provider_pubkey":c["provider_pubkey"],"client_nonce":c["client_nonce"]})
                )),
            );
            c.insert("nonce".into(), json!(d(18)));
            c.insert("issued_at_ms".into(), json!(now));
            c.insert("expires_at_ms".into(), json!(now + 60000));
            let mut domain = "mayhem/proxy/admission-auth/v1";
            match mutation {
                "origin" => challenge["public_origin"] = json!("https://attacker.invalid"),
                "network" => challenge["network"]["network_id"] = json!("different-network"),
                "provider" => challenge["provider_pubkey"] = json!(d(19)),
                "operation" => challenge["initial_operation_digest"] = json!(d(19)),
                "request" => challenge["request_digest"] = json!(d(19)),
                "nonce" => challenge["client_nonce"] = json!(d(19)),
                "action" => challenge["action"] = json!("invoice_checkout"),
                "operation_id" => challenge["operation_id"] = json!(d(19)),
                "challenge_id" => challenge["challenge_id"] = json!(d(19)),
                "domain" => domain = "mayhem/proxy/admission/v1",
                "expired" => challenge["expires_at_ms"] = json!(now - 1),
                "future" => challenge["issued_at_ms"] = json!(now + 120000),
                "extension" => challenge["transfer"] = json!({"amount":"1000"}),
                _ => unreachable!(),
            }
            let body=serde_json::to_vec(&json!({"schema_version":1,"purpose":"proxy_admission_fee","signing_domain":domain,"challenge":challenge})).unwrap();
            let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        let result = client
            .execute(
                &SigningKey::from_bytes(&[210; 32]),
                EnrollmentAction::Status,
                None,
            )
            .await;
        assert!(
            matches!(result, Err(Error::Invalid)),
            "{mutation}: challenge was not rejected before signature exchange"
        );
        server.await.unwrap();
        f.no_network_or_secret();
    }
}

/// Driven by the SITE actual Nest/Redis/PostgreSQL acceptance suite. Explicit
/// loopback-only input and a public fixed test key; never touches a real wallet.
#[tokio::test]
#[ignore]
async fn connected_enrollment_driver() {
    let input: Value =
        serde_json::from_str(&std::env::var("PROXY_ENROLLMENT_FIXTURE").unwrap()).unwrap();
    let origin = input["origin"].as_str().unwrap();
    let url = url::Url::parse(origin).unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let seed: u8 = input["seed"].as_u64().unwrap().try_into().unwrap();
    assert!((210..=212).contains(&seed));
    let rail: ProxyRail = serde_json::from_value(input["rail"].clone()).unwrap();
    let mut f = owned(seed);
    f.input.network = serde_json::from_value(input["network"].clone()).unwrap();
    f.store().create(f.input.clone()).unwrap();
    f.store().check(1).unwrap();
    let client = f.store().enrollment_client(2, origin, 15000).unwrap();
    let key = SigningKey::from_bytes(&[seed; 32]);
    let missing = client
        .execute(&key, EnrollmentAction::Status, None)
        .await
        .unwrap();
    assert_eq!(missing.state, "no_local_invoice");
    let first = client
        .execute(&key, EnrollmentAction::Create, Some(rail))
        .await
        .unwrap();
    let first_id = first.invoice.as_ref().unwrap().invoice_id.clone();
    assert!(first.original_operation_matches);
    assert!(!first.authorizes_publication);
    // A fresh client/new authorization recovers the provider's original invoice.
    let restarted = f.store().enrollment_client(2, origin, 15000).unwrap();
    let status = restarted
        .execute(&key, EnrollmentAction::Status, None)
        .await
        .unwrap();
    let repeat = restarted
        .execute(&key, EnrollmentAction::Create, Some(rail))
        .await
        .unwrap();
    assert_eq!(status.invoice.as_ref().unwrap().invoice_id, first_id);
    assert_eq!(repeat.invoice.as_ref().unwrap().invoice_id, first_id);
    if rail == ProxyRail::Fiat {
        let checkout = restarted
            .execute(&key, EnrollmentAction::Checkout, None)
            .await
            .unwrap();
        assert_eq!(checkout.state, "checkout");
        assert!(checkout
            .checkout_url
            .unwrap()
            .starts_with("https://checkout.stripe.com/"));
        assert_eq!(
            restarted
                .execute(&key, EnrollmentAction::Checkout, None)
                .await
                .unwrap()
                .state,
            "checkout"
        );
    }
    assert!(restarted
        .execute(
            &SigningKey::from_bytes(&[1; 32]),
            EnrollmentAction::Status,
            None
        )
        .await
        .is_err());
    assert!(restarted
        .execute(&key, EnrollmentAction::Status, Some(rail))
        .await
        .is_err());
    let out: EnrollmentResult = repeat;
    let json = serde_json::to_string(&out).unwrap();
    assert!(!json.contains("authorization"));
    assert!(!json.contains("never-read-secret"));
    println!("ENROLLMENT_RESULT {json}");
    f.no_network_or_secret();
}
