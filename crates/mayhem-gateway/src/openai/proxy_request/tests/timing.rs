use super::*;
use crate::openai::proxy_control::Prepared;
use mayhem_proxy::{
    catalog::Catalog,
    discovery::{Context, Entry, Identity, Mode, Page, Proof, QueryBinding, CATALOG_PREFIX},
    presence::CATALOG_AGE_MS,
};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, sync::Barrier};

fn identity() -> Identity {
    Identity {
        network_id: "918".into(),
        msb_bootstrap: "a".repeat(64),
        subnet_bootstrap: "b".repeat(64),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn fixture() -> (tempfile::TempDir, Arc<ProxyControl>, Arc<Request>) {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("bridge-token"), b"timing-test-only-token");
    let config = json!({"schema_version":1,"network":identity(),
        "peer_rpc_url":"http://127.0.0.1:1","state_dir":"state",
        "bridge":{"url":"ws://127.0.0.1:2","token_file":"bridge-token",
            "operation_timeout_ms":500,"frame_bytes":65536,"queue_events":8,"queue_bytes":131072},
        "max_markets":4,"max_presence_routes":16,"selected_markets":[]});
    let path = dir.path().join("gateway.json");
    write(&path, &serde_json::to_vec(&config).unwrap());
    let (control, lifecycle) = Prepared::load(&path, &identity()).unwrap().open().unwrap();
    // Only direct local catalog reads/commits; no lifecycle or socket is started.
    drop(lifecycle);
    let c = candidate(false);
    let offer = &c.offer;
    let member = &c.membership;
    let contract = member
        .endpoints
        .iter()
        .find(|e| e.endpoint == offer.endpoint)
        .unwrap();
    let mut network = json!(identity());
    network["enabled"] = json!(true);
    let row = |key: String, value: Value| Entry {
        key: format!("{CATALOG_PREFIX}{key}"),
        value,
    };
    let rows = vec![
        row("network/current".into(), network),
        row(format!("markets/{}", offer.market_id), json!(c.market)),
        row(
            format!("providers/{}", offer.provider_pubkey),
            json!({"provider_pubkey":offer.provider_pubkey,"admission_id":digest('7'),"sequence":1,"active_memberships":1}),
        ),
        row(
            format!("memberships/{}/{}", offer.market_id, offer.provider_pubkey),
            json!({"active":true,"revision":member.revision,"member":member,"offer_slots":1}),
        ),
        row(
            format!("offers/{}", c.id),
            json!({"active":true,"revision":offer.revision,"offer":offer,"digest":c.digest}),
        ),
        row(
            format!("families/{}", c.market.model.family_id),
            json!({"enabled":true,"label":"Timing fixture"}),
        ),
        row(
            format!("endpoints/{}", contract.contract_hash),
            json!({"enabled":true,"endpoint":offer.endpoint,"family":c.market.family,"max_context":member.served_context,
                "ctx_brackets":[offer.ctx_bracket],"outcome_classes":[offer.outcome_class]}),
        ),
        row(
            format!("metering/{}", offer.metering_policy_hash),
            json!({"enabled":true,"units":offer.rates.iter().map(|rate|rate.unit.clone()).collect::<Vec<_>>()}),
        ),
    ];
    apply(control.catalog(), rows, 1_000);
    let request = Arc::new(parse(&c, raw(&c, offer.accepted_rails[0])));
    (dir, control, request)
}

fn apply(catalog: &Catalog, mut entries: Vec<Entry>, observed: u64) {
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    let prior = catalog.read().unwrap().status().committed;
    let n = prior.as_ref().map_or(1, |c| c.proof.signed_length + 1);
    let network = identity();
    let page = Page {
        ok: true,
        lane: "proxy".into(),
        schema_version: 1,
        request_nonce: "c".repeat(64),
        query: QueryBinding::catalog(),
        context: Context {
            network_id: network.network_id,
            msb_bootstrap: network.msb_bootstrap,
            subnet_bootstrap: network.subnet_bootstrap,
            contract_version: network.contract_version,
            epoch: 1,
        },
        proof: Proof {
            view_key: "d".repeat(64),
            fork: 0,
            signed_length: n,
            tree_hash: format!("{n:064x}"),
        },
        mode: if prior.is_some() {
            Mode::Changes
        } else {
            Mode::Snapshot
        },
        base_proof: prior.map(|c| c.proof),
        entries,
        truncated: false,
        next_cursor: None,
        checkpoint: Some(format!("pdc1.timing{n}.{}", "e".repeat(128))),
    };
    catalog
        .apply(&catalog.refresh_ticket().unwrap(), &page, observed)
        .unwrap();
}

#[tokio::test]
async fn resolve_estimate_pins_snapshot_before_clock_across_concurrent_commit() {
    let (_dir, control, request) = fixture();
    let barrier = Arc::new(Barrier::new(2));
    let writer_barrier = barrier.clone();
    let catalog = control.catalog().clone();
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        apply(&catalog, vec![], 2_000);
        writer_barrier.wait();
    });
    let result = resolve_estimate_with_clock(control.clone(), request.clone(), move || {
        let now = 1_001;
        // Force a newer commit after sampling this time, before returning it.
        // The old timestamp-before-snapshot order reads the new commit as future
        // evidence and fails; the fixed order retains the pinned prior snapshot.
        barrier.wait();
        barrier.wait();
        now
    })
    .await;
    writer.join().unwrap();
    let result = result.unwrap();
    assert_eq!(result.published.catalog_observed_at_ms, Some(1_000));
    assert_eq!(result.expires_at_ms, 1_000 + CATALOG_AGE_MS);
    assert_eq!(
        control
            .catalog()
            .read()
            .unwrap()
            .status()
            .committed
            .unwrap()
            .observed_at_ms,
        Some(2_000)
    );
    let latest = resolve_estimate_with_clock(control, request, || 2_001)
        .await
        .unwrap();
    assert_eq!(latest.published.catalog_observed_at_ms, Some(2_000));
    assert_eq!(latest.expires_at_ms, 2_000 + CATALOG_AGE_MS);
}

#[tokio::test]
async fn resolve_estimate_timing_still_rejects_clock_reversal_stale_and_invalidated_evidence() {
    let (_dir, control, request) = fixture();
    assert!(matches!(
        resolve_estimate_with_clock(control.clone(), request.clone(), || 999).await,
        Err(Error::Catalog)
    ));
    assert!(matches!(
        resolve_estimate_with_clock(control.clone(), request.clone(), || 1_001 + CATALOG_AGE_MS)
            .await,
        Err(Error::Catalog)
    ));
    let catalog = control.catalog();
    catalog
        .invalidate(&catalog.refresh_ticket().unwrap())
        .unwrap();
    assert!(matches!(
        resolve_estimate_with_clock(control, request, || 1_001).await,
        Err(Error::Catalog)
    ));
}
