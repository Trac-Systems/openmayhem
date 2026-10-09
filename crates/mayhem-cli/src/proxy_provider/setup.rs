//! Thin CLI over the shared setup state. Public JSON never contains the private
//! draft, connection reference or loaded credential. No wallet is unlocked.
use anyhow::Result;
use clap::{Args, Subcommand};
use mayhem_proxy::setup::{profiles, Input, ProbePlan, ProfileInput, Store};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct DraftArgs {
    /// Existing owner-only setup directory, separate from all runtime stores.
    #[arg(long, value_name = "PATH")]
    directory: PathBuf,
}
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show local endpoint templates; this never contacts or certifies an upstream.
    Profiles,
    /// Explicit bounded model-list GET; credentials are resolved only for this read.
    Discover {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long, value_name = "PATH")]
        connection: PathBuf,
        /// Zero creates an inventory; otherwise use its exact current revision.
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, default_value_t = 5000)]
        timeout_ms: u64,
    },
    /// Inspect retained discovery without network I/O or automatic retry.
    Inventory {
        #[command(flatten)]
        args: DraftArgs,
        /// Include private upstream IDs for the local operator; never a public catalog.
        #[arg(long)]
        show_models: bool,
    },
    /// Build an explicit profile declaration without hand-authoring recipe hashes.
    Prepare {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long, value_name = "PATH")]
        input: PathBuf,
        /// Omit for first creation; updates require the exact original draft revision.
        #[arg(long)]
        expected_revision: Option<u64>,
    },
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
    #[command(alias = "resume")]
    Inspect(DraftArgs),
}
pub async fn run(command: Command) -> Result<()> {
    let command = match command {
        Command::Profiles => {
            println!("{}", serde_json::to_string(&profiles()?)?);
            return Ok(());
        }
        Command::Discover {
            args,
            connection,
            expected_revision,
            timeout_ms,
        } => {
            let store = tokio::task::spawn_blocking(move || Store::open(args.directory)).await??;
            let review = store
                .discover(&connection, expected_revision, timeout_ms)
                .await?;
            println!("{}", serde_json::to_string(&review)?);
            return Ok(());
        }
        Command::Inventory { args, show_models } => {
            let review = tokio::task::spawn_blocking(move || {
                Store::open(args.directory)?.inspect_connection(show_models)
            })
            .await??;
            println!("{}", serde_json::to_string(&review)?);
            return Ok(());
        }
        command => command,
    };
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
            | Command::Prepare { args, .. }
            | Command::Update { args, .. }
            | Command::Check { args, .. }
            | Command::RecoverProbe { args, .. }
            | Command::Probe { args, .. }
            | Command::Inspect(args) => args,
            Command::Profiles | Command::Discover { .. } | Command::Inventory { .. } => {
                unreachable!("handled before the blocking operation")
            }
        };
        let store = Store::open(&args.directory)?;
        match command {
            Command::Create { input, .. } => store.create(Input::load(&input)?),
            Command::Prepare {
                input,
                expected_revision,
                ..
            } => store.prepare(ProfileInput::load(&input)?, expected_revision),
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
            Command::Profiles | Command::Discover { .. } | Command::Inventory { .. } => {
                unreachable!("handled before the blocking operation")
            }
        }
    })
    .await??;
    println!("{}", serde_json::to_string(&review)?);
    Ok(())
}
