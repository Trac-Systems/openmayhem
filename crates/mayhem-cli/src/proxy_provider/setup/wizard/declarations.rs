use super::*;
use anyhow::Context;
use mayhem_proxy::{
    attempts::Digest,
    registry::{Support, TypedValue, ValueSchema},
    setup::DeclarationChoice,
};
use std::collections::BTreeMap;

pub(super) async fn review(flow: &Flow, view: &FlowView) -> Result<FlowAction> {
    let revision = view
        .review
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("save your selection first"))?
        .revision;
    let mut page = flow
        .execute(
            FlowAction::DeclarationFields {
                release_id: None,
                cursor: None,
            },
            None,
        )
        .await?
        .action_result;
    let mut choices = BTreeMap::<String, DeclarationChoice>::new();
    if let Some(saved) = &view.declaration {
        for c in &saved.plan.body.claims {
            choices.insert(
                c.field_id.clone(),
                DeclarationChoice {
                    field_id: c.field_id.clone(),
                    schema_revision: c.schema_revision,
                    status: c.status,
                    value: c.value.clone(),
                },
            );
        }
    }
    loop {
        let docs: Vec<mayhem_proxy::registry::publication::Document> =
            serde_json::from_value(page["data"].clone())?;
        println!("Published fields (declared promises; not verified compliance):");
        for (i, d) in docs.iter().enumerate() {
            println!(
                "{}: {} ({})",
                i + 1,
                d.definition
                    .labels
                    .get("en")
                    .or_else(|| d.definition.labels.values().next())
                    .unwrap_or(&d.field_id),
                d.field_id
            );
        }
        println!(
            "Selected {} / 32 fields. n: next page; done: review all selected promises",
            choices.len()
        );
        let choice = prompt("Field number, n or done", "done")?;
        if choice == "done" {
            break;
        }
        if choice == "n" {
            let cursor = page["next_cursor"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("there are no further fields"))?
                .to_owned();
            page = flow
                .execute(
                    FlowAction::DeclarationFields {
                        release_id: Some(
                            page["release_id"]
                                .as_str()
                                .context("missing release")?
                                .into(),
                        ),
                        cursor: Some(cursor),
                    },
                    None,
                )
                .await?
                .action_result;
            continue;
        }
        let index = choice
            .parse::<usize>()?
            .checked_sub(1)
            .context("choose a displayed number")?;
        let doc = docs.get(index).context("choose a displayed number")?;
        let definition = &doc.definition;
        if let Some(help) = definition
            .help
            .get("en")
            .or_else(|| definition.help.values().next())
        {
            println!("{help}");
        }
        let status = prompt("Declare value, unknown, unsupported, or omit", "omit")?;
        if status == "omit" {
            choices.remove(&doc.field_id);
            continue;
        }
        anyhow::ensure!(
            choices.contains_key(&doc.field_id) || choices.len() < 32,
            "at most 32 selected fields"
        );
        let (status, value) = match status.as_str() {
            "unknown" => (Support::Unknown, None),
            "unsupported" => (Support::Unsupported, None),
            "value" => {
                let value = match &definition.value_schema {
                    ValueSchema::Boolean => match prompt("true or false", "")?.as_str() {
                        "true" => TypedValue::Boolean(true),
                        "false" => TypedValue::Boolean(false),
                        _ => anyhow::bail!("choose true or false"),
                    },
                    ValueSchema::Enum { values } => {
                        println!("Choices: {}", values.join(", "));
                        TypedValue::Enum(prompt("Value", "")?)
                    }
                    ValueSchema::Set { values } => {
                        println!("Choices: {}", values.join(", "));
                        let mut selected = prompt("Comma-separated values", "")?
                            .split(',')
                            .map(|s| s.trim().to_owned())
                            .collect::<Vec<_>>();
                        selected.sort();
                        selected.dedup();
                        TypedValue::Set(selected)
                    }
                    ValueSchema::Integer { minimum, maximum } => TypedValue::Integer(
                        prompt(&format!("Whole number ({minimum} to {maximum})"), "")?.parse()?,
                    ),
                    ValueSchema::Decimal { minimum, maximum } => TypedValue::Decimal(prompt(
                        &format!("Exact decimal ({minimum} to {maximum})"),
                        "",
                    )?),
                    ValueSchema::Text { .. } => TypedValue::Text(prompt("Text", "")?),
                };
                definition.value_schema.accepts(&value)?;
                (Support::Supported, Some(value))
            }
            _ => anyhow::bail!("choose value, unknown, unsupported or omit"),
        };
        choices.insert(
            doc.field_id.clone(),
            DeclarationChoice {
                field_id: doc.field_id.clone(),
                schema_revision: doc.schema_revision,
                status,
                value,
            },
        );
    }
    anyhow::ensure!(
        !choices.is_empty(),
        "select at least one declaration; omit is not a withdrawal of an already signed promise"
    );
    let hours = prompt("Valid for how many hours? (no automatic renewal)", "")?.parse::<u64>()?;
    anyhow::ensure!(hours > 0, "expiry must be in the future");
    let now = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let expiry = hours
        .checked_mul(3600000)
        .and_then(|v| now.checked_add(v))
        .context("expiry overflow")?;
    Ok(FlowAction::DeclarationPlan {
        expected_revision: revision,
        expected_declaration_revision: view
            .declaration
            .as_ref()
            .map(|d| d.latest_revision)
            .unwrap_or(0),
        release_id: page["release_id"]
            .as_str()
            .context("missing release")?
            .into(),
        release_hash: Digest::new(
            page["release_hash"]
                .as_str()
                .context("missing release hash")?,
        )?,
        choices: choices.into_values().collect(),
        expires_at_ms: expiry,
    })
}

pub(super) fn withdraw(view: &FlowView) -> Result<FlowAction> {
    let declaration = view
        .declaration
        .as_ref()
        .context("no signed declaration to withdraw")?;
    let hours = prompt(
        "Withdrawal record validity in hours (no automatic renewal)",
        "",
    )?
    .parse::<u64>()?;
    anyhow::ensure!(hours > 0, "expiry must be in the future");
    let now = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let expires_at_ms = hours
        .checked_mul(3600000)
        .and_then(|n| now.checked_add(n))
        .context("expiry overflow")?;
    Ok(FlowAction::WithdrawDeclarationPlan {
        expected_revision: view
            .review
            .as_ref()
            .context("save your selection first")?
            .revision,
        expected_declaration_revision: declaration.latest_revision,
        expires_at_ms,
    })
}
