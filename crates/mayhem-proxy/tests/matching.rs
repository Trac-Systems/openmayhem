use mayhem_proto::proxy::ProxyMarketDescriptor;
use mayhem_proxy::{
    catalog::Catalog,
    discovery::*,
    matching::{MatchKind, SuggestionRequest},
};
use redb::{ReadableTable, TableDefinition};
use serde_json::Value;

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}
fn template() -> ProxyMarketDescriptor {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
    ))
    .unwrap();
    let mut market: ProxyMarketDescriptor =
        serde_json::from_value(fixture["cases"][0]["market"].clone()).unwrap();
    market.model.model_id = "vendor/model-a".into();
    market.model.revision = "rev1".into();
    market.model.quantization = "fp16".into();
    market
}
fn request() -> SuggestionRequest {
    let market = template();
    SuggestionRequest {
        schema_version: 1,
        report_digest: "a".repeat(64),
        family: market.family,
        model: market.model,
        endpoints: vec![market.endpoints[0].clone()],
        metering: market.metering,
        aliases: vec!["vendor/alias-a".into()],
    }
}
fn row(market: &ProxyMarketDescriptor) -> Entry {
    Entry {
        key: format!("{CATALOG_PREFIX}markets/{}", market.id().unwrap()),
        value: serde_json::to_value(market).unwrap(),
    }
}
fn page(entries: Vec<Entry>, base: Option<Proof>, next: Option<u64>) -> Page {
    let id = identity();
    let n = if base.is_some() { 20 } else { 10 };
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
            epoch: 1,
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
        entries,
        truncated: next.is_some(),
        next_cursor: next.map(|n| format!("pdc1.page{n}.{}", "a".repeat(128))),
        checkpoint: next
            .is_none()
            .then(|| format!("pdc1.complete{n}.{}", "a".repeat(128))),
    }
}
fn hydrate(c: &Catalog, markets: &[ProxyMarketDescriptor]) {
    let mut rows: Vec<_> = markets.iter().map(row).collect();
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    if rows.is_empty() {
        c.apply(&c.refresh_ticket().unwrap(), &page(vec![], None, None), 100)
            .unwrap();
    }
    let len = rows.len().div_ceil(100);
    for (i, rows) in rows.chunks(100).enumerate() {
        c.apply(
            &c.refresh_ticket().unwrap(),
            &page(rows.to_vec(), None, (i + 1 < len).then_some(i as u64)),
            100,
        )
        .unwrap();
    }
}

#[test]
fn exact_alias_and_normalized_names_are_ranked_with_explicit_conflicts_and_contract_filtering() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let exact = template();
    let mut second = exact.clone();
    second.slug = "second-name".into();
    let mut alias = exact.clone();
    alias.model.model_id = "vendor/alias-a".into();
    let mut normalized = exact.clone();
    normalized.model.model_id = "Vendor_ModelA".into();
    let mut conflict = normalized.clone();
    conflict.model.revision.clear();
    let mut metering = exact.clone();
    metering.metering.policy_hash = "1".repeat(64);
    let mut endpoint = exact.clone();
    endpoint.endpoints[0].contract_hash = "2".repeat(64);
    hydrate(
        &c,
        &[
            exact.clone(),
            second.clone(),
            alias.clone(),
            normalized.clone(),
            conflict.clone(),
            metering,
            endpoint,
        ],
    );
    let result = c.read().unwrap().suggest(&request(), None, 100).unwrap();
    assert_eq!(result.entries.len(), 5);
    let mut exact_ids = vec![exact.id().unwrap(), second.id().unwrap()];
    exact_ids.sort();
    assert_eq!(
        result.entries[..2]
            .iter()
            .map(|s| s.market_id.clone())
            .collect::<Vec<_>>(),
        exact_ids
    );
    assert!(result.entries[..2]
        .iter()
        .all(|s| s.match_kind == MatchKind::ExactDeclaredIdentity));
    assert_eq!(result.entries[2].market_id, alias.id().unwrap());
    assert_eq!(result.entries[2].match_kind, MatchKind::DeclaredAlias);
    assert_eq!(
        result
            .entries
            .iter()
            .find(|s| s.market_id == normalized.id().unwrap())
            .unwrap()
            .match_kind,
        MatchKind::NormalizedName
    );
    let conflict = result
        .entries
        .iter()
        .find(|s| s.market_id == conflict.id().unwrap())
        .unwrap();
    assert_eq!(conflict.match_kind, MatchKind::ConflictingClaim);
    assert!(conflict
        .conflicts
        .contains(&"revision_mismatch_or_undisclosed"));
    assert!(result.entries.iter().all(|s| s.requires_explicit_selection));
    assert!(result.next_cursor.is_none());
}

