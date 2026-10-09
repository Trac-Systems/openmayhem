use super::*;
#[path = "../../../../../mayhem-proxy/tests/support/setup_discovery.rs"]
mod canonical_fixture;
#[tokio::test]
async fn guided_cli_prompts_discover_choose_create_or_join_and_convert_exact_prices() {
    use std::os::unix::fs::PermissionsExt;
    struct Temporary(std::path::PathBuf);
    impl Temporary {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let home = Temporary(
        std::env::temp_dir().join(format!(
            "mayhem-guided-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
    );
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(home.path())
            .unwrap();
    }
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let token = home.path().join("tokenizer");
    std::fs::write(&token, b"approved-byte-fixture").unwrap();
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    let network = serde_json::json!({"network_id":"guided-cli-fixture","msb_bootstrap":"03".repeat(32),"subnet_bootstrap":"04".repeat(32),"contract_version":mayhem_proto::CONTRACT_VERSION});
    let peer = canonical_fixture::Server::start(network.clone()).await;
    peer.control
        .sequence
        .store(7, std::sync::atomic::Ordering::SeqCst);
    let host = Host {
        network: serde_json::from_value(network).unwrap(),
        provider_pubkey: Digest::new("07".repeat(32)).unwrap(),
        peer_rpc: peer.url.clone(),
        bridge_url: "ws://127.0.0.1:9/".into(),
        bridge_token_file: home.path().join("bridge"),
        worker_program: std::env::current_exe().unwrap(),
        wallet_password_file: None,
        admission_origin: None,
    };
    let args = InitArgs {
        wallet: WalletLocatorArgs {
            home: None,
            keypair: None,
            peer_store_name: "main".into(),
            wallet_password: None,
        },
        api_key_file: None,
        tokenizer_file: Some(token),
        restart_password_file: None,
        admission_origin: None,
        rpc_url: None,
    };
    for join in [false, true] {
        let mut lines = vec![
            format!("{}upstream/", peer.url),
            "chat".into(),
            "no".into(),
            "127.0.0.1/32".into(),
            "yes".into(),
            "none".into(),
            "yes".into(),
            "2".into(),
            "1".into(),
            if join { "join" } else { "create" }.into(),
        ];
        if join {
            lines.push("1".into());
        } else {
            lines.extend(["guided-created".into(), "Declared fixture".into()]);
        }
        lines.extend(
            [
                "4096", "2", "fiat", "1000000", "1.25", "1000000", "2.5", "0", "0", "no", "no",
                "no", "no", "none", "2", "0.000020", "0.000010", "32", "3000", "no", "1",
            ]
            .into_iter()
            .map(String::from),
        );
        let mut input = io::Cursor::new(lines.join("\n") + "\n");
        let mut output = Vec::new();
        let chosen = choices(&mut input, &mut output, &args, &host, home.path())
            .await
            .unwrap();
        assert_eq!(chosen.upstream_model, "fixture-model");
        assert_eq!(chosen.sequence, 8);
        assert_eq!(
            chosen.offers[0].rates[0].per_unit_au,
            1_250_000_000_000_000_000
        );
        assert_eq!(chosen.probe_budget.max_cost_microusd, 20);
        assert_eq!(chosen.probe_budget.per_attempt_cost_microusd, 10);
        assert_eq!(
            matches!(chosen.market, ProfileMarket::JoinMarket { .. }),
            join
        );
        Canonical::new(&host)
            .unwrap()
            .revalidate(&chosen)
            .await
            .unwrap();
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("Canonical next operation sequence: 8"));
    }
    assert_eq!(
        peer.control
            .models_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert!(!home.path().join("proxy-setup").exists());
}
