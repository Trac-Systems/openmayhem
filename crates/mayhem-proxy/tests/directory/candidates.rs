use super::*;
use mayhem_proxy::{
    directory::candidates::MAX_INDEX_READS,
    routing::{Policy, Target},
};

fn policy(rows: &[Entry]) -> Policy {
    let market: ProxyMarketDescriptor = serde_json::from_value(
        rows.iter()
            .find(|r| r.key.contains("/markets/"))
            .unwrap()
            .value
            .clone(),
    )
    .unwrap();
    let offer: ProxyOffer = serde_json::from_value(
        rows.iter()
            .find(|r| r.key.contains("/offers/"))
            .unwrap()
            .value["offer"]
            .clone(),
    )
    .unwrap();
    serde_json::from_value(json!({"schema_version":1,"lane":"proxy","endpoint":offer.endpoint,
        "target":{"kind":"category","family_ids":[market.model.family_id],"variants":[],"tags":[],"market_allowlist":null},
        "providers":{"allow":null,"deny":[],"require_verified_operator":false},"allowed_rails":["fiat","tap","tnk"],
        "prices":{"rates":offer.rates,"per_request_au":offer.per_request_au.to_string(),"min_session_au":offer.min_session_au.to_string(),"max_total_spend_au":u128::MAX.to_string()},
        "max_retail_cost_micro":"1000000","settlement_policies":[{"rail":"fiat","settlement_policy_hash":"c".repeat(64)},{"rail":"tap","settlement_policy_hash":"c".repeat(64)},{"rail":"tnk","settlement_policy_hash":"c".repeat(64)}],
        "constraints":{"minimum_context":null,"minimum_tokens_per_second":null,"output_units":if offer.endpoint==mayhem_proto::proxy::ProxyEndpoint::Decisions {Value::Null}else{json!(10)},"capabilities":[],"request_controls":[],"data_handling":[]},
        "ranking":"lowest_estimated_cost","continuity":"retain_compatible"})).unwrap()
}
fn mutate_offer(row: &mut Entry, f: impl FnOnce(&mut ProxyOffer)) {
    let mut o: ProxyOffer = serde_json::from_value(row.value["offer"].clone()).unwrap();
    f(&mut o);
    row.value["offer"] = json!(o);
    row.value["digest"] = json!(o.digest().unwrap());
    row.value["revision"] = json!(o.revision);
    row.key = format!(
        "{CATALOG_PREFIX}offers/{}/{}/{}",
        o.market_id,
        o.provider_pubkey,
        o.slot_id().unwrap()
    );
}
fn collect(
    c: &Catalog,
    p: &Policy,
    selected_rail: ProxyRail,
    limit: usize,
) -> (Vec<String>, usize) {
    let read = c.read().unwrap();
    let mut cursor = None;
    let mut found = Vec::new();
    let mut pages = 0;
    loop {
        let page = read
            .proxy_candidates(p, selected_rail, cursor.as_deref(), limit, 10001)
            .unwrap();
        assert_eq!(page.ordering, "index_traversal_not_cost");
        assert!(page.scanned_candidates <= MAX_CANDIDATES);
        assert!(page.index_reads <= MAX_INDEX_READS);
        assert!(serde_json::to_vec(&page.entries).unwrap().len() <= MAX_PAGE_ENTRY_BYTES + 2);
        assert_eq!(page.exhausted, page.next_cursor.is_none());
        found.extend(page.entries.iter().map(|v| v.id.clone()));
        cursor = page.next_cursor;
        pages += 1;
        assert!(pages < 10000);
        if cursor.is_none() {
            break;
        }
    }
    (found, pages)
}
#[test]
fn candidates_reach_all_categories_and_keep_snapshot_cursors_and_direct_ids() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let a = rows("A", "qwen", 420, false);
    let b = rows("B", "other", 390, false);
    let mut p = policy(&a);
    p.target = Target::Category {
        family_ids: vec!["other".into(), "qwen".into()],
        variants: vec![],
        tags: vec![],
        market_allowlist: None,
    };
    let mut all = a.clone();
    all.extend(b.clone());
    all.extend(rows("C", "llama", 10, false));
    all.extend(rows("D", "qwen", 3, true));
    hydrate(&c, all.clone());
    let (mut found, pages) = collect(&c, &p, ProxyRail::Fiat, 19);
    found.sort();
    let mut expected = ids(&a);
    expected.extend(ids(&b));
    expected.sort();
    assert_eq!(found, expected);
    assert!(pages > 40);
    let old = c.read().unwrap();
    let first = old
        .proxy_candidates(&p, ProxyRail::Fiat, None, 1, 10001)
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    assert!(old
        .proxy_offer(expected.last().unwrap(), 10001)
        .unwrap()
        .is_some());
    let changed = all.iter().find(|r| r.key.contains("/offers/")).unwrap();
    let mut change = changed.clone();
    mutate_offer(&mut change, |o| {
        o.revision += 1;
        o.per_request_au += 1;
    });
    apply(&c, vec![change]);
    assert!(old
        .proxy_candidates(&p, ProxyRail::Fiat, Some(&cursor), 10, 10001)
        .is_ok());
    assert!(matches!(
        c.read()
            .unwrap()
            .proxy_candidates(&p, ProxyRail::Fiat, Some(&cursor), 10, 10001),
        Err(Error::DirectoryCursorExpired)
    ));
    let mut other = p.clone();
    other.prices.max_total_spend_au -= 1;
    assert!(matches!(
        old.proxy_candidates(&other, ProxyRail::Fiat, Some(&cursor), 10, 10001),
        Err(Error::DirectoryCursorInvalid)
    ));
    assert!(matches!(
        old.proxy_candidates(&p, ProxyRail::Tap, Some(&cursor), 10, 10001),
        Err(Error::DirectoryCursorInvalid)
    ));
}
#[test]
fn selective_provider_context_price_and_rail_skip_deep_nonmatches() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let mut all = rows("A", "qwen", 1200, false);
    let mut p = policy(&all);
    let wanted = format!("{:064x}", 1199);
    // One high-context member and one cheap offer at the end of provider ordering.
    for row in &mut all {
        if row.key.contains("/memberships/") && row.value["member"]["provider_pubkey"] == wanted {
            row.value["member"]["served_context"] = json!(1_000_000);
        }
        if row.key.contains("/offers/") {
            mutate_offer(row, |o| {
                o.rates[0].per_unit_au = if o.provider_pubkey == wanted { 1 } else { 100 };
                o.rates[0].granularity = 1;
                if o.provider_pubkey != wanted {
                    o.accepted_rails = vec![ProxyRail::Fiat];
                }
            });
        }
    }
    hydrate(&c, all.clone());
    p.prices.rates[0].per_unit_au = 100;
    p.prices.rates[0].granularity = 1;
    for variant in 0..4 {
        let mut q = p.clone();
        let selected = if variant == 3 {
            ProxyRail::Tap
        } else {
            ProxyRail::Fiat
        };
        match variant {
            0 => q.providers.allow = Some(vec![wanted.clone()]),
            1 => q.constraints.minimum_context = Some(1_000_000),
            2 => q.prices.rates[0].per_unit_au = 1,
            _ => {}
        }
        let page = c
            .read()
            .unwrap()
            .proxy_candidates(&q, selected, None, 100, 10001)
            .unwrap();
        assert_eq!(page.entries.len(), 1, "variant {variant}");
        assert_eq!(page.entries[0].offer.provider_pubkey, wanted);
        assert!(page.scanned_candidates <= 2);
        assert!(page.exhausted);
        assert!(page.index_reads < 200);
    }
    // Allow/deny intersects rather than widening the selected category.
    p.providers.deny = vec![wanted];
    p.prices.rates[0].per_unit_au = 1;
    assert!(collect(&c, &p, ProxyRail::Fiat, 10).0.is_empty());
}
#[test]
fn full_price_map_and_fixed_charges_use_exact_rational_limits_without_rounding() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let mut all = rows("A", "qwen", 6, false);
    let mut p = policy(&all);
    p.prices.rates[0].per_unit_au = u128::MAX;
    p.prices.rates[0].granularity = 9_007_199_254_740_991;
    for row in &mut all {
        if row.key.contains("/offers/") {
            mutate_offer(row, |o| {
                let n = u64::from_str_radix(&o.provider_pubkey, 16).unwrap();
                o.rates[0] = p.prices.rates[0].clone();
                match n {
                    0 => {}
                    1 => o.rates[0].granularity -= 1,
                    2 => o.rates[0].per_unit_au -= 1,
                    3 => o.rates.pop().map(|_| ()).unwrap(),
                    4 => o.per_request_au += 1,
                    5 => o.min_session_au += 1,
                    _ => unreachable!(),
                }
            });
        }
    }
    hydrate(&c, all);
    let (found, _) = collect(&c, &p, ProxyRail::Fiat, 1);
    let providers: Vec<_> = found
        .iter()
        .map(|id| id.split('/').nth(1).unwrap().to_owned())
        .collect();
    assert_eq!(
        providers,
        vec![format!("{:064x}", 0), format!("{:064x}", 2)]
    );
    // Equal fractions with different granularities remain equal, and every
    // secondary unit has its own ceiling (never merely the first rate).
    let mut all = rows("B", "qwen", 3, false);
    let mut p = policy(&all);
    p.prices.rates[0].per_unit_au = 2;
    p.prices.rates[0].granularity = 6;
    for row in &mut all {
        if row.key.contains("/offers/") {
            mutate_offer(row, |o| {
                o.rates[0].per_unit_au = 1;
                o.rates[0].granularity = 3;
                if o.provider_pubkey == format!("{:064x}", 2) {
                    o.rates[1].per_unit_au += 1;
                }
            });
        }
    }
    let dir2 = tempfile::tempdir().unwrap();
    let c2 = Catalog::open(dir2.path().join("catalog"), identity()).unwrap();
    hydrate(&c2, all);
    assert_eq!(collect(&c2, &p, ProxyRail::Fiat, 10).0.len(), 2);
}
#[test]
fn index_updates_remove_old_prices_memberships_and_withdrawals_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let c = Catalog::open(&path, identity()).unwrap();
    let mut all = rows("A", "qwen", 5, false);
    let mut p = policy(&all);
    p.constraints.minimum_context = Some(1_000_000);
    let member = all
        .iter_mut()
        .find(|r| r.key.contains("/memberships/"))
        .unwrap();
    member.value["member"]["served_context"] = json!(1_000_000);
    let mut member = member.clone();
    hydrate(&c, all.clone());
    assert_eq!(collect(&c, &p, ProxyRail::Fiat, 10).0.len(), 1);
    member.value["member"]["served_context"] = json!(500);
    apply(&c, vec![member]);
    assert!(collect(&c, &p, ProxyRail::Fiat, 10).0.is_empty());
    p.constraints.minimum_context = None;
    let mut offer = all
        .iter()
        .find(|r| r.key.contains("/offers/"))
        .unwrap()
        .clone();
    mutate_offer(&mut offer, |o| {
        o.revision += 1;
        o.min_session_au += 1;
    });
    apply(&c, vec![offer.clone()]);
    assert_eq!(collect(&c, &p, ProxyRail::Fiat, 10).0.len(), 4);
    offer.value["active"] = json!(false);
    apply(&c, vec![offer.clone()]);
    assert_eq!(collect(&c, &p, ProxyRail::Fiat, 10).0.len(), 4);
    offer.value = Value::Null;
    apply(&c, vec![offer]);
    drop(c);
    let c = Catalog::open(&path, identity()).unwrap();
    assert_eq!(collect(&c, &p, ProxyRail::Fiat, 10).0.len(), 4);
}
#[test]
fn unobserved_predicates_and_controls_are_explicit_pending_checks_never_matches() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let all = rows("A", "qwen", 1, false);
    let mut p = policy(&all);
    hydrate(&c, all);
    p.providers.require_verified_operator = true;
    p.constraints.request_controls=serde_json::from_value(json!([{"field_id":"fixture.effort","schema_revision":1,"value":{"type":"enum","value":"High"}}])).unwrap();
    let page = c
        .read()
        .unwrap()
        .proxy_candidates(&p, ProxyRail::Fiat, None, 10, 10001)
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert!(page.requires_observation_resolution);
    assert!(page.requires_control_preparation);
    assert_eq!(page.entries[0].operator_verification, "unknown");
}

