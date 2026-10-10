use mayhem_proto::proxy::{
    ProxyFamily, ProxyMarketDescriptor, ProxyMembership, ProxyOffer, ProxyRail,
};
use mayhem_proxy::{
    catalog::Catalog,
    directory::{Query as Browse, MAX_CANDIDATES},
    discovery::*,
    Error,
};
use redb::{ReadableTable, TableDefinition};
use serde_json::{json, Value};

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}

fn page(c: &Catalog, rows: Vec<Entry>, more: Option<usize>) -> Page {
    let id = identity();
    let status = c.read().unwrap().status();
    let base = (!status.invalidated)
        .then(|| status.committed.as_ref().map(|c| c.proof.clone()))
        .flatten();
    let n = base.as_ref().map_or(10, |p| p.signed_length + 1);
    Page {
        ok: true,
        lane: "proxy".into(),
        schema_version: 1,
        request_nonce: "e".repeat(64),
        query: QueryBinding::catalog(),
        context: Context {
            network_id: id.network_id,
            msb_bootstrap: id.msb_bootstrap,
            subnet_bootstrap: id.subnet_bootstrap,
            contract_version: id.contract_version,
            epoch: n,
        },
        proof: Proof {
            view_key: "c".repeat(64),
            fork: 0,
            signed_length: n,
            tree_hash: format!("{n:064x}"),
        },
        mode: if base.is_some() {
            Mode::Changes
        } else {
            Mode::Snapshot
        },
        base_proof: base,
        entries: rows,
        truncated: more.is_some(),
        next_cursor: more.map(|n| format!("pdc1.page{n}.{}", "d".repeat(128))),
        checkpoint: more
            .is_none()
            .then(|| format!("pdc1.complete{n}.{}", "d".repeat(128))),
    }
}

fn apply(c: &Catalog, rows: Vec<Entry>) {
    c.apply(&c.refresh_ticket().unwrap(), &page(c, rows, None), 10_000)
        .unwrap();
}

fn hydrate(c: &Catalog, mut rows: Vec<Entry>) {
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    let count = rows.len().div_ceil(100);
    for (n, rows) in rows.chunks(100).enumerate() {
        c.apply(
            &c.refresh_ticket().unwrap(),
            &page(c, rows.to_vec(), (n + 1 < count).then_some(n)),
            10_000,
        )
        .unwrap();
    }
}

fn rows(name: &str, family: &str, count: usize, decision: bool) -> Vec<Entry> {
    variant_rows(name, family, count, decision, None)
}

fn variant_rows(
    name: &str,
    family: &str,
    count: usize,
    decision: bool,
    variant: Option<(&str, &str)>,
) -> Vec<Entry> {
    let v: Value = serde_json::from_str(include_str!(
        "../../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
    ))
    .unwrap();
    let fixture = &v["cases"][usize::from(decision)];
    let mut market: ProxyMarketDescriptor =
        serde_json::from_value(fixture["market"].clone()).unwrap();
    market.model.model_id = name.into();
    market.model.family_id = family.into();
    if let Some((revision, quantization)) = variant {
        market.model.revision = revision.into();
        market.model.quantization = quantization.into();
    }
    let id = market.id().unwrap();
    let mut result = vec![Entry {
        key: format!("{CATALOG_PREFIX}markets/{id}"),
        value: json!(market),
    }];
    for n in 0..count {
        let mut member: ProxyMembership =
            serde_json::from_value(fixture["membership"].clone()).unwrap();
        let mut offer: ProxyOffer = serde_json::from_value(fixture["offer"].clone()).unwrap();
        member.market_id = id.clone();
        member.provider_pubkey = format!("{n:064x}");
        offer.market_id = id.clone();
        offer.provider_pubkey = member.provider_pubkey.clone();
        offer.validate_for_membership(&market, &member).unwrap();
        result.push(Entry { key: format!("{CATALOG_PREFIX}memberships/{id}/{}", member.provider_pubkey), value: json!({"active":true,"revision":member.revision,"member":member,"offer_slots":1}) });
        result.push(Entry { key: format!("{CATALOG_PREFIX}offers/{id}/{}/{}", offer.provider_pubkey, offer.slot_id().unwrap()), value: json!({"active":true,"revision":offer.revision,"digest":offer.digest().unwrap(),"offer":offer}) });
    }
    result
}

