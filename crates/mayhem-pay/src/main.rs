#![forbid(unsafe_code)]

use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const TNK_E18: u128 = 1_000_000_000_000_000_000;
const DEFAULT_HOLDBACK_EPOCHS: u64 = 168;
const DEFAULT_CHALLENGE_EPOCHS: u64 = 6;
const DEFAULT_PAYOUT_MIN_MU: u64 = 1_000_000;
const DEFAULT_MSB_TX_PLACEHOLDER: &str = "PENDING_MSB_TX_HASH";

#[derive(Debug, Parser)]
#[command(name = "mayhem-pay", about = "Mayhem payment and payout helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compute deterministic payout wallet actions from a contract-state snapshot.
    PayoutPlan(PayoutPlanArgs),
}

#[derive(Debug, Args)]
struct PayoutPlanArgs {
    /// JSON snapshot containing providers, earnings, fee, params, and optionally epoch/rate.
    #[arg(long)]
    snapshot: PathBuf,
    /// Payout evidence epoch. Overrides snapshot.epoch.
    #[arg(long)]
    epoch: Option<u64>,
    /// Contract timestamp for payout_confirm commands.
    #[arg(long, default_value_t = 0)]
    at: u64,
    /// Fresh TNK/USD e6 oracle rate. Overrides snapshot.rate_tnk_usd_e6.
    #[arg(long)]
    rate_tnk_usd_e6: Option<u64>,
    /// Admin public key expected to have set provider payout targets. Overrides snapshot.admin.
    #[arg(long)]
    admin_pubkey: Option<String>,
    /// Router treasury payout target used for fee sweep intents.
    #[arg(long, default_value = "treasury")]
    treasury: String,
    /// Placeholder used in generated payout_confirm commands until MSB transfer tx hashes exist.
    #[arg(long, default_value = DEFAULT_MSB_TX_PLACEHOLDER)]
    msb_tx_hash: String,
    /// Placeholder used in generated payout_confirm commands until Stripe transfer IDs exist.
    #[arg(long, default_value = "PENDING_STRIPE_TRANSFER_ID")]
    stripe_transfer_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Snapshot {
    #[serde(default)]
    admin: Option<Value>,
    #[serde(default)]
    epoch: Option<u64>,
    #[serde(default)]
    rate_tnk_usd_e6: Option<u64>,
    #[serde(default)]
    params: Params,
    #[serde(default)]
    providers: Vec<ProviderRecord>,
    #[serde(default)]
    earnings: Vec<EarningRecord>,
    #[serde(default)]
    fee: Option<FeeRecord>,
}

#[derive(Debug, Clone, Deserialize)]
struct Params {
    #[serde(default = "default_holdback_epochs")]
    holdback_epochs: u64,
    #[serde(default = "default_challenge_epochs")]
    challenge_epochs: u64,
    #[serde(default = "default_payout_min_mu")]
    payout_min_mu: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            holdback_epochs: DEFAULT_HOLDBACK_EPOCHS,
            challenge_epochs: DEFAULT_CHALLENGE_EPOCHS,
            payout_min_mu: DEFAULT_PAYOUT_MIN_MU,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderRecord {
    provider: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    payout: Option<PayoutTarget>,
}

#[derive(Debug, Clone, Deserialize)]
struct PayoutTarget {
    addr: String,
    method: String,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    set_by: Option<String>,
    #[serde(default)]
    set_by_role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct EarningRecord {
    provider: String,
    denom: String,
    total_mu: u64,
    held_mu: u64,
    paid_cum_mu: u64,
    #[serde(default)]
    updated_epoch: Option<u64>,
    #[serde(default)]
    holdbacks: Option<Vec<HoldbackBucket>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct HoldbackBucket {
    epoch: u64,
    mu: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct FeeRecord {
    #[serde(default)]
    denom: Option<String>,
    cum_mu: u64,
    swept_cum_mu: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct PayoutPlan {
    epoch: u64,
    at: u64,
    rate_tnk_usd_e6: u64,
    provider_payouts: Vec<ProviderPayout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fee_sweep: Option<FeeSweep>,
    skipped: Vec<SkippedPayout>,
    totals: PlanTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ProviderPayout {
    provider: String,
    method: String,
    target: String,
    mu: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tnk_e18: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    msb_transfer: Option<MsbTransfer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stripe_transfer: Option<StripeTransfer>,
    confirm_command: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FeeSweep {
    target: String,
    mu: u64,
    tnk_e18: String,
    msb_transfer: MsbTransfer,
    confirm_command: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct MsbTransfer {
    to: String,
    tnk_e18: String,
    network_pays_fee: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct StripeTransfer {
    destination: String,
    amount_cents: u64,
    currency: String,
    rounding_mu: u64,
    metadata: PayoutMetadata,
    network_pays_fee: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct PayoutMetadata {
    mayhem_provider: String,
    mayhem_epoch: u64,
    mayhem_mu: u64,
    mayhem_denom: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mayhem_fiat_currency: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SkippedPayout {
    who: String,
    reason: String,
    released_mu: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct PlanTotals {
    provider_mu: u64,
    fee_sweep_mu: u64,
    transfer_count: usize,
}

#[derive(Debug, Clone)]
struct RailOptions {
    msb_tx_hash: String,
    stripe_transfer_id: String,
}

fn default_holdback_epochs() -> u64 {
    DEFAULT_HOLDBACK_EPOCHS
}

fn default_challenge_epochs() -> u64 {
    DEFAULT_CHALLENGE_EPOCHS
}

fn default_payout_min_mu() -> u64 {
    DEFAULT_PAYOUT_MIN_MU
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::PayoutPlan(args) => {
            let snapshot = read_snapshot(&args.snapshot)?;
            let epoch = args
                .epoch
                .or(snapshot.epoch)
                .context("payout epoch required via --epoch or snapshot.epoch")?;
            let rate = args.rate_tnk_usd_e6.or(snapshot.rate_tnk_usd_e6).context(
                "TNK/USD e6 rate required via --rate-tnk-usd-e6 or snapshot.rate_tnk_usd_e6",
            )?;
            let admin_pubkey = args
                .admin_pubkey
                .as_deref()
                .map(str::trim)
                .filter(|admin| !admin.is_empty())
                .map(str::to_string)
                .or_else(|| snapshot_admin_pubkey(snapshot.admin.as_ref()))
                .context("admin pubkey required via --admin-pubkey or snapshot.admin")?;
            let plan = build_plan(
                &snapshot,
                epoch,
                args.at,
                rate,
                &admin_pubkey,
                &args.treasury,
                &RailOptions {
                    msb_tx_hash: args.msb_tx_hash,
                    stripe_transfer_id: args.stripe_transfer_id,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
    }
    Ok(())
}

fn read_snapshot(path: &PathBuf) -> Result<Snapshot> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn build_plan(
    snapshot: &Snapshot,
    epoch: u64,
    at: u64,
    rate_tnk_usd_e6: u64,
    admin_pubkey: &str,
    treasury: &str,
    rail_options: &RailOptions,
) -> Result<PayoutPlan> {
    if rate_tnk_usd_e6 == 0 {
        bail!("rate_tnk_usd_e6 must be positive");
    }
    let admin_pubkey = admin_pubkey.trim();
    if admin_pubkey.is_empty() {
        bail!("admin_pubkey must be non-empty");
    }

    let providers: HashMap<&str, &ProviderRecord> = snapshot
        .providers
        .iter()
        .map(|provider| (provider.provider.as_str(), provider))
        .collect();
    let lock_epochs = snapshot
        .params
        .holdback_epochs
        .max(snapshot.params.challenge_epochs);

    let mut provider_payouts = Vec::new();
    let mut skipped = Vec::new();
    for earning in &snapshot.earnings {
        if earning.denom != "mu_usd" {
            bail!("earning {} has unsupported denomination", earning.provider);
        }
        let provider = match providers.get(earning.provider.as_str()) {
            Some(provider) => *provider,
            None => {
                skipped.push(SkippedPayout {
                    who: earning.provider.clone(),
                    reason: "provider_not_found".to_string(),
                    released_mu: 0,
                });
                continue;
            }
        };
        if provider.status.as_deref() != Some("active") {
            skipped.push(SkippedPayout {
                who: earning.provider.clone(),
                reason: "provider_not_active".to_string(),
                released_mu: 0,
            });
            continue;
        }
        let Some(target) = &provider.payout else {
            skipped.push(SkippedPayout {
                who: earning.provider.clone(),
                reason: "payout_target_not_set".to_string(),
                released_mu: 0,
            });
            continue;
        };
        let target_set_by_admin = target
            .set_by
            .as_deref()
            .map(str::trim)
            .is_some_and(|set_by| set_by == admin_pubkey)
            && target.set_by_role.as_deref() == Some("admin");
        if !target_set_by_admin {
            skipped.push(SkippedPayout {
                who: earning.provider.clone(),
                reason: "payout_target_not_admin_set".to_string(),
                released_mu: 0,
            });
            continue;
        }
        if target.method == "coinbase" {
            skipped.push(SkippedPayout {
                who: earning.provider.clone(),
                reason: "payout_method_retired".to_string(),
                released_mu: 0,
            });
            continue;
        }
        if !matches!(target.method.as_str(), "tnk" | "stripe") {
            skipped.push(SkippedPayout {
                who: earning.provider.clone(),
                reason: "unsupported_payout_method".to_string(),
                released_mu: 0,
            });
            continue;
        }
        if target.addr.trim().is_empty() {
            skipped.push(SkippedPayout {
                who: earning.provider.clone(),
                reason: "payout_target_not_set".to_string(),
                released_mu: 0,
            });
            continue;
        }
        let held_mu = held_after_lock(earning, epoch, lock_epochs)?;
        let released_mu = earning
            .total_mu
            .checked_sub(held_mu)
            .and_then(|mu| mu.checked_sub(earning.paid_cum_mu))
            .context("earning conservation underflow")?;
        if released_mu < snapshot.params.payout_min_mu {
            skipped.push(SkippedPayout {
                who: earning.provider.clone(),
                reason: "released_below_payout_min_mu".to_string(),
                released_mu,
            });
            continue;
        }
        let metadata = PayoutMetadata {
            mayhem_provider: earning.provider.clone(),
            mayhem_epoch: epoch,
            mayhem_mu: released_mu,
            mayhem_denom: "mu_usd",
            mayhem_fiat_currency: None,
        };
        let payout = match target.method.as_str() {
            "tnk" => {
                let tnk_e18 = mu_to_tnk_e18_ceil(released_mu, rate_tnk_usd_e6)?;
                ProviderPayout {
                    provider: earning.provider.clone(),
                    method: "tnk".to_string(),
                    target: target.addr.clone(),
                    mu: released_mu,
                    tnk_e18: Some(tnk_e18.clone()),
                    msb_transfer: Some(MsbTransfer {
                        to: target.addr.clone(),
                        tnk_e18: tnk_e18.clone(),
                        network_pays_fee: true,
                    }),
                    stripe_transfer: None,
                    confirm_command: payout_confirm_command(PayoutConfirmArgs {
                        kind: None,
                        rail: "tnk",
                        epoch,
                        who: &earning.provider,
                        mu: released_mu,
                        tnk_e18: Some(&tnk_e18),
                        external_ref: None,
                        msb_tx_hash: Some(&rail_options.msb_tx_hash),
                        fiat_currency: None,
                        fiat_amount_minor: None,
                        at,
                    }),
                }
            }
            "stripe" => {
                let currency = normalize_fiat_currency(
                    target
                        .currency
                        .as_deref()
                        .context("stripe payout target missing currency")?,
                )?;
                let amount_cents = mu_to_usd_cents_ceil(released_mu)?;
                let rounding_mu = amount_cents
                    .checked_mul(10_000)
                    .and_then(|mu| mu.checked_sub(released_mu))
                    .context("Stripe rounding overflow")?;
                let metadata = PayoutMetadata {
                    mayhem_fiat_currency: Some(currency.clone()),
                    ..metadata
                };
                ProviderPayout {
                    provider: earning.provider.clone(),
                    method: "stripe".to_string(),
                    target: target.addr.clone(),
                    mu: released_mu,
                    tnk_e18: None,
                    msb_transfer: None,
                    stripe_transfer: Some(StripeTransfer {
                        destination: target.addr.clone(),
                        amount_cents,
                        currency: currency.clone(),
                        rounding_mu,
                        metadata,
                        network_pays_fee: true,
                    }),
                    confirm_command: payout_confirm_command(PayoutConfirmArgs {
                        kind: None,
                        rail: "stripe",
                        epoch,
                        who: &earning.provider,
                        mu: released_mu,
                        tnk_e18: None,
                        external_ref: Some(&rail_options.stripe_transfer_id),
                        msb_tx_hash: None,
                        fiat_currency: Some(&currency),
                        fiat_amount_minor: Some(amount_cents),
                        at,
                    }),
                }
            }
            _ => unreachable!("unsupported payout methods are skipped above"),
        };
        provider_payouts.push(payout);
    }
    provider_payouts.sort_by(|a, b| a.provider.cmp(&b.provider));
    skipped.sort_by(|a, b| a.who.cmp(&b.who));

    let fee_sweep = match &snapshot.fee {
        Some(fee) => {
            if fee.denom.as_deref().unwrap_or("mu_usd") != "mu_usd" {
                bail!("fee record has unsupported denomination");
            }
            let available = fee
                .cum_mu
                .checked_sub(fee.swept_cum_mu)
                .context("fee conservation underflow")?;
            if available == 0 {
                None
            } else {
                let tnk_e18 = mu_to_tnk_e18_ceil(available, rate_tnk_usd_e6)?;
                Some(FeeSweep {
                    target: treasury.to_string(),
                    mu: available,
                    tnk_e18: tnk_e18.clone(),
                    msb_transfer: MsbTransfer {
                        to: treasury.to_string(),
                        tnk_e18: tnk_e18.clone(),
                        network_pays_fee: true,
                    },
                    confirm_command: payout_confirm_command(PayoutConfirmArgs {
                        kind: Some("fee_sweep"),
                        rail: "tnk",
                        epoch,
                        who: treasury,
                        mu: available,
                        tnk_e18: Some(&tnk_e18),
                        external_ref: None,
                        msb_tx_hash: Some(&rail_options.msb_tx_hash),
                        fiat_currency: None,
                        fiat_amount_minor: None,
                        at,
                    }),
                })
            }
        }
        None => None,
    };

    let provider_mu = provider_payouts.iter().map(|payout| payout.mu).sum();
    let fee_sweep_mu = fee_sweep.as_ref().map(|sweep| sweep.mu).unwrap_or(0);
    let transfer_count = provider_payouts.len() + usize::from(fee_sweep.is_some());
    Ok(PayoutPlan {
        epoch,
        at,
        rate_tnk_usd_e6,
        provider_payouts,
        fee_sweep,
        skipped,
        totals: PlanTotals {
            provider_mu,
            fee_sweep_mu,
            transfer_count,
        },
    })
}

fn snapshot_admin_pubkey(admin: Option<&Value>) -> Option<String> {
    let admin = admin?;
    value_string(admin).or_else(|| {
        let object = admin.as_object()?;
        ["pubkey", "public_key", "peer_pubkey", "admin"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(value_string))
    })
}

fn value_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_holdbacks(earning: &EarningRecord) -> Result<Vec<HoldbackBucket>> {
    let buckets = match &earning.holdbacks {
        Some(holdbacks) => holdbacks.clone(),
        None if earning.held_mu == 0 => Vec::new(),
        None => vec![HoldbackBucket {
            epoch: earning.updated_epoch.unwrap_or(0),
            mu: earning.held_mu,
        }],
    };

    let mut by_epoch = HashMap::<u64, u64>::new();
    for bucket in buckets {
        if bucket.mu == 0 {
            bail!("holdback bucket mu must be positive");
        }
        let entry = by_epoch.entry(bucket.epoch).or_insert(0);
        *entry = entry
            .checked_add(bucket.mu)
            .context("holdback bucket overflow")?;
    }
    let mut out: Vec<_> = by_epoch
        .into_iter()
        .map(|(epoch, mu)| HoldbackBucket { epoch, mu })
        .collect();
    out.sort_by_key(|bucket| bucket.epoch);
    Ok(out)
}

fn held_after_lock(earning: &EarningRecord, epoch: u64, lock_epochs: u64) -> Result<u64> {
    let holdbacks = normalize_holdbacks(earning)?;
    let held = holdbacks
        .into_iter()
        .filter(|bucket| bucket.epoch.saturating_add(lock_epochs) > epoch)
        .try_fold(0_u64, |acc, bucket| {
            acc.checked_add(bucket.mu).context("held holdback overflow")
        })?;
    if held > earning.total_mu {
        bail!("earning held_mu exceeds total_mu");
    }
    Ok(held)
}

fn mu_to_tnk_e18_ceil(mu: u64, rate_tnk_usd_e6: u64) -> Result<String> {
    if rate_tnk_usd_e6 == 0 {
        bail!("rate_tnk_usd_e6 must be positive");
    }
    let numerator = u128::from(mu)
        .checked_mul(TNK_E18)
        .context("TNK conversion overflow")?;
    let denom = u128::from(rate_tnk_usd_e6);
    let tnk_e18 = numerator.div_ceil(denom);
    Ok(tnk_e18.to_string())
}

fn mu_to_usd_cents_ceil(mu: u64) -> Result<u64> {
    if mu == 0 {
        bail!("mu must be positive");
    }
    Ok(mu.div_ceil(10_000))
}

fn normalize_fiat_currency(value: &str) -> Result<String> {
    let currency = value.trim().to_ascii_lowercase();
    match currency.as_str() {
        "usd" | "eur" => Ok(currency),
        _ => bail!("fiat currency must be usd or eur"),
    }
}

struct PayoutConfirmArgs<'a> {
    kind: Option<&'a str>,
    rail: &'a str,
    epoch: u64,
    who: &'a str,
    mu: u64,
    tnk_e18: Option<&'a str>,
    external_ref: Option<&'a str>,
    msb_tx_hash: Option<&'a str>,
    fiat_currency: Option<&'a str>,
    fiat_amount_minor: Option<u64>,
    at: u64,
}

fn payout_confirm_command(args: PayoutConfirmArgs<'_>) -> Value {
    let mut command = json!({
        "op": "payout_confirm",
        "epoch": args.epoch,
        "who": args.who,
        "mu": args.mu,
        "at": args.at,
    });
    if args.rail != "tnk" {
        command["rail"] = Value::String(args.rail.to_string());
    }
    if let Some(tnk_e18) = args.tnk_e18 {
        command["tnk_e18"] = Value::String(tnk_e18.to_string());
    }
    if let Some(external_ref) = args.external_ref {
        command["external_ref"] = Value::String(external_ref.to_string());
    }
    if let Some(msb_tx_hash) = args.msb_tx_hash {
        command["msb_tx_hash"] = Value::String(msb_tx_hash.to_string());
    }
    if let Some(fiat_currency) = args.fiat_currency {
        command["fiat_currency"] = Value::String(fiat_currency.to_string());
    }
    if let Some(fiat_amount_minor) = args.fiat_amount_minor {
        command["fiat_amount_minor"] = Value::from(fiat_amount_minor);
    }
    if let Some(kind) = args.kind {
        command["kind"] = Value::String(kind.to_string());
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADMIN: &str = "admin-pubkey";

    fn provider(provider: &str, target: &str) -> ProviderRecord {
        provider_with_method(provider, target, "tnk")
    }

    fn provider_with_method(provider: &str, target: &str, method: &str) -> ProviderRecord {
        ProviderRecord {
            provider: provider.to_string(),
            status: Some("active".to_string()),
            payout: Some(PayoutTarget {
                addr: target.to_string(),
                method: method.to_string(),
                currency: (method != "tnk").then(|| "usd".to_string()),
                set_by: Some(ADMIN.to_string()),
                set_by_role: Some("admin".to_string()),
            }),
        }
    }

    fn rail_options() -> RailOptions {
        RailOptions {
            msb_tx_hash: DEFAULT_MSB_TX_PLACEHOLDER.to_string(),
            stripe_transfer_id: "tr_test_pending".to_string(),
        }
    }

    fn earning(provider: &str) -> EarningRecord {
        EarningRecord {
            provider: provider.to_string(),
            denom: "mu_usd".to_string(),
            total_mu: 1_700_000,
            held_mu: 1_700_000,
            paid_cum_mu: 0,
            updated_epoch: Some(1),
            holdbacks: Some(vec![HoldbackBucket {
                epoch: 1,
                mu: 1_700_000,
            }]),
        }
    }

    #[test]
    fn payout_plan_respects_holdback_and_challenge_lock() {
        let snapshot = Snapshot {
            admin: Some(Value::String(ADMIN.to_string())),
            epoch: None,
            rate_tnk_usd_e6: None,
            params: Params::default(),
            providers: vec![provider("provider-a", "trac1provider")],
            earnings: vec![earning("provider-a")],
            fee: None,
        };

        let early = build_plan(
            &snapshot,
            100,
            1_900,
            2_000_000,
            ADMIN,
            "treasury",
            &rail_options(),
        )
        .unwrap();
        assert!(early.provider_payouts.is_empty());
        assert_eq!(early.skipped[0].reason, "released_below_payout_min_mu");

        let ready = build_plan(
            &snapshot,
            169,
            1_900,
            2_000_000,
            ADMIN,
            "treasury",
            &RailOptions {
                msb_tx_hash: "a".repeat(64),
                ..rail_options()
            },
        )
        .unwrap();
        assert_eq!(ready.provider_payouts.len(), 1);
        assert_eq!(ready.provider_payouts[0].mu, 1_700_000);
        assert_eq!(ready.provider_payouts[0].method, "tnk");
        assert_eq!(
            ready.provider_payouts[0].tnk_e18.as_deref(),
            Some("850000000000000000")
        );
        assert_eq!(
            ready.provider_payouts[0]
                .msb_transfer
                .as_ref()
                .expect("msb transfer")
                .tnk_e18,
            "850000000000000000"
        );
        assert_eq!(
            ready.provider_payouts[0].confirm_command["epoch"],
            Value::from(169)
        );
        assert_eq!(ready.totals.provider_mu, 1_700_000);
        assert_eq!(ready.totals.transfer_count, 1);
    }

    #[test]
    fn payout_plan_includes_router_fee_sweep() {
        let snapshot = Snapshot {
            admin: Some(Value::String(ADMIN.to_string())),
            epoch: None,
            rate_tnk_usd_e6: None,
            params: Params::default(),
            providers: vec![],
            earnings: vec![],
            fee: Some(FeeRecord {
                denom: Some("mu_usd".to_string()),
                cum_mu: 2_000_000,
                swept_cum_mu: 500_000,
            }),
        };

        let plan = build_plan(
            &snapshot,
            7,
            25_200,
            2_000_000,
            ADMIN,
            "treasury",
            &RailOptions {
                msb_tx_hash: "b".repeat(64),
                ..rail_options()
            },
        )
        .unwrap();
        let sweep = plan.fee_sweep.unwrap();
        assert_eq!(sweep.mu, 1_500_000);
        assert_eq!(sweep.tnk_e18, "750000000000000000");
        assert_eq!(sweep.confirm_command["kind"], Value::from("fee_sweep"));
        assert_eq!(plan.totals.fee_sweep_mu, 1_500_000);
        assert_eq!(plan.totals.transfer_count, 1);
    }

    #[test]
    fn payout_plan_includes_stripe_and_skips_retired_coinbase_targets() {
        let mut stripe_provider =
            provider_with_method("provider-stripe", "acct_provider", "stripe");
        stripe_provider
            .payout
            .as_mut()
            .expect("stripe payout")
            .currency = Some("eur".to_string());
        let snapshot = Snapshot {
            admin: Some(Value::String(ADMIN.to_string())),
            epoch: None,
            rate_tnk_usd_e6: None,
            params: Params::default(),
            providers: vec![
                stripe_provider,
                provider_with_method("provider-coinbase", "paymentMethod_provider", "coinbase"),
            ],
            earnings: vec![earning("provider-stripe"), earning("provider-coinbase")],
            fee: None,
        };

        let plan = build_plan(
            &snapshot,
            169,
            1_900,
            2_000_000,
            ADMIN,
            "treasury",
            &rail_options(),
        )
        .unwrap();
        assert_eq!(plan.provider_payouts.len(), 1);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].who, "provider-coinbase");
        assert_eq!(plan.skipped[0].reason, "payout_method_retired");
        assert_eq!(plan.totals.provider_mu, 1_700_000);
        assert_eq!(plan.totals.transfer_count, 1);

        let stripe = plan
            .provider_payouts
            .iter()
            .find(|payout| payout.method == "stripe")
            .expect("stripe payout");
        assert_eq!(stripe.mu, 1_700_000);
        assert_eq!(stripe.confirm_command["rail"], "stripe");
        assert_eq!(stripe.confirm_command["external_ref"], "tr_test_pending");
        assert_eq!(stripe.confirm_command["fiat_currency"], "eur");
        assert_eq!(stripe.confirm_command["fiat_amount_minor"], 170);
        let stripe_transfer = stripe.stripe_transfer.as_ref().expect("stripe transfer");
        assert_eq!(stripe_transfer.amount_cents, 170);
        assert_eq!(stripe_transfer.destination, "acct_provider");
        assert_eq!(stripe_transfer.currency, "eur");
        assert_eq!(stripe_transfer.metadata.mayhem_denom, "mu_usd");
        assert_eq!(
            stripe_transfer.metadata.mayhem_fiat_currency.as_deref(),
            Some("eur")
        );
    }

    #[test]
    fn payout_plan_skips_non_admin_set_payout_targets() {
        let mut provider_set = provider("provider-set", "trac1provider");
        provider_set.payout.as_mut().expect("payout").set_by = Some("provider-set".to_string());
        let mut missing_set_by = provider("missing-set-by", "trac1missing");
        missing_set_by.payout.as_mut().expect("payout").set_by = None;
        let mut wrong_role = provider("wrong-role", "trac1wrongrole");
        wrong_role.payout.as_mut().expect("payout").set_by_role = Some("provider".to_string());
        let snapshot = Snapshot {
            admin: Some(Value::String(ADMIN.to_string())),
            epoch: None,
            rate_tnk_usd_e6: None,
            params: Params::default(),
            providers: vec![provider_set, missing_set_by, wrong_role],
            earnings: vec![
                earning("provider-set"),
                earning("missing-set-by"),
                earning("wrong-role"),
            ],
            fee: None,
        };

        let plan = build_plan(
            &snapshot,
            169,
            1_900,
            2_000_000,
            ADMIN,
            "treasury",
            &rail_options(),
        )
        .unwrap();

        assert!(plan.provider_payouts.is_empty());
        assert_eq!(plan.skipped.len(), 3);
        assert!(plan
            .skipped
            .iter()
            .all(|skip| skip.reason == "payout_target_not_admin_set"));
        assert_eq!(plan.totals.provider_mu, 0);
        assert_eq!(plan.totals.transfer_count, 0);
    }

    #[test]
    fn payout_plan_requires_explicit_active_provider_status() {
        let mut missing_status = provider("missing-status", "trac1missingstatus");
        missing_status.status = None;
        let mut banned = provider("banned-provider", "trac1banned");
        banned.status = Some("banned".to_string());
        let snapshot = Snapshot {
            admin: Some(Value::String(ADMIN.to_string())),
            epoch: None,
            rate_tnk_usd_e6: None,
            params: Params::default(),
            providers: vec![missing_status, banned],
            earnings: vec![earning("missing-status"), earning("banned-provider")],
            fee: None,
        };

        let plan = build_plan(
            &snapshot,
            169,
            1_900,
            2_000_000,
            ADMIN,
            "treasury",
            &rail_options(),
        )
        .unwrap();

        assert!(plan.provider_payouts.is_empty());
        assert_eq!(plan.skipped.len(), 2);
        assert!(plan
            .skipped
            .iter()
            .all(|skip| skip.reason == "provider_not_active"));
        assert_eq!(plan.totals.provider_mu, 0);
        assert_eq!(plan.totals.transfer_count, 0);
    }

    #[test]
    fn snapshot_admin_pubkey_accepts_string_or_admin_object() {
        assert_eq!(
            snapshot_admin_pubkey(Some(&json!(ADMIN))).as_deref(),
            Some(ADMIN)
        );
        assert_eq!(
            snapshot_admin_pubkey(Some(&json!({ "peer_pubkey": ADMIN }))).as_deref(),
            Some(ADMIN)
        );
        assert_eq!(
            snapshot_admin_pubkey(Some(&json!({ "pubkey": "  admin-trimmed  " }))).as_deref(),
            Some("admin-trimmed")
        );
        assert_eq!(snapshot_admin_pubkey(Some(&json!({ "pubkey": "" }))), None);
    }

    #[test]
    fn payout_plan_requires_admin_pubkey() {
        let snapshot = Snapshot {
            admin: None,
            epoch: None,
            rate_tnk_usd_e6: None,
            params: Params::default(),
            providers: vec![provider("provider-a", "trac1provider")],
            earnings: vec![earning("provider-a")],
            fee: None,
        };

        let err = build_plan(
            &snapshot,
            169,
            1_900,
            2_000_000,
            "",
            "treasury",
            &rail_options(),
        )
        .expect_err("empty admin must fail");

        assert_eq!(err.to_string(), "admin_pubkey must be non-empty");
    }
}