#[test]
fn sparse_intersections_continue_empty_pages_and_retain_an_exact_market_scope() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let mut all = rows("A", "qwen", 1025, false);
    let mut p = policy(&all);
    let market = ids(&all)[0].split('/').next().unwrap().to_owned();
    p.target = Target::ExactMarket { market_id: market };
    for r in &mut p.prices.rates {
        r.per_unit_au = 1;
        r.granularity = 1;
    }
    for row in &mut all {
        if row.key.contains("/offers/") {
            mutate_offer(row, |o| {
                let n = u64::from_str_radix(&o.provider_pubkey, 16).unwrap();
                o.rates[0].per_unit_au = if n < 512 || n == 1024 { 1 } else { 2 };
                o.rates[0].granularity = 1;
                o.rates[1].per_unit_au = if n >= 512 { 1 } else { 2 };
                o.rates[1].granularity = 1;
            });
        }
    }
    let expected = ids(&all).pop().unwrap();
    all.extend(rows("Foreign", "qwen", 1, false));
    hydrate(&c, all);
    let first = c
        .read()
        .unwrap()
        .proxy_candidates(&p, ProxyRail::Fiat, None, 100, 10001)
        .unwrap();
    assert!(first.entries.is_empty());
    assert!(first.next_cursor.is_some());
    assert!(!first.exhausted);
    let (found, pages) = collect(&c, &p, ProxyRail::Fiat, 100);
    assert_eq!(found, vec![expected]);
    assert!(pages >= 5);
}

