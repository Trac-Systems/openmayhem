//! Add only the explicitly configured controller to the existing supervisor.
//! All restart credentials remain protected file references, never process args.
mod run;
use crate as cli;
use crate::WalletLocatorArgs;
use anyhow::{ensure, Context, Result};
use clap::Args;
use mayhem_proxy::{managed::Prepared, signing::Authority};
pub(crate) use run::Host;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Args)]
pub struct AddArgs {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    keypair: Option<PathBuf>,
    #[arg(long, default_value = "main")]
    peer_store_name: String,
    /// Protected password file retained for restarts. Defaults to the existing
    /// <home>/secrets/wallet-password, if present. Never copies a password into args.
    #[arg(long, value_name = "PATH")]
    wallet_password_file: Option<PathBuf>,
}

pub async fn add(args: AddArgs) -> Result<()> {
    let home = cli::absolutize(
        args.home
            .clone()
            .map(Ok)
            .unwrap_or_else(cli::default_home)?,
    )?;
    let config = cli::absolutize(args.config)?;
    let config_path = config.clone();
    let prepared =
        tokio::task::spawn_blocking(move || Prepared::load_supervised(&config_path)).await??;
    let config = std::fs::canonicalize(&config)?;
    let password_file = args
        .wallet_password_file
        .map(cli::absolutize)
        .transpose()?
        .or_else(|| {
            let path = home.join("secrets/wallet-password");
            path.exists().then_some(path)
        });
    let password = password_file.as_deref().map(read_password).transpose()?;
    ensure_restart_password(
        password.as_deref(),
        std::env::var_os("MAYHEM_WALLET_PASSWORD").is_some(),
    )?;
    let wallet = WalletLocatorArgs {
        home: Some(home.clone()),
        keypair: args.keypair,
        peer_store_name: args.peer_store_name,
        wallet_password: password,
    };
    let keypair = std::fs::canonicalize(cli::resolve_wallet_keypair_path(&wallet)?)?;
    let key = cli::cached_wallet_signing_key(
        &keypair,
        wallet.wallet_password.as_deref().unwrap_or_default(),
    )
    .await?;
    // Verify normal wallet identity before installing; do not open capacity or
    // recovery stores, increment fences, run probes, or publish a ledger write.
    let _signer = Authority::from_unlocked_wallet(key, prepared.identity().clone())?;
    let name = child_name(prepared.identity())?;
    let binary = std::env::current_exe()?.canonicalize()?;
    let child = child_config(
        &name,
        &binary,
        &config,
        &home,
        &keypair,
        password_file.as_deref(),
    )?;
    let supervisor_url = cli::mayhemd_control_url(&home)?;
    ensure_loopback(&supervisor_url)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    let status: Value = client
        .get(format!("{supervisor_url}/status"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    require_persistence_capability(&status)?;
    let token = cli::load_or_create_mayhemd_control_token(&home)?;
    let response = cli::post_mayhemd_json(
        &client,
        &format!("{supervisor_url}/children/add"),
        &child,
        &token,
    )
    .await?;
    ensure!(
        response["ok"] == true && response["persistent"] == true && response["name"] == name,
        "mayhemd did not confirm persistent installation; inspect its status before retrying"
    );
    println!(
        "{}",
        json!({"ok":true,"name":name,"persistent":true,"state":"starting","status_snapshot_saved":response["status_snapshot_saved"]})
    );
    Ok(())
}

fn ensure_restart_password(password: Option<&str>, environment_present: bool) -> Result<()> {
    ensure!(password.is_some_and(|value| !value.is_empty()) || !environment_present,
        "supervised startup requires a nonempty --wallet-password-file instead of an environment-only password");
    Ok(())
}

fn require_persistence_capability(status: &Value) -> Result<()> {
    ensure!(
        status
            .get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|values| values
                .iter()
                .any(|v| v.as_str() == Some("persistent_children_v1"))),
        "running mayhemd does not support persistent proxy setup; update and restart mayhemd first"
    );
    Ok(())
}
fn ensure_loopback(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)?;
    let loopback = url
        .host_str()
        .and_then(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|ip| ip.is_loopback());
    ensure!(
        url.scheme() == "http"
            && loopback
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "mayhemd control requires a literal loopback HTTP address"
    );
    Ok(())
}
fn child_name(identity: &mayhem_proxy::attempts::Identity) -> Result<String> {
    // One controller per wallet+network on this local supervisor; putting aliases
    // into another file must not create competing authorities for the same wallet.
    let hash = blake3::hash(&serde_json::to_vec(identity)?);
    Ok(format!("provider-proxy-{}", hash.to_hex()))
}
fn child_config(
    name: &str,
    binary: &Path,
    config: &Path,
    home: &Path,
    keypair: &Path,
    password: Option<&Path>,
) -> Result<Value> {
    fn path(value: &Path) -> Result<String> {
        ensure!(value.is_absolute(), "supervised paths must be absolute");
        Ok(value
            .to_str()
            .context("supervised paths require UTF-8")?
            .to_owned())
    }
    let mut args = vec![
        "provider".into(),
        "proxy".into(),
        "serve".into(),
        "--supervised".into(),
        "--config".into(),
        path(config)?,
        "--home".into(),
        path(home)?,
        "--keypair".into(),
        path(keypair)?,
    ];
    if let Some(password) = password {
        args.extend(["--wallet-password-file".into(), path(password)?]);
    }
    Ok(
        json!({"name":name,"command":path(binary)?,"args":args,"env":{},"restart":true,
        "restart_backoff_ms":1000,"restart_stable_after_ms":cli::MAYHEMD_RESTART_STABLE_AFTER_MILLIS,
        "crash_loop_threshold":cli::MAYHEMD_CRASH_LOOP_THRESHOLD,"persistent":true}),
    )
}

