//! Thin CLI over the shared setup state. Public JSON never contains the private
//! draft, connection reference or loaded credential. No wallet is unlocked.
use anyhow::Result;
use clap::{Args, Subcommand};
use mayhem_proxy::setup::{Input, ProbePlan, Store};
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
    /// Run one explicit bounded operator probe; may consume upstream allowance.
    Probe {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
        /// Protected plan with the request, cumulative allowance and shared capacity scope.
        #[arg(long, value_name = "PATH")]
        plan: PathBuf,
    },
    /// Reconcile the original interrupted probe; never dispatch or clear unknown work.
    RecoverProbe {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Resume the same draft and display its redacted public review.
    Inspect(DraftArgs),
}
pub async fn run(command: Command) -> Result<()> {
    if let Command::Probe {
        args,
        expected_revision,
        plan,
    } = command
    {
        let (store, plan) = tokio::task::spawn_blocking(move || {
            Ok::<_, mayhem_proxy::setup::Error>((
                Store::open(args.directory)?,
                ProbePlan::load(&plan)?,
            ))
        })
        .await??;
        let review = store.probe(expected_revision, plan).await?;
        println!("{}", serde_json::to_string(&review)?);
        return Ok(());
    }
    let review = tokio::task::spawn_blocking(move || {
        let args = match &command {
            Command::Create { args, .. }
            | Command::Update { args, .. }
            | Command::Check { args, .. }
            | Command::RecoverProbe { args, .. }
            | Command::Probe { args, .. }
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
            Command::RecoverProbe {
                expected_revision, ..
            } => store.recover_probe(expected_revision),
            Command::Probe { .. } => unreachable!("handled before the blocking operation"),
            Command::Inspect(_) => store.inspect(),
        }
    })
    .await??;
    println!("{}", serde_json::to_string(&review)?);
    Ok(())
}
