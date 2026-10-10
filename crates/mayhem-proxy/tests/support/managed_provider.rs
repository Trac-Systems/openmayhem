use super::*;
use ed25519_dalek::SigningKey;
use mayhem_proxy::{managed, signing::Authority};
use std::path::{Path, PathBuf};
use tokio::sync::watch;
#[allow(dead_code)]
#[path = "exchange_bridge.rs"]
mod bridge;

pub(crate) fn private(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn key() -> SigningKey {
    SigningKey::from_bytes(&[87; 32])
}
fn public_key() -> String {
    key()
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn save(root: &Path, value: &Value) -> PathBuf {
    let path = root.join("provider.json");
    private(&path, &serde_json::to_vec(value).unwrap());
    path
}
pub(crate) fn config(root: &Path, base: &str, bridge_url: &str, endpoint: ProxyEndpoint) -> Value {
    private(&root.join("bridge-token"), b"test-provider\n");
    private(&root.join("connection.json"), &serde_json::to_vec(&json!({"schema_version":1,"id":"fixture","revision":1,"base_url":format!("{}/",base.trim_end_matches('/')),
        "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},
        "paths":{"chat_completions":"chat/completions","decisions":"decisions"},"error_profile":"open_ai"})).unwrap());
    let family = if endpoint == ProxyEndpoint::Decisions {
        mayhem_proto::ENDPOINT_MAYHEM_DECISIONS
    } else {
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS
    };
    let adapter = Adapter::new(
        endpoint,
        endpoint_family_contract_template(family).unwrap(),
        "upstream-model".into(),
        Limits {
            request_bytes: 1024 * 1024,
            response_bytes: 1024 * 1024,
            choices: 8,
            tools: 16,
            questions: 16,
            decision_options: 32,
        },
    )
    .unwrap();
    let tokenizer=serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,"model":{"type":"WordLevel","vocab":{"[UNK]":0,"one":1,"two":2,"three":3,"four":4},"unk_token":"[UNK]"}})).unwrap();
    private(&root.join("tokenizer.json"), &tokenizer);
    let schedule = json!({"interval_ms":1000,"page_pause_ms":1,"retry_initial_ms":100,"retry_max_ms":1000,"jitter_percent":0});
    let pool = PoolLimits {
        max_children: 2,
        max_buffer_bytes: 64 * 1024 * 1024,
        startup_timeout: Duration::from_secs(5),
        processing_timeout: Duration::from_secs(3),
    };
    let probe = if endpoint == ProxyEndpoint::Decisions {
        json!({"request":{"model":"public-model","state":"hi","questions":{"q":{"type":"noul","instructions":"hello?"}}},"streaming":false,"max_output_tokens":32,"timeout_ms":5000})
    } else {
        json!({"request":{"model":"public-model","messages":[{"role":"user","content":"hello"}],"max_tokens":32,"stream":true},"streaming":true,"max_output_tokens":32,"timeout_ms":5000})
    };
    json!({"schema_version":1,"network":{"network_id":"918","msb_bootstrap":d(1),"subnet_bootstrap":d(2),"contract_version":mayhem_proto::CONTRACT_VERSION},
        "provider_pubkey":public_key(),"peer_rpc_url":"http://127.0.0.1:1/v1","bridge":{"url":bridge_url,"token_file":"bridge-token","operation_timeout_ms":5000,"frame_bytes":65536,"queue_events":16,"queue_bytes":2097152,"logical_message_bytes":4194304},
        "state_dir":"state","worker_program":env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
        "health":{"max_routes":8,"max_classes_per_route":8,"evidence_ttl_ms":60000,"successes_to_increase":2,"bad_samples_to_reduce":2,"latency_baseline_samples":3,"latency_multiplier":4,"latency_increase_ms":1000,"min_native_tok_s":5,"recovery":schedule},
        "limits":{"max_groups":8,"max_routes":8,"max_leases":16,"tokenizer_bytes":4194304,"tokenizer_workers":4,"financial_reads":4,"sessions":8,"registrations":8,"observation_ms":50,"recovery_interval_ms":50,
            "serving":{"sessions":4,"per_buyer":2,"outbound_messages":16,"outbound_bytes":4194304,"control_wait":Duration::from_secs(5),"proposals":{"pending":4,"per_buyer":2,"request_bytes":1048576,"total_request_bytes":2097152,"storage_operations":4,"unsigned_lifetime":Duration::from_secs(30)}},
            "worker":pool,"recovery_worker":pool,"settlement_worker":pool,"journal":{"max_records":50,"max_unfinished":16,"closed_retention_ms":1000,"max_payload_bytes":134217728},"maintenance":{"page_size":4,"schedule":schedule}},
        "connections":[{"group":d(10),"ceiling":2,"config_file":"connection.json","probe_budget":{"max_attempts":1,"max_cost_microusd":10,"per_attempt_cost_microusd":10}}],
        "allocations":[{"id":d(11),"ceiling":2}],"routes":[{"id":d(20),"connection":d(10),"ceiling":2,"constraints":[d(11)],"adapter":adapter.snapshot(),
            "offers":[{"schema_version":1,"lane":"proxy","market_id":d(30),"provider_pubkey":public_key(),"membership_revision":1,"revision":1,"endpoint":endpoint,"ctx_bracket":"ctx_8k","outcome_class":"","metering_policy_hash":d(31),"rates":[{"unit":"input","per_unit_au":"1","granularity":1}],"per_request_au":"1","min_session_au":"0","accepted_rails":["fiat","tap","tnk"]}],
            "settlement_policy":{"schema_version":1,"lane":"proxy","payable_outcomes":["complete"],"allow_checkpoints":false},
            "tokenizer":if endpoint==ProxyEndpoint::Decisions {Value::Null} else {json!({"file":"tokenizer.json","digest":blake3::hash(&tokenizer).to_hex().as_str(),"limits":{"artifact_bytes":1048576,"output_bytes":1048576,"channels":16,"workers":2,"minimum_tokens":2}})},"recovery":probe}]})
}
fn open(root: &Path, value: &Value) -> managed::Provider {
    let c: managed::Config = serde_json::from_value(value.clone()).unwrap();
    for route in c.routes {
        for offer in route.offers {
            offer.validate().unwrap();
        }
        route.settlement_policy.validate().unwrap();
    }
    let prepared = managed::Prepared::load(&save(root, value)).unwrap();
    let signer =
        Arc::new(Authority::from_unlocked_wallet(key(), prepared.identity().clone()).unwrap());
    prepared.open(signer).unwrap()
}
async fn ready(health: &mut watch::Receiver<managed::Health>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if health
                .borrow()
                .routes
                .get(&d(20))
                .is_some_and(|v| v.allowance > 0)
            {
                return;
            }
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn managed_provider_bootstraps_real_stream_then_restart_preserves_spent_budget_and_requires_fresh_speed(
) {
    let backend =
        native_execution::paced(native_execution::chat_pieces(Duration::from_millis(100))).await;
    let root = dir();
    let bridge = bridge::Bridge::start(d(70).as_str(), &public_key()).await;
    let value = config(
        root.path(),
        &backend.base,
        bridge.config(false).url.as_str(),
        ProxyEndpoint::Chat,
    );
    let provider = open(root.path(), &value);
    let (view, slots) = provider.route_status(&d(20)).unwrap();
    assert_eq!(view.allowance, 0);
    assert_eq!(slots.available, 0);
    let (stop, rx) = watch::channel(false);
    let (updates, mut health) = watch::channel(managed::Health::default());
    let task = tokio::spawn(provider.run(rx, updates));
    ready(&mut health).await;
    assert!(health.borrow().routes[&d(20)].meets_native_floor(5));
    tokio::time::sleep(Duration::from_millis(160)).await;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    stop.send_replace(true);
    task.await.unwrap().unwrap();
    assert!(!health.borrow().serving.running);
    assert!(
        health.borrow().routes.contains_key(&d(20)),
        "final diagnosis retains the bounded route snapshot"
    );
    let provider = open(root.path(), &value);
    assert_eq!(provider.route_status(&d(20)).unwrap().1.available, 0);
    let (stop, rx) = watch::channel(false);
    let (updates, health) = watch::channel(managed::Health::default());
    let task = tokio::spawn(provider.run(rx, updates));
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert!(health.borrow().serving.running);
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        1,
        "restart must not refill spent probes"
    );
    assert_eq!(health.borrow().routes[&d(20)].allowance, 0);
    stop.send_replace(true);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn managed_provider_decision_readiness_requires_real_validated_result_without_llm_token_floor(
) {
    let backend = backend(
        200,
        json!({"id":"u","answers":{"q":{"type":"noul","noul":0.7}}}),
        Duration::ZERO,
    )
    .await;
    let root = dir();
    let bridge = bridge::Bridge::start(d(70).as_str(), &public_key()).await;
    let value = config(
        root.path(),
        &backend.base,
        bridge.config(false).url.as_str(),
        ProxyEndpoint::Decisions,
    );
    let provider = open(root.path(), &value);
    assert_eq!(provider.route_status(&d(20)).unwrap().1.available, 0);
    let (stop, rx) = watch::channel(false);
    let (updates, mut health) = watch::channel(managed::Health::default());
    let task = tokio::spawn(provider.run(rx, updates));
    ready(&mut health).await;
    assert!(health.borrow().routes[&d(20)].native_speed.is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    stop.send_replace(true);
    task.await.unwrap().unwrap();
    assert!(
        !bridge
            .frames
            .lock()
            .await
            .iter()
            .any(|f| f["type"] == "send"),
        "healthy local backend must not publish without canonical admission/offer evidence"
    );
}

#[tokio::test]
async fn managed_provider_bad_bridge_does_not_spend_probe_budget_or_start_inference() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let root = dir();
    let value = config(
        root.path(),
        &backend.base,
        "ws://127.0.0.1:1",
        ProxyEndpoint::Chat,
    );
    let provider = open(root.path(), &value);
    let (_stop, rx) = watch::channel(false);
    let (updates, _) = watch::channel(managed::Health::default());
    assert!(provider.run(rx, updates).await.is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn managed_provider_rejects_identity_policy_pin_and_scope_errors_before_creating_state() {
    let root = dir();
    let value = config(
        root.path(),
        "http://127.0.0.1:1/v1",
        "ws://127.0.0.1:1",
        ProxyEndpoint::Chat,
    );
    for (path, replacement) in [
        ("/schema_version", json!(2)),
        ("/network/contract_version", json!(1)),
        ("/routes/0/tokenizer", Value::Null),
        ("/routes/0/tokenizer/digest", json!(d(999))),
        ("/routes/0/offers/0/provider_pubkey", json!(d(888))),
        ("/routes/0/constraints", json!([d(999)])),
        ("/routes/0/recovery/streaming", json!(false)),
        ("/limits/tokenizer_bytes", json!(8)),
        ("/routes/0/ceiling", json!(3)),
        ("/bridge/url", json!("ws://example.com")),
        ("/health/min_native_tok_s", json!(4)),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(path).unwrap() = replacement;
        assert!(
            managed::Prepared::load(&save(root.path(), &changed)).is_err(),
            "{path}"
        );
        assert!(!root.path().join("state").exists());
    }
    let prepared = managed::Prepared::load(&save(root.path(), &value)).unwrap();
    let mut identity = prepared.identity().clone();
    let other = SigningKey::from_bytes(&[88; 32]);
    identity.controller_pubkey = Digest::new(
        other
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    let signer = Arc::new(Authority::from_unlocked_wallet(other, identity).unwrap());
    assert!(matches!(
        prepared.open(signer),
        Err(managed::Error::Identity)
    ));
    assert!(!root.path().join("state").exists());
    let path = save(root.path(), &value);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(managed::Prepared::load(&path).is_err());
    let link = root.path().join("link.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(managed::Prepared::load(&link).is_err());
}

#[test]
fn managed_supervised_credentials_require_restartable_files_before_opening_stores() {
    let root = dir();
    let value = config(
        root.path(),
        "http://127.0.0.1:9",
        "ws://127.0.0.1:9",
        ProxyEndpoint::Decisions,
    );
    let path = save(root.path(), &value);
    let connection = root.path().join("connection.json");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&connection).unwrap()).unwrap();
    managed::Prepared::load_supervised(&path).unwrap();
    for kind in ["bearer", "header"] {
        document["authentication"] = json!({"type":kind,"secret":{"source":"environment","name":"PROXY_TEST_INSTALL_SHELL_ONLY"}});
        if kind == "header" {
            document["authentication"]["name"] = json!("x-api-key");
        }
        private(&connection, &serde_json::to_vec(&document).unwrap());
        assert!(matches!(
            managed::Prepared::load_supervised(&path),
            Err(managed::Error::RestartCredential)
        ));
        assert!(!root.path().join("state").exists());
    }
    private(&root.path().join("upstream-secret"), b"test-fixture");
    document["authentication"] =
        json!({"type":"bearer","secret":{"source":"file","path":"upstream-secret"}});
    private(&connection, &serde_json::to_vec(&document).unwrap());
    managed::Prepared::load_supervised(&path).unwrap();
    assert!(!root.path().join("state").exists());
}

#[tokio::test]
async fn managed_provider_aliases_share_health_and_durable_probe_budget() {
    let backend =
        native_execution::paced(native_execution::chat_pieces(Duration::from_millis(100))).await;
    let root = dir();
    let bridge = bridge::Bridge::start(d(70).as_str(), &public_key()).await;
    let mut value = config(
        root.path(),
        &backend.base,
        bridge.config(false).url.as_str(),
        ProxyEndpoint::Chat,
    );
    let mut alias = value["routes"][0].clone();
    alias["id"] = json!(d(21));
    alias["offers"][0]["market_id"] = json!(d(32));
    value["routes"].as_array_mut().unwrap().push(alias);
    let provider = open(root.path(), &value);
    let (stop, rx) = watch::channel(false);
    let (updates, health) = watch::channel(managed::Health::default());
    let task = tokio::spawn(provider.run(rx, updates));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if health.borrow().routes.values().any(|r| r.allowance > 0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        1,
        "shared credential allowance cannot multiply per alias"
    );
    assert_eq!(
        health
            .borrow()
            .routes
            .values()
            .filter(|r| r.allowance > 0)
            .count(),
        1,
        "one model probe cannot qualify another model's route"
    );
    stop.send_replace(true);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn managed_provider_uncertain_probe_retains_capacity_and_does_not_resend_after_restart() {
    let backend = backend(
        200,
        json!({"id":"u","answers":{"q":{"type":"noul","noul":0.7}}}),
        Duration::from_secs(1),
    )
    .await;
    let root = dir();
    let bridge = bridge::Bridge::start(d(70).as_str(), &public_key()).await;
    let mut value = config(
        root.path(),
        &backend.base,
        bridge.config(false).url.as_str(),
        ProxyEndpoint::Decisions,
    );
    value["routes"][0]["recovery"]["timeout_ms"] = json!(100);
    value["connections"][0]["probe_budget"]["max_attempts"] = json!(5);
    value["connections"][0]["probe_budget"]["max_cost_microusd"] = json!(50);
    for pass in 0..2 {
        let provider = open(root.path(), &value);
        if pass == 1 {
            assert_eq!(provider.route_status(&d(20)).unwrap().1.group_occupied, 1);
        }
        let (stop, rx) = watch::channel(false);
        let (updates, mut health) = watch::channel(managed::Health::default());
        let task = tokio::spawn(provider.run(rx, updates));
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if health.borrow().recovery.failed > 0 {
                    break;
                }
                health.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "unknown execution cannot become a new probe"
        );
        stop.send_replace(true);
        task.await.unwrap().unwrap();
    }

    // Operator resolution is separate from automatic health/restart and never
    // resets spent budget or manufactures successful model evidence.
    let path = save(root.path(), &value);
    let prepared = managed::Prepared::load_supervised(&path).unwrap();
    let identity = prepared.identity().clone();
    let signer = Authority::from_unlocked_wallet(key(), identity.clone()).unwrap();
    let limits = mayhem_proxy::capacity::Limits {
        max_groups: 8,
        max_routes: 8,
        max_leases: 16,
        max_evidence_age: Duration::from_secs(60),
    };
    let capacity_path = root.path().join("state/capacity.redb");
    let a = mayhem_proxy::capacity::Authority::open_existing(
        &capacity_path,
        identity.clone(),
        limits.clone(),
    )
    .unwrap();
    let probe = a.probe_for_group(&d(10)).unwrap().unwrap();
    let budget = a.probe_budget(&d(10)).unwrap().unwrap();
    assert!(
        managed::Prepared::load_supervised(&path)
            .unwrap()
            .recovery_status(&signer)
            .is_err(),
        "a running authority cannot be inspected or fenced"
    );
    drop(a);
    let status = managed::Prepared::load_supervised(&path)
        .unwrap()
        .recovery_status(&signer)
        .unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].probe.as_ref().unwrap().id, probe.id);
    assert_eq!(status[0].budget.as_ref().unwrap(), &budget);
    let confirmation = |id, connection, stopped| managed::RecoveryConfirmation {
        schema_version: 1,
        probe_id: id,
        connection_digest: connection,
        evidence_digest: d(998),
        upstream_stopped: stopped,
    };
    for input in [
        confirmation(d(999), probe.specification.connection_digest.clone(), true),
        confirmation(probe.id.clone(), d(999), true),
        confirmation(
            probe.id.clone(),
            probe.specification.connection_digest.clone(),
            false,
        ),
    ] {
        assert!(managed::Prepared::load_supervised(&path)
            .unwrap()
            .resolve_recovery_probe(&signer, input)
            .is_err());
    }
    let a = mayhem_proxy::capacity::Authority::open_existing(
        &capacity_path,
        identity.clone(),
        limits.clone(),
    )
    .unwrap();
    assert!(
        managed::Prepared::load_supervised(&path)
            .unwrap()
            .resolve_recovery_probe(
                &signer,
                confirmation(
                    probe.id.clone(),
                    probe.specification.connection_digest.clone(),
                    true
                )
            )
            .is_err(),
        "resolution cannot fence a live controller"
    );
    assert_eq!(a.probe_for_group(&d(10)).unwrap().unwrap().id, probe.id);
    drop(a);
    let resolved = managed::Prepared::load_supervised(&path)
        .unwrap()
        .resolve_recovery_probe(
            &signer,
            confirmation(
                probe.id.clone(),
                probe.specification.connection_digest.clone(),
                true,
            ),
        )
        .unwrap();
    assert!(resolved.released);
    let again = managed::Prepared::load_supervised(&path)
        .unwrap()
        .resolve_recovery_probe(
            &signer,
            confirmation(
                probe.id.clone(),
                probe.specification.connection_digest,
                true,
            ),
        )
        .unwrap();
    assert!(!again.released);
    let a =
        mayhem_proxy::capacity::Authority::open_existing(&capacity_path, identity, limits).unwrap();
    assert!(a.probe_for_group(&d(10)).unwrap().is_none());
    let after = a.probe_budget(&d(10)).unwrap().unwrap();
    assert_eq!(after.used_attempts, budget.used_attempts);
    assert_eq!(
        after.allocated_cost_microusd,
        budget.allocated_cost_microusd
    );
    assert_eq!(
        after.last_completed.unwrap().evidence,
        resolved.evidence_digest
    );
    drop(a);
    let provider = open(root.path(), &value);
    let (health, capacity) = provider.route_status(&d(20)).unwrap();
    assert_eq!(
        health.allowance, 0,
        "operator confirmation is not successful inference evidence"
    );
    assert_eq!(capacity.group_occupied, 0);
}