#[test]
fn candidate_index_v2_upgrade_rehydrates_without_scanning_stale_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let c = Catalog::open(&path, identity()).unwrap();
    let all = rows("A", "qwen", 4, false);
    let p = policy(&all);
    hydrate(&c, all.clone());
    let cursor = c
        .read()
        .unwrap()
        .proxy_candidates(&p, ProxyRail::Fiat, None, 1, 10001)
        .unwrap()
        .next_cursor
        .unwrap();
    drop(c);
    {
        let db = redb::Database::create(&path).unwrap();
        let tx = db.begin_write().unwrap();
        let mut meta = tx
            .open_table(TableDefinition::<&str, &[u8]>::new(
                "proxy_catalog_metadata_v1",
            ))
            .unwrap();
        let mut state: Value =
            serde_json::from_slice(meta.get("state").unwrap().unwrap().value()).unwrap();
        state["index_version"] = json!(2);
        meta.insert("state", serde_json::to_vec(&state).unwrap().as_slice())
            .unwrap();
        drop(meta);
        tx.commit().unwrap();
    }
    let c = Catalog::open(&path, identity()).unwrap();
    assert!(c.read().unwrap().get(&all[0].key).unwrap().is_some());
    assert!(c
        .read()
        .unwrap()
        .proxy_candidates(&p, ProxyRail::Fiat, None, 1, 10001)
        .is_err());
    hydrate(&c, all);
    assert!(matches!(
        c.read()
            .unwrap()
            .proxy_candidates(&p, ProxyRail::Fiat, Some(&cursor), 1, 10001),
        Err(Error::DirectoryCursorExpired)
    ));
    assert_eq!(collect(&c, &p, ProxyRail::Fiat, 100).0.len(), 4);
}