fn ids(rows: &[Entry]) -> Vec<String> {
    let prefix = format!("{CATALOG_PREFIX}offers/");
    let mut result: Vec<_> = rows
        .iter()
        .filter_map(|r| r.key.strip_prefix(&prefix).map(str::to_owned))
        .collect();
    result.sort();
    result
}

#[test]
fn exact_model_scope_preserves_revision_quantization_case_and_reverse_navigation() {
    use mayhem_proxy::registry::publication::taxonomy::Model;
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let wanted = variant_rows("Vendor/Model", "qwen", 11, false, Some(("r1", "fp4")));
    let mut all = wanted.clone();
    for (name, family, revision, quant) in [
        ("Vendor/Model", "qwen", "r2", "fp4"),
        ("Vendor/Model", "qwen", "r1", "fp8"),
        ("Vendor/Model", "other", "r1", "fp4"),
        ("vendor/model", "qwen", "r1", "fp4"),
        ("Vendor/Model-longer", "qwen", "r1", "fp4"),
    ] {
        all.extend(variant_rows(
            name,
            family,
            35,
            false,
            Some((revision, quant)),
        ));
    }
    hydrate(&c, all);
    let query = Browse {
        name_prefix: "VENDOR/M".into(),
        model: Some(Model {
            family_id: "qwen".into(),
            model_id: "Vendor/Model".into(),
            revision: "r1".into(),
            quantization: "fp4".into(),
        }),
        ..Default::default()
    };
    let read = c.read().unwrap();
    let forward = read.proxy_offers(&query, None, 100, 10001).unwrap();
    assert_eq!(
        forward
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>(),
        ids(&wanted)
    );
    // Other variants cost a market index step, not 35 provider reads each.
    assert!(forward.scanned_candidates < 20);
    let tail = read.proxy_offers_from_end(&query, 4, 10001).unwrap();
    assert_eq!(
        tail.entries
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>(),
        ids(&wanted)[7..]
    );
    assert!(tail.next_cursor.is_none());
    let previous = read
        .proxy_offers(&query, tail.previous_cursor.as_deref(), 4, 10001)
        .unwrap();
    assert_eq!(
        previous
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>(),
        ids(&wanted)[3..7]
    );
    let returned = read
        .proxy_offers(&query, previous.next_cursor.as_deref(), 4, 10001)
        .unwrap();
    assert_eq!(
        returned
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>(),
        ids(&wanted)[7..]
    );
    let mut wrong = query.clone();
    wrong.model.as_mut().unwrap().revision = "r2".into();
    assert!(matches!(
        read.proxy_offers(&wrong, tail.previous_cursor.as_deref(), 4, 10001),
        Err(Error::DirectoryCursorInvalid)
    ));
    wrong.family_id = Some("other".into());
    assert!(wrong.key().is_err());
    wrong = query.clone();
    wrong.name_prefix = "unrelated".into();
    assert!(read
        .proxy_offers(&wrong, None, 100, 10001)
        .unwrap()
        .entries
        .is_empty());
    let default = Browse::default();
    let old_wire = json!({"kind":null,"family_id":null,"name_prefix":"","endpoint":null,"minimum_context":null,"rail":null});
    assert_eq!(serde_json::to_value(&default).unwrap(), old_wire);
}

