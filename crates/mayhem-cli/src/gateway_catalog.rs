//! Current gateway catalog reads. Historical price versions are never an
//! admission dependency. Startup supplies the existing chart history; later
//! reads retain its bounded window and fetch only newly applied epochs.
use super::*;

const HISTORY_PER_MARKET: usize = 720;
const PRICE_READ_CONCURRENCY: usize = 8;

#[derive(Clone, Default)]
pub(super) struct PriceHistory {
    markets: BTreeMap<String, Vec<Value>>,
    through_epoch: u64,
}

impl PriceHistory {
    pub(super) fn from_catalog(contract: &ContractCatalog) -> Self {
        let mut history = Self::default();
        history.insert(contract.price_derivations.iter().cloned());
        history
    }

    fn insert(&mut self, rows: impl IntoIterator<Item = Value>) {
        for row in rows {
            let Some(enclave) = row.get("enclave_id").and_then(Value::as_str) else {
                continue;
            };
            let key =
                ledger_price_market_key(enclave, row.get("ctx_bracket").and_then(Value::as_str));
            self.through_epoch = self
                .through_epoch
                .max(row.get("epoch").and_then(Value::as_u64).unwrap_or(0));
            let rows = self.markets.entry(key).or_default();
            let sort_key = price_derivation_sort_key(&row);
            if let Err(index) = rows.binary_search_by_key(&sort_key, price_derivation_sort_key) {
                if index == 0 && rows.len() == HISTORY_PER_MARKET {
                    continue;
                }
                rows.insert(index, row);
                if rows.len() > HISTORY_PER_MARKET {
                    rows.remove(0);
                }
            }
        }
    }

    fn rows(&self) -> Vec<Value> {
        self.markets.values().flatten().cloned().collect()
    }
}

async fn records<T: DeserializeOwned>(
    rpc: &PeerRpcClient,
    prefix: &str,
    length: u64,
    include: impl Fn(&str) -> bool,
) -> Result<Vec<T>> {
    read_prefix_entries_at(rpc, prefix, Some(length))
        .await?
        .1
        .into_iter()
        .filter(|row| row.key.strip_prefix(prefix).is_some_and(&include))
        .map(|row| {
            serde_json::from_value(row.value)
                .with_context(|| format!("parsing current catalog record {}", row.key))
        })
        .collect()
}

fn price_keys(
    enclaves: &[LedgerEnclave],
    table: &CtxBracketSchedule,
) -> BTreeMap<String, (String, String, Option<String>)> {
    let brackets: BTreeSet<_> = table
        .current
        .brackets
        .iter()
        .chain(
            table
                .pending
                .iter()
                .flat_map(|pending| pending.brackets.iter()),
        )
        .map(|bracket| bracket.id.clone())
        .collect();
    let mut keys = BTreeMap::new();
    for enclave in enclaves.iter().filter(|enclave| enclave.status == "active") {
        let markets: Vec<_> = if enclave.model_class == DEFAULT_MODEL_CLASS {
            brackets.iter().cloned().map(Some).collect()
        } else {
            vec![None]
        };
        for bracket in markets {
            keys.insert(
                format!(
                    "price/{}",
                    ledger_price_market_key(&enclave.enclave_id, bracket.as_deref())
                ),
                (
                    enclave.enclave_id.clone(),
                    enclave.model_id.clone(),
                    bracket,
                ),
            );
        }
    }
    keys
}

