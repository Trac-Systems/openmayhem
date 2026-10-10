use super::*;
use clap::Parser;
use serde_json::{json, Value};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).unwrap();
        let path = std::env::temp_dir().join(format!(
            "mayhem-gateway-buyer-{}",
            u64::from_le_bytes(bytes)
        ));
        private_dir(&path, true).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn key() -> SigningKey {
    SigningKey::from_bytes(&[73; 32])
}
fn network() -> discovery::Identity {
    discovery::Identity {
        network_id: "proxy-cli-test".into(),
        msb_bootstrap: "1".repeat(64),
        subnet_bootstrap: "2".repeat(64),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}
fn config() -> Value {
    json!({
        "schema_version":1,"network":network(),
        "buyer_pubkey":super::super::hex_encode(&key().verifying_key().to_bytes()),
        "peer_rpc_url":"http://127.0.0.1:1/v1", "state_dir":"state",
        "worker_program":std::env::current_exe().unwrap(),
        "bridge":{"url":"ws://127.0.0.1:1","token_file":"bridge-token",
            "operation_timeout_ms":1000,"frame_bytes":65536,"queue_events":4,"queue_bytes":65536},
        "worker":{"max_children":1,"max_buffer_bytes":4194304,
            "startup_timeout":{"secs":1,"nanos":0},"processing_timeout":{"secs":1,"nanos":0}},
        "protocol":{"request_bytes":16384,"response_bytes":32768,"choices":4,"tools":4,"questions":4,"decision_options":8},
        "sessions":1,"buffer_bytes":4194304,"message_bytes":65536,"control_wait_ms":1000,
        "financial_reads":2,"storage_workers":2,"max_records":8,"max_payload_bytes":1048576,
        "max_record_bytes":131072,"closed_retention_ms":1000,
        "max_budget_tokens":8,"max_budget_reservations":32,"policy_revision":"3".repeat(64),
        "settlement_policy":{"schema_version":1,"lane":"proxy","payable_outcomes":["complete"],"allow_checkpoints":false},
        "acceptance_epochs":1,"reservation_epochs":2,"receipt_grace_epochs":1
    })
}
fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}
fn fixture() -> (Directory, PathBuf) {
    let dir = Directory::new();
    let path = dir.0.join("buyer.json");
    write(&path, &serde_json::to_vec(&config()).unwrap());
    write(&dir.0.join("bridge-token"), b"fixture-only-token");
    (dir, path)
}

#[tokio::test]
async fn dedicated_financial_peer_preserves_network_and_wallet_guards() {
    let (dir, path) = fixture();
    let original = Config::load(&path).unwrap();
    assert!(original.financial_rpc_url.is_none());
    assert!(original.financial_client().is_ok());
    let mut value = config();
    value["financial_rpc_url"] = json!("http://127.0.0.1:2/v1");
    write(&path, &serde_json::to_vec(&value).unwrap());
    let dedicated = Config::load(&path).unwrap();
    assert!(dedicated.financial_client().is_ok());
    assert_eq!(dedicated.identity().unwrap(), original.identity().unwrap());
    // Selecting a financial peer does not bypass the existing native/discovery
    // peer binding, network pins, wallet check or journal provisioning fence.
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:2/v1".into(),
        key()
    )
    .await
    .is_err());
    let mut foreign = network();
    foreign.network_id = "other".into();
    assert!(
        prepare(path.clone(), foreign, "http://127.0.0.1:1/v1".into(), key())
            .await
            .is_err()
    );
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:1/v1".into(),
        SigningKey::from_bytes(&[74; 32])
    )
    .await
    .is_err());
    assert!(!dir.0.join("state").exists());
    for invalid in [
        "http://example.com/v1",
        "https://user:pass@example.com/v1",
        "https://example.com/v1?token=x",
        "file:///tmp/peer",
    ] {
        value["financial_rpc_url"] = json!(invalid);
        write(&path, &serde_json::to_vec(&value).unwrap());
        assert!(Config::load(&path).unwrap().financial_client().is_err());
    }
}

