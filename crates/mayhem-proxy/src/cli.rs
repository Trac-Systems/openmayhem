//! Read-only proxy control commands shared with `mayhem provider proxy`. No
//! registration, payment, model download or upstream execution occurs here.

use crate::{
    catalog::{Catalog, RefreshOutcome},
    discovery::{DiscoveryClient, Identity, CATALOG_PREFIX},
    require,
    supervisor::{self, RefreshPolicy},
    Result,
};
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::json;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::watch;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Refresh or inspect the separate proxy market catalog (no ledger writes).
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Local proxy control JSON file. Contains network identity, peer RPC and cache path, not model credentials.
    #[arg(long, value_name = "PATH")]
    pub config: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// Resume a refresh through its complete snapshot, then exit.
    Sync(ConfigArgs),
    /// Keep the catalog current until stopped. Intended for supervised Core control.
    Watch(ConfigArgs),
    /// Inspect a stopped controller's persistent cache and freshness metadata.
    Status(ConfigArgs),
    /// List one bounded page of proxy markets from the cache.
    Markets {
        #[command(flatten)]
        args: ConfigArgs,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 40)]
        limit: usize,
    },
    /// Suggest compatible markets from a saved local discovery report, without joining.
    Suggest {
        #[command(flatten)]
        args: ConfigArgs,
        #[arg(long, value_name = "PATH")]
        report: PathBuf,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 40)]
        limit: usize,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlConfig {
    pub schema_version: u32,
    pub identity: Identity,
    pub peer_rpc_url: String,
    pub catalog_file: PathBuf,
    #[serde(default)]
    pub refresh: RefreshPolicy,
}

impl ControlConfig {
    pub fn load(path: &Path) -> Result<Self> {
        // A bounded, explicit control file; never load a model, .env or arbitrary
        // recursively included configuration as a side effect of discovery.
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
        require(bytes.len() <= 16 * 1024, "proxy control file exceeds bound")?;
        let mut config: Self = serde_json::from_slice(&bytes)?;
        require(
            config.schema_version == 1
                && config.identity.contract_version == mayhem_proto::CONTRACT_VERSION,
            "proxy control configuration requires this Core contract version",
        )?;
        config.identity.validate()?;
        config.refresh.validate()?;
        if config.catalog_file.is_relative() {
            config.catalog_file = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&config.catalog_file);
        }
        require(
            config.catalog_file.file_name().is_some(),
            "proxy catalog file is required",
        )?;
        Ok(config)
    }
}

pub async fn run(command: Command) -> Result<()> {
    let Command::Catalog { command } = command;
    let args = match &command {
        CatalogCommand::Sync(args) | CatalogCommand::Watch(args) | CatalogCommand::Status(args) => {
            args
        }
        CatalogCommand::Markets { args, .. } | CatalogCommand::Suggest { args, .. } => args,
    };
    let config = ControlConfig::load(&args.config)?;
    let catalog = Arc::new(Catalog::open(
        &config.catalog_file,
        config.identity.clone(),
    )?);
    match command {
        CatalogCommand::Status(_) => {
            let status = catalog.read()?.status();
            println!(
                "{}",
                json!({"lane":"proxy","generation":status.generation,"invalidated":status.invalidated,
                "refresh_in_progress":status.refresh_in_progress,"committed":status.committed,
                "warning":"Catalog data does not establish live capacity or authorize inference."})
            );
        }
        CatalogCommand::Markets { after, limit, .. } => {
            let read = catalog.read()?;
            let page = read.page(
                &format!("{CATALOG_PREFIX}markets/"),
                after.as_deref(),
                limit,
            )?;
            println!(
                "{}",
                json!({"lane":"proxy","generation":read.status().generation,"invalidated":read.status().invalidated,
                "entries":page.entries,"next_after":page.next_after})
            );
        }
        CatalogCommand::Suggest {
            report,
            cursor,
            limit,
            ..
        } => {
            let mut bytes = Vec::new();
            std::fs::File::open(report)?
                .take(16 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            require(
                bytes.len() <= 16 * 1024,
                "proxy discovery report exceeds bound",
            )?;
            let request = serde_json::from_slice(&bytes)?;
            let result = catalog
                .read()?
                .suggest(&request, cursor.as_deref(), limit)?;
            println!("{}", serde_json::to_string(&result)?);
        }
        CatalogCommand::Sync(_) => {
            let client = DiscoveryClient::new(&config.peer_rpc_url, config.identity)?;
            loop {
                match catalog.refresh_page(&client, supervisor::unix_ms()).await? {
                    RefreshOutcome::Committed(status) => {
                        println!("{}", json!({"lane":"proxy","state":"ready","generation":status.generation,"committed":status.committed}));
                        break;
                    }
                    RefreshOutcome::Staged(_) => tokio::time::sleep(std::time::Duration::from_millis(config.refresh.page_pause_ms)).await,
                    RefreshOutcome::Invalidated => return Err(crate::invalid("catalog cursor expired or changed; a fresh traversal is prepared, run sync again")),
                }
            }
        }
        CatalogCommand::Watch(_) => {
            let client = DiscoveryClient::new(&config.peer_rpc_url, config.identity)?;
            let (shutdown, receive_shutdown) = watch::channel(false);
            let (updates, mut receive_updates) = watch::channel(supervisor::Health::default());
            let runner = supervisor::run(
                catalog,
                client,
                config.refresh,
                supervisor::unix_ms() ^ u64::from(std::process::id()),
                receive_shutdown,
                updates,
            );
            tokio::pin!(runner);
            let stopped = stop_signal();
            tokio::pin!(stopped);
            let mut last = None;
            loop {
                tokio::select! {
                    result = &mut runner => return result,
                    result = &mut stopped => { result?; let _ = shutdown.send(true); return runner.await; },
                    result = receive_updates.changed() => {
                        if result.is_err() { return runner.await; }
                        let health = receive_updates.borrow_and_update().clone();
                        // Report phase/cause changes, not every page/heartbeat.
                        let state = (health.phase, health.error_code);
                        if last != Some(state) { println!("{}", serde_json::to_string(&health)?); last = Some(state); }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn stop_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