/// Explicit local scale acceptance: no network, server or ledger history scan.
#[test]
#[ignore = "100,000-offer local indexed acceptance; run explicitly"]
fn hundred_thousand_offers_support_deep_selective_queries_and_complete_bounded_traversal() {
    let started = std::time::Instant::now();
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let mut all = rows("Scale", "qwen", 100_000, false);
    let mut p = policy(&all);
    let wanted = format!("{:064x}", 99_999);
    for row in &mut all {
        if row.key.contains("/memberships/") && row.value["member"]["provider_pubkey"] == wanted {
            row.value["member"]["served_context"] = json!(1_000_000);
        }
        if row.key.contains("/offers/") {
            mutate_offer(row, |o| {
                o.rates[0].per_unit_au = if o.provider_pubkey == wanted { 1 } else { 100 };
                o.rates[0].granularity = 1;
                if o.provider_pubkey != wanted {
                    o.accepted_rails = vec![ProxyRail::Fiat];
                }
            });
        }
    }
    hydrate(&c, all);
    let hydrated = started.elapsed();
    p.prices.rates[0].per_unit_au = 100;
    p.prices.rates[0].granularity = 1;
    let mut stats = Vec::new();
    for variant in 0..4 {
        let mut q = p.clone();
        let selected = if variant == 3 {
            ProxyRail::Tap
        } else {
            ProxyRail::Fiat
        };
        match variant {
            0 => q.providers.allow = Some(vec![wanted.clone()]),
            1 => q.constraints.minimum_context = Some(1_000_000),
            2 => q.prices.rates[0].per_unit_au = 1,
            _ => {}
        }
        let page = c
            .read()
            .unwrap()
            .proxy_candidates(&q, selected, None, 100, 10001)
            .unwrap();
        assert!(page.exhausted);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].offer.provider_pubkey, wanted);
        assert!(page.scanned_candidates <= 2);
        assert!(page.index_reads < 200);
        stats.push(json!({"driver":page.index_driver,"index_reads":page.index_reads,"scanned_candidates":page.scanned_candidates}));
    }
    let (found, pages) = collect(&c, &p, ProxyRail::Fiat, 100);
    assert_eq!(found.len(), 100_000);
    let unique: std::collections::BTreeSet<_> = found.iter().collect();
    assert_eq!(unique.len(), 100_000);
    assert!(found.last().unwrap().contains(&wanted));
    println!(
        "{}",
        json!({"offers":100000,"hydrate_ms":hydrated.as_millis(),"total_ms":started.elapsed().as_millis(),"bounded_pages":pages,"selective":stats})
    );
}