/// Read one confirmed snapshot. Work scales with live catalog records and
/// at most one bounded history window after a long observation gap, never
/// with every historical price version in the ledger.
pub(super) async fn read_current_catalog(
    rpc: &PeerRpcClient,
    history: &mut PriceHistory,
) -> Result<ContractCatalog> {
    let length = confirmed_snapshot_signed_length(rpc, "rules/current").await?;
    let (enclaves, table, epoch, rules) = tokio::try_join!(
        records::<LedgerEnclave>(rpc, "enclave/", length, |tail| !tail.contains('/')),
        read_state_value_at(rpc, "ctx_brackets", length),
        read_state_value_at(rpc, "epoch/apply/state", length),
        read_state_value_at(rpc, "rules/current", length),
    )?;
    let ctx_bracket_schedule: CtxBracketSchedule = table
        .map(serde_json::from_value)
        .transpose()
        .context("parsing current context bracket schedule")?
        .unwrap_or_else(default_ctx_bracket_schedule);
    validate_ctx_bracket_schedule(&ctx_bracket_schedule).map_err(anyhow::Error::msg)?;
    let active_billing_epoch = active_billing_epoch_from_state(epoch.as_ref())?;
    let applied_epoch = active_billing_epoch.saturating_sub(1);
    ensure!(
        applied_epoch >= history.through_epoch,
        "confirmed catalog epoch moved behind retained price history"
    );
    let rules = rules
        .map(serde_json::from_value)
        .transpose()
        .context("parsing current catalog rules")?;

    let mut pending = tokio::task::JoinSet::new();
    let mut keys = price_keys(&enclaves, &ctx_bracket_schedule).into_iter();
    let mut prices_by_key = BTreeMap::new();
    loop {
        while pending.len() < PRICE_READ_CONCURRENCY {
            let Some((key, identity)) = keys.next() else {
                break;
            };
            let rpc = rpc.clone();
            pending.spawn(async move {
                let price = read_state_value_at(&rpc, &key, length)
                    .await?
                    .map(serde_json::from_value::<LedgerPriceSchedule>)
                    .transpose()
                    .with_context(|| format!("parsing current price {key}"))?;
                if let Some(price) = &price {
                    ensure!(
                        (&price.enclave_id, &price.model_id, &price.ctx_bracket)
                            == (&identity.0, &identity.1, &identity.2),
                        "current price identity does not match its canonical key"
                    );
                }
                Ok::<_, anyhow::Error>((key, price))
            });
        }
        let Some(result) = pending.join_next().await else {
            break;
        };
        let (key, price) = result.context("current catalog price task failed")??;
        if let Some(price) = price {
            prices_by_key.insert(key, price);
        }
    }
    let (
        rooms,
        roomserve,
        serves,
        providers,
        reputations,
        kyb,
        tier3_measurements,
        active_payout_revisions,
    ) = tokio::try_join!(
        records(rpc, "room/", length, |tail| !tail.contains('/')),
        records(rpc, "roomserve/", length, |_| true),
        records(rpc, "serve/", length, |_| true),
        records(rpc, "prov/", length, |tail| !tail.contains('/')),
        records(rpc, "rep/", length, |tail| !tail.contains('/')),
        records(rpc, "kyb/", length, |tail| !tail.contains('/')),
        records(rpc, "tier3/measurement/", length, |tail| !tail
            .contains('/')),
        read_active_provider_payout_revisions_at(rpc, active_billing_epoch, Some(length)),
    )?;
    // Refresh the current epoch too: a previous observation can precede its
    // price evidence. Commit the cache only after every snapshot read succeeds.
    let mut next = history.clone();
    let first = history
        .through_epoch
        .max(applied_epoch.saturating_sub(HISTORY_PER_MARKET as u64 - 1));
    for epoch in first..=applied_epoch {
        let prefix = format!("ev/price/{epoch}/");
        let entries = read_prefix_entries_at(rpc, &prefix, Some(length)).await?.1;
        next.insert(entries.into_iter().map(|entry| entry.value));
    }
    next.through_epoch = applied_epoch;
    // Retired enclave histories are not retained forever in the live reader.
    let active: BTreeSet<_> = enclaves
        .iter()
        .filter(|enclave| enclave.status == "active")
        .map(|enclave| enclave.enclave_id.as_str())
        .collect();
    next.markets
        .retain(|key, _| active.contains(key.split('/').next().unwrap_or_default()));
    let price_derivations = next.rows();
    *history = next;
    Ok(ContractCatalog {
        enclaves,
        rooms,
        roomserve,
        serves,
        providers,
        reputations,
        kyb,
        prices: prices_by_key.into_values().collect(),
        price_derivations,
        tier3_measurements,
        ctx_bracket_schedule,
        rules,
        active_billing_epoch,
        active_payout_revisions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_history_is_bounded_per_market_and_deduplicates_refreshes() {
        let mut history = PriceHistory::default();
        history.insert((1..=1_000).flat_map(|epoch| {
            ["a", "b"].map(move |enclave| {
                json!({
                    "epoch": epoch, "enclave_id": enclave, "price_ver": epoch,
                })
            })
        }));
        history.insert([json!({"epoch": 1_000, "enclave_id": "a", "price_ver": 1_000})]);
        assert_eq!(history.markets["a"].len(), HISTORY_PER_MARKET);
        assert_eq!(history.markets["b"].len(), HISTORY_PER_MARKET);
        assert_eq!(history.markets["a"][0]["epoch"], 281);
        assert_eq!(history.through_epoch, 1_000);
    }
}
