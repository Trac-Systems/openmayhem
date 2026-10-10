//! Reuse the existing signed payment registration primitives only AFTER paid
//! proxy admission. Never join a native enclave or choose/rotate payout targets.
use anyhow::{ensure, Context, Result};
use clap::Args as ClapArgs;
use mayhem_proxy::setup::{AdmissionReport, FlowConfig, Store};
use mayhem_proto::proxy::ProxyRail;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Existing protected wizard configuration.
    #[arg(long)]
    pub(super) config: PathBuf,
    #[arg(long)]
    pub(super) expected_revision: u64,
    /// Explicitly submit missing consent/registration and enable the selected rails.
    #[arg(long)]
    pub(super) submit: bool,
    /// Exact current rules hash from the preceding review; required for submission.
    #[arg(long, requires = "submit")]
    pub(super) accept_rules_hash: Option<String>,
    #[command(flatten)]
    pub(super) wallet: crate::WalletLocatorArgs,
}

fn require_admitted(report: &AdmissionReport) -> Result<()> {
    let evidence = report.evidence.as_ref().context("canonical admission is unavailable")?;
    ensure!(report.status == "observed_admitted"
        && report.for_current_configuration && !report.expired
        && evidence.registry_enabled && evidence.provider.is_some()
        && !evidence.provider_revoked && !evidence.admission_revoked,
        "paid proxy admission must be confirmed before payment registration");
    Ok(())
}

fn retained_rails(existing: Option<&Value>, selected: &[String]) -> Result<Vec<String>> {
    let mut rails: BTreeSet<String> = selected.iter().cloned().collect();
    if let Some(existing) = existing {
        ensure!(existing["status"] == "active", "existing provider registration is not active");
        for rail in existing["accepted_rails"].as_array().context("invalid existing provider rails")? {
            rails.insert(rail.as_str().context("invalid existing provider rail")?.to_owned());
        }
    }
    // Use the existing canonical order/validation. Never remove a native rail
    // when the same provider adds a more narrowly configured proxy offer.
    crate::normalize_provider_accepted_rails_arg(&rails.into_iter().collect::<Vec<_>>().join(","))
}

pub async fn run(args: Args) -> Result<()> {
    println!("{}", serde_json::to_string(&execute(args).await?)?);
    Ok(())
}