#[test]
fn selective_family_driver_preserves_nested_cursor_and_rejects_forged_parent() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let wanted = rows("ZZZ", "qwen", 7, false);
    let p = policy(&wanted);
    let mut all = rows("AAA", "other", 300, false);
    all.extend(wanted.clone());
    hydrate(&c, all);
    let read = c.read().unwrap();
    let page = read
        .proxy_candidates(&p, ProxyRail::Fiat, None, 2, 10001)
        .unwrap();
    assert_eq!(page.index_driver, "target");
    assert_eq!(page.entries.len(), 2);
    let cursor = page.next_cursor.unwrap();
    let parsed: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&cursor).unwrap()).unwrap();
    assert!(parsed["child"].is_object());
    let (mut found, pages) = collect(&c, &p, ProxyRail::Fiat, 2);
    found.sort();
    assert_eq!(found, ids(&wanted));
    assert!(pages >= 4);
    let mut forged = parsed.clone();
    forged["child"]["parent"] = json!(format!("{CATALOG_PREFIX}markets/{}", "f".repeat(64)));
    let token = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
    assert!(matches!(
        read.proxy_candidates(&p, ProxyRail::Fiat, Some(&token), 2, 10001),
        Err(Error::DirectoryCursorInvalid)
    ));
    let mut forged = parsed;
    forged["unexpected"] = json!(true);
    let token = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
    assert!(matches!(
        read.proxy_candidates(&p, ProxyRail::Fiat, Some(&token), 2, 10001),
        Err(Error::DirectoryCursorInvalid)
    ));
}

