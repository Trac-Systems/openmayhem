#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "mayhem-pay", about = "Mayhem payment helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Retired: use the rail-specific epoch settlement runners.
    PayoutPlan(PayoutPlanArgs),
}

#[derive(Debug, Args)]
struct PayoutPlanArgs {
    /// Historical argument retained only so the command can explain the replacement.
    #[arg(long)]
    snapshot: Option<PathBuf>,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::PayoutPlan(_) => retire_payout_plan(),
    }
}

fn retire_payout_plan() -> Result<()> {
    bail!(
        "mayhem-pay payout-plan is retired; TNK settlement uses one treasury-signed MSB batch recorded by `mayhem admin tnk-settlement`, and fiat settlement is handled by the paygate rail runner"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payout_plan_is_retired_instead_of_emitting_placeholder_commands() {
        let err = retire_payout_plan().expect_err("retired command must fail");
        assert!(err.to_string().contains("one treasury-signed MSB batch"));
        assert!(!err.to_string().contains("placeholder"));
        assert!(!err.to_string().contains("payout_confirm"));
    }
}