#[test]
fn buyer_cli_is_separate_explicit_opt_in_with_explicit_provisioning() {
    assert!(crate::UseArgs::try_parse_from(["use"])
        .unwrap()
        .proxy_buyer_config
        .is_none());
    assert!(crate::UseArgs::try_parse_from(["use", "--proxy-buyer-config", "buyer.json"]).is_err());
    let args = crate::UseArgs::try_parse_from([
        "use",
        "--proxy-config",
        "discovery.json",
        "--proxy-buyer-config",
        "buyer.json",
    ])
    .unwrap();
    assert_eq!(args.proxy_buyer_config, Some(PathBuf::from("buyer.json")));
    assert!(crate::UseArgs::try_parse_from([
        "use",
        "--proxy-config",
        "discovery.json",
        "--proxy-buyer-config",
        "buyer.json",
        "--dev-embedded-catalog"
    ])
    .is_err());
    assert!(crate::Cli::try_parse_from([
        "mayhem",
        "proxy",
        "buyer-init",
        "--config",
        "buyer.json",
        "--home",
        "fixture-home"
    ])
    .is_ok());
    assert!(crate::Cli::try_parse_from(["mayhem", "proxy", "buyer-init"]).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn buyer_identity_policy_and_protected_config_fail_before_stores() {
    let (dir, path) = fixture();
    let mut foreign = network();
    foreign.network_id = "other".into();
    assert!(
        prepare(path.clone(), foreign, "http://127.0.0.1:1/v1".into(), key())
            .await
            .is_err()
    );
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:2/v1".into(),
        key()
    )
    .await
    .is_err());
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:1/v1".into(),
        SigningKey::from_bytes(&[74; 32])
    )
    .await
    .is_err());
    assert!(!dir.0.join("state").exists());
    for field in ["seed", "private_key", "default_spend_cap"] {
        let mut value = config();
        value[field] = json!("not-accepted");
        write(&path, &serde_json::to_vec(&value).unwrap());
        assert!(Config::load(&path).is_err());
    }
    let mut value = config();
    value.as_object_mut().unwrap().remove("settlement_policy");
    write(&path, &serde_json::to_vec(&value).unwrap());
    assert!(Config::load(&path).is_err());
    value = config();
    value["reservation_epochs"] = json!(1);
    write(&path, &serde_json::to_vec(&value).unwrap());
    assert!(Config::load(&path).is_err());
    write(&path, &vec![b' '; 65537]);
    assert!(Config::load(&path).is_err());
    write(&path, &serde_json::to_vec(&config()).unwrap());
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Config::load(&path).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn buyer_provisioning_is_exclusive_and_serving_cannot_recreate_lost_journal() {
    let (dir, path) = fixture();
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:1/v1".into(),
        key()
    )
    .await
    .is_err());
    assert!(!dir.0.join("state").exists());
    initialize(InitArgs {
        config: path.clone(),
        home: Some(dir.0.clone()),
    })
    .await
    .unwrap();
    assert!(initialize(InitArgs {
        config: path.clone(),
        home: Some(dir.0.clone())
    })
    .await
    .is_err());
    let prepared = prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:1/v1".into(),
        key(),
    )
    .await
    .unwrap();
    assert_eq!(prepared.budget_path, dir.0.join("state").join(BUDGET));
    drop(prepared);
    let recovery = dir.0.join("state").join(RECOVERY);
    std::fs::remove_file(&recovery).unwrap();
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:1/v1".into(),
        key()
    )
    .await
    .is_err());
    assert!(!recovery.exists());
    write(&recovery, b"");
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:1/v1".into(),
        key()
    )
    .await
    .is_err());
    assert_eq!(std::fs::metadata(&recovery).unwrap().len(), 0);
    assert!(initialize(InitArgs {
        config: path,
        home: Some(dir.0.clone())
    })
    .await
    .is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn buyer_decoder_and_bridge_remain_local_protected_owner_inputs() {
    let (dir, path) = fixture();
    initialize(InitArgs {
        config: path.clone(),
        home: Some(dir.0.clone()),
    })
    .await
    .unwrap();
    write(&dir.0.join("state/decoder-work/unexpected"), b"untrusted");
    assert!(prepare(
        path.clone(),
        network(),
        "http://127.0.0.1:1/v1".into(),
        key()
    )
    .await
    .is_err());
    std::fs::remove_file(dir.0.join("state/decoder-work/unexpected")).unwrap();
    for url in [
        "ws://example.invalid",
        "ws://localhost:1",
        "ws://127.0.0.1:1?token=not-allowed",
    ] {
        let mut value = config();
        value["bridge"]["url"] = json!(url);
        write(&path, &serde_json::to_vec(&value).unwrap());
        assert!(prepare(
            path.clone(),
            network(),
            "http://127.0.0.1:1/v1".into(),
            key()
        )
        .await
        .is_err());
    }
    let link = dir.0.join("worker-link");
    std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
    let mut value = config();
    value["worker_program"] = json!(link);
    write(&path, &serde_json::to_vec(&value).unwrap());
    assert!(
        prepare(path, network(), "http://127.0.0.1:1/v1".into(), key())
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn durable_activation_survives_native_only_restart_and_missing_marker_fails_closed() {
    let (dir, path) = fixture();
    let tokens = mayhem_gateway::openai::GatewayTokenStore::empty();
    assert!(budget_activation(&dir.0, &tokens).unwrap().is_none());
    initialize(InitArgs {
        config: path.clone(),
        home: Some(dir.0.clone()),
    })
    .await
    .unwrap();
    let token_path = super::super::gateway_token_store_path(&dir.0);
    let tokens = super::super::read_gateway_token_store(&token_path).unwrap();
    assert_eq!(tokens.version, 2);
    let activation = budget_activation(&dir.0, &tokens).unwrap().unwrap();
    let limits = activation.limits();
    // No buyer runtime, discovery configuration or wallet is needed to retain
    // common durable counters on a native-only restart.
    let native = GatewayAccessControl::new(true, tokens.clone(), Some(token_path))
        .with_durable_key_budget(activation.path, limits)
        .unwrap();
    drop(native);
    let marker = dir.0.join(ACTIVATION);
    let bytes = std::fs::read(&marker).unwrap();
    std::fs::remove_file(&marker).unwrap();
    assert!(budget_activation(&dir.0, &tokens).is_err());
    write(&marker, &bytes);
    assert!(
        budget_activation(&dir.0, &mayhem_gateway::openai::GatewayTokenStore::empty()).is_err()
    );
    write(&marker, b"");
    assert!(budget_activation(&dir.0, &tokens).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn provisioning_excludes_active_gateway_and_rejects_switched_budget_authority() {
    let (dir, path) = fixture();
    let lock = budget_owner_lock(&dir.0, false).unwrap();
    assert!(budget_owner_lock(&dir.0, true).is_err());
    assert!(budget_owner_lock(&dir.0, false).is_ok());
    assert!(initialize(InitArgs {
        config: path.clone(),
        home: Some(dir.0.clone())
    })
    .await
    .is_err());
    assert!(!dir.0.join("state").exists());
    drop(lock);
    initialize(InitArgs {
        config: path.clone(),
        home: Some(dir.0.clone()),
    })
    .await
    .unwrap();
    let tokens =
        super::super::read_gateway_token_store(&super::super::gateway_token_store_path(&dir.0))
            .unwrap();
    let mut activation = budget_activation(&dir.0, &tokens).unwrap().unwrap();
    let prepared = prepare(path, network(), "http://127.0.0.1:1/v1".into(), key())
        .await
        .unwrap();
    assert!(activation.matches(&prepared));
    activation.path = dir.0.join("another-budget.redb");
    assert!(!activation.matches(&prepared));
}

#[test]
fn buyer_retail_callback_uses_protected_fixed_operator_configuration() {
    let (_dir, path) = fixture();
    let mut value = config();
    value["retail_authorization"] = json!({"url":"http://127.0.0.1:3010/internal/proxy-hold",
        "credential":"fixture-only-machine-key","owner_token_ids":["retail-owner"],"timeout_ms":1000});
    write(&path, &serde_json::to_vec(&value).unwrap());
    assert!(Config::load(&path).unwrap().retail_authorization.is_some());
    value["retail_authorization"]["url"] = json!("http://untrusted.example/hold");
    write(&path, &serde_json::to_vec(&value).unwrap());
    assert!(Config::load(&path).is_err());
    value["retail_authorization"]["url"] = json!("https://retail.example/internal/proxy-hold");
    value["retail_authorization"]["timeout_ms"] = json!(15000);
    write(&path, &serde_json::to_vec(&value).unwrap());
    assert!(Config::load(&path).is_err());
}

#[test]
fn buyer_profile_resolution_configuration_preserves_defaults_and_rejects_invalid_budgets_before_startup(
) {
    let (dir, path) = fixture();
    let original = Config::load(&path).unwrap();
    assert!(original.profile_resolution.is_none());
    let original_policy = original.settlement_policy.digest().unwrap();
    let mut value = config();
    value["profile_resolution"] = json!({"retained_sessions":7,"retained_bytes":67108864,
        "active_steps":2,"candidates_per_step":3,"ttl_ms":300000,"observation_timeout_ms":10000});
    write(&path, &serde_json::to_vec(&value).unwrap());
    let explicit = Config::load(&path).unwrap();
    assert_eq!(
        explicit
            .profile_resolution
            .as_ref()
            .unwrap()
            .retained_sessions,
        7
    );
    assert_eq!(
        explicit.settlement_policy.digest().unwrap(),
        original_policy
    );
    for (field, invalid) in [
        ("retained_sessions", 0),
        ("retained_bytes", 0),
        ("active_steps", 17),
        ("candidates_per_step", 17),
        ("ttl_ms", 0),
        ("observation_timeout_ms", 60001),
    ] {
        let mut bad = value.clone();
        bad["profile_resolution"][field] = json!(invalid);
        write(&path, &serde_json::to_vec(&bad).unwrap());
        assert!(Config::load(&path).is_err(), "{field}");
        assert!(!dir.0.join("state").exists());
    }
    value["profile_resolution"]["per_customer_concurrency"] = json!(2);
    write(&path, &serde_json::to_vec(&value).unwrap());
    assert!(Config::load(&path).is_err());
}