#[test]
fn taxonomy_scopes_use_indexed_exact_models_and_bind_all_continuations() {
    use mayhem_proxy::registry::publication::taxonomy::{DocumentReference, Reference, Scope};
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("taxonomy"), identity()).unwrap();
    let mut all = Vec::new();
    for n in 0..96 {
        all.extend(rows(&format!("model{n}"), "other", 2, false));
    }
    let mut p = policy(&all);
    hydrate(&c, all);
    p.target = Target::TaxonomyCategory {
        taxonomy: Reference {
            release_id: "00000000-0000-4000-8000-000000000001".into(),
            release_hash: "a".repeat(64),
            entry_id: "new_category".into(),
            schema_revision: 1,
        },
        variants: vec![],
        tags: vec![],
        market_allowlist: None,
    };
    let source = DocumentReference {
        entry_id: "model95".into(),
        schema_revision: 1,
        version: 1,
        document_hash: "b".repeat(64),
    };
    let scope = Scope::Model {
        source,
        family_id: "other".into(),
        model_id: "model95".into(),
        revision: "".into(),
        quantization: "".into(),
    };
    let read = c.read().unwrap();
    assert!(read
        .proxy_candidates(&p, ProxyRail::Fiat, None, 1, 10001)
        .is_err());
    let first = read
        .proxy_candidates_in_scope(&p, ProxyRail::Fiat, Some(&scope), None, 1, 10001)
        .unwrap();
    assert_eq!(first.entries.len(), 1);
    assert!(first.index_reads < 100);
    assert_eq!(first.entries[0].market.model.model_id, "model95");
    assert!(p
        .check_offer(&first.entries[0], p.endpoint, ProxyRail::Fiat)
        .is_err());
    let cursor = first.next_cursor.unwrap();
    let second = read
        .proxy_candidates_in_scope(&p, ProxyRail::Fiat, Some(&scope), Some(&cursor), 10, 10001)
        .unwrap();
    assert!(second.exhausted);
    assert_eq!(second.entries.len(), 1);
    let mut other = scope.clone();
    if let Scope::Model { model_id, .. } = &mut other {
        *model_id = "model94".into();
    }
    assert!(matches!(
        read.proxy_candidates_in_scope(&p, ProxyRail::Fiat, Some(&other), Some(&cursor), 10, 10001),
        Err(Error::DirectoryCursorInvalid)
    ));
    let mut changed = p.clone();
    changed.prices.rates[0].per_unit_au = 0;
    assert!(read
        .proxy_candidates_in_scope(&changed, ProxyRail::Fiat, Some(&scope), None, 10, 10001)
        .unwrap()
        .entries
        .is_empty());
}

#[test]
fn taxonomy_scope_planner_keeps_selective_provider_and_price_drivers() {
    use mayhem_proxy::registry::publication::taxonomy::{DocumentReference, Reference, Scope};
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("scope-selectivity"), identity()).unwrap();
    let all = rows("large", "other", 1200, false);
    let mut p = policy(&all);
    hydrate(&c, all);
    p.target = Target::TaxonomyCategory {
        taxonomy: Reference {
            release_id: "00000000-0000-4000-8000-000000000001".into(),
            release_hash: "a".repeat(64),
            entry_id: "category".into(),
            schema_revision: 1,
        },
        variants: vec![],
        tags: vec![],
        market_allowlist: None,
    };
    let scope = Scope::Family {
        source: DocumentReference {
            entry_id: "family".into(),
            schema_revision: 1,
            version: 1,
            document_hash: "b".repeat(64),
        },
        family_id: "other".into(),
    };
    let read = c.read().unwrap();
    let broad = read
        .proxy_candidates_in_scope(&p, ProxyRail::Fiat, Some(&scope), None, 1, 10001)
        .unwrap();
    assert_eq!(broad.index_driver, "target");
    p.providers.allow = Some(vec![format!("{:064x}", 1199)]);
    let narrow = read
        .proxy_candidates_in_scope(&p, ProxyRail::Fiat, Some(&scope), None, 10, 10001)
        .unwrap();
    assert_eq!(narrow.entries.len(), 1);
    assert_eq!(narrow.index_driver, "provider");
    assert!(narrow.index_reads < 100);
    assert!(narrow.exhausted);
    p.providers.allow = None;
    p.prices.rates[0].per_unit_au = 0;
    let price = read
        .proxy_candidates_in_scope(&p, ProxyRail::Fiat, Some(&scope), None, 10, 10001)
        .unwrap();
    assert_eq!(price.index_driver, "unit_price");
    assert!(price.exhausted && price.entries.is_empty() && price.index_reads < 100);
}
