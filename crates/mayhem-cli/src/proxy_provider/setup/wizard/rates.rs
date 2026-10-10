//! Commercial-only prompts; revisions and operation sequences are canonical facts.
use super::*;
use mayhem_proxy::setup::guided::{au_to_usd, usd_to_au};

pub(super) fn review(view: &FlowView) -> Result<FlowAction> {
    let revision = view
        .review
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("publish your existing configuration first"))?
        .revision;
    let mut choices = view.rate_choices.clone();
    anyhow::ensure!(!choices.is_empty(), "no published offers to change");
    println!("Change prices for the existing submarkets and rails only. Review reads fresh canonical revisions and sequence; it does not publish or spend.");
    for (index, choice) in choices.iter_mut().enumerate() {
        let offer = &view.selection.offers[index];
        println!(
            "Offer {}: {} / {}",
            index + 1,
            offer.ctx_bracket,
            offer.outcome_class
        );
        choice.per_request_au = usd_to_au(&prompt(
            "USD per request",
            &au_to_usd(choice.per_request_au),
        )?)?;
        choice.min_session_au = usd_to_au(&prompt(
            "USD minimum session",
            &au_to_usd(choice.min_session_au),
        )?)?;
        for rate in &mut choice.rates {
            rate.per_unit_au = usd_to_au(&prompt(
                &format!("USD per {} {}", rate.granularity, rate.unit),
                &au_to_usd(rate.per_unit_au),
            )?)?;
        }
    }
    Ok(FlowAction::RatePlan {
        expected_revision: revision,
        choices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn rate_review_uses_existing_wizard_parser_and_strict_retained_publish_action() {
        assert!(crate::Cli::try_parse_from([
            "mayhem",
            "provider",
            "proxy",
            "setup",
            "wizard",
            "--config",
            "/private/wizard.json",
            "--action-file",
            "/private/rates.json"
        ])
        .is_ok());
        assert!(crate::Cli::try_parse_from([
            "mayhem",
            "provider",
            "proxy",
            "setup",
            "wizard",
            "--config",
            "/private/wizard.json",
            "--inspect",
            "--action-file",
            "/private/rates.json"
        ])
        .is_err());
        let body = serde_json::json!({"action":"publish_rates","expected_revision":12,"plan_digest":"ab".repeat(32)});
        assert!(matches!(
            serde_json::from_value::<FlowAction>(body.clone()).unwrap(),
            FlowAction::PublishRates {
                expected_revision: 12,
                ..
            }
        ));
        let mut altered = body;
        altered["offers_only"] = serde_json::json!(false);
        assert!(serde_json::from_value::<FlowAction>(altered).is_err());
    }
}
