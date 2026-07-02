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
    target: String,
    mu: u64,
    tnk_e18: String,
    msb_transfer: MsbTransfer,
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
                &args.msb_tx_hash,
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
    msb_tx_hash: &str,
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
        let target = match &provider.payout {
            Some(target) if target.method == "tnk" => target,
            Some(_) => {
                skipped.push(SkippedPayout {
                    who: earning.provider.clone(),
                    reason: "non_tnk_payout_target_deferred_to_phase7".to_string(),
                    released_mu: 0,
                });
                continue;
            }
            None => {
                skipped.push(SkippedPayout {
                    who: earning.provider.clone(),
                    reason: "payout_target_not_set".to_string(),
                    released_mu: 0,
                });
                continue;
            }
        };
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
        let tnk_e18 = mu_to_tnk_e18_ceil(released_mu, rate_tnk_usd_e6)?;
        provider_payouts.push(ProviderPayout {
            provider: earning.provider.clone(),
            target: target.addr.clone(),
            mu: released_mu,
            tnk_e18: tnk_e18.clone(),
            msb_transfer: MsbTransfer {
                to: target.addr.clone(),
                tnk_e18: tnk_e18.clone(),
                network_pays_fee: true,
            },
            confirm_command: payout_confirm_command(
                None,
                epoch,
                &earning.provider,
                released_mu,
                &tnk_e18,
                msb_tx_hash,
                at,
            ),
        });
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
                    confirm_command: payout_confirm_command(
                        Some("fee_sweep"),
                        epoch,
                        treasury,
                        available,
                        &tnk_e18,
                        msb_tx_hash,
                        at,
                    ),
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

fn payout_confirm_command(
    kind: Option<&str>,
    epoch: u64,
    who: &str,
    mu: u64,
    tnk_e18: &str,
    msb_tx_hash: &str,
    at: u64,
) -> Value {
    let mut command = json!({
        "op": "payout_confirm",
        "epoch": epoch,
        "who": who,
        "mu": mu,
        "tnk_e18": tnk_e18,
        "msb_tx_hash": msb_tx_hash,
        "at": at,
    });
    if let Some(kind) = kind {
        command["kind"] = Value::String(kind.to_string());
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(provider: &str, target: &str) -> ProviderRecord {
        ProviderRecord {
            provider: provider.to_string(),
            status: Some("active".to_string()),
            payout: Some(PayoutTarget {
                addr: target.to_string(),
                method: "tnk".to_string(),
            }),
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
            DEFAULT_MSB_TX_PLACEHOLDER,
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
            "a".repeat(64).as_str(),
        )
        .unwrap();
        assert_eq!(ready.provider_payouts.len(), 1);
        assert_eq!(ready.provider_payouts[0].mu, 1_700_000);
        assert_eq!(ready.provider_payouts[0].tnk_e18, "850000000000000000");
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
            "b".repeat(64).as_str(),
        )
        .unwrap();
        let sweep = plan.fee_sweep.unwrap();
        assert_eq!(sweep.mu, 1_500_000);
        assert_eq!(sweep.tnk_e18, "750000000000000000");
        assert_eq!(sweep.confirm_command["kind"], Value::from("fee_sweep"));
        assert_eq!(plan.totals.fee_sweep_mu, 1_500_000);
        assert_eq!(plan.totals.transfer_count, 1);
    }
}
