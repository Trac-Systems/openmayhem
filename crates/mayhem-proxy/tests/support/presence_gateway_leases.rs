use super::*;

#[tokio::test]
async fn shared_observation_leases_preserve_explicit_selections_and_other_readers() {
    let mut f = Fixture::new();
    f.refresh(now());
    let bridge = TestBridge::start().await;
    let (gateway, supervisor) = Gateway::new(
        bridge.config.clone(),
        f.catalog.clone(),
        Arc::new(f.table(8)),
        2,
    )
    .unwrap();
    let market = Digest::new(&f.offer.market_id).unwrap();
    let mut health = gateway.health();
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(supervisor.run(stopped));
    health_until(&mut health, |h| h.running).await;
    gateway.select(vec![market.clone()]).unwrap();
    health_until(&mut health, |h| h.connected && h.selected_markets == 1).await;
    let a = gateway.observe_market(d(70)).unwrap();
    let b = gateway.observe_market(d(70)).unwrap();
    health_until(&mut health, |h| h.connected && h.selected_markets == 2).await;
    assert!(gateway.observe_market(d(71)).is_err());
    assert!(gateway.select(vec![market.clone(), d(71)]).is_err());
    let attempts = health.borrow().attempts;
    drop(a);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(health.borrow().attempts, attempts);
    assert_eq!(health.borrow().selected_markets, 2);
    drop(b);
    health_until(&mut health, |h| h.connected && h.selected_markets == 1).await;
    let own = gateway.observe_market(market.clone()).unwrap();
    gateway.select(vec![]).unwrap();
    assert!(gateway.observe_market(d(72)).is_ok());
    drop(own);
    health_until(&mut health, |h| h.selected_markets == 0).await;
    stop.send_replace(true);
    task.await.unwrap().unwrap();
    assert!(gateway.observe_market(market).is_err());
}

#[tokio::test]
async fn newly_observed_market_waits_for_authentic_heartbeat_and_drop_removes_only_interest() {
    let mut f = Fixture::new();
    f.refresh(now());
    let bridge = TestBridge::start().await;
    let (gateway, supervisor) = Gateway::new(
        bridge.config.clone(),
        f.catalog.clone(),
        Arc::new(f.table(8)),
        1,
    )
    .unwrap();
    let market = Digest::new(&f.offer.market_id).unwrap();
    let provider = Digest::new(&f.offer.provider_pubkey).unwrap();
    let slot = Digest::new(f.offer.slot_id().unwrap()).unwrap();
    let status = || gateway.status(&market, &provider, &slot, None).unwrap();
    let mut health = gateway.health();
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(supervisor.run(stopped));
    health_until(&mut health, |h| h.running).await;
    let lease = gateway.observe_market(market.clone()).unwrap();
    health_until(&mut health, |h| h.connected).await;
    assert_eq!(status(), Eligibility::HeartbeatMissing);
    bridge.send(&f.sign(current_body(&f))).await;
    health_until(&mut health, |h| h.accepted == 1).await;
    assert_eq!(status(), Eligibility::Available);
    drop(lease);
    assert_eq!(status(), Eligibility::HeartbeatMissing);
    let retained = gateway.observe_market(market.clone()).unwrap();
    assert_eq!(
        status(),
        Eligibility::Available,
        "only still-fresh authenticated Table evidence is reused"
    );
    stop.send_replace(true);
    task.await.unwrap().unwrap();
    assert_eq!(status(), Eligibility::HeartbeatMissing);
    assert!(gateway.observe_market(market).is_err());
    drop(retained);
}

#[tokio::test]
async fn simultaneous_readers_cannot_exceed_shared_market_quota() {
    let f = Fixture::new();
    let bridge = TestBridge::start().await;
    let (gateway, supervisor) = Gateway::new(
        bridge.config.clone(),
        f.catalog.clone(),
        Arc::new(f.table(8)),
        1,
    )
    .unwrap();
    let mut health = gateway.health();
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(supervisor.run(stopped));
    health_until(&mut health, |h| h.running).await;
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for market in [d(70), d(71)] {
            let gateway = gateway.clone();
            let barrier = barrier.clone();
            let send = send.clone();
            scope.spawn(move || {
                barrier.wait();
                send.send(gateway.observe_market(market)).unwrap();
            });
        }
        barrier.wait();
    });
    let results = [receive.recv().unwrap(), receive.recv().unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    drop(results);
    assert!(gateway.observe_market(d(72)).is_ok());
    stop.send_replace(true);
    task.await.unwrap().unwrap();
}