#[test]
fn paging_reaches_every_market_once_and_rejects_cross_query_or_changed_snapshot_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let markets: Vec<_> = (0..450)
        .map(|n| {
            let mut m = template();
            m.slug = format!("model-{n}");
            m
        })
        .collect();
    hydrate(&c, &markets);
    let r = c.read().unwrap();
    let query = request();
    let mut cursor = None;
    let mut ids = Vec::new();
    let mut pages = 0;
    loop {
        let result = r.suggest(&query, cursor.as_deref(), 37).unwrap();
        pages += 1;
        assert!(
            pages < 30,
            "bounded continuation must keep making progress, including duplicate-name skips"
        );
        ids.extend(result.entries.into_iter().map(|s| s.market_id));
        cursor = result.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let mut expected = markets.iter().map(|m| m.id().unwrap()).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(ids, expected);
    let first = r.suggest(&query, None, 1).unwrap();
    let cursor = first.next_cursor.as_deref().unwrap();
    let mut different = request();
    different.model.quantization = "int8".into();
    assert!(r.suggest(&different, Some(cursor), 1).is_err());
    let base = c.read().unwrap().status().committed.unwrap().proof;
    let mut removal = row(&markets[0]);
    removal.value = Value::Null;
    c.apply(
        &c.refresh_ticket().unwrap(),
        &page(vec![removal], Some(base), None),
        200,
    )
    .unwrap();
    assert!(c.read().unwrap().suggest(&query, Some(cursor), 1).is_err());
    assert!(
        r.suggest(&query, Some(cursor), 1).is_ok(),
        "old MVCC readers keep their pinned proof"
    );
}

#[test]
fn delta_index_updates_are_atomic_with_catalog_visibility_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let c = Catalog::open(&path, identity()).unwrap();
    let old = template();
    hydrate(&c, &[old.clone()]);
    let mut new = template();
    new.slug = "new-publication".into();
    let base = c.read().unwrap().status().committed.unwrap().proof;
    let mut rows = vec![
        Entry {
            key: row(&old).key,
            value: Value::Null,
        },
        row(&new),
    ];
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    c.apply(
        &c.refresh_ticket().unwrap(),
        &page(vec![rows[0].clone()], Some(base.clone()), Some(4)),
        200,
    )
    .unwrap();
    assert_eq!(
        c.read()
            .unwrap()
            .suggest(&request(), None, 100)
            .unwrap()
            .entries[0]
            .market_id,
        old.id().unwrap()
    );
    drop(c);
    let c = Catalog::open(&path, identity()).unwrap();
    c.apply(
        &c.refresh_ticket().unwrap(),
        &page(vec![rows[1].clone()], Some(base), None),
        200,
    )
    .unwrap();
    let result = c.read().unwrap().suggest(&request(), None, 100).unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].market_id, new.id().unwrap());
}

#[test]
fn previous_cache_format_rehydrates_indexes_without_discarding_public_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    {
        let c = Catalog::open(&path, identity()).unwrap();
        hydrate(&c, &[template()]);
    }
    // Represent the preceding committed cache format: records and proof remain,
    // but its metadata did not yet declare a derived index version.
    {
        let db = redb::Database::open(&path).unwrap();
        let tx = db.begin_write().unwrap();
        let meta: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_catalog_metadata_v1");
        let mut table = tx.open_table(meta).unwrap();
        let mut state: Value =
            serde_json::from_slice(table.get("state").unwrap().unwrap().value()).unwrap();
        state.as_object_mut().unwrap().remove("index_version");
        table
            .insert("state", serde_json::to_vec(&state).unwrap().as_slice())
            .unwrap();
        drop(table);
        tx.delete_table(TableDefinition::<&str, &str>::new("proxy_match_current_v1"))
            .unwrap();
        tx.commit().unwrap();
    }
    let c = Catalog::open(&path, identity()).unwrap();
    assert!(c.read().unwrap().status().invalidated);
    assert!(c
        .read()
        .unwrap()
        .get(&row(&template()).key)
        .unwrap()
        .is_some());
    assert!(c.read().unwrap().suggest(&request(), None, 1).is_err());
    assert!(c.refresh_ticket().unwrap().query().since.is_none());
    hydrate(&c, &[template()]);
    assert_eq!(
        c.read()
            .unwrap()
            .suggest(&request(), None, 10)
            .unwrap()
            .entries
            .len(),
        1
    );
}

#[test]
fn decisions_never_cross_into_chat_and_unknown_identity_does_not_become_an_exact_match() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path().join("catalog"), identity()).unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
    ))
    .unwrap();
    let decision: ProxyMarketDescriptor =
        serde_json::from_value(fixture["cases"][1]["market"].clone()).unwrap();
    hydrate(&c, &[decision.clone(), template()]);
    let query = SuggestionRequest {
        schema_version: 1,
        report_digest: "a".repeat(64),
        family: decision.family,
        model: decision.model.clone(),
        endpoints: decision.endpoints.clone(),
        metering: decision.metering.clone(),
        aliases: vec![],
    };
    let result = c.read().unwrap().suggest(&query, None, 100).unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].market_id, decision.id().unwrap());
    let mut unknown = request();
    unknown.model.model_id.clear();
    assert!(unknown.validate().is_err());
    let mut wrong = request();
    wrong.endpoints = decision.endpoints;
    assert!(wrong.validate().is_err());
}
