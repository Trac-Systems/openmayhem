//! Thin CLI over the shared setup state. Public JSON never contains the private
//! draft, connection reference or loaded credential. Explicit enrollment signs
//! scoped identity challenges; Publish signs reviewed registry operations.
use super::{cached_wallet_signing_key, resolve_wallet_keypair_path, WalletLocatorArgs};
use anyhow::Result;
use clap::{Args, Subcommand};
use mayhem_proxy::setup::{
    profiles, AdmissionPermit, EnrollmentAction, EnrollmentQuote, Input, ProbePlan, ProfileInput, Store,
};
use std::path::PathBuf;
mod bootstrap;
mod wizard;

#[derive(Debug, Args)]
pub struct DraftArgs {
    /// Existing owner-only setup directory, separate from all runtime stores.
    #[arg(long, value_name = "PATH")]
    directory: PathBuf,
}
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create protected standard-profile setup through explicit prompts; no hand-authored JSON.
    Init(bootstrap::InitArgs),
    /// Guided local setup using the same retained state as the authenticated dashboard.
    Wizard(wizard::WizardArgs),
    /// Recover the original admission invoice, or explicitly open fee checkout.
    /// No funds are sent and no registration or model serving is started.
    Enrollment {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
        /// Trusted admission API origin, HTTPS or literal loopback for local tests.
        #[arg(long)]
        admission_origin: String,
        #[arg(long, default_value_t = 15000)]
        timeout_ms: u64,
        #[arg(long, value_enum, default_value_t = EnrollmentCommand::Status)]
        action: EnrollmentCommand,
        /// Required for create, forbidden for status/checkout/refresh. Does not send money.
        #[arg(long, value_enum)]
        rail: Option<EnrollmentRail>,
        /// Exact old quote from status; both fields are required only for refresh.
        #[arg(long, requires = "invoice_commitment")]
        invoice_id: Option<String>,
        #[arg(long, requires = "invoice_id")]
        invoice_commitment: Option<String>,
        #[command(flatten)]
        wallet: WalletLocatorArgs,
    },
    /// Inspect, preview or export signed data-only connector recipes offline.
    Recipe {
        #[command(subcommand)]
        command: RecipeCommand,
    },
    /// Review exact public operations without signing, network I/O or fees.
    PublicationPlan {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
        /// Change only saved offers on an existing membership, retaining its identity.
        #[arg(long)]
        offers_only: bool,
    },
    /// Sign and retain the exact reviewed operations, then request guarded publication.
    /// Does not collect a fee, issue a permit, install serving or start a model.
    Publish {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        peer_rpc: String,
        #[arg(long, default_value_t = 5000)]
        timeout_ms: u64,
        #[arg(long)]
        offers_only: bool,
        /// Protected verifier-signed permit for the original initial operation.
        #[arg(long, value_name = "PATH")]
        admission_permit: Option<PathBuf>,
        #[command(flatten)]
        wallet: WalletLocatorArgs,
    },
    /// Resume the original signed publication. No wallet unlock or new permit.
    RecoverPublication {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        peer_rpc: String,
        #[arg(long, default_value_t = 5000)]
        timeout_ms: u64,
    },
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
    /// Read canonical admission/sequence facts; never request payment or publish.
    AdmissionCheck {
        #[command(flatten)]
        args: DraftArgs,
        #[arg(long)]
        expected_revision: u64,
        /// Explicit trusted Core peer RPC base, HTTPS or literal loopback HTTP.
        #[arg(long)]
        peer_rpc: String,
        #[arg(long, default_value_t = 5000)]
        timeout_ms: u64,
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
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum EnrollmentCommand {
    Create,
    Status,
    Checkout,
    Refresh,
}
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum EnrollmentRail {
    Fiat,
    Tnk,
    Tap,
}
#[derive(Debug, Subcommand)]
pub enum RecipeCommand {
    /// Verify signature/mapping fixtures and show public identity, without network I/O.
    Inspect {
        #[arg(long, value_name = "PATH")]
        recipe: PathBuf,
    },
    /// Explicit synthetic local sample: show mapping/common shapes; schemas require a probe.
    Preview {
        #[arg(long, value_name = "PATH")]
        recipe: PathBuf,
        #[arg(long, value_name = "PATH")]
        sample: PathBuf,
    },
    /// Emit only the reusable signed recipe to stdout; no private connection export.
    Export {
        #[arg(long, value_name = "PATH")]
        recipe: PathBuf,
    },
}
pub async fn run(command: Command) -> Result<()> {
    let command = match command {
        Command::Init(args) => return bootstrap::run(args).await,
        Command::Wizard(args) => return wizard::run(args).await,
        Command::Enrollment {
            args,
            expected_revision,
            admission_origin,
            timeout_ms,
            action,
            rail,
            invoice_id,
            invoice_commitment,
            wallet,
        } => {
            anyhow::ensure!(
                matches!(action, EnrollmentCommand::Create) == rail.is_some(),
                "--rail is required only for --action create"
            );
            anyhow::ensure!(
                matches!(action, EnrollmentCommand::Refresh) == invoice_id.is_some()
                    && invoice_id.is_some() == invoice_commitment.is_some(),
                "--invoice-id and --invoice-commitment are required only for --action refresh"
            );
            let quote = invoice_id.zip(invoice_commitment).map(|(invoice_id, invoice_commitment)| {
                serde_json::from_value::<EnrollmentQuote>(serde_json::json!({
                    "invoice_id": invoice_id, "invoice_commitment": invoice_commitment
                }))
            }).transpose()?;
            let client = tokio::task::spawn_blocking(move || {
                Store::open(args.directory)?.enrollment_client(
                    expected_revision,
                    &admission_origin,
                    timeout_ms,
                )
            })
            .await??;
            let keypair = resolve_wallet_keypair_path(&wallet)?;
            let key = cached_wallet_signing_key(
                &keypair,
                wallet.wallet_password.as_deref().unwrap_or_default(),
            )
            .await?;
            let action = match action {
                EnrollmentCommand::Create => EnrollmentAction::Create,
                EnrollmentCommand::Status => EnrollmentAction::Status,
                EnrollmentCommand::Checkout => EnrollmentAction::Checkout,
                EnrollmentCommand::Refresh => EnrollmentAction::Refresh,
            };
            let rail = rail.map(|r| match r {
                EnrollmentRail::Fiat => mayhem_proto::proxy::ProxyRail::Fiat,
                EnrollmentRail::Tnk => mayhem_proto::proxy::ProxyRail::Tnk,
                EnrollmentRail::Tap => mayhem_proto::proxy::ProxyRail::Tap,
            });
            let result = client.execute_with_quote(&key, action, rail, quote).await?;
            drop(key);
            println!("{}", serde_json::to_string(&result)?);
            return Ok(());
        }
        Command::Recipe { command } => {
            let result = tokio::task::spawn_blocking(move || -> Result<String> {
                match command {
                    RecipeCommand::Inspect { recipe } => Ok(serde_json::to_string(
                        &mayhem_proxy::recipe::Signed::load(&recipe)?.review()?,
                    )?),
                    RecipeCommand::Preview { recipe, sample } => Ok(serde_json::to_string(
                        &mayhem_proxy::recipe::Signed::load(&recipe)?.preview_file(&sample)?,
                    )?),
                    RecipeCommand::Export { recipe } => Ok(String::from_utf8(
                        mayhem_proxy::recipe::Signed::load(&recipe)?.export()?,
                    )?),
                }
            })
            .await??;
            println!("{result}");
            return Ok(());
        }
        Command::PublicationPlan {
            args,
            expected_revision,
            offers_only,
        } => {
            let plan = tokio::task::spawn_blocking(move || {
                Store::open(args.directory)?.publication_plan(expected_revision, offers_only)
            })
            .await??;
            println!("{}", serde_json::to_string(&plan)?);
            return Ok(());
        }
        Command::Publish {
            args,
            expected_revision,
            peer_rpc,
            timeout_ms,
            offers_only,
            admission_permit,
            wallet,
        } => {
            let (store, plan, permit) = tokio::task::spawn_blocking(move || {
                let store = Store::open(args.directory)?;
                let plan = store.publication_plan(expected_revision, offers_only)?;
                let permit = admission_permit
                    .as_deref()
                    .map(AdmissionPermit::load)
                    .transpose()?;
                Ok::<_, mayhem_proxy::setup::Error>((store, plan, permit))
            })
            .await??;
            let keypair = resolve_wallet_keypair_path(&wallet)?;
            let key = cached_wallet_signing_key(
                &keypair,
                wallet.wallet_password.as_deref().unwrap_or_default(),
            )
            .await?;
            let authorization = plan.authorize(&key, permit)?;
            drop(key);
            let review = store
                .publish(expected_revision, &peer_rpc, timeout_ms, authorization)
                .await?;
            println!("{}", serde_json::to_string(&review)?);
            return Ok(());
        }
        Command::RecoverPublication {
            args,
            expected_revision,
            peer_rpc,
            timeout_ms,
        } => {
            let store = tokio::task::spawn_blocking(move || Store::open(args.directory)).await??;
            let review = store
                .recover_publication(expected_revision, &peer_rpc, timeout_ms)
                .await?;
            println!("{}", serde_json::to_string(&review)?);
            return Ok(());
        }
        Command::AdmissionCheck {
            args,
            expected_revision,
            peer_rpc,
            timeout_ms,
        } => {
            let store = tokio::task::spawn_blocking(move || Store::open(args.directory)).await??;
            let review = store
                .admission_check(expected_revision, &peer_rpc, timeout_ms)
                .await?;
            println!("{}", serde_json::to_string(&review)?);
            return Ok(());
        }
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
            Command::Init(_)
            | Command::Wizard(_)
            | Command::Enrollment { .. }
            | Command::Recipe { .. }
            | Command::Profiles
            | Command::PublicationPlan { .. }
            | Command::Publish { .. }
            | Command::RecoverPublication { .. }
            | Command::Discover { .. }
            | Command::Inventory { .. }
            | Command::AdmissionCheck { .. } => {
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
            Command::Init(_)
            | Command::Wizard(_)
            | Command::Enrollment { .. }
            | Command::Recipe { .. }
            | Command::Profiles
            | Command::PublicationPlan { .. }
            | Command::Publish { .. }
            | Command::RecoverPublication { .. }
            | Command::Discover { .. }
            | Command::Inventory { .. }
            | Command::AdmissionCheck { .. } => {
                unreachable!("handled before the blocking operation")
            }
        }
    })
    .await??;
    println!("{}", serde_json::to_string(&review)?);
    Ok(())
}

