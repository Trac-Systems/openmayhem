//! Explicit buyer recovery startup. Uses the normal protected Core wallet; this
//! command does not start models, accept new spending or change native providers.
use super::{cached_wallet_signing_key, resolve_wallet_keypair_path, WalletLocatorArgs};
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use mayhem_proxy::{
    attempts::{Digest, Identity},
    discovery,
    financial::{
        self,
        recovery::{
            runner::{Health, Policy, Runner},
            BuyerRecovery, Limits, Store,
        },
    },
    signing::Authority,
};
use serde::Deserialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::watch;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Recover existing proxy buyer holds without dispatching replacement inference.
    BuyerRecovery {
        #[command(subcommand)]
        command: RecoveryCommand,
    },
}
#[derive(Debug, Subcommand)]
pub enum RecoveryCommand {
    /// Process one bounded page and print its summary.
    Once(RecoveryArgs),
    /// Keep recovering saved work until interrupted; print one final summary.
    Watch(RecoveryArgs),
}
#[derive(Debug, Args)]
pub struct RecoveryArgs {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[command(flatten)]
    wallet: WalletLocatorArgs,
    /// Do not unlock the wallet. Publish only already-signed saved intentions.
    #[arg(long)]
    locked: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    network: discovery::Identity,
    buyer_pubkey: Digest,
    peer_rpc_url: String,
    recovery_file: PathBuf,
    max_records: u64,
    closed_retention_ms: u64,
    #[serde(default)]
    recovery: Policy,
}
impl Config {
    fn load(path: &Path) -> Result<Self> {
        let file =
            std::fs::File::open(path).context("opening proxy buyer recovery configuration")?;
        if !file.metadata()?.is_file() {
            bail!("recovery configuration must be a regular file");
        }
        let mut bytes = Vec::new();
        file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 {
            bail!("recovery configuration exceeds 16 KiB");
        }
        let mut config: Self =
            serde_json::from_slice(&bytes).context("invalid recovery configuration")?;
        if config.schema_version != 1 || config.max_records == 0 || config.closed_retention_ms == 0
        {
            bail!("invalid recovery schema or retention limits");
        }
        config.network.validate()?;
        config.recovery.validate()?;
        // The wallet/store identity has no release or contract pin. The RPC
        // observation still has to match this explicit network/contract context.
        config.identity()?;
        if config.recovery_file.is_relative() {
            config.recovery_file = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&config.recovery_file);
        }
        if config.recovery_file.file_name().is_none() {
            bail!("recovery file is required");
        }
        Ok(config)
    }
    fn identity(&self) -> Result<Identity> {
        Ok(Identity {
            network_id: self.network.network_id.clone(),
            msb_bootstrap: Digest::new(&self.network.msb_bootstrap)?,
            subnet_bootstrap: Digest::new(&self.network.subnet_bootstrap)?,
            controller_pubkey: self.buyer_pubkey.clone(),
        })
    }
}

