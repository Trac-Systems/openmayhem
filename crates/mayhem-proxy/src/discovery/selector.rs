//! Bounded operator selectors. These never hydrate or replace the full catalog.
//! The fixed trusted peer authenticates canonical service responses and cursors.
use super::*;

fn validate(query: &Query) -> Result<()> {
    let b = &query.binding;
    require(
        (1..=MAX_PAGE_ENTRIES).contains(&b.limit)
            && query.since.is_none()
            && query.cursor.as_deref().is_none_or(token),
        "invalid selector bound",
    )?;
    if let Some(key) = &b.lookup {
        require(
            b.filter.is_empty()
                && query.cursor.is_none()
                && match b.kind.as_str() {
                    "families" => identifier(key),
                    "markets" | "providers" | "endpoints" | "metering" => hex(key),
                    "network" => key == "current",
                    _ => false,
                },
            "invalid exact selector",
        )
    } else {
        require(
            match b.kind.as_str() {
                "families" => b.filter.is_empty(),
                // Broad market scans are deliberately not a guided setup primitive.
                "markets" => {
                    b.filter.len() == 2
                        && b.filter.get("family_id").is_some_and(|s| identifier(s))
                        && b.filter
                            .get("endpoint_family")
                            .is_some_and(|s| matches!(s.as_str(), "llm" | "decisions"))
                }
                _ => false,
            },
            "unsupported selector",
        )
    }
}
impl DiscoveryClient {
    /// One signed, bounded indexed page or exact lookup. Whole-catalog `page`
    /// validation is unchanged: selective results cannot be used for hydration.
    pub async fn select(&self, query: &Query) -> Result<Page> {
        validate(query)?;
        let page = self.fetch(query).await?;
        page.validate_binding(&self.identity, &query.binding)?;
        require(
            page.mode == Mode::Snapshot
                && page.base_proof.is_none()
                && page.entries.len() <= query.binding.limit
                && page
                    .next_cursor
                    .as_ref()
                    .is_none_or(|v| Some(v) != query.cursor.as_ref()),
            "invalid selector page",
        )?;
        let prefix = format!("{CATALOG_PREFIX}{}/", query.binding.kind);
        for entry in &page.entries {
            require(
                entry.key.starts_with(&prefix),
                "selector namespace mismatch",
            )?;
            if let Some(key) = &query.binding.lookup {
                require(
                    entry.key == format!("{prefix}{key}"),
                    "exact selector mismatch",
                )?;
            }
            if query.binding.kind == "markets" && query.binding.lookup.is_none() {
                require(
                    entry.value["model"]["family_id"].as_str()
                        == query.binding.filter.get("family_id").map(String::as_str)
                        && entry.value["family"].as_str()
                            == query
                                .binding
                                .filter
                                .get("endpoint_family")
                                .map(String::as_str),
                    "market filter mismatch",
                )?;
            }
        }
        if query.binding.lookup.is_some() {
            require(
                page.entries.len() <= 1 && !page.truncated,
                "exact selector returned a page",
            )?;
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_cannot_be_mistaken_for_full_hydration_or_broad_market_scan() {
        let mut q = Query {
            binding: QueryBinding {
                kind: "families".into(),
                filter: BTreeMap::new(),
                lookup: None,
                limit: 40,
            },
            cursor: None,
            since: None,
        };
        assert!(validate(&q).is_ok());
        assert!(q.validate().is_err());
        q.binding.kind = "markets".into();
        assert!(validate(&q).is_err());
        q.binding.filter = BTreeMap::from([
            ("family_id".into(), "fixture".into()),
            ("endpoint_family".into(), "llm".into()),
        ]);
        assert!(validate(&q).is_ok());
        q.binding.filter.insert("name_prefix".into(), "x".into());
        assert!(validate(&q).is_err());
        q = Query::catalog();
        assert!(validate(&q).is_err());
        assert!(q.validate().is_ok());
    }
}