#[cfg(test)]
mod recipe_tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Command,
    }
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../mayhem-proxy/tests/fixtures/recipes")
            .join(name)
    }
    #[test]
    fn enrollment_defaults_to_status_and_has_no_transfer_or_publication_option() {
        let common = [
            "setup",
            "enrollment",
            "--directory",
            "private-draft",
            "--expected-revision",
            "2",
            "--admission-origin",
            "https://admission.invalid",
        ];
        assert!(matches!(
            Cli::try_parse_from(common).unwrap().command,
            Command::Enrollment {
                action: EnrollmentCommand::Status,
                rail: None,
                ..
            }
        ));
        for rail in ["fiat", "tnk", "tap"] {
            let mut args = common.to_vec();
            args.extend(["--action", "create", "--rail", rail]);
            assert!(matches!(
                Cli::try_parse_from(args).unwrap().command,
                Command::Enrollment {
                    action: EnrollmentCommand::Create,
                    rail: Some(_),
                    ..
                }
            ));
        }
        for forbidden in ["--send", "--publish", "--issuer-key", "--amount"] {
            let mut args = common.to_vec();
            args.push(forbidden);
            assert!(Cli::try_parse_from(args).is_err());
        }
        let id = "11".repeat(32);
        let commitment = "22".repeat(32);
        let mut args = common.to_vec();
        args.extend(["--action", "refresh", "--invoice-id", &id]);
        assert!(Cli::try_parse_from(args.clone()).is_err());
        args.extend(["--invoice-commitment", &commitment]);
        assert!(matches!(
            Cli::try_parse_from(args).unwrap().command,
            Command::Enrollment {
                action: EnrollmentCommand::Refresh, rail: None,
                invoice_id: Some(_), invoice_commitment: Some(_), ..
            }
        ));
    }
    #[test]
    fn recipe_commands_are_readonly_and_require_explicit_local_inputs() {
        for args in [
            vec!["setup", "recipe", "inspect", "--recipe", "sample.json"],
            vec!["setup", "recipe", "export", "--recipe", "sample.json"],
            vec![
                "setup",
                "recipe",
                "preview",
                "--recipe",
                "sample.json",
                "--sample",
                "preview.json",
            ],
        ] {
            assert!(matches!(
                Cli::try_parse_from(args).unwrap().command,
                Command::Recipe { .. }
            ));
        }
        for args in [
            vec!["setup", "recipe", "inspect"],
            vec!["setup", "recipe", "preview", "--recipe", "a.json"],
            vec![
                "setup",
                "recipe",
                "inspect",
                "--recipe",
                "a.json",
                "--wallet-password",
                "no",
            ],
            vec![
                "setup",
                "recipe",
                "inspect",
                "--recipe",
                "a.json",
                "--url",
                "https://forbidden.invalid",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
    #[tokio::test]
    async fn recipe_inspect_export_and_preview_use_the_real_offline_handlers() {
        run(Command::Recipe {
            command: RecipeCommand::Inspect {
                recipe: fixture("Chat.json"),
            },
        })
        .await
        .unwrap();
        run(Command::Recipe {
            command: RecipeCommand::Export {
                recipe: fixture("Decisions.json"),
            },
        })
        .await
        .unwrap();
        run(Command::Recipe {
            command: RecipeCommand::Preview {
                recipe: fixture("Chat.json"),
                sample: fixture("Chat-preview.json"),
            },
        })
        .await
        .unwrap();
        assert!(run(Command::Recipe {
            command: RecipeCommand::Preview {
                recipe: fixture("Decisions.json"),
                sample: fixture("Chat-preview.json")
            }
        })
        .await
        .is_err());
    }
}
