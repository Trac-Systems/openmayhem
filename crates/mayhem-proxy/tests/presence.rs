#![cfg(unix)]
use ed25519_dalek::{Signer, SigningKey};
use mayhem_proto::proxy::{ProxyMarketDescriptor, ProxyMembership, ProxyOffer};
use mayhem_proxy::{attempts::Digest, catalog::Catalog, discovery::*, presence::*};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[allow(dead_code)]
#[path = "support/exchange_bridge.rs"]
mod bridge;
#[path = "support/presence_gateway.rs"]
mod gateway;
fn d(n: u8) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
struct Fixture {
    dir: tempfile::TempDir,
    catalog: Arc<Catalog>,
    network: Identity,
    key: SigningKey,
    market: ProxyMarketDescriptor,
    member: ProxyMembership,
    offer: ProxyOffer,
    rows: Vec<Entry>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let network = Identity {
            network_id: "918".into(),
            msb_bootstrap: d(1).as_str().into(),
            subnet_bootstrap: d(2).as_str().into(),
            contract_version: mayhem_proto::CONTRACT_VERSION,
        };
        let key = SigningKey::from_bytes(&[42; 32]);
        let pk = hex(&key.verifying_key().to_bytes());
        let value: Value = serde_json::from_str(include_str!(
            "../../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
        ))
        .unwrap();
        let market: ProxyMarketDescriptor =
            serde_json::from_value(value["cases"][0]["market"].clone()).unwrap();
        let mut member: ProxyMembership =
            serde_json::from_value(value["cases"][0]["membership"].clone()).unwrap();
        let mut offer: ProxyOffer =
            serde_json::from_value(value["cases"][0]["offer"].clone()).unwrap();
        member.provider_pubkey = pk.clone();
        offer.provider_pubkey = pk;
        let catalog =
            Arc::new(Catalog::open(dir.path().join("catalog.redb"), network.clone()).unwrap());
        let mut f = Self {
            dir,
            catalog,
            network,
            key,
            market,
            member,
            offer,
            rows: vec![],
        };
        let mut network = serde_json::to_value(&f.network).unwrap();
        network["enabled"] = json!(true);
        let endpoint = f.member.endpoints[0].clone();
        f.row("network/current".into(), network);
        f.row(format!("markets/{}", f.offer.market_id), json!(f.market));
        f.row(format!("providers/{}",f.offer.provider_pubkey),json!({"provider_pubkey":f.offer.provider_pubkey,"admission_id":d(7),"sequence":1,"active_memberships":1}));
        f.row(
            format!(
                "memberships/{}/{}",
                f.offer.market_id, f.offer.provider_pubkey
            ),
            json!({"active":true,"revision":1,"member":f.member,"offer_slots":1}),
        );
        f.row(
            format!(
                "offers/{}/{}/{}",
                f.offer.market_id,
                f.offer.provider_pubkey,
                f.offer.slot_id().unwrap()
            ),
            json!({"active":true,"revision":1,"offer":f.offer,"digest":f.offer.digest().unwrap()}),
        );
        f.row(
            "families/other".into(),
            json!({"enabled":true,"label":"Other"}),
        );
        f.row(format!("endpoints/{}",endpoint.contract_hash),json!({"enabled":true,"endpoint":endpoint.endpoint,"family":"llm","max_context":262144,"ctx_brackets":["le256k"],"outcome_classes":[""]}));
        f.row(
            format!("metering/{}", f.offer.metering_policy_hash),
            json!({"enabled":true,"units":["input_token","output_token"]}),
        );
        f.refresh(10_000);
        f
    }
    fn row(&mut self, key: String, value: Value) {
        let key = format!("{CATALOG_PREFIX}{key}");
        self.rows.retain(|r| r.key != key);
        self.rows.push(Entry { key, value })
    }
    fn refresh(&mut self, now: u64) {
        self.rows.sort_by(|a, b| a.key.cmp(&b.key));
        let committed = self.catalog.read().unwrap().status().committed;
        let base = committed.map(|c| c.proof);
        let p = Page {
            ok: true,
            lane: "proxy".into(),
            schema_version: 1,
            request_nonce: d(8).as_str().into(),
            query: QueryBinding::catalog(),
            context: Context {
                network_id: self.network.network_id.clone(),
                msb_bootstrap: self.network.msb_bootstrap.clone(),
                subnet_bootstrap: self.network.subnet_bootstrap.clone(),
                contract_version: self.network.contract_version,
                epoch: 100,
            },
            proof: Proof {
                view_key: d(9).as_str().into(),
                fork: 0,
                signed_length: now,
                tree_hash: format!("{now:064x}"),
            },
            mode: if base.is_some() {
                Mode::Changes
            } else {
                Mode::Snapshot
            },
            base_proof: base,
            entries: self.rows.clone(),
            truncated: false,
            next_cursor: None,
            checkpoint: Some(format!("pdc1.test{now}.{}", "a".repeat(128))),
        };
        self.catalog
            .apply(&self.catalog.refresh_ticket().unwrap(), &p, now)
            .unwrap();
    }
    fn registered(&self, now: u64) -> Registered {
        Registered::read(
            &self.catalog.read().unwrap(),
            &Digest::new(&self.offer.market_id).unwrap(),
            &Digest::new(&self.offer.provider_pubkey).unwrap(),
            &Digest::new(self.offer.slot_id().unwrap()).unwrap(),
            now,
        )
        .unwrap()
    }
    fn body(&self) -> Body {
        Body {
            t: "proxy.hb".into(),
            schema_version: 1,
            network: self.network.clone(),
            provider: Digest::new(&self.offer.provider_pubkey).unwrap(),
            market: Digest::new(&self.offer.market_id).unwrap(),
            slot: Digest::new(self.offer.slot_id().unwrap()).unwrap(),
            offer: Digest::new(self.offer.digest().unwrap()).unwrap(),
            membership: Digest::new(self.member.digest().unwrap()).unwrap(),
            membership_revision: 1,
            offer_revision: 1,
            fence: 1,
            boot: d(4),
            sequence: 1,
            issued_ms: 10000,
            expires_ms: 24000,
            evidence_expires_ms: 20000,
            state: State::Ready,
            reason: Reason::Fresh,
            allowance: 2,
            free_slots: 2,
            speed: Some(Speed {
                tok_s_milli: 10000,
                tokenizer: d(5),
                observed_ms: 9000,
                expires_ms: 19000,
            }),
        }
    }
    fn sign(&self, body: Body) -> Signed {
        let signature = hex(&self.key.sign(&body.signing_bytes().unwrap()).to_bytes());
        Signed { body, signature }
    }
    fn table(&self, quota: u64) -> Table {
        Table::open(
            &self.dir.path().join("presence.redb"),
            self.network.clone(),
            quota,
        )
        .unwrap()
    }
}
#[test]
fn clock_rollback_cannot_keep_registration_available_after_monotonic_expiry() {
    let f = Fixture::new();
    let table = f.table(8);
    let registered = f.registered(10_000);
    let mut body = f.body();
    body.issued_ms = 24_998;
    body.expires_ms = 34_998;
    body.evidence_expires_ms = 34_998;
    let speed = body.speed.as_mut().unwrap();
    speed.observed_ms = 24_990;
    speed.expires_ms = 34_998;
    table
        .receive(f.sign(body.clone()), &registered, 24_999)
        .unwrap();
    std::thread::sleep(Duration::from_millis(3));
    assert_eq!(
        eligibility(&body, &f.offer, 25_010, None),
        Eligibility::Available
    );
    let observed = table.observe(&registered, 10_001, None).unwrap();
    assert_eq!(observed.status, Eligibility::CatalogUnavailable);
    assert_eq!(observed.observed_at_ms, 10_001);
    assert_eq!(observed.expires_at_ms, None);
    assert_eq!(
        table.status(&registered, 10_001, None).unwrap(),
        Eligibility::CatalogUnavailable
    );
}