pub async fn run(command: Command) -> Result<()> {
    let Command::BuyerRecovery { command } = command;
    let once = matches!(&command, RecoveryCommand::Once(_));
    let args = match command {
        RecoveryCommand::Once(v) | RecoveryCommand::Watch(v) => v,
    };
    let config = Config::load(&args.config)?;
    let identity = config.identity()?;
    let client = Arc::new(financial::Client::new(
        &config.peer_rpc_url,
        config.network,
        config.buyer_pubkey.as_str().to_owned(),
        1,
    )?);
    let signer = if args.locked {
        None
    } else {
        let keypair = resolve_wallet_keypair_path(&args.wallet)?;
        let key = cached_wallet_signing_key(
            &keypair,
            args.wallet.wallet_password.as_deref().unwrap_or_default(),
        )
        .await
        .context("unlocking the existing proxy buyer wallet")?;
        Some(Authority::from_unlocked_wallet(key, identity.clone())?)
    };
    // Fail mismatched signing identity before opening or creating the store.
    let store = Arc::new(Store::open(
        &config.recovery_file,
        identity,
        Limits {
            max_records: config.max_records,
            closed_retention_ms: config.closed_retention_ms,
        },
    )?);
    let recovery = Arc::new(BuyerRecovery::new(store, client, 1)?);
    let mut seed = [0u8; 8];
    getrandom::fill(&mut seed).context("initializing recovery scheduling")?;
    let mut runner = Runner::new(recovery, signer, config.recovery, u64::from_le_bytes(seed))?;
    if once {
        let page = runner.page().await?;
        println!("{}", serde_json::to_string(&page)?);
        if page.error_code.is_some() {
            bail!("proxy buyer recovery needs another attempt; saved work retained");
        }
        return Ok(());
    }
    let (stop, stopped) = watch::channel(false);
    let (updates, health) = watch::channel(Health::default());
    let work = runner.run(stopped, updates);
    tokio::pin!(work);
    let result = tokio::select! {
        value = &mut work => value.map_err(anyhow::Error::new),
        signal = stop_signal() => {
            stop.send_replace(true);
            let value = work.await;
            signal?;
            value.map_err(anyhow::Error::new)
        }
    };
    println!("{}", serde_json::to_string(&*health.borrow())?);
    result
}
async fn stop_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { r = tokio::signal::ctrl_c() => r?, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let mut seed = [0u8; 8];
            getrandom::fill(&mut seed).unwrap();
            let path = std::env::temp_dir()
                .join(format!("mayhem-proxy-buyer-{}", u64::from_le_bytes(seed)));
            std::fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn config(buyer: &str) -> serde_json::Value {
        json!({"schema_version":1,"network":{"network_id":"proxy-test",
            "msb_bootstrap":"1".repeat(64),"subnet_bootstrap":"2".repeat(64),
            "contract_version":mayhem_proto::CONTRACT_VERSION},"buyer_pubkey":buyer,
            "peer_rpc_url":"http://127.0.0.1:1/v1", "recovery_file":"buyer.redb",
            "max_records":8,"closed_retention_ms":1000})
    }
    fn args(path: &Path, keypair: &Path, locked: bool) -> RecoveryArgs {
        RecoveryArgs {
            config: path.to_owned(),
            locked,
            wallet: WalletLocatorArgs {
                home: None,
                keypair: Some(keypair.to_owned()),
                peer_store_name: "main".into(),
                wallet_password: Some("proxy-test-password".into()),
            },
        }
    }
    #[test]
    fn proxy_recovery_cli_is_explicit_and_configuration_is_bounded() {
        let cli = crate::Cli::try_parse_from([
            "mayhem",
            "proxy",
            "buyer-recovery",
            "once",
            "--config",
            "control.json",
            "--locked",
        ])
        .unwrap();
        assert!(matches!(cli.command, crate::Commands::Proxy { .. }));
        assert!(
            crate::Cli::try_parse_from(["mayhem", "proxy", "buyer-recovery", "watch"]).is_err()
        );
        let d = Directory::new();
        let path = d.0.join("control.json");
        let original = config(&"3".repeat(64));
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(
            Config::load(&path).unwrap().recovery_file,
            d.0.join("buyer.redb")
        );
        for field in ["new_spend_authorization", "seed"] {
            let mut value = original.clone();
            value[field] = json!("not-accepted");
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(Config::load(&path).is_err());
        }
        let mut value = original;
        value["recovery"] = json!({"page_size":65});
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::write(&path, vec![b' '; 16 * 1024 + 1]).unwrap();
        assert!(Config::load(&path).is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn proxy_recovery_locked_start_needs_no_wallet_or_rpc_when_index_empty() {
        let d = Directory::new();
        let path = d.0.join("control.json");
        std::fs::write(&path, serde_json::to_vec(&config(&"3".repeat(64))).unwrap()).unwrap();
        run(Command::BuyerRecovery {
            command: RecoveryCommand::Once(args(&path, &d.0.join("missing-wallet"), true)),
        })
        .await
        .unwrap();
        assert!(d.0.join("buyer.redb").is_file());
        assert!(!d.0.join("missing-wallet").exists());
    }
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires the hydrated local Pear wallet helper; run explicitly"]
    async fn proxy_recovery_native_wallet_unlock_matches_before_store_creation() {
        let d = Directory::new();
        let path = d.0.join("control.json");
        let keypair = d.0.join("keypair.json");
        let created =
            crate::create_wallet(&keypair, "proxy-test-password", None, false, None, None)
                .await
                .unwrap();
        std::fs::write(&path, serde_json::to_vec(&config(&"3".repeat(64))).unwrap()).unwrap();
        assert!(run(Command::BuyerRecovery {
            command: RecoveryCommand::Once(args(&path, &keypair, false))
        })
        .await
        .is_err());
        assert!(!d.0.join("buyer.redb").exists());
        std::fs::write(
            &path,
            serde_json::to_vec(&config(&created.public_key)).unwrap(),
        )
        .unwrap();
        run(Command::BuyerRecovery {
            command: RecoveryCommand::Once(args(&path, &keypair, false)),
        })
        .await
        .unwrap();
        assert!(d.0.join("buyer.redb").exists());
    }
}