#[test]
fn all_offers_remain_reachable_with_name_family_and_endpoint_separation() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let wanted = rows("Vendor/Model", "qwen", 931, false);
    let mut all = wanted.clone();
    all.extend(rows("vendor/other", "qwen", 7, false));
    all.extend(rows("Vendor/Model", "other", 4, false));
    all.extend(rows("Vendor/Model", "qwen", 3, true));
    hydrate(&c, all);
    let query = Browse {
        kind: Some(ProxyFamily::Llm),
        family_id: Some("qwen".into()),
        name_prefix: "VENDOR/MO".into(),
        ..Default::default()
    };
    let mut cursor = None;
    let mut found = Vec::new();
    let mut pages = 0;
    loop {
        let p = c
            .read()
            .unwrap()
            .proxy_offers(&query, cursor.as_deref(), 31, 10001)
            .unwrap();
        assert!(p.scanned_candidates <= MAX_CANDIDATES);
        assert!(serde_json::to_vec(&p.entries).unwrap().len() <= MAX_PAGE_ENTRY_BYTES + 2);
        found.extend(p.entries.iter().map(|e| e.id.clone()));
        cursor = p.next_cursor;
        pages += 1;
        assert!(pages < 100);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(found, ids(&wanted));
    assert!(pages > 20, "the catalog has no hidden total cap");
    let decision = c
        .read()
        .unwrap()
        .proxy_offers(
            &Browse {
                kind: Some(ProxyFamily::Decisions),
                ..Default::default()
            },
            None,
            100,
            10001,
        )
        .unwrap();
    assert_eq!(decision.entries.len(), 3);
}

#[test]
fn adjacent_backward_pages_restore_the_same_rows_and_selection_is_independent() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let mut all = rows("Alpha", "other", 17, false);
    all.extend(rows("Beta", "other", 21, false));
    hydrate(&c, all.clone());
    let q = Browse::default();
    let read = c.read().unwrap();
    let a = read.proxy_offers(&q, None, 13, 10001).unwrap();
    let b = read
        .proxy_offers(&q, a.next_cursor.as_deref(), 13, 10001)
        .unwrap();
    let back = read
        .proxy_offers(&q, b.previous_cursor.as_deref(), 13, 10001)
        .unwrap();
    assert_eq!(
        a.entries.iter().map(|x| &x.id).collect::<Vec<_>>(),
        back.entries.iter().map(|x| &x.id).collect::<Vec<_>>()
    );
    let again = read
        .proxy_offers(&q, back.next_cursor.as_deref(), 13, 10001)
        .unwrap();
    assert_eq!(
        b.entries.iter().map(|x| &x.id).collect::<Vec<_>>(),
        again.entries.iter().map(|x| &x.id).collect::<Vec<_>>()
    );
    let id = ids(&all).pop().unwrap();
    let exact = read.proxy_offer(&id, 10001).unwrap().unwrap();
    assert_eq!(exact.id, id);
    assert_eq!(exact.operator_verification, "unknown");
    assert!(
        !exact.catalog_eligible,
        "incomplete registration is not eligible, even though rates are published"
    );
    assert_eq!(exact.offer.rates.len(), 2);
    assert!(read.proxy_offer("native:model", 10001).is_err());
    assert!(read
        .proxy_offer(
            &format!("{}/{}/{}", "f".repeat(64), "f".repeat(64), "f".repeat(64)),
            10001
        )
        .unwrap()
        .is_none());
}

#[test]
fn no_op_ledger_updates_keep_cursors_but_repricing_expires_them_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let c = Catalog::open(&path, identity()).unwrap();
    let original = rows("Model", "other", 9, false);
    hydrate(&c, original.clone());
    let q = Browse::default();
    let first = c.read().unwrap().proxy_offers(&q, None, 2, 10001).unwrap();
    let cursor = first.next_cursor.unwrap();
    let old_snapshot = first.snapshot;
    apply(&c, vec![]);
    let same_row = original
        .iter()
        .find(|r| r.key.contains("/offers/"))
        .unwrap()
        .clone();
    apply(&c, vec![same_row.clone()]);
    drop(c);
    let c = Catalog::open(&path, identity()).unwrap();
    let read = c.read().unwrap();
    assert_eq!(
        read.proxy_offers(&q, Some(&cursor), 2, 10001)
            .unwrap()
            .snapshot,
        old_snapshot
    );
    let mut changed = same_row;
    let mut offer: ProxyOffer = serde_json::from_value(changed.value["offer"].clone()).unwrap();
    offer.revision += 1;
    offer.rates[0].per_unit_au = 999999;
    changed.value = json!({"active":true,"revision":offer.revision,"digest":offer.digest().unwrap(),"offer":offer});
    let id = changed
        .key
        .strip_prefix(&format!("{CATALOG_PREFIX}offers/"))
        .unwrap()
        .to_owned();
    apply(&c, vec![changed]);
    assert!(matches!(
        c.read().unwrap().proxy_offers(&q, Some(&cursor), 2, 10001),
        Err(Error::DirectoryCursorExpired)
    ));
    assert!(
        read.proxy_offers(&q, Some(&cursor), 2, 10001).is_ok(),
        "existing short MVCC readers retain their snapshot"
    );
    assert_eq!(
        c.read()
            .unwrap()
            .proxy_offer(&id, 10001)
            .unwrap()
            .unwrap()
            .offer
            .rates[0]
            .per_unit_au,
        999999
    );
    let changed_query = Browse {
        family_id: Some("other".into()),
        ..Default::default()
    };
    assert!(read
        .proxy_offers(&changed_query, Some(&cursor), 2, 10001)
        .is_err());
}

