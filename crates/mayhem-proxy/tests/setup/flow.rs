use super::*;
use mayhem_proxy::setup::{Flow, FlowAction, FlowConfig, ProfileInput};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(super) fn config(f: &Fixture) -> FlowConfig {
    let i = &f.input;
    let profile:ProfileInput=serde_json::from_value(json!({"schema_version":1,"network":i.network,"provider_pubkey":i.provider_pubkey,"connection_file":i.connection_file,
 "profile":{"kind":"custom","endpoint":i.adapter.endpoint,"contract":i.adapter.contract},"upstream_model":i.adapter.upstream_model,"limits":i.adapter.limits,
 "market":{"action":"create_market","slug":i.market.slug,"model":i.market.model},
 "membership":{"revision":i.membership.revision,"served_context":i.membership.served_context,"max_concurrency":i.membership.max_concurrency,"capacity_group":i.membership.capacity_group,"accepted_rails":i.membership.accepted_rails},
 "offers":i.offers.iter().map(|o|json!({"revision":o.revision,"ctx_bracket":o.ctx_bracket,"outcome_class":o.outcome_class,"rates":o.rates,"per_request_au":o.per_request_au.to_string(),"min_session_au":o.min_session_au.to_string(),"accepted_rails":o.accepted_rails})).collect::<Vec<_>>(),
 "sequence":i.sequence,"settlement_policy":i.settlement_policy})).unwrap();
    FlowConfig {
        schema_version: 1,
        directory: f.store.clone(),
        profile,
        probe_plan: None,
        peer_rpc: None,
        admission_origin: None,
        declaration_registry: None,
        timeout_ms: 2000,
        run: None,
    }
}
#[tokio::test]
async fn shared_flow_retains_exact_choices_across_restart_and_stale_clients_without_private_exports(
) {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let flow = Flow::open(config(&f)).unwrap();
    assert!(flow.view().unwrap().review.is_none());
    assert_eq!(
        flow.execute(FlowAction::Connect {}, None)
            .await
            .unwrap()
            .action_result["network_request"],
        false
    );
    let mut choice = flow.view().unwrap().selection;
    choice.offers[0].per_request_au = u128::MAX / 2;
    let saved = flow
        .execute(
            FlowAction::Select {
                expected_revision: None,
                choice: choice.clone(),
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(saved.view.review.unwrap().revision, 1);
    let checked = flow
        .execute(
            FlowAction::Check {
                expected_revision: 1,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(checked.view.review.as_ref().unwrap().revision, 2);
    let restarted = Flow::open(config(&f)).unwrap();
    let v = restarted.view().unwrap();
    assert_eq!(v.selection.offers[0].per_request_au, u128::MAX / 2);
    assert!(matches!(
        flow.execute(
            FlowAction::Select {
                expected_revision: Some(1),
                choice: choice.clone()
            },
            None
        )
        .await,
        Err(Error::Conflict)
    ));
    assert_eq!(restarted.view().unwrap().review.unwrap().revision, 2);
    let exposed = serde_json::to_string(&v).unwrap();
    for hidden in [
        "never-read-secret",
        "127.0.0.1",
        "connection_file",
        "worker_program",
        "private-connection.json",
    ] {
        assert!(!exposed.contains(hidden), "{hidden}");
    }
    assert_eq!(v.capabilities["run"], false);
    assert_eq!(v.review.unwrap().serving_status, "not_started");
    assert!(flow
        .execute(
            FlowAction::Enrollment {
                expected_revision: 2,
                operation: mayhem_proxy::setup::EnrollmentAction::Create,
                rail: Some(ProxyRail::Fiat)
            },
            None
        )
        .await
        .is_err());
    assert!(flow
        .execute(
            FlowAction::Publish {
                expected_revision: 2,
                offers_only: false,
                plan_digest: d(99)
            },
            None
        )
        .await
        .is_err());
    assert!(!f.store.join("wizard-enrollment.json").exists());
    f.no_network_or_secret();
}
#[tokio::test]
async fn flow_real_http_discovery_to_explicit_worker_probe_preserves_budget_and_original_revision()
{
    let mut f = Fixture::new(ProxyEndpoint::Chat);
    f.connection
        .as_object_mut()
        .unwrap()
        .remove("authentication");
    f.connection["paths"]["models"] = json!("models");
    f.connection["error_profile"] = json!("open_ai");
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    let listener = tokio::net::TcpListener::from_std(f.listener.try_clone().unwrap()).unwrap();
    let server = tokio::spawn(async move {
        for expected in ["GET /v1/models ", "POST /v1/chat/completions "] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 4096];
            while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let at = bytes.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
            let header = String::from_utf8_lossy(&bytes[..at]);
            assert!(header.starts_with(expected));
            assert!(!header.to_ascii_lowercase().contains("authorization:"));
            let length = header
                .lines()
                .find_map(|s| {
                    s.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while bytes.len() < at + length {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            if length > 0 {
                let value: Value = serde_json::from_slice(&bytes[at..at + length]).unwrap();
                assert_eq!(value["model"], "private-upstream-model");
            }
            let body=if expected.starts_with("GET"){json!({"object":"list","data":[{"id":"private-upstream-model"}],"secret":"discarded","capacity":99999})}else{json!({"id":"local","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]})}.to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let work = f.dir.path().join("worker");
    std::fs::create_dir(&work).unwrap();
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = f.dir.path().join("probe.json");
    private(&path,&serde_json::to_vec(&json!({"schema_version":1,"scope":{"capacity_file":f.dir.path().join("capacity.redb"),"route":d(50),"connection_group":d(2),"connection_ceiling":2,"route_ceiling":2,"constraints":[{"id":d(51),"ceiling":2}]},"budget":{"max_attempts":1,"max_cost_microusd":10,"per_attempt_cost_microusd":10},"worker_program":env!("CARGO_BIN_EXE_mayhem-proxy-worker"),"worker_directory":work,"request":{"model":"public","messages":[{"role":"user","content":"fixture"}],"max_tokens":16},"streaming":false,"max_output_tokens":16,"timeout_ms":2000})).unwrap());
    let mut cfg = config(&f);
    cfg.probe_plan = Some(path.clone());
    let flow = Flow::open(cfg.clone()).unwrap();
    let inventory = flow
        .execute(
            FlowAction::Discover {
                expected_inventory_revision: 0,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(inventory.view.inventory.unwrap().model_count, 1);
    let choice = flow.view().unwrap().selection;
    flow.execute(
        FlowAction::Select {
            expected_revision: None,
            choice,
        },
        None,
    )
    .await
    .unwrap();
    flow.execute(
        FlowAction::Check {
            expected_revision: 1,
        },
        None,
    )
    .await
    .unwrap();
    assert!(flow
        .execute(
            FlowAction::Probe {
                expected_revision: 2,
                probe_plan_digest: d(99)
            },
            None
        )
        .await
        .is_err());
    let digest =
        serde_json::from_value(flow.view().unwrap().probe_plan.unwrap()["digest"].clone()).unwrap();
    let result = flow
        .execute(
            FlowAction::Probe {
                expected_revision: 2,
                probe_plan_digest: digest,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        result.view.review.as_ref().unwrap().probe_status,
        "protocol_validated"
    );
    let revision = result.view.review.unwrap().revision;
    let restored = Flow::open(cfg).unwrap();
    assert_eq!(restored.view().unwrap().review.unwrap().revision, revision);
    let digest =
        serde_json::from_value(restored.view().unwrap().probe_plan.unwrap()["digest"].clone())
            .unwrap();
    let reused = restored
        .execute(
            FlowAction::Probe {
                expected_revision: revision,
                probe_plan_digest: digest,
            },
            None,
        )
        .await
        .unwrap();
    assert_ne!(
        reused.view.review.unwrap().probe_status,
        "protocol_validated",
        "exhausted probe allowance cannot confer a new successful check"
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
#[test]
fn browser_actions_cannot_choose_private_paths_wallets_origins_or_extend_signing_bodies() {
    for value in [
        json!({"action":"connect","path":"/etc/passwd"}),
        json!({"action":"enrollment","expected_revision":1,"operation":"create","rail":"fiat","receiver":"caller"}),
        json!({"action":"publish","expected_revision":1,"offers_only":false,"plan_digest":d(1),"body":{}}),
        json!({"action":"run"}),
    ] {
        assert!(serde_json::from_value::<FlowAction>(value).is_err());
    }
    let f = Fixture::new(ProxyEndpoint::Chat);
    for origin in [
        "http://remote.invalid",
        "https://example.invalid/path",
        "https://user@example.invalid",
    ] {
        let mut cfg = config(&f);
        cfg.admission_origin = Some(origin.into());
        assert!(Flow::open(cfg).is_err());
    }
    f.no_network_or_secret();
}

#[tokio::test]
async fn flow_scoped_signed_enrollment_retains_short_payment_across_checkout_and_reconciles_original(
) {
    use ed25519_dalek::{Signature, SigningKey, Verifier};
    let f = Fixture::new(ProxyEndpoint::Chat);
    let key = SigningKey::from_bytes(&[201; 32]);
    let mut cfg = config(&f);
    cfg.profile.provider_pubkey = Digest::new(
        key.verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    cfg.admission_origin = Some(origin.clone());
    let network = cfg.profile.network.clone();
    let pubkey = key.verifying_key();
    let server = tokio::spawn(async move {
        fn canonical(domain: &str, v: &Value) -> Vec<u8> {
            let mut b = domain.as_bytes().to_vec();
            b.push(0);
            b.extend(mayhem_proto::stable_json_bytes(v).unwrap());
            b
        }
        fn hash(domain: &str, v: &Value) -> String {
            blake3::hash(&canonical(domain, v)).to_hex().to_string()
        }
        async fn read(l: &tokio::net::TcpListener) -> (tokio::net::TcpStream, String, Value) {
            let (mut socket, _) = l.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 4096];
            while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let at = bytes.windows(4).position(|b| b == b"\r\n\r\n").unwrap() + 4;
            let header = String::from_utf8(bytes[..at].to_vec()).unwrap();
            let len = header
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap();
            while bytes.len() < at + len {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            (
                socket,
                header,
                serde_json::from_slice(&bytes[at..at + len]).unwrap(),
            )
        }
        async fn reply(mut socket: tokio::net::TcpStream, v: Value) {
            let body = v.to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
        let mut retained_operation = None;
        for action in ["invoice_create", "invoice_checkout", "invoice_status"] {
            let (socket, header, request) = read(&listener).await;
            assert!(header.starts_with("POST /v1/proxy/admission/auth/challenge "));
            assert_eq!(request["action"], action);
            let operation = request["initial_operation_digest"].clone();
            if let Some(previous) = &retained_operation {
                assert_eq!(previous, &operation)
            } else {
                retained_operation = Some(operation.clone());
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let operation_id = hash(
                "mayhem/proxy/admission-operation/v1",
                &json!({"network":network,"public_origin":origin,"provider_pubkey":request["provider_pubkey"],"initial_operation_digest":operation}),
            );
            let challenge_id = hash(
                "mayhem/proxy/admission-challenge-id/v1",
                &json!({"network":network,"public_origin":origin,"provider_pubkey":request["provider_pubkey"],"client_nonce":request["client_nonce"]}),
            );
            let claims = json!({"schema_version":1,"purpose":"proxy_admission_fee","network":network,"public_origin":origin,"provider_pubkey":request["provider_pubkey"],"initial_operation_digest":operation,"operation_id":operation_id,"action":action,"request_digest":request["request_digest"],"issued_at_ms":now,"expires_at_ms":now+60000});
            let mut challenge = claims.clone();
            challenge["challenge_id"] = json!(challenge_id);
            challenge["nonce"] = json!(d(92));
            challenge["client_nonce"] = request["client_nonce"].clone();
            reply(socket,json!({"schema_version":1,"purpose":"proxy_admission_fee","signing_domain":"mayhem/proxy/admission-auth/v1","challenge":challenge})).await;
            let (socket, header, signature) = read(&listener).await;
            assert!(header.starts_with("POST /v1/proxy/admission/auth/verify "));
            assert_eq!(signature["challenge_id"], challenge_id);
            let signature = signature["provider_signature"].as_str().unwrap();
            let bytes = (0..signature.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&signature[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>();
            pubkey
                .verify(
                    &canonical("mayhem/proxy/admission-auth/v1", &challenge),
                    &Signature::from_slice(&bytes).unwrap(),
                )
                .unwrap();
            reply(socket,json!({"schema_version":1,"purpose":"proxy_admission_fee","authorization":{"scheme":"ProxyAdmission","token":d(93),"claims":claims}})).await;
            let (socket, header, invoice_request) = read(&listener).await;
            assert!(header.starts_with(&format!(
                "POST /v1/proxy/admission/invoice/{} ",
                action.trim_start_matches("invoice_")
            )));
            assert!(header
                .to_ascii_lowercase()
                .contains("authorization: proxyadmission "));
            assert_eq!(invoice_request["operation_id"], operation_id);
            assert_eq!(
                hash(
                    "mayhem/proxy/admission-request/v1",
                    &json!({"action":action,"request":invoice_request["request"]})
                ),
                request["request_digest"]
            );
            let response = if action == "invoice_checkout" {
                json!({"state":"checkout","url":"https://checkout.stripe.com/c/pay/synthetic-local-only"})
            } else {
                json!({"schema_version":1,"purpose":"proxy_admission_fee","state":"invoice","invoice_id":"same_invoice","provider_pubkey":request["provider_pubkey"],"initial_operation_digest":operation,"rail":"fiat","payment_status":if action=="invoice_create"{"short_payment"}else{"issuing"},"quote_expires_at_ms":now+30000,"quote_expired":false,"fee_usd":"10.00","amount_base_units":"1000","received_amount_base_units":if action=="invoice_create"{"250"}else{"1000"},"missing_amount_base_units":if action=="invoice_create"{"750"}else{"0"},"excess_amount_base_units":"0","collection":{"currency":"usd"},"review_code":null,"permit":null,"publication_status":"not_checked","replayed":true})
            };
            reply(socket, response).await;
        }
    });
    let flow = Flow::open(cfg.clone()).unwrap();
    let choice = flow.view().unwrap().selection;
    flow.execute(
        FlowAction::Select {
            expected_revision: None,
            choice,
        },
        None,
    )
    .await
    .unwrap();
    flow.execute(
        FlowAction::Check {
            expected_revision: 1,
        },
        None,
    )
    .await
    .unwrap();
    let create = flow
        .execute(
            FlowAction::Enrollment {
                expected_revision: 2,
                operation: mayhem_proxy::setup::EnrollmentAction::Create,
                rail: Some(ProxyRail::Fiat),
            },
            Some(&key),
        )
        .await
        .unwrap();
    assert_eq!(
        create.view.enrollment.as_ref().unwrap()["invoice"]["missing_amount_base_units"],
        "750"
    );
    let before = std::fs::read(f.store.join("wizard-enrollment.json")).unwrap();
    let checkout = flow
        .execute(
            FlowAction::Enrollment {
                expected_revision: 2,
                operation: mayhem_proxy::setup::EnrollmentAction::Checkout,
                rail: None,
            },
            Some(&key),
        )
        .await
        .unwrap();
    assert!(checkout.action_result["checkout_url"]
        .as_str()
        .unwrap()
        .starts_with("https://checkout.stripe.com/"));
    assert_eq!(
        std::fs::read(f.store.join("wizard-enrollment.json")).unwrap(),
        before
    );
    assert_eq!(
        checkout.view.enrollment.unwrap()["invoice"]["invoice_id"],
        "same_invoice"
    );
    let restarted = Flow::open(cfg).unwrap();
    let status = restarted
        .execute(
            FlowAction::Enrollment {
                expected_revision: 2,
                operation: mayhem_proxy::setup::EnrollmentAction::Status,
                rail: None,
            },
            Some(&key),
        )
        .await
        .unwrap();
    assert_eq!(
        status.view.enrollment.as_ref().unwrap()["invoice"]["missing_amount_base_units"],
        "0"
    );
    assert_eq!(
        status.view.enrollment.unwrap()["authorizes_publication"],
        false
    );
    let retained = std::fs::read_to_string(f.store.join("wizard-enrollment.json")).unwrap();
    for secret in [
        d(93).as_str(),
        "checkout.stripe.com",
        "provider_signature",
        "client_nonce",
        "authorization",
    ] {
        assert!(!retained.contains(secret));
    }
    assert_eq!(restarted.view().unwrap().review.unwrap().revision, 2);
    server.await.unwrap();
    f.no_network_or_secret();
}
