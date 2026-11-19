//! Bound explicit FIAT/TNK dispatch to the canonical frozen output, its
//! immutable preparation, and authoritative per-output receipts.
use super::*;

pub(super) fn parse_payout_output_id(value: &str) -> std::result::Result<String, String> {
    if is_hex_len(value, 64) && value.bytes().all(|b| !b.is_ascii_uppercase()) {
        Ok(value.to_owned())
    } else {
        Err("output ID must be 64 lowercase hex digits".into())
    }
}

// Prior versions persisted plans and preparations without a rail-wide
// outstanding index. Each signed page is bounded; confirmed cursor progress
// survives process loss. No processor credentials are used for inventory.
pub(super) async fn ensure_payout_history(
    rpc: &PeerRpcClient,
    signer: &TargetedPayoutPreparationSigner,
    rail: &str,
) -> Result<()> {
    let key = format!("payout/history/{rail}");
    for _ in 0..64 {
        let state = read_confirmed_state_value(rpc, &key).await?;
        if state.as_ref().is_some_and(|s| s["complete"] == true) {
            return Ok(());
        }
        let after = match &state {
            Some(s) => s["through_epoch"]
                .as_u64()
                .context("invalid payout history cursor")?,
            None => 0,
        };
        let head = read_confirmed_state_value(rpc, "epoch/apply/state")
            .await?
            .and_then(|s| s["updated_epoch"].as_u64())
            .context("missing applied payout epoch")?;
        let end = after.saturating_add(32).min(head);
        ensure!(end > after, "inconsistent payout history boundary");
        let value = json!({
            "op":"index_targeted_payout_history", "contract_version": CONTRACT_VERSION,
            "rail":rail, "epoch":end, "after_epoch":after, "admin":"", "admin_sig":""
        });
        let indexed =
            submit_targeted_payout_control(rpc, signer, &value, &key, state.as_ref(), |record| {
                record["through_epoch"].as_u64().is_some_and(|n| n >= end)
            })
            .await?;
        if indexed["complete"] == true {
            return Ok(());
        }
    }
    bail!("payout history inventory advanced 64 pages; resume the same command");
}

pub(super) async fn require_output_claim(
    rpc: &PeerRpcClient,
    rail: &str,
    plan: &Value,
    output: &Value,
) -> Result<()> {
    let id = output["economic_op_id"]
        .as_str()
        .context("missing payout output ID")?;
    let claim = read_confirmed_state_value(rpc, &format!("payout/claim/{rail}/{id}"))
        .await?
        .context("frozen output has no independently dispatchable claim")?;
    ensure!(
        claim["economic_op_id"] == id
            && claim["rail"] == rail
            && claim["epoch"] == plan["epoch"]
            && claim["plan_root"] == plan["plan_root"]
            && claim["output_index"] == output["output_index"]
            && claim.get("consumed_by") == Some(&Value::Null),
        "payout claim conflicts with the frozen output"
    );
    if let Some(reason) = claim["blocked_reason"].as_str() {
        bail!("historical payout held: {reason}");
    }
    if let Some(group_key) = claim["legacy_group"].as_str() {
        let group = read_confirmed_state_value(rpc, group_key)
            .await?
            .context("missing historical payout reservation")?;
        ensure!(
            group["owner"] == id
                && group["blocked"] == false
                && group.get("consumed_by") == Some(&Value::Null),
            "historical payout reservation requires reconciliation"
        );
    }
    Ok(())
}

