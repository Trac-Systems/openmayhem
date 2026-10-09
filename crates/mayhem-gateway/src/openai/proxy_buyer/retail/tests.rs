use super::*;
use serde_json::json;

#[test]
fn retail_config_has_fixed_authority_bounded_io_and_no_credential_fingerprint() {
    fn config() -> Config {
        Config {
            url: "http://127.0.0.1:3010/internal/hold".into(),
            credential: "fixture-only".into(),
            owner_token_ids: vec!["owner".into()],
            timeout_ms: 1000,
        }
    }
    for url in [
        "http://example.org/hold",
        "https://user:password@example.org/hold",
        "https://example.org/hold?target=other",
        "https://example.org/hold#x",
        "file:///tmp/hold",
    ] {
        let mut c = config();
        c.url = url.into();
        assert!(c.validate().is_err());
    }
    for timeout in [0, 999, 5001, u64::MAX] {
        let mut c = config();
        c.timeout_ms = timeout;
        assert!(c.validate().is_err());
    }
    let mut c = config();
    c.owner_token_ids.clear();
    assert!(c.validate().is_err());
    let mut c = config();
    c.credential = "bad\r\nheader".into();
    assert!(c.validate().is_err());
    let first = Authority::new(config()).unwrap();
    let mut rotated = config();
    rotated.credential = "rotated-fixture-only".into();
    let rotated = Authority::new(rotated).unwrap();
    assert_eq!(first.commitment, rotated.commitment);
    assert!(first.correlation("owner", None, &json!({})).is_err());
    assert!(first
        .correlation("other", None, &json!({}))
        .unwrap()
        .is_none());
    let original = digest("test", &[]);
    let one = first
        .correlation("owner", Some("request-one"), &json!({"input":"hi"}))
        .unwrap()
        .unwrap();
    let two = first
        .correlation("owner", Some("request-two"), &json!({"input":"hi"}))
        .unwrap()
        .unwrap();
    assert_ne!(
        first.fingerprint(&original, &one),
        first.fingerprint(&original, &two)
    );
    assert_ne!(first.fingerprint(&original, &one), original);
}

#[test]
fn retail_content_digest_cross_language_vectors_and_exact_integer_boundaries() {
    let vectors: Value = serde_json::from_str(include_str!("retail-content-v1.json")).unwrap();
    for vector in vectors.as_array().unwrap() {
        assert_eq!(
            request_content_digest(&vector["value"]).unwrap(),
            vector["sha256"].as_str().unwrap(),
            "{}",
            vector["name"]
        );
    }
    for value in [
        json!(u64::MAX),
        json!(i64::MAX),
        json!(9_007_199_254_740_993_u64),
    ] {
        assert!(request_content_digest(&value).is_err(), "{value}");
    }
    assert_eq!(
        request_content_digest(&json!(0)),
        request_content_digest(&json!(-0.0))
    );
    assert_eq!(
        request_content_digest(&json!(1)),
        request_content_digest(&json!(1.0))
    );
    let mut nested = Value::Null;
    for _ in 0..64 {
        nested = json!([nested]);
    }
    assert!(request_content_digest(&nested).is_ok());
    assert!(request_content_digest(&json!([nested])).is_err());
}
