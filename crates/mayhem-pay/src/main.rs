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
    /// Router treasury payout target used for fee sweep intents.
    #[arg(long, default_value = "treasury")]
    treasury: String,
    /// Placeholder used in generated payout_confirm commands until MSB transfer tx hashes exist.
    #[arg(long, default_value = DEFAULT_MSB_TX_PLACEHOLDER)]
    msb_tx_hash: String,
    /// Placeholder used in generated payout_confirm commands until Stripe transfer IDs exist.
    #[arg(long, default_value = "PENDING_STRIPE_TRANSFER_ID")]
    stripe_transfer_id: String,
    /// Placeholder used in generated payout_confirm commands until Coinbase transfer IDs exist.
    #[arg(long, default_value = "PENDING_COINBASE_TRANSFER_ID")]
    coinbase_transfer_id: String,
    /// Coinbase CDP custodial source account used for generated transfer intents.
    #[arg(long, default_value = "COINBASE_SOURCE_ACCOUNT")]
    coinbase_source_account: String,
    /// Coinbase transfer asset for generated intents.
    #[arg(long, default_value = "usd")]
    coinbase_asset: String,
    /// Coinbase onchain target network when the target is an address.
    #[arg(long, default_value = "base")]
    coinbase_network: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Snapshot {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    coinbase_transfer: Option<CoinbaseTransfer>,
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
    currency: &'static str,
    rounding_mu: u64,
    metadata: PayoutMetadata,
    network_pays_fee: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct CoinbaseTransfer {
    source: CoinbaseTransferSource,
    target: Value,
    amount: String,
    asset: String,
    rounding_mu: u64,
    execute: bool,
    metadata: PayoutMetadata,
    network_pays_fee: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct CoinbaseTransferSource {
    #[serde(rename = "accountId")]
    account_id: String,
    asset: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct PayoutMetadata {
    mayhem_provider: String,
    mayhem_epoch: u64,
    mayhem_mu: u64,
    mayhem_denom: &'static str,
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
    coinbase_transfer_id: String,
    coinbase_source_account: String,
    coinbase_asset: String,
    coinbase_network: String,
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
            let plan = build_plan(
                &snapshot,
                epoch,
                args.at,
                rate,
                &args.treasury,
                &RailOptions {
                    msb_tx_hash: args.msb_tx_hash,
                    stripe_transfer_id: args.stripe_transfer_id,
                    coinbase_transfer_id: args.coinbase_transfer_id,
                    coinbase_source_account: args.coinbase_source_account,
                    coinbase_asset: args.coinbase_asset,
                    coinbase_network: args.coinbase_network,
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
    treasury: &str,
    rail_options: &RailOptions,
) -> Result<PayoutPlan> {
    if rate_tnk_usd_e6 == 0 {
        bail!("rate_tnk_usd_e6 must be positive");
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
        if provider.status.as_deref().unwrap_or("active") != "active" {
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
        if !matches!(target.method.as_str(), "tnk" | "stripe" | "coinbase") {
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
                    coinbase_transfer: None,
                    confirm_command: payout_confirm_command(PayoutConfirmArgs {
                        kind: None,
                        rail: "tnk",
                        epoch,
                        who: &earning.provider,
                        mu: released_mu,
                        tnk_e18: Some(&tnk_e18),
                        external_ref: None,
                        msb_tx_hash: Some(&rail_options.msb_tx_hash),
                        at,
                    }),
                }
            }
            "stripe" => {
                let amount_cents = mu_to_usd_cents_ceil(released_mu)?;
                let rounding_mu = amount_cents
                    .checked_mul(10_000)
                    .and_then(|mu| mu.checked_sub(released_mu))
                    .context("Stripe rounding overflow")?;
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
                        currency: "usd",
                        rounding_mu,
                        metadata,
                        network_pays_fee: true,
                    }),
                    coinbase_transfer: None,
                    confirm_command: payout_confirm_command(PayoutConfirmArgs {
                        kind: None,
                        rail: "stripe",
                        epoch,
                        who: &earning.provider,
                        mu: released_mu,
                        tnk_e18: None,
                        external_ref: Some(&rail_options.stripe_transfer_id),
                        msb_tx_hash: None,
                        at,
                    }),
                }
            }
            "coinbase" => {
                let amount_cents = mu_to_usd_cents_ceil(released_mu)?;
                let rounding_mu = amount_cents
                    .checked_mul(10_000)
                    .and_then(|mu| mu.checked_sub(released_mu))
                    .context("Coinbase rounding overflow")?;
                ProviderPayout {
                    provider: earning.provider.clone(),
                    method: "coinbase".to_string(),
                    target: target.addr.clone(),
                    mu: released_mu,
                    tnk_e18: None,
                    msb_transfer: None,
                    stripe_transfer: None,
                    coinbase_transfer: Some(CoinbaseTransfer {
                        source: CoinbaseTransferSource {
                            account_id: rail_options.coinbase_source_account.clone(),
                            asset: rail_options.coinbase_asset.clone(),
                        },
                        target: coinbase_target_payload(
                            &target.addr,
                            &rail_options.coinbase_asset,
                            &rail_options.coinbase_network,
                        ),
                        amount: usd_cents_to_amount(amount_cents),
                        asset: rail_options.coinbase_asset.clone(),
                        rounding_mu,
                        execute: true,
                        metadata,
                        network_pays_fee: true,
                    }),
                    confirm_command: payout_confirm_command(PayoutConfirmArgs {
                        kind: None,
                        rail: "coinbase",
                        epoch,
                        who: &earning.provider,
                        mu: released_mu,
                        tnk_e18: None,
                        external_ref: Some(&rail_options.coinbase_transfer_id),
                        msb_tx_hash: None,
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

fn usd_cents_to_amount(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn coinbase_target_payload(target: &str, asset: &str, network: &str) -> Value {
    if target.starts_with("paymentMethod_") {
        json!({
            "paymentMethodId": target,
            "asset": asset,
        })
    } else if target.starts_with("account_") {
        json!({
            "accountId": target,
            "asset": asset,
        })
    } else if target.contains('@') {
        json!({
            "email": target,
            "asset": asset,
        })
    } else {
        json!({
            "address": target,
            "network": network,
            "asset": asset,
        })
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
    if let Some(kind) = args.kind {
        command["kind"] = Value::String(kind.to_string());
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

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
            }),
        }
    }

    fn rail_options() -> RailOptions {
        RailOptions {
            msb_tx_hash: DEFAULT_MSB_TX_PLACEHOLDER.to_string(),
            stripe_transfer_id: "tr_test_pending".to_string(),
            coinbase_transfer_id: "transfer_test_pending".to_string(),
            coinbase_source_account: "account_test_source".to_string(),
            coinbase_asset: "usd".to_string(),
            coinbase_network: "base".to_string(),
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
    fn payout_plan_includes_stripe_and_coinbase_provider_intents() {
        let snapshot = Snapshot {
            epoch: None,
            rate_tnk_usd_e6: None,
            params: Params::default(),
            providers: vec![
                provider_with_method("provider-stripe", "acct_provider", "stripe"),
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
            "treasury",
            &rail_options(),
        )
        .unwrap();
        assert_eq!(plan.provider_payouts.len(), 2);
        assert_eq!(plan.totals.provider_mu, 3_400_000);
        assert_eq!(plan.totals.transfer_count, 2);

        let coinbase = plan
            .provider_payouts
            .iter()
            .find(|payout| payout.method == "coinbase")
            .expect("coinbase payout");
        assert_eq!(coinbase.mu, 1_700_000);
        assert_eq!(coinbase.confirm_command["rail"], "coinbase");
        assert_eq!(
            coinbase.confirm_command["external_ref"],
            "transfer_test_pending"
        );
        let coinbase_transfer = coinbase
            .coinbase_transfer
            .as_ref()
            .expect("coinbase transfer");
        assert_eq!(coinbase_transfer.amount, "1.70");
        assert_eq!(coinbase_transfer.source.account_id, "account_test_source");
        assert_eq!(
            coinbase_transfer.target["paymentMethodId"],
            "paymentMethod_provider"
        );
        assert_eq!(coinbase_transfer.metadata.mayhem_denom, "mu_usd");

        let stripe = plan
            .provider_payouts
            .iter()
            .find(|payout| payout.method == "stripe")
            .expect("stripe payout");
        assert_eq!(stripe.mu, 1_700_000);
        assert_eq!(stripe.confirm_command["rail"], "stripe");
        assert_eq!(stripe.confirm_command["external_ref"], "tr_test_pending");
        let stripe_transfer = stripe.stripe_transfer.as_ref().expect("stripe transfer");
        assert_eq!(stripe_transfer.amount_cents, 170);
        assert_eq!(stripe_transfer.destination, "acct_provider");
        assert_eq!(stripe_transfer.metadata.mayhem_denom, "mu_usd");
    }
}