// Check the same immutable locks Core requires for accounting before using
// an already prepared operation to contact a payment provider.
pub(super) async fn require_prepared_output_locks(
    rpc: &PeerRpcClient,
    plan: &Value,
    output: &Value,
    record: &Value,
) -> Result<()> {
    let rail = plan["rail"].as_str().context("missing payout rail")?;
    let id = output["economic_op_id"]
        .as_str()
        .context("missing output identity")?;
    let payload_hash = opaque_hash(
        "mayhem-targeted-payout-preparation-payload-v1",
        &json!({
            "economic_op_id":id, "rail":rail, "epoch":plan["epoch"], "epoch_apply_hash":plan["epoch_apply_hash"],
            "kind":record["kind"], "output_index":output["output_index"], "payload":record["payload"],
        }),
    );
    let liability = targeted_payout_output_liability(rail, plan, output)?;
    ensure!(
        record["economic_op_id"] == id
            && record["payload_hash"] == payload_hash
            && record["payload"]["plan_root"] == plan["plan_root"]
            && record["payload"]["output"] == *output
            && record["liability"] == liability.unwrap_or(Value::Null),
        "prepared payout conflicts with the immutable output"
    );
    if !record["liability"].is_null() {
        let liability = &record["liability"];
        let provider = liability["provider"]
            .as_str()
            .context("missing preparation provider")?;
        let revision = liability["payout_revision"]
            .as_str()
            .context("missing preparation revision")?;
        let before = liability["paid_cum_au_before"]
            .as_str()
            .context("missing revision watermark")?;
        let key = format!("payout/preparation-lock/{rail}/{provider}/{revision}/{before}");
        let lock = read_confirmed_state_value(rpc, &key)
            .await?
            .context("missing preparation liability lock")?;
        ensure!(
            lock["economic_op_id"] == id
                && lock["rail"] == rail
                && lock["provider"] == provider
                && lock["payout_revision"] == revision
                && lock["paid_cum_au_before"] == before,
            "preparation liability lock has a different owner or watermark"
        );
    }
    for effect in record["external_effect_ids"]
        .as_array()
        .context("missing preparation effects")?
    {
        let effect = effect.as_str().context("invalid prepared effect")?;
        let key = format!("payout/preparation-effect/{rail}/{effect}");
        let lock = read_confirmed_state_value(rpc, &key)
            .await?
            .context("missing prepared effect lock")?;
        ensure!(
            lock["economic_op_id"] == id
                && lock["effect_id"] == effect
                && lock["consumed"] == false,
            "prepared effect is consumed or belongs to a different output"
        );
    }
    Ok(())
}

pub(super) async fn require_prepared_fiat_attempt(
    rpc: &PeerRpcClient,
    record: &Value,
    plan: &Value,
    output: &Value,
) -> Result<()> {
    let preparation = &record["preparation"];
    let id = output["economic_op_id"]
        .as_str()
        .context("missing output identity")?;
    let attempt = preparation["attempt_id"]
        .as_str()
        .context("missing attempt identity")?;
    ensure!(
        record["economic_op_id"] == id
            && record["attempt_id"] == attempt
            && record["consumed"] == false
            && preparation["economic_op_id"] == id
            && preparation["plan_root"] == plan["plan_root"]
            && preparation["epoch"] == plan["epoch"]
            && preparation["epoch_apply_hash"] == plan["epoch_apply_hash"]
            && preparation["output_index"] == output["output_index"],
        "FIAT attempt conflicts with the original frozen output"
    );
    let key_hash = preparation["idempotency_key_hash"]
        .as_str()
        .context("missing attempt idempotency hash")?;
    let lock =
        read_confirmed_state_value(rpc, &format!("payout/attempt-idempotency/fiat/{key_hash}"))
            .await?
            .context("missing attempt idempotency lock")?;
    ensure!(
        lock["economic_op_id"] == id && lock["attempt_id"] == attempt,
        "FIAT attempt idempotency lock mismatch"
    );
    if let Some(quote) = preparation["request"]["fx_quote_id"].as_str() {
        let lock = read_confirmed_state_value(rpc, &format!("payout/attempt-quote/fiat/{quote}"))
            .await?
            .context("missing frozen quote lock")?;
        ensure!(
            lock["economic_op_id"] == id
                && lock["attempt_id"] == attempt
                && lock["quote_hash"] == preparation["request"]["fx_quote_hash"],
            "FIAT attempt quote lock mismatch"
        );
    }
    Ok(())
}

pub(super) fn selected_payout_index(plan: &Value, id: &str) -> Result<usize> {
    let outputs = plan["outputs"]
        .as_array()
        .context("frozen plan has no outputs")?;
    let mut matches = outputs
        .iter()
        .enumerate()
        .filter(|(_, out)| out["economic_op_id"] == id);
    let (index, output) = matches
        .next()
        .context("--output-id is absent from the frozen plan")?;
    ensure!(
        matches.next().is_none() && output["output_index"].as_u64() == Some(index as u64),
        "ambiguous or inconsistent frozen output identity"
    );
    Ok(index)
}

pub(super) async fn confirmed_payout_close(
    rpc: &PeerRpcClient,
    plan: &Value,
) -> Result<Option<Value>> {
    let rail = plan["rail"].as_str().context("missing rail")?;
    let epoch = plan["epoch"].as_u64().context("missing epoch")?;
    let row = read_confirmed_state_value(rpc, &format!("settle/targeted/{rail}/{epoch}")).await?;
    if let Some(row) = &row {
        ensure!(
            row["type"] == "targeted_payout_epoch_close"
                && row["rail"] == rail
                && row["epoch"] == epoch
                && row["plan_root"] == plan["plan_root"]
                && row["value"]["epoch_apply_hash"] == plan["epoch_apply_hash"]
                && row["outputs_root"] == plan["outputs_root"]
                && row["carry_root"] == plan["carry_root"],
            "confirmed epoch close conflicts with the frozen plan"
        );
    }
    Ok(row)
}

