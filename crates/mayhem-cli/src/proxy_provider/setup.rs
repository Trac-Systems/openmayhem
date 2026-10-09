//! Thin CLI over the shared setup state. Public JSON never contains the private
//! draft, connection reference or loaded credential. No wallet is unlocked.
use anyhow::Result;
use clap::{Args, Subcommand};
use mayhem_proxy::setup::{Input, Store};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct DraftArgs {
    /// Existing owner-only setup directory, separate from all runtime stores.
    #[arg(long, value_name = "PATH")]
    directory: PathBuf,
}
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Save an explicit private setup declaration; no probes or payment.
    Create {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long, value_name = "PATH")]
        input: PathBuf,
    },
    /// Replace one original draft revision and invalidate its local check.
    Update {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, value_name = "PATH")]
        input: PathBuf,
    },
    /// Check local structural bindings; not a model/conformance check.
    Check {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Resume the same draft and display its redacted public review.
    Inspect(DraftArgs),
}
pub async fn run(command: Command) -> Result<()> {
    let review = tokio::task::spawn_blocking(move || {
        let args = match &command {
            Command::Create { args, .. }
            | Command::Update { args, .. }
            | Command::Check { args, .. }
            | Command::Inspect(args) => args,
        };
        let store = Store::open(&args.directory)?;
        match command {
            Command::Create { input, .. } => store.create(Input::load(&input)?),
            Command::Update {
                input,
                expected_revision,
                ..
            } => store.update(expected_revision, Input::load(&input)?),
            Command::Check {
                expected_revision, ..
            } => store.check(expected_revision),
            Command::Inspect(_) => store.inspect(),
        }
    })
    .await??;
    println!("{}", serde_json::to_string(&review)?);
    Ok(())
}