#[test]
fn sparse_filters_have_bounded_empty_pages_and_eventually_reach_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    hydrate(&c, rows("Model", "other", 800, false));
    let query = Browse {
        minimum_context: Some(u32::MAX),
        rail: Some(ProxyRail::Fiat),
        ..Default::default()
    };
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let p = c
            .read()
            .unwrap()
            .proxy_offers(&query, cursor.as_deref(), 100, 10001)
            .unwrap();
        assert!(p.entries.is_empty());
        assert!(p.scanned_candidates <= MAX_CANDIDATES);
        if let Some(next) = &p.next_cursor {
            assert_ne!(Some(next), cursor.as_ref());
        }
        cursor = p.next_cursor;
        pages += 1;
        assert!(pages < 10);
        if cursor.is_none() {
            break;
        }
    }
    assert!(pages > 1);
}

#[test]
fn deletion_and_withdrawal_remove_list_rows_but_keep_exact_historical_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let all = rows("Model", "other", 3, false);
    hydrate(&c, all.clone());
    let mut offers: Vec<_> = all
        .iter()
        .filter(|r| r.key.contains("/offers/"))
        .cloned()
        .collect();
    offers.sort_by(|a, b| a.key.cmp(&b.key));
    let withdrawn_id = offers[0]
        .key
        .strip_prefix(&format!("{CATALOG_PREFIX}offers/"))
        .unwrap()
        .to_owned();
    let deleted_id = offers[1]
        .key
        .strip_prefix(&format!("{CATALOG_PREFIX}offers/"))
        .unwrap()
        .to_owned();
    offers[0].value["active"] = json!(false);
    offers[1].value = Value::Null;
    apply(&c, offers[..2].to_vec());
    let read = c.read().unwrap();
    assert_eq!(
        read.proxy_offers(&Browse::default(), None, 100, 10001)
            .unwrap()
            .entries
            .len(),
        1
    );
    assert!(
        !read
            .proxy_offer(&withdrawn_id, 10001)
            .unwrap()
            .unwrap()
            .active
    );
    assert!(read.proxy_offer(&deleted_id, 10001).unwrap().is_none());
}

#[test]
fn preceding_index_migrates_without_scanning_or_discarding_public_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let all = rows("Model", "other", 3, false);
    let old_cursor;
    {
        let c = Catalog::open(&path, identity()).unwrap();
        hydrate(&c, all.clone());
        old_cursor = c
            .read()
            .unwrap()
            .proxy_offers(&Browse::default(), None, 1, 10001)
            .unwrap()
            .next_cursor
            .unwrap();
    }
    {
        let db = redb::Database::open(&path).unwrap();
        let tx = db.begin_write().unwrap();
        let mut table = tx
            .open_table(TableDefinition::<&str, &[u8]>::new(
                "proxy_catalog_metadata_v1",
            ))
            .unwrap();
        let mut state: Value =
            serde_json::from_slice(table.get("state").unwrap().unwrap().value()).unwrap();
        state["index_version"] = json!(1);
        state.as_object_mut().unwrap().remove("incarnation");
        state.as_object_mut().unwrap().remove("content_revision");
        table
            .insert("state", serde_json::to_vec(&state).unwrap().as_slice())
            .unwrap();
        drop(table);
        tx.commit().unwrap();
    }
    let c = Catalog::open(&path, identity()).unwrap();
    assert!(c.read().unwrap().get(&all[0].key).unwrap().is_some());
    assert!(c
        .read()
        .unwrap()
        .proxy_offers(&Browse::default(), None, 1, 10001)
        .is_err());
    hydrate(&c, all);
    assert!(matches!(
        c.read()
            .unwrap()
            .proxy_offers(&Browse::default(), Some(&old_cursor), 1, 10001),
        Err(Error::DirectoryCursorExpired)
    ));
    assert_eq!(
        c.read()
            .unwrap()
            .proxy_offers(&Browse::default(), None, 100, 10001)
            .unwrap()
            .entries
            .len(),
        3
    );
}