#[test]
fn public_route_checks_signature_network_offer_membership_and_bounds() {
    let f = Fixture::new();
    let t = f.table(10);
    let registered = f.registered(10000);
    let good = f.sign(f.body());
    Signed::parse(&serde_json::to_vec(&good).unwrap(), &f.network, 10000).unwrap();
    let mut bad = good.clone();
    bad.body.free_slots = 1;
    assert!(t.receive(bad, &registered, 10000).is_err());
    for mutation in 0..6 {
        let mut b = f.body();
        match mutation {
            0 => b.network.network_id = "elsewhere".into(),
            1 => b.offer = d(99),
            2 => b.membership = d(99),
            3 => b.allowance = 3,
            4 => b.issued_ms = 16000,
            _ => b.offer_revision = 2,
        };
        assert!(t.receive(f.sign(b), &registered, 10000).is_err());
    }
    assert!(Signed::parse(&vec![b' '; MAX_BYTES + 1], &f.network, 10000).is_err());
    t.receive(good, &registered, 10000).unwrap();
    assert_eq!(
        t.status(&registered, 10000, None).unwrap(),
        Eligibility::Available
    );
}
#[test]
fn renewed_heartbeat_does_not_renew_speed_or_health_and_explicit_floor_is_enforced() {
    let f = Fixture::new();
    let t = f.table(10);
    let registered = f.registered(10000);
    t.receive(f.sign(f.body()), &registered, 10000).unwrap();
    assert_eq!(
        t.status(&registered, 10000, Some(11)).unwrap(),
        Eligibility::ThroughputFloor
    );
    let mut next = f.body();
    next.sequence = 2;
    next.issued_ms = 18000;
    next.expires_ms = 24000;
    t.receive(f.sign(next), &registered, 18000).unwrap();
    assert_eq!(
        t.status(&registered, 19000, None).unwrap(),
        Eligibility::ThroughputUnverified
    );
    assert_eq!(
        t.status(&registered, 20000, None).unwrap(),
        Eligibility::StaleEvidence
    );
    assert_eq!(
        t.status(&registered, 24000, None).unwrap(),
        Eligibility::HeartbeatMissing
    );
    assert_eq!(
        t.status(&registered, 25000, None).unwrap(),
        Eligibility::CatalogUnavailable
    );
}
#[test]
fn withdrawal_and_restart_reject_old_ready_even_when_signature_is_fresh() {
    let f = Fixture::new();
    let registered = f.registered(10000);
    let ready = f.sign(f.body());
    {
        let t = f.table(10);
        t.receive(ready.clone(), &registered, 10000).unwrap();
        let mut b = f.body();
        b.sequence = 2;
        b.free_slots = 0;
        b.state = State::Draining;
        b.reason = Reason::Draining;
        t.receive(f.sign(b), &registered, 10001).unwrap();
        assert_eq!(
            t.status(&registered, 10001, None).unwrap(),
            Eligibility::Draining
        );
    }
    let t = f.table(10);
    assert_eq!(
        t.status(&registered, 10002, None).unwrap(),
        Eligibility::HeartbeatMissing
    );
    assert!(t.receive(ready, &registered, 10002).is_err());
    let mut restarted = f.body();
    restarted.fence = 2;
    restarted.boot = d(6);
    t.receive(f.sign(restarted), &registered, 10002).unwrap();
    let mut zombie = f.body();
    zombie.sequence = 999;
    zombie.issued_ms = 10003;
    assert!(t.receive(f.sign(zombie), &registered, 10003).is_err());
    assert_eq!(
        t.status(&registered, 10003, None).unwrap(),
        Eligibility::Available
    );
}
#[test]
fn duplicate_stores_fail_closed_and_conflict_survives_receiver_restart() {
    let f = Fixture::new();
    let registered = f.registered(10000);
    {
        let t = f.table(10);
        t.receive(f.sign(f.body()), &registered, 10000).unwrap();
        let mut duplicate = f.body();
        duplicate.boot = d(88);
        assert!(t.receive(f.sign(duplicate), &registered, 10000).is_err());
        assert_eq!(
            t.status(&registered, 10000, None).unwrap(),
            Eligibility::ControllerConflict
        );
    }
    let t = f.table(10);
    let mut original = f.body();
    original.sequence = 99;
    assert!(t.receive(f.sign(original), &registered, 10000).is_err());
    assert_eq!(
        t.status(&registered, 10000, None).unwrap(),
        Eligibility::ControllerConflict
    );
    let mut fixed = f.body();
    fixed.fence = 2;
    fixed.boot = d(89);
    t.receive(f.sign(fixed), &registered, 10000).unwrap();
    assert_eq!(
        t.status(&registered, 10000, None).unwrap(),
        Eligibility::Available
    );
}
#[test]
fn current_canonical_revocation_blocks_previously_healthy_presence() {
    let mut f = Fixture::new();
    let registered = f.registered(10000);
    let t = f.table(10);
    t.receive(f.sign(f.body()), &registered, 10000).unwrap();
    f.row(
        format!("admission_status/{}", d(7).as_str()),
        json!({"revision":1,"reason_hash":d(13)}),
    );
    f.refresh(11000);
    assert!(Registered::read(
        &f.catalog.read().unwrap(),
        &Digest::new(&f.offer.market_id).unwrap(),
        &Digest::new(&f.offer.provider_pubkey).unwrap(),
        &Digest::new(f.offer.slot_id().unwrap()).unwrap(),
        11000
    )
    .is_err());
}
#[test]
fn busy_and_decision_routes_do_not_claim_token_measurements() {
    let f = Fixture::new();
    let mut b = f.body();
    b.free_slots = 0;
    b.state = State::Busy;
    assert_eq!(eligibility(&b, &f.offer, 10000, None), Eligibility::Busy);
    let mut offer = f.offer.clone();
    offer.endpoint = mayhem_proto::proxy::ProxyEndpoint::Decisions;
    b.speed = None;
    b.state = State::Ready;
    b.free_slots = 1;
    assert_eq!(eligibility(&b, &offer, 10000, None), Eligibility::Available);
    assert_eq!(
        eligibility(&b, &offer, 10000, Some(5)),
        Eligibility::ThroughputUnverified
    );
}

