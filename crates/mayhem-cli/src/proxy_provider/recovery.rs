use super::{cached_wallet_signing_key, resolve_wallet_keypair_path, WalletLocatorArgs};
use anyhow::{ensure, Result};
use clap::Args as ClapArgs;
use mayhem_proxy::{
    managed::{Prepared, RecoveryConfirmation},
    signing::Authority,
};
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
pub struct StatusArgs {
    #[arg(long)]
    config: PathBuf,
    #[command(flatten)]
    wallet: WalletLocatorArgs,
}

pub async fn status(args: StatusArgs) -> Result<()> {
    let prepared = Prepared::load_supervised(&args.config)?;
    let keypair = resolve_wallet_keypair_path(&args.wallet)?;
    let key = cached_wallet_signing_key(
        &keypair,
        args.wallet.wallet_password.as_deref().unwrap_or_default(),
    )
    .await?;
    let signer = Authority::from_unlocked_wallet(key, prepared.identity().clone())?;
    let result = tokio::task::spawn_blocking(move || prepared.recovery_status(&signer)).await??;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(long)]
    config: PathBuf,
    /// Owner-only JSON confirmation binding the exact probe, upstream and evidence digest.
    #[arg(long)]
    confirmation: PathBuf,
    /// A timeout is insufficient: independently check the original upstream has stopped.
    #[arg(long, required = true)]
    confirm_upstream_stopped: bool,
    #[command(flatten)]
    wallet: WalletLocatorArgs,
}

pub async fn resolve(args: Args) -> Result<()> {
    ensure!(
        args.confirm_upstream_stopped,
        "independent upstream-stop confirmation is required"
    );
    let prepared = Prepared::load_supervised(&args.config)?;
    let bytes = mayhem_proxy::connector::config::private_file(&args.confirmation, 16 * 1024)?;
    let confirmation: RecoveryConfirmation = serde_json::from_slice(&bytes)?;
    let keypair = resolve_wallet_keypair_path(&args.wallet)?;
    let key = cached_wallet_signing_key(
        &keypair,
        args.wallet.wallet_password.as_deref().unwrap_or_default(),
    )
    .await?;
    let signer = Authority::from_unlocked_wallet(key, prepared.identity().clone())?;
    let result =
        tokio::task::spawn_blocking(move || prepared.resolve_recovery_probe(&signer, confirmation))
            .await??;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }
    #[test]
    fn confirmation_is_explicit_and_has_no_force_or_payment_override() {
        let base = [
            "proxy",
            "--config",
            "/private/provider.json",
            "--confirmation",
            "/private/checked.json",
        ];
        assert!(Cli::try_parse_from(base).is_err());
        assert!(
            Cli::try_parse_from(base.into_iter().chain(["--confirm-upstream-stopped"])).is_ok()
        );
        assert!(Cli::try_parse_from(base.into_iter().chain(["--force"])).is_err());
    }
}
