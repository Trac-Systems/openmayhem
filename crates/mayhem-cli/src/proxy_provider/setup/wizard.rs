//! Guided CLI over the same retained Flow used by the local dashboard.
use super::*;
use mayhem_proxy::setup::{Flow, FlowAction, FlowConfig, FlowView, ProfileMarket};
use std::io::{self, Write};
mod declarations;

#[derive(Debug, Args)]
pub struct WizardArgs {
    /// Protected host configuration: private connection, profile and optional probe/admission services.
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    /// Print the local operator projection without changing state or contacting services.
    #[arg(long, conflicts_with = "action_file")]
    inspect: bool,
    /// Execute one explicitly authored typed action from an owner-only file (unattended use).
    #[arg(long, value_name = "PATH")]
    action_file: Option<PathBuf>,
    #[command(flatten)]
    wallet: WalletLocatorArgs,
}
pub(super) async fn resume(config: PathBuf, wallet: WalletLocatorArgs) -> Result<()> {
    run(WizardArgs {
        config,
        inspect: false,
        action_file: None,
        wallet,
    })
    .await
}
fn prompt(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    io::stdout().flush()?;
    let mut value = String::new();
    if io::stdin().read_line(&mut value)? == 0 {
        anyhow::bail!("input ended; retained setup is unchanged")
    }
    let value = value.trim();
    Ok(if value.is_empty() {
        default.into()
    } else {
        value.into()
    })
}
fn show(view: &FlowView) -> Result<()> {
    println!(
        "\nProvider setup • {} • {}",
        view.connection.id,
        serde_json::to_string(&view.endpoint)?
    );
    println!(
        "Draft revision: {}",
        view.review
            .as_ref()
            .map(|r| r.revision.to_string())
            .unwrap_or("not saved".into())
    );
    for step in &view.steps {
        println!(
            "  {}: {}",
            step["step"],
            step.get("state").unwrap_or(&step["structural"])
        );
    }
    if let Some(saved) = &view.enrollment {
        if saved["state"] == "admitted" {
            println!("Admission recorded: {}", saved["entitlement_id"]);
            if let Some(code) = saved["review_code"].as_str() {
                println!("Payment requires review: {code}. Admission history is retained; reconcile status for updates.");
            }
        }
        let invoice = &saved["invoice"];
        if !invoice.is_null() {
            println!(
                "Invoice {} • {} • {} • fee USD {}",
                invoice["invoice_id"],
                invoice["rail"],
                invoice["payment_status"],
                invoice["fee_usd"]
            );
            println!("Required {} / verified {} / short {} / excess {} (rail base units; FIAT minor units). Quote expired: {}",invoice["amount_base_units"],invoice["received_amount_base_units"],invoice["missing_amount_base_units"],invoice["excess_amount_base_units"],invoice["quote_expired"]);
            println!(
                "Payment instructions: {}",
                serde_json::to_string_pretty(&invoice["collection"])?
            );
            println!(
                "Review: {}. Current draft projection: {}",
                invoice["review_code"], saved["for_current_revision"]
            );
        }
    }
    Ok(())
}
async fn execute(
    flow: &Flow,
    action: FlowAction,
    wallet: &WalletLocatorArgs,
) -> Result<mayhem_proxy::setup::FlowResult> {
    if matches!(
        action,
        FlowAction::Enrollment { .. } | FlowAction::Publish { .. } | FlowAction::ConfirmDeclaration { .. }
    ) {
        let keypair = resolve_wallet_keypair_path(wallet)?;
        let key = cached_wallet_signing_key(
            &keypair,
            wallet.wallet_password.as_deref().unwrap_or_default(),
        )
        .await?;
        Ok(flow.execute(action, Some(&key)).await?)
    } else {
        Ok(flow.execute(action, None).await?)
    }
}
pub async fn run(args: WizardArgs) -> Result<()> {
    let mut flow = Flow::open(FlowConfig::load(&args.config)?)?;
    if let Some(settings) = flow.run_settings() {
        let home = args
            .wallet
            .home
            .clone()
            .map(Ok)
            .unwrap_or_else(crate::default_home)?;
        let host = super::super::supervisor::Host::new(
            home,
            resolve_wallet_keypair_path(&args.wallet)?,
            settings.wallet_password_file.clone(),
            flow.provider().clone(),
        )?;
        flow = flow.with_run_lifecycle(std::sync::Arc::new(host));
    }
    if args.inspect {
        println!("{}", serde_json::to_string(&flow.view()?)?);
        return Ok(());
    }
    if let Some(path) = args.action_file {
        let bytes = mayhem_proxy::connector::config::private_file(&path, 64 * 1024)?;
        let action: FlowAction = serde_json::from_slice(&bytes)?;
        let result = execute(&flow, action, &args.wallet).await?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    println!("Existing connection and wallet only. No automatic payment, probe, publication or model-server restart.");
    loop {
        let view = flow.view()?;
        show(&view)?;
        if let Some(d) = &view.declaration { println!("Data handling: {} (declared, not verified)", d.state); }
        println!("j Review data-handling declarations  y Sign retained declaration review  w Review withdrawal");
        println!("c Connect  d Discover  s Select/price  k Check  p Probe  a Admission facts\ni Invoice/create  t Status  f FIAT checkout  v Review publication  u Publish\nr Recover original probe  o Recover original publication  g Review Run  b Begin Run  h Reconcile Run  x Exit");
        let command = prompt("Action", "x")?;
        if command == "x" {
            return Ok(());
        }
        let revision = view.review.as_ref().map(|r| r.revision);
        let needs_revision =
            || revision.ok_or_else(|| anyhow::anyhow!("save your selection first"));
        let action = match command.as_str() {
            "j" => declarations::review(&flow, &view).await?,
            "w" => declarations::withdraw(&view)?,
            "y" => {
                let pending=view.pending_declaration.as_ref().ok_or_else(|| anyhow::anyhow!("review declarations first"))?;
                println!("{}",serde_json::to_string_pretty(pending)?);
                if prompt("Sign exactly these promises until their stated expiry? (yes/no)","no")? != "yes" { continue; }
                FlowAction::ConfirmDeclaration { expected_revision:needs_revision()?, plan_digest:pending.plan.plan_digest.clone() }
            }
            "c" => FlowAction::Connect {},
            "d" => FlowAction::Discover {
                expected_inventory_revision: view
                    .inventory
                    .as_ref()
                    .map(|i| i.revision)
                    .unwrap_or(0),
            },
            "s" => {
                if let Some(inventory) = &view.inventory {
                    println!(
                        "Private upstream inventory: {}",
                        serde_json::to_string(inventory)?
                    )
                }
                let mut choice = view.selection;
                choice.upstream_model = prompt(
                    "Upstream model (manual IDs are supported)",
                    &choice.upstream_model,
                )?;
                if let ProfileMarket::CreateMarket { slug, model } = &mut choice.market {
                    *slug = prompt("Market slug", slug)?;
                    model.model_id = prompt("Declared public model label", &model.model_id)?;
                }
                choice.membership.served_context = prompt(
                    "Served context",
                    &choice.membership.served_context.to_string(),
                )?
                .parse()?;
                choice.membership.max_concurrency = prompt(
                    "Shared concurrency",
                    &choice.membership.max_concurrency.to_string(),
                )?
                .parse()?;
                choice.membership.revision = prompt(
                    "Membership revision",
                    &choice.membership.revision.to_string(),
                )?
                .parse()?;
                let rails: Vec<mayhem_proto::proxy::ProxyRail> = serde_json::from_str(&prompt(
                    "Accepted rails JSON (fiat/tap/tnk)",
                    &serde_json::to_string(&choice.membership.accepted_rails)?,
                )?)?;
                choice.membership.accepted_rails = rails.clone();
                for offer in &mut choice.offers {
                    println!("Offer {}", offer.ctx_bracket);
                    offer.revision =
                        prompt("Offer revision", &offer.revision.to_string())?.parse()?;
                    offer.per_request_au =
                        prompt("Per request AU", &offer.per_request_au.to_string())?.parse()?;
                    offer.min_session_au =
                        prompt("Minimum session AU", &offer.min_session_au.to_string())?.parse()?;
                    for rate in &mut offer.rates {
                        rate.per_unit_au = prompt(
                            &format!("{} AU per {} units", rate.unit, rate.granularity),
                            &rate.per_unit_au.to_string(),
                        )?
                        .parse()?;
                    }
                    offer.accepted_rails.retain(|r| rails.contains(r));
                }
                println!(
                    "Exact declaration: {}",
                    serde_json::to_string_pretty(&choice)?
                );
                if prompt("Save exact selection? type save", "cancel")? != "save" {
                    continue;
                }
                FlowAction::Select {
                    expected_revision: revision,
                    choice,
                }
            }
            "g" | "b" => {
                let expected_revision = needs_revision()?;
                let plan = flow
                    .execute(FlowAction::RunPlan { expected_revision }, None)
                    .await?;
                println!(
                    "Managed Run plan: {}",
                    serde_json::to_string_pretty(&plan.action_result)?
                );
                if command=="g" || prompt("Install exactly this persistent controller? Configured recovery probes may consume the existing allowance. Type run","cancel")? != "run" { continue; }
                FlowAction::StartRun {
                    expected_revision,
                    plan_digest: serde_json::from_value(plan.action_result["plan_digest"].clone())?,
                }
            }
            "h" => FlowAction::RecoverRun {},
            "k" => FlowAction::Check {
                expected_revision: needs_revision()?,
            },
            "p" => {
                let plan = view
                    .probe_plan
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("no protected probe plan configured"))?;
                println!(
                    "Probe scope/budget: {}",
                    serde_json::to_string_pretty(plan)?
                );
                if prompt(
                    "May consume upstream allowance. Type probe to run once",
                    "cancel",
                )? != "probe"
                {
                    continue;
                }
                FlowAction::Probe {
                    expected_revision: needs_revision()?,
                    probe_plan_digest: serde_json::from_value(plan["digest"].clone())?,
                }
            }
            "r" => FlowAction::RecoverProbe {
                expected_revision: needs_revision()?,
            },
            "a" => FlowAction::AdmissionCheck {
                expected_revision: needs_revision()?,
            },
            "i" | "t" | "f" => {
                let operation = match command.as_str() {
                    "i" => EnrollmentAction::Create,
                    "t" => EnrollmentAction::Status,
                    _ => EnrollmentAction::Checkout,
                };
                let rail = if command == "i" {
                    Some(serde_json::from_value(serde_json::Value::String(prompt(
                        "Invoice rail: fiat/tnk/tap",
                        "fiat",
                    )?))?)
                } else {
                    None
                };
                if command != "t"
                    && prompt(
                        "Create/recover original invoice or checkout; no funds sent. Type continue",
                        "cancel",
                    )? != "continue"
                {
                    continue;
                }
                FlowAction::Enrollment {
                    expected_revision: needs_revision()?,
                    operation,
                    rail,
                }
            }
            "v" | "u" => {
                let expected_revision = needs_revision()?;
                let plan = flow
                    .execute(
                        FlowAction::PublicationPlan {
                            expected_revision,
                            offers_only: false,
                        },
                        None,
                    )
                    .await?;
                println!(
                    "Exact publication: {}",
                    serde_json::to_string_pretty(&plan.action_result)?
                );
                if command == "v"
                    || prompt("Sign and submit exactly this plan? type publish", "cancel")?
                        != "publish"
                {
                    continue;
                }
                FlowAction::Publish {
                    expected_revision,
                    offers_only: false,
                    plan_digest: serde_json::from_value(plan.action_result["plan_digest"].clone())?,
                }
            }
            "o" => FlowAction::RecoverPublication {
                expected_revision: needs_revision()?,
            },
            _ => {
                println!("Choose an action from the menu.");
                continue;
            }
        };
        match execute(&flow,action,&args.wallet).await{
            Ok(result)=>{println!("{}",serde_json::to_string_pretty(&result.action_result)?);show(&result.view)?;},
            Err(error)=>eprintln!("Action refused or unavailable: {error}. Inspect retained state; do not repeat payment."),
        }
    }
}
