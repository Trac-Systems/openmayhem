use super::*;
use mayhem_proxy::{
    connector::config::{Authentication, ConnectionConfig, NetworkPolicy},
    managed,
    setup::{
        bootstrap::{self, Choices, Credential, Host},
        Flow, FlowAction, FlowConfig, OfferInput, ProbePlan, ProfileMarket, RunTemplate,
    },
};
use std::{
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn host(f: &Fixture) -> Host {
    let bridge_token_file = f.dir.path().join("bridge-token");
    private(&bridge_token_file, b"synthetic-bridge-token\n");
    Host {
        network: f.input.network.clone(),
        provider_pubkey: f.input.provider_pubkey.clone(),
        peer_rpc: "http://127.0.0.1:1/".into(),
        bridge_url: "ws://127.0.0.1:1/".into(),
        bridge_token_file,
        worker_program: PathBuf::from(env!("CARGO_BIN_EXE_mayhem-proxy-worker")),
        wallet_password_file: None,
        admission_origin: None,
    }
}
fn choices(f: &Fixture, endpoint: ProxyEndpoint) -> Choices {
    let tokenizer = if endpoint == ProxyEndpoint::Decisions {
        None
    } else {
        let bytes=serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,
            "pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,"model":{"type":"WordLevel","vocab":{"[UNK]":0,"hello":1,"world":2},"unk_token":"[UNK]"}})).unwrap();
        let file = f.dir.path().join("approved-tokenizer.json");
        private(&file, &bytes);
        Some(managed::Tokenizer {
            file,
            digest: Digest::new(blake3::hash(&bytes).to_hex().to_string()).unwrap(),
            limits: mayhem_proxy::health::native::Limits {
                artifact_bytes: 1024 * 1024,
                output_bytes: 1024 * 1024,
                channels: 16,
                workers: 1,
                minimum_tokens: 2,
            },
        })
    };
    Choices {
        base_url: format!("http://{}/v1/", f.listener.local_addr().unwrap()),
        network_policy: serde_json::from_value(
            json!({"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true}),
        )
        .unwrap(),
        credential: Credential::BearerValue(zeroize::Zeroizing::new(
            b"synthetic-upstream-key".to_vec(),
        )),
        endpoint,
        upstream_model: "external-model".into(),
        market: ProfileMarket::CreateMarket {
            slug: "bootstrap-fixture".into(),
            model: f.input.market.model.clone(),
        },
        served_context: 4096,
        concurrency: 2,
        accepted_rails: vec![ProxyRail::Fiat],
        offers: vec![OfferInput {
            revision: 1,
            ctx_bracket: "ctx4k".into(),
            outcome_class: "".into(),
            rates: Policy::for_endpoint(endpoint)
                .contract()
                .units
                .into_iter()
                .map(|unit| ProxyRate {
                    unit,
                    per_unit_au: 123,
                    granularity: 1000,
                })
                .collect(),
            per_request_au: 2,
            min_session_au: 3,
            accepted_rails: vec![ProxyRail::Fiat],
        }],
        sequence: 1,
        settlement_policy: f.input.settlement_policy.clone(),
        probe_budget: mayhem_proxy::capacity::probes::Budget {
            max_attempts: 2,
            max_cost_microusd: 20,
            per_attempt_cost_microusd: 10,
        },
        probe_output_limit: 32,
        probe_timeout_ms: 3000,
        allow_recovery_probes: true,
        tokenizer,
        closed_retention_ms: 86400000,
    }
}
#[tokio::test]
async fn bootstrap_generates_all_four_standard_profiles_and_validates_run_without_network_or_capacity(
) {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let f = Fixture::new(endpoint);
        let target = f.dir.path().join("bundle");
        let b = bootstrap::create(&target, host(&f), choices(&f, endpoint)).unwrap();
        assert!(
            !b.capacity_created
                && !b.authorizes_probe
                && !b.authorizes_run
                && !b.authorizes_publication
        );
        assert_eq!(b.network_requests, 0);
        let config = FlowConfig::load(&b.config_file).unwrap();
        let input = config.profile.clone().prepare().unwrap();
        assert_eq!(input.offers[0].rates[0].per_unit_au, 123);
        assert_eq!(input.adapter.upstream_model, "external-model");
        assert_eq!(input.settlement_policy, f.input.settlement_policy);
        assert!(input
            .connection_file
            .starts_with(std::fs::canonicalize(&target).unwrap()));
        let flow = Flow::open(config).unwrap();
        let view = flow.view().unwrap();
        assert!(view.review.is_none());
        let exposed = serde_json::to_string(&view).unwrap();
        for hidden in [
            "synthetic-upstream-key",
            "bridge-token",
            "\"secret\"",
            "base_url",
            ".proxy-bootstrap-",
        ] {
            assert!(!exposed.contains(hidden), "{hidden}");
        }
        for filename in [
            "wizard.json",
            "connection.json",
            "probe.json",
            "runtime-policy.json",
            "upstream-key",
        ] {
            let p = target.join(filename);
            assert_eq!(std::fs::metadata(&p).unwrap().mode() & 0o777, 0o600);
            if filename.ends_with(".json") {
                assert!(!std::fs::read_to_string(p)
                    .unwrap()
                    .contains(".proxy-bootstrap-"));
            }
        }
        assert_eq!(std::fs::metadata(&target).unwrap().mode() & 0o777, 0o700);
        assert!(!target.join("runtime/capacity.redb").exists());
        assert!(!target.join("state/draft.json").exists());
        assert!(f.listener.accept().is_err());
        if endpoint != ProxyEndpoint::Decisions {
            std::fs::remove_file(f.dir.path().join("approved-tokenizer.json")).unwrap();
            assert!(RunTemplate::load(&target.join("runtime-policy.json"))
                .unwrap()
                .tokenizer
                .unwrap()
                .file
                .exists());
        }
    }
}
#[tokio::test]
async fn bootstrap_rejects_invalid_secret_network_policy_tokenizer_and_financial_choices_atomically(
) {
    for case in 0..13 {
        let f = Fixture::new(ProxyEndpoint::Chat);
        let target = f.dir.path().join("bundle");
        let mut c = choices(&f, ProxyEndpoint::Chat);
        let h = host(&f);
        match case {
            0 => c.tokenizer = None,
            1 => c.tokenizer.as_mut().unwrap().digest = d(99),
            2 => {
                c.credential = Credential::BearerValue(zeroize::Zeroizing::new(
                    b"key\r\ninjected:true".to_vec(),
                ))
            }
            3 => c.base_url = "https://key:password@example.com/v1/".into(),
            4 => c.network_policy = NetworkPolicy::PublicHttps,
            5 => c.settlement_policy.payable_outcomes = vec![ProxyReceiptOutcome::Running],
            6 => c.offers[0].rates.clear(),
            7 => c.probe_budget.per_attempt_cost_microusd = 21,
            8 => c.probe_timeout_ms = 30001,
            9 => c.concurrency = 0,
            10 => c.base_url = "https://example.com/v1/?token=secret".into(),
            11 => {
                let p = f.dir.path().join("weak-key");
                private(&p, b"key");
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
                c.credential = Credential::BearerFile(p);
            }
            12 => c.closed_retention_ms = 0,
            _ => unreachable!(),
        }
        assert!(bootstrap::create(&target, h, c).is_err(), "case {case}");
        assert!(!target.exists(), "case {case}");
        assert!(!std::fs::read_dir(f.dir.path()).unwrap().any(|v| v
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".proxy-bootstrap-")));
        assert!(f.listener.accept().is_err());
    }
}
#[tokio::test]
async fn bootstrap_never_replaces_existing_or_symlinked_setup_and_preserves_secret_references() {
    let f = Fixture::new(ProxyEndpoint::Decisions);
    let target = f.dir.path().join("bundle");
    let key = f.dir.path().join("private-key");
    private(&key, b"external-secret-reference\n");
    let mut c = choices(&f, ProxyEndpoint::Decisions);
    c.credential = Credential::BearerFile(key.clone());
    bootstrap::create(&target, host(&f), c).unwrap();
    assert!(!target.join("upstream-key").exists());
    assert!(matches!(
        ConnectionConfig::load(&target.join("connection.json"))
            .unwrap()
            .authentication,
        Authentication::Bearer { .. }
    ));
    let original = std::fs::read(target.join("wizard.json")).unwrap();
    assert!(matches!(
        bootstrap::create(&target, host(&f), choices(&f, ProxyEndpoint::Decisions)),
        Err(Error::Conflict)
    ));
    assert_eq!(original, std::fs::read(target.join("wizard.json")).unwrap());
    let link = f.dir.path().join("alias");
    symlink(&target, &link).unwrap();
    assert!(matches!(
        bootstrap::create(&link, host(&f), choices(&f, ProxyEndpoint::Decisions)),
        Err(Error::Conflict)
    ));
    assert_eq!(std::fs::read(key).unwrap(), b"external-secret-reference\n");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_concurrent_first_creation_has_one_atomic_winner() {
    let f = Fixture::new(ProxyEndpoint::Decisions);
    let target = f.dir.path().join("bundle");
    let first = (host(&f), choices(&f, ProxyEndpoint::Decisions));
    let second = (host(&f), choices(&f, ProxyEndpoint::Decisions));
    let barrier = Arc::new(Barrier::new(2));
    let b = barrier.clone();
    let path = target.clone();
    let one = tokio::task::spawn_blocking(move || {
        b.wait();
        bootstrap::create(&path, first.0, first.1)
    });
    let two = tokio::task::spawn_blocking(move || {
        barrier.wait();
        bootstrap::create(&target, second.0, second.1)
    });
    let a = one.await.unwrap();
    let b = two.await.unwrap();
    assert_ne!(a.is_ok(), b.is_ok());
    let bundle = a.or(b).unwrap();
    Flow::open(FlowConfig::load(&bundle.config_file).unwrap()).unwrap();
}

#[tokio::test]
async fn bootstrap_actual_openai_models_stream_probe_reuses_original_allowance_after_restart() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let target = f.dir.path().join("bundle");
    let listener = tokio::net::TcpListener::from_std(f.listener.try_clone().unwrap()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 1024];
            let boundary = loop {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 65536);
                if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..boundary]).to_lowercase();
            assert!(headers.contains("authorization: bearer synthetic-upstream-key"));
            let models = headers.starts_with("get /v1/models ");
            let (kind, body) = if models {
                (
                    "application/json",
                    json!({"object":"list","data":[{"id":"external-model","object":"model"}]})
                        .to_string(),
                )
            } else {
                assert!(headers.starts_with("post /v1/chat/completions "));
                let len = headers
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .map(|s| s.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                while bytes.len() < boundary + len {
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                }
                let value: Value =
                    serde_json::from_slice(&bytes[boundary..boundary + len]).unwrap();
                assert_eq!(value["model"], "external-model");
                assert_eq!(value["stream"], true);
                assert_eq!(value["max_tokens"], 32);
                count.fetch_add(1, Ordering::SeqCst);
                (
                    "text/event-stream",
                    format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        json!({"id":"test","object":"chat.completion.chunk","model":"external-model","choices":[{"index":0,"delta":{"role":"assistant","content":"hello world"},"finish_reason":null}]}),
                        json!({"id":"test","object":"chat.completion.chunk","model":"external-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
                    ),
                )
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let bundle = bootstrap::create(&target, host(&f), choices(&f, ProxyEndpoint::Chat)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let flow = Flow::open(FlowConfig::load(&bundle.config_file).unwrap()).unwrap();
    assert_eq!(
        flow.execute(
            FlowAction::Discover {
                expected_inventory_revision: 0
            },
            None
        )
        .await
        .unwrap()
        .view
        .inventory
        .unwrap()
        .model_count,
        1
    );
    flow.execute(
        FlowAction::Select {
            expected_revision: None,
            choice: flow.view().unwrap().selection,
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
                probe_plan_digest: d(98)
            },
            None
        )
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let probe_plan_digest =
        serde_json::from_value(flow.view().unwrap().probe_plan.unwrap()["digest"].clone()).unwrap();
    let done = flow
        .execute(
            FlowAction::Probe {
                expected_revision: 2,
                probe_plan_digest,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        done.view.review.as_ref().unwrap().probe_status,
        "protocol_validated"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let original_revision = done.view.review.unwrap().revision;
    let reopened = Flow::open(FlowConfig::load(&bundle.config_file).unwrap()).unwrap();
    reopened
        .execute(
            FlowAction::RecoverProbe {
                expected_revision: original_revision,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let probe = ProbePlan::load(&target.join("probe.json")).unwrap();
    let authority = mayhem_proxy::capacity::Authority::open(
        &probe.scope.capacity_file,
        mayhem_proxy::attempts::Identity {
            network_id: f.input.network.network_id.clone(),
            msb_bootstrap: d(3),
            subnet_bootstrap: d(4),
            controller_pubkey: d(1),
        },
        mayhem_proxy::capacity::Limits {
            max_groups: 1024,
            max_routes: 4096,
            max_leases: 65536,
            max_evidence_age: Duration::from_secs(60),
        },
    )
    .unwrap();
    let budget = authority
        .probe_budget(&probe.scope.connection_group)
        .unwrap()
        .unwrap();
    assert_eq!(budget.used_attempts, 1);
    assert_eq!(budget.allocated_cost_microusd, 10);
    if let Some(path) = std::env::var_os("MAYHEM_BOOTSTRAP_EVIDENCE") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"schema_version":1,"fixture":"local_synthetic_openai_stream_with_actual_decoder_worker","profiles_validated":4,"upstream_probe_calls":1,"recovery_did_not_probe":true,"budget":budget,"publication":false,"run":false,"secret_in_review":false})).unwrap()).unwrap();
    }
    server.abort();
    let _ = server.await;
}

/// Explicitly built candidate CLI, normal encrypted-wallet helper, disposable
/// verified assets, and a read-only trusted-peer identity double. No live reads.
#[tokio::test]
#[ignore = "requires explicitly built MAYHEM_SETUP_CLI_BINARY and installed fixture Node dependencies"]
async fn bootstrap_cli_actual_guided_first_bundle_and_conflict_recovery() {
    use std::process::Stdio;
    use tokio::process::Command;
    let binary = PathBuf::from(std::env::var_os("MAYHEM_SETUP_CLI_BINARY").expect("CLI binary"));
    let f = publication::owned(ProxyEndpoint::Decisions, 123);
    let home = f.dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let assets = f.dir.path().join("verified-assets");
    let output = Command::new("node")
        .arg(root.join("crates/mayhem-proxy/tests/setup/run_assets.mjs"))
        .arg(&root)
        .arg(&assets)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "isolated fixture assets failed");
    let password = home.join("fixture-password");
    private(&password, b"synthetic-fixture-password\n");
    private(&home.join("sc-bridge-token"), b"synthetic-bridge-token\n");
    let keypair = home.join("fixture-wallet.json");
    let wallet_script = r#"import fs from 'node:fs';import crypto from 'trac-crypto-api';const publicKey=process.argv[2];const data={publicKey,secretKey:'7b'.repeat(32)+publicKey,mnemonic:null,derivationPath:null};const e=crypto.data.encrypt(Buffer.from(JSON.stringify(data)),Buffer.from('synthetic-fixture-password'));fs.writeFileSync(process.argv[1],JSON.stringify({nonce:e.nonce.toString('hex'),salt:e.salt.toString('hex'),ciphertext:e.ciphertext.toString('hex')}),{mode:0o600});"#;
    assert!(Command::new("node")
        .args(["--input-type=module", "-e", wallet_script])
        .arg(&keypair)
        .arg(f.input.provider_pubkey.as_str())
        .current_dir(root.join("intercom"))
        .output()
        .await
        .unwrap()
        .status
        .success());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    private(&home.join("config.toml"),format!("[network]\nrpc_url='http://{address}'\nsc_bridge_url='ws://127.0.0.1:1/'\nmsb_bootstrap='{}'\nsubnet_bootstrap='{}'\nadmin_peer_pubkey='{}'\n",d(3).as_str(),d(4).as_str(),d(5).as_str()).as_bytes());
    let reads = Arc::new(AtomicUsize::new(0));
    let count = reads.clone();
    let peer = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 2048];
            while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 16384);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with("GET "));
            let body = if request.starts_with("GET /status ") {
                json!({"peer":{"admin":d(5),"subnetBootstrapHex":d(4)},"msb":{"networkId":918,"bootstrapHex":d(3)}})
            } else if request.starts_with("GET /health ") {
                json!({"contract_version":mayhem_proto::CONTRACT_VERSION})
            } else {
                assert!(request.starts_with("GET /state?"));
                assert!(request.contains("key=admin"));
                json!({"value":d(5)})
            };
            count.fetch_add(1, Ordering::SeqCst);
            let body = body.to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let mut command = Command::new(&binary);
    command
        .args(["provider", "proxy", "setup", "init", "--home"])
        .arg(&home)
        .arg("--keypair")
        .arg(&keypair)
        .arg("--restart-password-file")
        .arg(&password)
        .env("MAYHEM_ASSET_DIR", &assets)
        .env_remove("MAYHEM_WALLET_PASSWORD")
        .env_remove("MAYHEM_HOME")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let lines = [
        "https://example.invalid/v1/",
        "decisions",
        "yes",
        "none",
        "external",
        "other",
        "Public model",
        "bootstrap-market",
        "4096",
        "2",
        "fiat",
        "1",
        "123",
        "7",
        "9",
        "unclassified",
        "1",
        "no",
        "no",
        "no",
        "no",
        "none",
        "3",
        "20",
        "5",
        "32",
        "3000",
        "yes",
        "7",
        "yes",
        "no",
    ]
    .join("\n")
        + "\n";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(lines.as_bytes())
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "CLI setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    for hidden in [
        "synthetic-fixture-password",
        "synthetic-bridge-token",
        "secretKey",
    ] {
        assert!(!stdout.contains(hidden));
    }
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    let target = home.join("proxy-setup");
    let cfg = FlowConfig::load(&target.join("wizard.json")).unwrap();
    assert_eq!(cfg.profile.provider_pubkey, f.input.provider_pubkey);
    assert_eq!(cfg.profile.network.network_id, "918");
    assert_eq!(cfg.profile.offers[0].per_request_au, 7);
    assert_eq!(cfg.profile.offers[0].min_session_au, 9);
    let flow = Flow::open(cfg).unwrap();
    let original = std::fs::read(target.join("wizard.json")).unwrap();
    let retry = command.stdin(Stdio::null()).output().await.unwrap();
    assert!(!retry.status.success());
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    assert_eq!(original, std::fs::read(target.join("wizard.json")).unwrap());
    assert!(flow.view().unwrap().review.is_none());
    assert!(!target.join("runtime/capacity.redb").exists());
    if let Some(path) = std::env::var_os("MAYHEM_BOOTSTRAP_CLI_EVIDENCE") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"schema_version":1,"fixture":"actual_cli_guided_create_with_normal_synthetic_encrypted_wallet_and_read_only_peer_double","network_identity_reads":3,"upstream_calls":0,"hand_authored_setup_json":false,"same_wallet":true,"retained_after_retry":true,"capacity_created":false,"published":false,"run":false})).unwrap()).unwrap();
    }
    peer.abort();
    let _ = peer.await;
}
