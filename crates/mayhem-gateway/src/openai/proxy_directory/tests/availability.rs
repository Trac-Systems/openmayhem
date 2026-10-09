use super::*;
use ed25519_dalek::{Signer, SigningKey};
use mayhem_bridge::ScBridgeConfig;
use mayhem_proxy::{
    attempts::Digest,
    presence::{self, Body as Heartbeat, Reason, Signed, Speed, Table},
};
use tokio::sync::watch;

fn digest(value: u8) -> Digest {
    Digest::new(format!("{value:064x}")).unwrap()
}
fn canonical_rows(decision: bool, key: &SigningKey) -> (ProxyOffer, ProxyMembership, Vec<Entry>) {
    let source: Value = serde_json::from_str(include_str!(
        "../../../../../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
    ))
    .unwrap();
    let fixture = &source["cases"][usize::from(decision)];
    let market: ProxyMarketDescriptor = serde_json::from_value(fixture["market"].clone()).unwrap();
    let mut member: ProxyMembership =
        serde_json::from_value(fixture["membership"].clone()).unwrap();
    let mut offer: ProxyOffer = serde_json::from_value(fixture["offer"].clone()).unwrap();
    let provider = hex::encode(key.verifying_key().to_bytes());
    member.provider_pubkey = provider.clone();
    offer.provider_pubkey = provider.clone();
    let endpoint = member
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
        row(format!("markets/{}", offer.market_id), json!(market)),
        row(
            format!("providers/{provider}"),
            json!({"provider_pubkey":provider,"admission_id":digest(7),"sequence":1,"active_memberships":1}),
        ),
        row(
            format!("memberships/{}/{}", offer.market_id, provider),
            json!({"active":true,"revision":member.revision,"member":member,"offer_slots":1}),
        ),
        row(
            format!(
                "offers/{}/{}/{}",
                offer.market_id,
                provider,
                offer.slot_id().unwrap()
            ),
            json!({"active":true,"revision":offer.revision,"offer":offer,"digest":offer.digest().unwrap()}),
        ),
        row(
            format!("families/{}", market.model.family_id),
            json!({"enabled":true,"label":"Local fixture"}),
        ),
        row(
            format!("endpoints/{}", endpoint.contract_hash),
            json!({"enabled":true,"endpoint":endpoint.endpoint,"family":market.family,
            "max_context":member.served_context,"ctx_brackets":[offer.ctx_bracket],"outcome_classes":[offer.outcome_class]}),
        ),
        row(
            format!("metering/{}", offer.metering_policy_hash),
            json!({"enabled":true,"units":offer.rates.iter().map(|rate|rate.unit.clone()).collect::<Vec<_>>()}),
        ),
    ];
    (offer, member, rows)
}
fn heartbeat(offer: &ProxyOffer, member: &ProxyMembership, now: u64) -> Heartbeat {
    Heartbeat {
        t: "proxy.hb".into(),
        schema_version: 1,
        network: identity(),
        provider: Digest::new(&offer.provider_pubkey).unwrap(),
        market: Digest::new(&offer.market_id).unwrap(),
        slot: Digest::new(offer.slot_id().unwrap()).unwrap(),
        offer: Digest::new(offer.digest().unwrap()).unwrap(),
        membership: Digest::new(member.digest().unwrap()).unwrap(),
        membership_revision: member.revision,
        offer_revision: offer.revision,
        fence: 1,
        boot: digest(4),
        sequence: 1,
        issued_ms: now,
        expires_ms: now + 14_000,
        evidence_expires_ms: now + 13_000,
        state: presence::State::Ready,
        reason: Reason::Fresh,
        allowance: member.max_concurrency,
        free_slots: member.max_concurrency,
        speed: (offer.endpoint != ProxyEndpoint::Decisions).then(|| Speed {
            tok_s_milli: 10_000,
            tokenizer: digest(5),
            observed_ms: now,
            expires_ms: now + 12_000,
        }),
    }
}
fn sign(key: &SigningKey, body: Heartbeat) -> Signed {
    let signature = hex::encode(key.sign(&body.signing_bytes().unwrap()).to_bytes());
    Signed { body, signature }
}
fn assert_projection(value: &Value, offer: &ProxyOffer, status: &str) {
    assert_eq!(value["availability"]["status"], status);
    assert_eq!(value["offer"], json!(offer));
    assert_eq!(value["digest"], offer.digest().unwrap());
    let observation = &value["availability"];
    let observed = observation["observed_at_ms"].as_u64().unwrap();
    assert!(observed <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER);
    if let Some(expiry) = observation["expires_at_ms"].as_u64() {
        assert!(expiry >= observed && expiry <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER);
        if status == "available" {
            assert!(expiry > observed);
        }
    } else {
        assert_ne!(status, "available");
    }
    assert_eq!(observation.as_object().unwrap().len(), 3);
    for key in [
        "free_slots",
        "allowance",
        "controller",
        "boot",
        "signature",
        "upstream_url",
        "token",
        "capacity",
    ] {
        assert!(
            value.get(key).is_none(),
            "protected/unsupported field {key}"
        );
        assert!(observation.get(key).is_none());
    }
    if status == "available" {
        assert_eq!(value["active"], true);
        assert_eq!(value["catalog_eligible"], true);
    }
}