pub(super) async fn confirmed_payout_output(
    rpc: &PeerRpcClient,
    plan: &Value,
    output: &Value,
) -> Result<Option<Value>> {
    let rail = plan["rail"].as_str().context("missing rail")?;
    let epoch = plan["epoch"].as_u64().context("missing epoch")?;
    let id = output["economic_op_id"]
        .as_str()
        .context("missing output identity")?;
    let key = format!("settle/targeted/{rail}/{epoch}/output/{id}");
    let receipt = read_confirmed_state_value(rpc, &key).await?;
    if let Some(row) = &receipt {
        validate_payout_receipt(plan, output, row)?;
    }
    Ok(receipt)
}

pub(super) fn validate_payout_receipt(plan: &Value, output: &Value, row: &Value) -> Result<()> {
    let rail = plan["rail"].as_str().context("missing payout rail")?;
    let id = output["economic_op_id"]
        .as_str()
        .context("missing output identity")?;
    let value = &row["value"];
    ensure!(
        row["type"] == format!("targeted_{rail}_output_settlement")
            && row["rail"] == rail
            && row["epoch"] == plan["epoch"]
            && row["economic_op_id"] == id
            && value["op"] == format!("settle_targeted_{rail}_output")
            && value["rail"] == rail
            && value["epoch"] == plan["epoch"]
            && value["economic_op_id"] == id
            && value["preparation_id"] == id
            && value["plan_root"] == plan["plan_root"]
            && value["output_index"] == output["output_index"]
            && value["epoch_apply_hash"] == plan["epoch_apply_hash"],
        "confirmed payout receipt conflicts with the frozen output"
    );
    match rail {
        "tnk" => {
            let transfer = &value["msb_transfer"];
            let signed = transfer["confirmed_length"]
                .as_u64()
                .context("missing transfer confirmation height")?;
            ensure!(
                signed > 0
                    && transfer["observed_signed_length"]
                        .as_u64()
                        .is_some_and(|n| n >= signed)
                    && transfer["tx_hash"]
                        .as_str()
                        .is_some_and(|h| is_hex_len(h, 64))
                    && transfer["tx_hash"] == value["external_effect_id"]
                    && transfer["to"] == output["to"]
                    && transfer["amount_e18"] == output["tnk_e18"],
                "TNK receipt has conflicting transfer evidence"
            );
        }
        "fiat" => {
            let transfer = &value["stripe_transfer"];
            let kind = if output["role"] == "provider" {
                "stripe_transfer"
            } else {
                "platform_balance"
            };
            ensure!(
                transfer["kind"] == kind
                    && value["attempt_id"]
                        .as_str()
                        .is_some_and(|id| is_hex_len(id, 64))
                    && transfer["attempt_id"] == value["attempt_id"]
                    && transfer["ref"].as_str().is_some_and(|id| !id.is_empty())
                    && transfer["destination"] == output["to"]
                    && transfer["source_currency"] == output["source_currency"]
                    && transfer["source_amount_minor"] == output["source_amount_minor"],
                "FIAT receipt has conflicting transfer evidence"
            );
        }
        _ => bail!("unsupported payout receipt rail"),
    }
    Ok(())
}

pub(super) enum ExplicitPayout {
    Dispatch(Option<usize>),
    Report(Value),
}

