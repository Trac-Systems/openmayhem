//! Provider startup uses the existing encrypted wallet and protected SC-Bridge.
mod supervisor;
use super::{cached_wallet_signing_key, resolve_wallet_keypair_path, WalletLocatorArgs};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mayhem_proxy::{
    managed::{Health, Prepared},
    signing::Authority,
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Refresh or inspect proxy markets without ledger writes.
    Catalog {
        #[command(subcommand)]
        command: mayhem_proxy::cli::CatalogCommand,
    },
    /// Serve explicitly configured proxy routes and recover retained sessions.
    /// Does not register markets, pay admission or download/start an upstream model.
    Serve(ServeArgs),
    /// Install a restartable proxy controller in the existing local mayhemd.
    Add(supervisor::AddArgs),
}
#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[command(flatten)]
    wallet: WalletLocatorArgs,
    #[arg(long, hide = true)]
    supervised: bool,
}
pub async fn run(command: Command) -> Result<()> {
    let args = match command {
        Command::Catalog { command } => {
            return mayhem_proxy::cli::run(mayhem_proxy::cli::Command::Catalog { command })
                .await
                .map_err(Into::into)
        }
        Command::Serve(args) => args,
        Command::Add(args) => return supervisor::add(args).await,
    };
    let config = args.config;
    let prepared = tokio::task::spawn_blocking(move || {
        if args.supervised {
            Prepared::load_supervised(&config)
        } else {
            Prepared::load(&config)
        }
    })
    .await
    .context("preparing proxy provider configuration")??;
    let keypair = resolve_wallet_keypair_path(&args.wallet)?;
    let key = cached_wallet_signing_key(
        &keypair,
        args.wallet.wallet_password.as_deref().unwrap_or_default(),
    )
    .await
    .context("unlocking the existing proxy provider wallet")?;
    let signer = Arc::new(Authority::from_unlocked_wallet(
        key,
        prepared.identity().clone(),
    )?);
    let provider = prepared.open(signer)?;
    let (shutdown, stop) = watch::channel(false);
    let (updates, health) = watch::channel(Health::default());
    let work = provider.run(stop, updates);
    tokio::pin!(work);
    let result = tokio::select! {
        value = &mut work => value.map_err(anyhow::Error::new),
        signal = stop_signal() => {
            shutdown.send_replace(true);
            let result = work.await;
            signal?;
            result.map_err(anyhow::Error::new)
        }
    };
    // One bounded summary; never log prompts, endpoint URLs, credentials or
    // every maintenance tick. The supervisor owns restart/backoff policy.
    println!("{}", serde_json::to_string(&*health.borrow())?);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Command,
    }
    #[test]
    fn proxy_provider_serve_uses_existing_wallet_arguments_and_preserves_catalog() {
        let parsed = Cli::try_parse_from([
            "proxy",
            "serve",
            "--config",
            "provider.json",
            "--home",
            "/tmp/example-wallet",
        ])
        .unwrap();
        let Command::Serve(args) = parsed.command else {
            panic!("serve required")
        };
        assert_eq!(args.config, PathBuf::from("provider.json"));
        assert_eq!(args.wallet.home, Some(PathBuf::from("/tmp/example-wallet")));
        assert!(Cli::try_parse_from(["proxy", "serve"]).is_err());
        assert!(Cli::try_parse_from([
            "proxy",
            "serve",
            "--config",
            "provider.json",
            "--private-key",
            "not-a-supported-option"
        ])
        .is_err());
        assert!(matches!(
            Cli::try_parse_from(["proxy", "catalog", "status", "--config", "catalog.json"])
                .unwrap()
                .command,
            Command::Catalog { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "proxy",
                "add",
                "--config",
                "provider.json",
                "--wallet-password-file",
                "/private/reference-only"
            ])
            .unwrap()
            .command,
            Command::Add(_)
        ));
        let Command::Serve(supervised) = Cli::try_parse_from([
            "proxy",
            "serve",
            "--config",
            "provider.json",
            "--supervised",
        ])
        .unwrap()
        .command
        else {
            panic!("serve required")
        };
        assert!(supervised.supervised);
    }
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
