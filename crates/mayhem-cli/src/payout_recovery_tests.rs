use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn balance(available: i64, pending: i64) -> Value {
    json!({"object":"balance", "livemode":false,
        "available":[{"currency":"usd","amount":available}],
        "pending":[{"currency":"usd","amount":pending}]})
}

fn valuation_quote(status: &str) -> Value {
    json!({"id":"fxq_fixture","object":"fx_quote","created":100,
        "lock_duration":"hour","lock_expires_at":3700,"lock_status":status,
        "to_currency":"eur","usage":{"type":"transfer","transfer":{"destination":"acct_fixture"}},
        "rates":{"usd":{"exchange_rate":"0.9","rate_details":{"base_rate":"0.9"}}}})
}

#[test]
fn quote_expiry_preserves_signed_snapshot_without_accepting_other_mutations() {
    let original = stripe_fx_quote_result(&valuation_quote("active")).unwrap();
    let hash = stripe_fx_quote_hash(&original).unwrap();
    let recovered = canonical_fiat_quote_snapshot(
        stripe_fx_quote_result(&valuation_quote("expired")).unwrap(),
        &hash,
    )
    .unwrap();
    assert_eq!(stripe_fx_quote_hash(&recovered).unwrap(), hash);
    assert_eq!(recovered.expires_at, Some(3700));
    for (pointer, value) in [
        ("/rates/usd/exchange_rate", json!("0.8")),
        ("/usage/transfer/destination", json!("acct_other")),
        ("/lock_expires_at", json!(3800)),
        ("/id", json!("fxq_other")),
        ("/lock_status", json!("none")),
    ] {
        let mut changed = valuation_quote("expired");
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            canonical_fiat_quote_snapshot(stripe_fx_quote_result(&changed).unwrap(), &hash)
                .is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn funding_preserves_currency_and_excludes_pending_cash() {
    let output = json!({"source_currency":"usd","source_amount_minor":"100"});
    let mut cash = balance(-20, 500);
    cash["available"]
        .as_array_mut()
        .unwrap()
        .push(json!({"currency":"eur","amount":99999}));
    let status = fiat_funding_status(&output, &cash).unwrap();
    assert_eq!(status["shortfall_minor"], "120");
    assert_eq!(status["sufficient"], false);
    assert_eq!(
        fiat_funding_status(&output, &balance(100, 0)).unwrap()["sufficient"],
        true
    );
    let absent = json!({"object":"balance","available":[],"pending":[]});
    assert_eq!(
        fiat_funding_status(&output, &absent).unwrap()["shortfall_minor"],
        "100"
    );
    for invalid in [
        Value::Null,
        json!({"object":"balance","available":[],"pending":null}),
        json!({"object":"balance","available":[{"currency":"usd","amount":"100"}],"pending":[]}),
        json!({"object":"balance","available":[{"currency":"usd","amount":100},{"currency":"usd","amount":100}],"pending":[]}),
    ] {
        assert!(fiat_funding_status(&output, &invalid).is_err());
    }
}

#[test]
fn historical_preparation_keeps_every_economic_field_and_original_signature() {
    let expected = json!({"op":"prepare_targeted_payout","contract_version":CONTRACT_VERSION,
        "rail":"fiat","epoch":7,"economic_op_id":"a".repeat(64),"admin":"b".repeat(64),
        "admin_sig":"c".repeat(128),"payload":{"amount":"100","to":"acct_fixture"},
        "prepared_at":100,"liability":{"paid_cum_au_before":"0"}});
    for version in 27..=u64::from(CONTRACT_VERSION) {
        let mut record = expected.clone();
        record["type"] = json!("targeted_payout_preparation");
        record["consumed"] = json!(false);
        record["contract_version"] = json!(version);
        record["admin_sig"] = json!("d".repeat(128));
        assert!(canonical_payout_preparation_matches(&record, &expected));
        for (field, changed) in [
            ("payload", json!({"amount":"101","to":"acct_fixture"})),
            ("liability", json!({"paid_cum_au_before":"1"})),
            ("admin", json!("e".repeat(64))),
            ("consumed", json!(true)),
            ("rail", json!("tnk")),
            ("epoch", json!(8)),
            ("contract_version", json!(CONTRACT_VERSION + 1)),
            ("admin_sig", json!("invalid")),
        ] {
            let mut invalid = record.clone();
            invalid[field] = changed;
            assert!(
                !canonical_payout_preparation_matches(&invalid, &expected),
                "{field}"
            );
        }
    }
}

async fn stripe_fixture(
    responses: Vec<(&'static str, u16, Value)>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut captured = Vec::new();
        for (route, status, response) in responses {
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let n = stream.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let len = header
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
                assert!(request.len() < 32768);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(
                request.starts_with(route),
                "expected {route}, got {}",
                request.lines().next().unwrap()
            );
            captured.push(request);
            let body = response.to_string();
            stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        captured
    });
    (base, task)
}

fn attempt_fixture() -> (Value, Value, Value, Value, Value) {
    let output = json!({"role":"provider","provider":"a".repeat(64),"payout_revision":"b".repeat(64),
        "economic_op_id":"c".repeat(64),"to":"acct_fixture","output_index":0,
        "source_currency":"usd","source_amount_minor":"100","destination_currency":"usd",
        "destination_amount_min_minor":"100","destination_amount_max_minor":"100",
        "liability_au": AU_PER_USD.to_string(),"paid_au":AU_PER_USD.to_string(),"dust_au":"0"});
    let record = json!({"status":"prepared","preparation":{"epoch":7,"attempt_id":"d".repeat(64),
        "prepared_at":1,"request":{"transfer_group":"test_group","fx_quote_id":null}}});
    let transfer = json!({"id":"tr_fixture","amount":100,"currency":"usd","destination":"acct_fixture",
        "destination_payment":"py_fixture","balance_transaction":"txn_fixture","created":100,
        "reversed":false,"amount_reversed":0,"transfer_group":"test_group","fx_quote":null,
        "metadata":{"mayhem_schema":"fiat_fx_v1","mayhem_operation_hash":"c".repeat(64),
            "mayhem_attempt_id":"d".repeat(64),"mayhem_provider":"a".repeat(64),"mayhem_payout_revision":"b".repeat(64),
            "mayhem_liability_au":AU_PER_USD.to_string(),"mayhem_paid_cum_au_before":"0",
            "mayhem_paid_au":AU_PER_USD.to_string(),"mayhem_fx_quote":"direct-usd",
            "mayhem_epoch":"7","mayhem_output_index":"0","mayhem_epoch_apply_hash":"e".repeat(64)}});
    let payment = json!({"id":"py_fixture","amount":100,"currency":"usd","paid":true,"captured":true,
        "source_transfer":"tr_fixture","balance_transaction":"txn_destination"});
    let transaction = json!({"id":"txn_destination","object":"balance_transaction","type":"payment","source":"py_fixture","amount":100,"currency":"usd","fee":0,"net":100,"exchange_rate":null});
    (output, record, transfer, payment, transaction)
}

#[tokio::test]
async fn funding_then_restart_pays_once_and_recovers_without_balance() {
    let (output, record, transfer, payment, transaction) = attempt_fixture();
    let empty = json!({"object":"list","has_more":false,"data":[]});
    let responses = vec![
        ("GET /v1/transfers?", 200, empty.clone()),
        ("GET /v1/balance ", 200, balance(0, 500)),
        ("GET /v1/transfers?", 200, empty),
        ("GET /v1/balance ", 200, balance(100, 0)),
        ("POST /v1/transfers ", 200, transfer.clone()),
        ("GET /v1/transfers/tr_fixture ", 200, transfer.clone()),
        ("GET /v1/charges/py_fixture ", 200, payment.clone()),
        (
            "GET /v1/balance_transactions/txn_destination ",
            200,
            transaction.clone(),
        ),
        (
            "GET /v1/transfers?",
            200,
            json!({"object":"list","has_more":false,"data":[transfer.clone()]}),
        ),
        ("GET /v1/transfers/tr_fixture ", 200, transfer),
        ("GET /v1/charges/py_fixture ", 200, payment),
        (
            "GET /v1/balance_transactions/txn_destination ",
            200,
            transaction,
        ),
    ];
    let (base, server) = stripe_fixture(responses).await;
    let client = reqwest::Client::new();
    let first = execute_canonical_stripe_attempt(
        &client,
        &base,
        "sk_test_fixture",
        &output,
        &record,
        &"e".repeat(64),
        1,
        0,
    )
    .await;
    let error = first.err().expect("must wait for funds");
    assert_eq!(
        error.downcast_ref::<FiatFundingWait>().unwrap().0["shortfall_minor"],
        "100"
    );
    for _ in 0..2 {
        assert!(matches!(
            execute_canonical_stripe_attempt(
                &reqwest::Client::new(),
                &base,
                "sk_test_fixture",
                &output,
                &record,
                &"e".repeat(64),
                1,
                0
            )
            .await
            .unwrap(),
            CanonicalFiatAttemptExecution::Succeeded { .. }
        ));
    }
    let requests = server.await.unwrap();
    let writes: Vec<_> = requests.iter().filter(|r| r.starts_with("POST ")).collect();
    assert_eq!(writes.len(), 1);
    assert!(writes[0].to_ascii_lowercase().contains(&format!(
        "idempotency-key: {}",
        targeted_fiat_attempt_idempotency_key(&"d".repeat(64))
    )));
}

#[tokio::test]
async fn funding_mode_mismatch_fails_before_transfer() {
    let mut wrong = balance(100, 0);
    wrong["livemode"] = json!(true);
    let (base, server) = stripe_fixture(vec![("GET /v1/balance ", 200, wrong)]).await;
    assert!(require_fiat_output_funding(
        &reqwest::Client::new(),
        &base,
        "sk_test_fixture",
        &json!({"source_currency":"usd","source_amount_minor":"100"}),
        1,
        0
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("mode mismatch"));
    server.await.unwrap();
}

#[tokio::test]
async fn expired_valuation_quote_can_reconcile_paid_output_without_more_cash() {
    let (mut output, mut record, mut transfer, mut payment, mut transaction) = attempt_fixture();
    output["source_currency"] = json!("eur");
    output["destination_currency"] = json!("eur");
    transfer["currency"] = json!("eur");
    transfer["metadata"]["mayhem_fx_quote"] = json!("fxq_fixture");
    payment["currency"] = json!("eur");
    transaction["currency"] = json!("eur");
    let hash =
        stripe_fx_quote_hash(&stripe_fx_quote_result(&valuation_quote("active")).unwrap()).unwrap();
    record["preparation"]["request"]["fx_quote_id"] = json!("fxq_fixture");
    record["preparation"]["request"]["fx_quote_hash"] = json!(hash);
    let (base, server) = stripe_fixture(vec![
        (
            "GET /v1/fx_quotes/fxq_fixture ",
            200,
            valuation_quote("expired"),
        ),
        (
            "GET /v1/transfers?",
            200,
            json!({"object":"list","has_more":false,"data":[transfer.clone()]}),
        ),
        ("GET /v1/transfers/tr_fixture ", 200, transfer),
        ("GET /v1/charges/py_fixture ", 200, payment),
        (
            "GET /v1/balance_transactions/txn_destination ",
            200,
            transaction,
        ),
    ])
    .await;
    match execute_canonical_stripe_attempt(
        &reqwest::Client::new(),
        &base,
        "sk_test_fixture",
        &output,
        &record,
        &"e".repeat(64),
        1,
        0,
    )
    .await
    .unwrap()
    {
        CanonicalFiatAttemptExecution::Succeeded { evidence, .. } => {
            assert_eq!(evidence["fx_quote_hash"], hash)
        }
        _ => panic!("already paid output must reconcile"),
    }
    assert!(server
        .await
        .unwrap()
        .iter()
        .all(|request| request.starts_with("GET ")));
}