#[tokio::test]
async fn same_signed_presence_and_default_policy_overlay_without_subscriptions_or_capacity_claims()
{
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    let key = SigningKey::from_bytes(&[42; 32]);
    let (offer, member, rows) = canonical_rows(false, &key);
    apply(&control, rows);
    let table = Arc::new(
        Table::open(
            &dir.path().join("state/overlay-presence.redb"),
            identity(),
            8,
        )
        .unwrap(),
    );
    let config = ScBridgeConfig::new("ws://127.0.0.1:1", "synthetic-unused")
        .unwrap()
        .with_operation_deadline(Some(std::time::Duration::from_millis(300)))
        .with_queue_limits(16, 128 * 1024);
    let (gateway, supervisor) =
        Gateway::new(config, control.catalog().clone(), table.clone(), 1).unwrap();
    let (stop, stopping) = watch::channel(false);
    let mut health = gateway.health();
    let task = tokio::spawn(supervisor.run(stopping));
    while !health.borrow().running {
        health.changed().await.unwrap();
    }
    // Current-thread runtime: every observation below is synchronous, and stop
    // is signalled before yielding again. No bridge connection is opened.
    gateway
        .select(vec![Digest::new(&offer.market_id).unwrap()])
        .unwrap();
    let snapshot = control.catalog().read().unwrap();
    let now = now_millis_u64();
    let id = format!(
        "{}/{}/{}",
        offer.market_id,
        offer.provider_pubkey,
        offer.slot_id().unwrap()
    );
    let registered = Registered::read(
        &snapshot,
        &Digest::new(&offer.market_id).unwrap(),
        &Digest::new(&offer.provider_pubkey).unwrap(),
        &Digest::new(offer.slot_id().unwrap()).unwrap(),
        now,
    )
    .unwrap();
    let project = || {
        serde_json::to_value(
            observe(
                &snapshot,
                &gateway,
                snapshot
                    .proxy_offer(&id, now_millis_u64())
                    .unwrap()
                    .unwrap(),
                now_millis_u64(),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let missing = project();
    assert_projection(&missing, &offer, "heartbeat_missing");
    let mut frames = Vec::new();
    let mut body = heartbeat(&offer, &member, now);
    for (sequence, expected, state, speed, free) in [
        (1, "available", presence::State::Ready, Some(10_000), 1),
        (2, "busy", presence::State::Busy, Some(10_000), 0),
        (
            3,
            "throughput_floor",
            presence::State::Ready,
            Some(4_999),
            1,
        ),
        (4, "throughput_unverified", presence::State::Ready, None, 1),
        (5, "checking", presence::State::Checking, Some(10_000), 0),
        (
            6,
            "unavailable",
            presence::State::Unavailable,
            Some(10_000),
            0,
        ),
        (7, "draining", presence::State::Draining, Some(10_000), 0),
    ] {
        body.sequence = sequence;
        body.state = state;
        body.free_slots = free;
        body.reason = match state {
            presence::State::Ready | presence::State::Busy => Reason::Fresh,
            presence::State::Draining => Reason::Draining,
            presence::State::Checking => Reason::NoEvidence,
            _ => Reason::RecoveryRequired,
        };
        body.speed = speed.map(|tok_s_milli| Speed {
            tok_s_milli,
            tokenizer: digest(5),
            observed_ms: now,
            expires_ms: now + 12_000,
        });
        table
            .receive(sign(&key, body.clone()), &registered, now_millis_u64())
            .unwrap();
        let value = project();
        assert_projection(&value, &offer, expected);
        assert_eq!(
            value["availability"]["status"],
            json!(gateway
                .status(
                    &Digest::new(&offer.market_id).unwrap(),
                    &Digest::new(&offer.provider_pubkey).unwrap(),
                    &Digest::new(offer.slot_id().unwrap()).unwrap(),
                    None
                )
                .unwrap())
        );
        frames.push(value);
    }
    let page = observe_page(
        &snapshot,
        &gateway,
        snapshot
            .proxy_offers(&directory::Query::default(), None, 100, now_millis_u64())
            .unwrap(),
        now_millis_u64(),
    )
    .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].availability.status, Eligibility::Draining);
    assert_eq!(health.borrow().attempts, 0);
    assert_eq!(health.borrow().selected_markets, 0);
    stop.send_replace(true);
    drop(snapshot);
    task.await.unwrap().unwrap();
    if let Ok(path) = std::env::var("PROXY_DIRECTORY_AVAILABILITY_FIXTURE") {
        std::fs::write(path, serde_json::to_vec_pretty(&json!({"schema_version":1,
            "source":"exact directory wrapper with real canonical catalog and signed presence table; no bridge or paid calls",
            "missing":missing,"observations":frames})).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn stopped_presence_and_stale_or_revoked_catalog_fail_closed_in_real_http() {
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    let key = SigningKey::from_bytes(&[43; 32]);
    let (offer, _, rows) = canonical_rows(false, &key);
    apply(&control, rows);
    let state = state(Some(control.clone()));
    let path = format!(
        "/v1/proxy/offers/{}/{}/{}",
        offer.market_id,
        offer.provider_pubkey,
        offer.slot_id().unwrap()
    );
    let (status, _, value) = request(&state, &path, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_projection(&value, &offer, "heartbeat_missing");
    let snapshot = control.catalog().read().unwrap();
    let future = now_millis_u64() + presence::CATALOG_AGE_MS + 1;
    let stale = observe(
        &snapshot,
        control.presence(),
        snapshot
            .proxy_offer(&value["id"].as_str().unwrap(), future)
            .unwrap()
            .unwrap(),
        future,
    )
    .unwrap();
    assert_eq!(stale.availability.status, Eligibility::CatalogUnavailable);
    drop(snapshot);
    apply(
        &control,
        vec![Entry {
            key: format!("{CATALOG_PREFIX}admission_status/{}", digest(7).as_str()),
            value: json!({"revision":1,"reason_hash":digest(13)}),
        }],
    );
    let (_, _, revoked) = request(&state, &path, Some(TOKEN)).await;
    assert_projection(&revoked, &offer, "catalog_unavailable");
    apply(
        &control,
        vec![Entry {
            key: format!("{CATALOG_PREFIX}offers/{}", value["id"].as_str().unwrap()),
            value: json!({"active":false,"revision":offer.revision,"offer":offer,"digest":offer.digest().unwrap()}),
        }],
    );
    let (_, _, value) = request(&state, &path, Some(TOKEN)).await;
    assert_eq!(value["active"], false);
    assert_projection(&value, &offer, "catalog_unavailable");
    assert_eq!(control.health().unwrap().presence.attempts, 0);
}

#[tokio::test]
async fn decision_availability_uses_no_token_floor_and_explicit_unselected_market_is_missing() {
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    let key = SigningKey::from_bytes(&[44; 32]);
    let (offer, member, rows) = canonical_rows(true, &key);
    apply(&control, rows);
    let table = Arc::new(
        Table::open(
            &dir.path().join("state/decision-presence.redb"),
            identity(),
            8,
        )
        .unwrap(),
    );
    let (gateway, supervisor) = Gateway::new(
        ScBridgeConfig::new("ws://127.0.0.1:1", "unused")
            .unwrap()
            .with_operation_deadline(Some(std::time::Duration::from_millis(300)))
            .with_queue_limits(16, 128 * 1024),
        control.catalog().clone(),
        table.clone(),
        1,
    )
    .unwrap();
    let (stop, stopping) = watch::channel(false);
    let mut health = gateway.health();
    let task = tokio::spawn(supervisor.run(stopping));
    while !health.borrow().running {
        health.changed().await.unwrap();
    }
    let now = now_millis_u64();
    let snapshot = control.catalog().read().unwrap();
    let market = Digest::new(&offer.market_id).unwrap();
    let provider = Digest::new(&offer.provider_pubkey).unwrap();
    let slot = Digest::new(offer.slot_id().unwrap()).unwrap();
    let registered = Registered::read(&snapshot, &market, &provider, &slot, now).unwrap();
    table
        .receive(
            sign(&key, heartbeat(&offer, &member, now)),
            &registered,
            now,
        )
        .unwrap();
    let id = format!(
        "{}/{}/{}",
        market.as_str(),
        provider.as_str(),
        slot.as_str()
    );
    let missing = observe(
        &snapshot,
        &gateway,
        snapshot.proxy_offer(&id, now).unwrap().unwrap(),
        now,
    )
    .unwrap();
    assert_eq!(missing.availability.status, Eligibility::HeartbeatMissing);
    gateway.select(vec![market.clone()]).unwrap();
    let available = observe(
        &snapshot,
        &gateway,
        snapshot.proxy_offer(&id, now).unwrap().unwrap(),
        now,
    )
    .unwrap();
    assert_projection(
        &serde_json::to_value(available).unwrap(),
        &offer,
        "available",
    );
    assert_eq!(
        gateway.status(&market, &provider, &slot, Some(5)).unwrap(),
        Eligibility::ThroughputUnverified
    );
    stop.send_replace(true);
    drop(snapshot);
    task.await.unwrap().unwrap();
}