// An explicit replay or close uses only the retained canonical plan and Core
// receipts. Only an actual close needs the Core admin signer.
pub(super) async fn explicit_payout(
    rpc: &PeerRpcClient,
    tx: &AdminTxArgs,
    home: &Path,
    rail: &str,
    epoch: u64,
    output_id: Option<&str>,
    close_only: bool,
    freeze_only: bool,
) -> Result<ExplicitPayout> {
    if output_id.is_none() && !close_only && !freeze_only {
        return Ok(ExplicitPayout::Dispatch(None));
    }
    let Some(record) =
        read_confirmed_state_value(rpc, &format!("payout/epoch-plan/{rail}/{epoch}")).await?
    else {
        ensure!(
            freeze_only,
            "explicit payout requires a confirmed frozen epoch plan"
        );
        return Ok(ExplicitPayout::Dispatch(None));
    };
    let plan = canonical_targeted_payout_plan(&record, rail, epoch)?;
    let outputs = plan["outputs"]
        .as_array()
        .context("missing frozen outputs")?;
    let already_settled = confirmed_payout_close(rpc, &plan).await?;
    let mut settlement_state = already_settled.clone();
    let mut receipts = Vec::new();
    let mut pending = Vec::new();
    let mut submitted = false;
    if let Some(id) = output_id {
        let index = selected_payout_index(&plan, id)?;
        if let Some(row) = confirmed_payout_output(rpc, &plan, &outputs[index]).await? {
            receipts.push(row);
        } else {
            ensure!(
                already_settled.is_none(),
                "closed epoch is missing its selected output receipt"
            );
            return Ok(ExplicitPayout::Dispatch(Some(index)));
        }
    } else if close_only && settlement_state.is_none() {
        for output in outputs {
            if let Some(receipt) = confirmed_payout_output(rpc, &plan, output).await? {
                receipts.push(receipt);
            } else {
                pending.push(json!({"economic_op_id":output["economic_op_id"],
                    "output_index":output["output_index"], "reason":"awaiting_output_receipt"}));
            }
        }
        if pending.is_empty() {
            let signer = targeted_payout_preparation_signer(tx, home, rpc).await?;
            let at = plan["at"]
                .as_u64()
                .context("missing frozen plan timestamp")?;
            settlement_state = Some(close_targeted_payout_epoch(rpc, &signer, &plan, at).await?);
            submitted = true;
        }
    }
    let feature_result =
        submitted.then(|| json!({"plan":record,"outputs":receipts,"close":settlement_state}));
    let no_work = already_settled.is_none()
        && if rail == "tnk" {
            outputs.is_empty()
        } else {
            plan["outcome"] == "no_work"
        };
    let carry_forward = already_settled.is_none()
        && if rail == "tnk" {
            outputs.is_empty()
        } else {
            plan["outcome"] == "carry"
        };
    Ok(ExplicitPayout::Report(json!({
        "ok":true, "submitted":submitted, "sim":false, "epoch":epoch, "settlement":plan,
        "skipped_providers":[],
        "freeze_only":freeze_only, "close_only":close_only,
        "no_work":no_work, "carry_forward":carry_forward, "selected_output_id":output_id,
        "already_settled":already_settled,"settlement_state":settlement_state,"feature":record,"feature_result":feature_result,
        "pending":settlement_state.is_none(), "pending_outputs":pending,"output_settlements":receipts,
    })))
}

pub(super) fn print_explicit_payout(report: &Value, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print_payout_progress(report);
    }
    Ok(())
}

pub(super) fn print_payout_progress(report: &Value) {
    if let Some(id) = report["selected_output_id"].as_str() {
        let confirmed = report["output_settlements"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty());
        println!("Selected output {id} confirmed: {confirmed}");
    }
    println!(
        "Epoch pending: {}",
        report["pending"].as_bool().unwrap_or(false)
    );
    for item in report["pending_outputs"].as_array().into_iter().flatten() {
        println!(
            "  {}  {}",
            item["economic_op_id"].as_str().unwrap_or(""),
            item["reason"].as_str().unwrap_or("")
        );
    }
}

#[derive(Default)]
pub(super) struct BookedPayout {
    pub(super) paid: MoneyAu,
    pub(super) dust: MoneyAu,
    pub(super) through: MoneyAu,
}

impl BookedPayout {
    pub(super) fn cursor(&self, paid: MoneyAu) -> Result<MoneyAu> {
        Ok(paid
            .checked_add(self.paid)
            .context("payout reservation overflow")?
            .max(self.through))
    }

    pub(super) fn available(
        &self,
        outstanding: MoneyAu,
        held: MoneyAu,
    ) -> Result<(MoneyAu, MoneyAu, MoneyAu)> {
        let reserved = self
            .paid
            .checked_add(self.dust)
            .context("reserved payout overflow")?;
        let free = outstanding.saturating_sub(reserved);
        let free_held = free.min(held);
        Ok((free, free_held, free - free_held))
    }
}

pub(super) async fn booked_payout(
    rpc: &PeerRpcClient,
    rail: &str,
    scope: &str,
    length: u64,
) -> Result<BookedPayout> {
    let key = format!("payout/booked/{rail}/{scope}");
    let Some(row) = read_state_value_at(rpc, &key, length).await? else {
        return Ok(BookedPayout::default());
    };
    Ok(BookedPayout {
        paid: value_money_au(&row["pending_au"]).context("invalid booked payout AU")?,
        dust: value_money_au(&row["pending_dust_au"]).context("invalid booked payout dust")?,
        through: value_money_au(&row["allocated_through_au"])
            .context("invalid payout allocation cursor")?,
    })
}