#[cfg(unix)]
fn read_password(path: &Path) -> Result<String> {
    use rustix::fs::{fstat, open, FileType, OFlags};
    use std::io::Read;
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let stat = fstat(&fd)?;
    ensure!(
        FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
            && stat.st_uid == rustix::process::geteuid().as_raw()
            && stat.st_mode & 0o077 == 0
            && stat.st_nlink == 1,
        "wallet password requires an owner-only regular file"
    );
    let file = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    file.take(8193).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 8192,
        "wallet password file exceeds its size limit"
    );
    let mut value = String::from_utf8(bytes)?;
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    Ok(value)
}
#[cfg(windows)]
fn read_password(path: &Path) -> Result<String> {
    let bytes = mayhem_proxy::connector::config::private_file(path, 8192)
        .map_err(|_| anyhow::anyhow!("wallet password requires a protected owner-only regular file"))?;
    let mut value = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("wallet password must be UTF-8"))?
        .to_owned();
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') { value.pop(); }
    }
    Ok(value)
}
#[cfg(not(any(unix, windows)))]
fn read_password(_: &Path) -> Result<String> {
    anyhow::bail!("supervised proxy credentials require supported filesystem protection")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_child_uses_file_references_and_fixed_existing_binary() {
        let child = child_config(
            "fixture",
            Path::new("/opt/mayhem"),
            Path::new("/private/provider.json"),
            Path::new("/wallet"),
            Path::new("/wallet/keypair.json"),
            Some(Path::new("/private/password")),
        )
        .unwrap();
        assert_eq!(child["command"], "/opt/mayhem");
        assert_eq!(child["persistent"], true);
        assert_eq!(child["restart"], true);
        assert_eq!(child["env"], json!({}));
        assert!(child["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "--wallet-password-file"));
        assert!(!child.to_string().contains("MAYHEM_WALLET_PASSWORD"));
        assert!(child_config(
            "fixture",
            Path::new("relative"),
            Path::new("/config"),
            Path::new("/wallet"),
            Path::new("/keypair"),
            None
        )
        .is_err());
    }
    #[test]
    fn legacy_supervisors_and_remote_control_are_rejected() {
        assert!(require_persistence_capability(&json!({"ok":true})).is_err());
        require_persistence_capability(&json!({"capabilities":["persistent_children_v1"]}))
            .unwrap();
        for url in ["http://127.0.0.1:11437", "http://[::1]:11437"] {
            ensure_loopback(url).unwrap();
        }
        for url in [
            "https://127.0.0.1",
            "http://example.com",
            "http://localhost",
            "http://user@127.0.0.1",
            "http://127.0.0.1/?token=x",
        ] {
            assert!(ensure_loopback(url).is_err());
        }
    }
    #[test]
    fn wallet_unlock_cannot_depend_on_the_installing_shell() {
        for password in [None, Some("")] {
            assert!(ensure_restart_password(password, true).is_err());
            ensure_restart_password(password, false).unwrap();
        }
        ensure_restart_password(Some("test-fixture"), true).unwrap();
        ensure_restart_password(Some("test-fixture"), false).unwrap();
    }
}