#[test]
fn many_empty_markets_do_not_hide_later_offers_or_exhaust_one_request() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let mut all = Vec::new();
    for n in 0..600 {
        all.extend(rows(&format!("Empty{n:05}"), "other", 0, false));
    }
    let expected = rows("ZZZ", "other", 3, false);
    all.extend(expected.clone());
    hydrate(&c, all);
    let mut cursor = None;
    let mut found = Vec::new();
    let mut pages = 0;
    loop {
        let page = c
            .read()
            .unwrap()
            .proxy_offers(&Browse::default(), cursor.as_deref(), 10, 10001)
            .unwrap();
        assert!(page.scanned_candidates <= MAX_CANDIDATES);
        found.extend(page.entries.into_iter().map(|e| e.id));
        cursor = page.next_cursor;
        pages += 1;
        assert!(pages < 10);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(found, ids(&expected));
    assert!(pages > 2);
}

#[test]
fn byte_bounded_pages_and_empty_terminal_backtracking_preserve_every_offer() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let all = rows(&"X".repeat(512), "other", 250, false);
    hydrate(&c, all.clone());
    let q = Browse::default();
    let mut cursor = None;
    let mut forward = Vec::new();
    let mut limited = false;
    let mut final_page;
    loop {
        final_page = c
            .read()
            .unwrap()
            .proxy_offers(&q, cursor.as_deref(), 100, 10001)
            .unwrap();
        limited |= final_page.next_cursor.is_some() && final_page.entries.len() < 100;
        forward.extend(final_page.entries.iter().map(|e| e.id.clone()));
        cursor = final_page.next_cursor.clone();
        assert!(serde_json::to_vec(&final_page.entries).unwrap().len() <= MAX_PAGE_ENTRY_BYTES + 2);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(forward, ids(&all));
    assert!(
        limited,
        "large rows must stop on bytes, not just item count"
    );
    let mut backward: Vec<_> = final_page
        .entries
        .iter()
        .rev()
        .map(|e| e.id.clone())
        .collect();
    cursor = final_page.previous_cursor;
    let mut count = 0;
    while let Some(token) = cursor {
        let page = c
            .read()
            .unwrap()
            .proxy_offers(&q, Some(&token), 100, 10001)
            .unwrap();
        backward.extend(page.entries.iter().rev().map(|e| e.id.clone()));
        cursor = page.previous_cursor;
        count += 1;
        assert!(count < 20);
    }
    let mut expected = ids(&all);
    expected.reverse();
    assert_eq!(backward, expected);

    // A full final page may have a conservative continuation. Returning from
    // that empty terminal page must include its previously consumed anchor.
    let first = c.read().unwrap().proxy_offers(&q, None, 1, 10001).unwrap();
    let mut page = first;
    while let Some(token) = page.next_cursor {
        page = c
            .read()
            .unwrap()
            .proxy_offers(&q, Some(&token), 1, 10001)
            .unwrap();
    }
    assert!(page.entries.is_empty());
    let back = c
        .read()
        .unwrap()
        .proxy_offers(&q, page.previous_cursor.as_deref(), 1, 10001)
        .unwrap();
    assert_eq!(back.entries[0].id, ids(&all).pop().unwrap());
}

#[path = "directory/candidates.rs"]
mod candidates;