pub(super) async fn execute(args: Args) -> Result<Value> {
    let config = FlowConfig::load(&args.config)?;
    let store = Store::open(&config.directory)?;
    let before = store.inspect()?;
    ensure!(before.revision == args.expected_revision, "provider setup revision changed");
    let keypair = crate::resolve_wallet_keypair_path(&args.wallet)?;
    let password = args.wallet.wallet_password.as_deref().unwrap_or_default();
    let wallet = crate::inspect_wallet(&keypair, password).await?;
    ensure!(wallet.public_key == before.provider_pubkey.as_str()
        && before.provider_pubkey == config.profile.provider_pubkey,
        "payment registration wallet differs from the admitted proxy identity");
    let peer = config.peer_rpc.as_deref().context("canonical peer RPC is not configured")?;
    let review = store.admission_check(args.expected_revision, peer, config.timeout_ms).await?;
    require_admitted(review.admission.as_ref().context("canonical admission is missing")?)?;
    let rpc = crate::PeerRpcClient::new(peer)?;
    let rules = crate::resolve_rules(None, None, &rpc, None).await?;
    let selected: Vec<String> = review.offers.iter().flat_map(|offer| offer.accepted_rails.iter())
        .map(|rail| match rail { ProxyRail::Fiat => "fiat", ProxyRail::Tap => "tap", ProxyRail::Tnk => "tnk" }.to_owned())
        .collect::<BTreeSet<_>>().into_iter().collect();
    let registration_key = format!("prov/{}", wallet.public_key);
    let existing = crate::read_confirmed_state_value(&rpc, &registration_key).await?;
    let rails = retained_rails(existing.as_ref(), &selected)?;
    if args.submit {
        ensure!(args.accept_rules_hash.as_deref() == Some(rules.hash.as_str()),
            "review and explicitly accept the exact current rules hash before submission");
        let consent = crate::read_consent_state(&rpc, &wallet.public_key).await?;
        if !crate::consent_matches(consent.as_ref(), &rules) {
            // Sign only the reviewed rules, never a second automatic rules read.
            crate::submit_consent(&rpc, &keypair, password, &wallet, rules.clone(), false).await?;
        }
        crate::ensure_provider_registered(&rpc, &keypair, password, &wallet, false).await?;
        if existing.as_ref().map(|v| &v["accepted_rails"]) != Some(&json!(rails)) {
            crate::submit_provider_lifecycle_intent(crate::ProviderLifecycleSubmitContext {
                rpc: &rpc, keypair_path: &keypair, password, wallet: &wallet, sim: false,
            }, crate::provider_rails_intent(&wallet.public_key, &rails)?).await?;
            crate::wait_for_state(&rpc, &registration_key, |value| value["accepted_rails"] == json!(rails)).await?;
        }
    }
    let epoch = crate::active_billing_epoch(&rpc).await?;
    let mut payouts = serde_json::Map::new();
    for rail in &selected {
        let revision = crate::active_provider_payout_revision(&rpc, &wallet.public_key, rail, epoch).await;
        payouts.insert(rail.clone(), match revision {
            Ok(revision) => json!({"status":"observed_ready", "revision":revision}),
            Err(_) => json!({"status":"needs_setup_or_refresh", "command":if rail == "fiat" {
                "provider stripe onboard / adopt / status"
            } else { "provider payout set / get" }}),
        });
    }
    Ok(json!({
        "action":"proxy.payment_registration", "provider":wallet.public_key,
        "draft_revision":review.revision, "submitted":args.submit,
        "registered":args.submit || existing.is_some(), "rules":rules,
        "selected_rails":selected, "retained_rails":rails, "payouts":payouts,
        "payout_targets_changed":false, "serving_readiness":"not_proven",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mayhem_proxy::{attempts::Digest, setup::{AdmissionEvidence, AdmissionState}};

    fn admitted() -> AdmissionReport {
        let d = |n: u8| format!("{n:064x}");
        let evidence: AdmissionEvidence = serde_json::from_value(json!({
            "context":{"network_id":"918", "msb_bootstrap":d(1), "subnet_bootstrap":d(2),
                "contract_version":30, "epoch":100},
            "proof":{"view_key":d(3), "tree_hash":d(4), "signed_length":10, "fork":0},
            "registry_enabled":true, "fee_policy_hash":d(5),
            "provider":{"sequence":1, "operation_digest":d(6), "entitlement_id":d(7)},
            "provider_revoked":false, "admission_revoked":false
        })).unwrap();
        AdmissionReport {
            state: AdmissionState::Observed, status:"observed_admitted",
            for_current_configuration:true, expired:false, observed_at_ms:1000,
            expires_at_ms:16000, initial_operation_digest:Digest::new(d(6)).unwrap(),
            next_sequence:Some(2), sequence_matches:Some(false), operation_already_applied:Some(true),
            evidence:Some(evidence), payment_status:"not_checked", authorizes_publication:false,
        }
    }

    #[test]
    fn payment_registration_requires_current_enabled_unrevoked_paid_admission() {
        require_admitted(&admitted()).unwrap();
        for fault in 0..8 {
            let mut report = admitted();
            match fault {
                0 => report.status = "observed_not_registered",
                1 => report.expired = true,
                2 => report.for_current_configuration = false,
                3 => report.evidence.as_mut().unwrap().registry_enabled = false,
                4 => report.evidence.as_mut().unwrap().provider = None,
                5 => report.evidence.as_mut().unwrap().provider_revoked = true,
                6 => report.evidence.as_mut().unwrap().admission_revoked = true,
                _ => report.evidence = None,
            }
            assert!(require_admitted(&report).is_err(), "fault {fault}");
        }
    }

    #[test]
    fn payment_registration_keeps_native_rails_and_rejects_inactive_identity() {
        let selected = vec!["tnk".to_owned()];
        assert_eq!(retained_rails(Some(&json!({"status":"active", "accepted_rails":["fiat","tap"]})), &selected).unwrap(), vec!["fiat","tap","tnk"]);
        for status in ["banned", "inactive", ""] {
            assert!(retained_rails(Some(&json!({"status":status, "accepted_rails":["fiat"]})), &selected).is_err());
        }
        assert!(retained_rails(None, &["unknown".to_owned()]).is_err());
    }
}
