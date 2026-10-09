//! Local authenticated protocol fixture only, not a Noise/relay acceptance test.
use super::*;
use futures_util::{SinkExt, StreamExt};
use mayhem_bridge::ScBridgeConfig;
use mayhem_proxy::presence::gateway::{Gateway, Health};
use std::sync::Mutex;
use tokio::{net::TcpListener, sync::mpsc, sync::watch, task::JoinHandle};
use tokio_tungstenite::{accept_async, tungstenite::Message};

enum Command {
    Frame(Value),
    Disconnect,
}

struct TestBridge {
    config: ScBridgeConfig,
    commands: mpsc::Sender<Command>,
    subscriptions: Arc<Mutex<Vec<Vec<String>>>>,
    task: JoinHandle<()>,
}

impl Drop for TestBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestBridge {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = ScBridgeConfig::new(
            format!("ws://{}", listener.local_addr().unwrap()),
            "test-gateway",
        )
        .unwrap()
        .with_operation_deadline(Some(Duration::from_millis(300)))
        .with_queue_limits(16, 128 * 1024);
        let (commands, mut incoming) = mpsc::channel(8);
        let subscriptions = Arc::new(Mutex::new(Vec::new()));
        let captured = subscriptions.clone();
        let task = tokio::spawn(async move {
            for _ in 0..16 {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(socket).await.unwrap();
                let request = socket.next().await.unwrap().unwrap();
                let auth: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
                assert_eq!(auth["token"], "test-gateway");
                socket
                    .send(Message::Text(
                        json!({"type":"auth_ok","id":auth["id"]}).to_string().into(),
                    ))
                    .await
                    .unwrap();
                loop {
                    let request = tokio::select! {
                        command = incoming.recv() => {
                            match command {
                                Some(Command::Frame(value)) => {
                                    socket.send(Message::Text(value.to_string().into())).await.unwrap();
                                    continue;
                                },
                                Some(Command::Disconnect) | None => break,
                            }
                        },
                        message = socket.next() => {
                            let Some(Ok(message)) = message else { break };
                            if message.is_close() { break }
                            serde_json::from_str::<Value>(message.to_text().unwrap()).unwrap()
                        },
                    };
                    let reply = match request["type"].as_str().unwrap() {
                        "subscribe" => {
                            let channels = request["channels"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|v| v.as_str().unwrap().to_owned())
                                .collect::<Vec<_>>();
                            assert!(channels.iter().all(|c| c != "*"));
                            if !channels.is_empty() {
                                let mut recorded = captured.lock().unwrap();
                                assert!(recorded.len() < 16);
                                recorded.push(channels);
                            }
                            "subscribed"
                        }
                        "join" => "joined",
                        "clear_filter" => "filter_set",
                        other => panic!("unexpected presence operation {other}"),
                    };
                    if socket
                        .send(Message::Text(
                            json!({"type":reply,"id":request["id"]}).to_string().into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            config,
            commands,
            subscriptions,
            task,
        }
    }

    async fn send(&self, signed: &Signed) {
        self.commands
            .send(Command::Frame(json!({
                "type":"sidechannel_message",
                "channel":channel(&signed.body.network, &signed.body.market).unwrap(),
                "message":signed,
            })))
            .await
            .unwrap();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn health_until(health: &mut watch::Receiver<Health>, test: impl Fn(&Health) -> bool) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if test(&health.borrow()) {
                return;
            }
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

fn current_body(f: &Fixture) -> Body {
    let mut body = f.body();
    let now = now();
    body.issued_ms = now;
    body.expires_ms = now + 14_000;
    body.evidence_expires_ms = now + 13_000;
    let speed = body.speed.as_mut().unwrap();
    speed.observed_ms = now;
    speed.expires_ms = now + 12_000;
    body
}

#[tokio::test]
async fn reconnect_preserves_withdrawal_and_reselection_is_explicit_and_fail_closed() {
    let mut f = Fixture::new();
    f.refresh(now());
    let bridge = TestBridge::start().await;
    let table = Arc::new(f.table(8));
    let (gateway, supervisor) =
        Gateway::new(bridge.config.clone(), f.catalog.clone(), table, 1).unwrap();
    let market = Digest::new(&f.offer.market_id).unwrap();
    let provider = Digest::new(&f.offer.provider_pubkey).unwrap();
    let slot = Digest::new(f.offer.slot_id().unwrap()).unwrap();
    let status = || gateway.status(&market, &provider, &slot, None).unwrap();
    let mut health = gateway.health();
    let (stop, stopping) = watch::channel(false);
    let task = tokio::spawn(supervisor.run(stopping));
    health_until(&mut health, |h| h.running).await;
    assert_eq!(health.borrow().attempts, 0);
    gateway.select(vec![market.clone()]).unwrap();
    health_until(&mut health, |h| h.connected).await;
    let ready = f.sign(current_body(&f));
    bridge.send(&ready).await;
    health_until(&mut health, |h| h.accepted == 1).await;
    assert_eq!(status(), Eligibility::Available);
    assert_eq!(
        gateway.status(&market, &provider, &slot, Some(11)).unwrap(),
        Eligibility::ThroughputFloor
    );

    // Identical selection and rejected over-quota selection preserve the session.
    let attempts = health.borrow().attempts;
    gateway.select(vec![market.clone()]).unwrap();
    assert!(gateway.select(vec![market.clone(), d(90)]).is_err());
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(health.borrow().attempts, attempts);

    let mut withdrawal = ready.body.clone();
    withdrawal.sequence = 2;
    withdrawal.state = State::Draining;
    withdrawal.free_slots = 0;
    bridge.send(&f.sign(withdrawal)).await;
    health_until(&mut health, |h| h.accepted == 2).await;
    bridge.commands.send(Command::Disconnect).await.unwrap();
    health_until(&mut health, |h| h.receiver_failures == 1).await;
    assert_eq!(status(), Eligibility::Draining);
    health_until(&mut health, |h| h.connected && h.attempts == 2).await;
    bridge.send(&ready).await;
    health_until(&mut health, |h| h.rejected == 1).await;
    assert_eq!(status(), Eligibility::Draining);

    let other = d(90);
    gateway.select(vec![other.clone()]).unwrap();
    assert_eq!(status(), Eligibility::HeartbeatMissing);
    health_until(&mut health, |h| h.connected && h.attempts == 3).await;
    // Even correctly signed old-market traffic is rejected by the new receiver.
    bridge.send(&ready).await;
    health_until(&mut health, |h| h.rejected == 2).await;
    gateway.select(vec![market.clone()]).unwrap();
    health_until(&mut health, |h| h.connected && h.attempts == 4).await;
    bridge.send(&ready).await;
    health_until(&mut health, |h| h.rejected == 3).await;
    assert_eq!(status(), Eligibility::Draining);
    let subscriptions = bridge.subscriptions.lock().unwrap().clone();
    assert_eq!(
        subscriptions,
        vec![
            vec![channel(&f.network, &market).unwrap()],
            vec![channel(&f.network, &market).unwrap()],
            vec![channel(&f.network, &other).unwrap()],
            vec![channel(&f.network, &market).unwrap()],
        ]
    );

    // Canonical revocation takes effect without a new heartbeat or caller-held
    // Registered object; both discovery and routing use this same status method.
    let mut renewed = current_body(&f);
    renewed.sequence = 3;
    bridge.send(&f.sign(renewed)).await;
    health_until(&mut health, |h| h.accepted == 3).await;
    assert_eq!(status(), Eligibility::Available);
    f.row(
        format!("provider_status/{}", provider.as_str()),
        json!({"revision":1,"reason_hash":d(91)}),
    );
    f.refresh(now());
    assert_eq!(status(), Eligibility::CatalogUnavailable);
    gateway.select(vec![]).unwrap();
    health_until(&mut health, |h| h.selected_markets == 0 && !h.connected).await;
    assert_eq!(health.borrow().attempts, 4);
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!health.borrow().running);
    assert_eq!(status(), Eligibility::HeartbeatMissing);
}

#[tokio::test]
async fn unreachable_bridge_retries_with_backoff_and_stop_sender_loss_cancels_wait() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let config = ScBridgeConfig::new(format!("ws://{address}"), "test-gateway")
        .unwrap()
        .with_operation_deadline(Some(Duration::from_millis(300)));
    let (gateway, supervisor) =
        Gateway::new(config, f.catalog.clone(), Arc::new(f.table(2)), 1).unwrap();
    gateway.select(vec![d(90)]).unwrap();
    let mut health = gateway.health();
    let (stop, stopping) = watch::channel(false);
    let task = tokio::spawn(supervisor.run(stopping));
    health_until(&mut health, |h| h.receiver_failures == 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(health.borrow().attempts, 1);
    health_until(&mut health, |h| h.receiver_failures == 2).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(health.borrow().attempts, 2);
    drop(stop);
    tokio::time::timeout(Duration::from_millis(200), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!health.borrow().running);
    assert!(!health.borrow().connected);
}

#[tokio::test]
async fn stale_catalog_is_unavailable_and_closing_all_control_handles_stops_receiver() {
    let f = Fixture::new(); // Deliberately old complete catalog snapshot.
    let bridge = TestBridge::start().await;
    let (gateway, supervisor) = Gateway::new(
        bridge.config.clone(),
        f.catalog.clone(),
        Arc::new(f.table(2)),
        1,
    )
    .unwrap();
    let market = Digest::new(&f.offer.market_id).unwrap();
    gateway.select(vec![market.clone()]).unwrap();
    let mut health = gateway.health();
    let (_stop, stopping) = watch::channel(false);
    let task = tokio::spawn(supervisor.run(stopping));
    health_until(&mut health, |h| h.connected).await;
    assert_eq!(
        gateway
            .status(
                &market,
                &Digest::new(&f.offer.provider_pubkey).unwrap(),
                &Digest::new(f.offer.slot_id().unwrap()).unwrap(),
                None
            )
            .unwrap(),
        Eligibility::CatalogUnavailable
    );
    drop(gateway);
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!health.borrow().running);
}