#[tokio::test]
async fn authenticated_bridge_delivers_only_registered_market_bound_signed_presence() {
    let mut f = Fixture::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    f.refresh(now);
    let table = Arc::new(f.table(10));
    let bridge = bridge::Bridge::start(d(11).as_str(), &f.offer.provider_pubkey).await;
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let (updates, mut health) = tokio::sync::watch::channel(ReceiverHealth::default());
    let task = tokio::spawn(receive_bridge(
        bridge.config(true),
        f.catalog.clone(),
        table.clone(),
        vec![Digest::new(&f.offer.market_id).unwrap()],
        1,
        stopping,
        updates,
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        while !health.borrow().connected {
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let mut sender = mayhem_bridge::ScBridgeClient::connect(bridge.config(false))
        .await
        .unwrap();
    sender.mute_sidechannel_events().await.unwrap();
    let topic = channel(&f.network, &Digest::new(&f.offer.market_id).unwrap()).unwrap();
    sender.join(&topic).await.unwrap();
    let mut b = f.body();
    b.issued_ms = now;
    b.expires_ms = now + 14000;
    b.evidence_expires_ms = now + 10000;
    let speed = b.speed.as_mut().unwrap();
    speed.observed_ms = now - 1000;
    speed.expires_ms = now + 9000;
    let good = f.sign(b.clone());
    sender.send(&topic, &good).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while health.borrow().accepted < 1 {
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        table.status(&f.registered(now), now, None).unwrap(),
        Eligibility::Available
    );
    sender.send(&topic, &good).await.unwrap();
    let mut crossed = b.clone();
    crossed.market = d(22);
    crossed.sequence = 2;
    sender.send(&topic, f.sign(crossed)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while health.borrow().rejected < 2 {
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(health.borrow().accepted, 1);
    b.sequence = 2;
    b.state = State::Draining;
    b.reason = Reason::Draining;
    b.free_slots = 0;
    sender.send(&topic, f.sign(b)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while health.borrow().accepted < 2 {
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        table.status(&f.registered(now), now, None).unwrap(),
        Eligibility::Draining
    );
    stop.send_replace(true);
    task.await.unwrap().unwrap();
    assert!(!health.borrow().connected);
}

#[test]
fn low_speed_can_recover_but_heartbeat_refresh_cannot_invent_a_measurement() {
    let f = Fixture::new();
    let registered = f.registered(10000);
    let table = f.table(2);
    let mut b = f.body();
    b.speed.as_mut().unwrap().tok_s_milli = 3840;
    table
        .receive(f.sign(b.clone()), &registered, 10000)
        .unwrap();
    assert_eq!(
        table.status(&registered, 10000, None).unwrap(),
        Eligibility::ThroughputFloor
    );
    b.sequence = 2;
    b.issued_ms = 10001;
    b.speed.as_mut().unwrap().tok_s_milli = 50000;
    b.speed.as_mut().unwrap().observed_ms = 10001;
    table.receive(f.sign(b), &registered, 10001).unwrap();
    assert_eq!(
        table.status(&registered, 10001, None).unwrap(),
        Eligibility::Available
    );
    assert_eq!(
        table.status(&registered, 10001, Some(60)).unwrap(),
        Eligibility::ThroughputFloor
    );
}
#[test]
fn frozen_or_rolled_back_wall_clock_does_not_extend_presence() {
    let f = Fixture::new();
    let registered = f.registered(10000);
    let table = f.table(2);
    let mut b = f.body();
    b.expires_ms = 10020;
    table.receive(f.sign(b), &registered, 10000).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        table.status(&registered, 10000, None).unwrap(),
        Eligibility::HeartbeatMissing
    );
    assert_eq!(
        table.status(&registered, 9000, None).unwrap(),
        Eligibility::CatalogUnavailable
    );
}
#[test]
fn storage_quota_rejects_new_routes_without_forgetting_withdrawal_watermarks() {
    let mut f = Fixture::new();
    let table = f.table(1);
    let old = f.sign(f.body());
    let mut withdrawn = old.body.clone();
    withdrawn.sequence = 2;
    withdrawn.state = State::Draining;
    withdrawn.free_slots = 0;
    table
        .receive(f.sign(withdrawn), &f.registered(10000), 10000)
        .unwrap();
    let second = ProxyOffer {
        ctx_bracket: "le128k".into(),
        ..f.offer.clone()
    };
    f.row(
        format!(
            "offers/{}/{}/{}",
            second.market_id,
            second.provider_pubkey,
            second.slot_id().unwrap()
        ),
        json!({"active":true,"revision":1,"offer":second,"digest":second.digest().unwrap()}),
    );
    let endpoint = f.member.endpoints[0].clone();
    f.row(format!("endpoints/{}",endpoint.contract_hash),json!({"enabled":true,"endpoint":endpoint.endpoint,"family":"llm","max_context":262144,"ctx_brackets":["le128k","le256k"],"outcome_classes":[""]}));
    f.refresh(11000);
    let registered = Registered::read(
        &f.catalog.read().unwrap(),
        &Digest::new(&second.market_id).unwrap(),
        &Digest::new(&second.provider_pubkey).unwrap(),
        &Digest::new(second.slot_id().unwrap()).unwrap(),
        11000,
    )
    .unwrap();
    let mut b = f.body();
    b.offer = Digest::new(second.digest().unwrap()).unwrap();
    b.slot = Digest::new(second.slot_id().unwrap()).unwrap();
    assert!(table.receive(f.sign(b), &registered, 11000).is_err());
    assert!(table.receive(old, &f.registered(11000), 11000).is_err());
    assert_eq!(
        table.status(&f.registered(11000), 11000, None).unwrap(),
        Eligibility::Draining
    );
}
