use super::*;
use crate::catalog::Catalog;
use mayhem_bridge::{BridgeError, ScBridgeClient, ScBridgeConfig};
use std::{collections::BTreeSet, time::Duration};
use tokio::sync::watch;

#[derive(Clone, Default, Serialize)]
pub struct ReceiverHealth {
    pub connected: bool,
    pub accepted: u64,
    pub rejected: u64,
}
/// A bounded watcher for the caller's selected markets, on the existing
/// authenticated bridge. Run separately from inference. No global all-market
/// firehose, request-history scan or unbounded diagnostic log. The gateway
/// supervisor owns subscription changes/reconnection and current catalog sync.
pub async fn receive_bridge(
    config: ScBridgeConfig,
    catalog: Arc<Catalog>,
    table: Arc<Table>,
    markets: Vec<Digest>,
    max_markets: usize,
    mut stop: watch::Receiver<bool>,
    updates: watch::Sender<ReceiverHealth>,
) -> Result<()> {
    require(
        max_markets > 0 && markets.len() <= max_markets,
        "presence subscription quota exceeded",
    )?;
    crate::exchange::channel::validate_config(
        &config,
        crate::exchange::Limits {
            max_message_bytes: MAX_BYTES,
        },
    )
    .map_err(|_| invalid("invalid protected presence bridge"))?;
    let network = table.network().clone();
    let channels = markets
        .into_iter()
        .map(|market| channel(&network, &market))
        .collect::<Result<BTreeSet<_>>>()?;
    let mut bridge = tokio::select! {
        r=ScBridgeClient::connect(config)=>r.map_err(|_|invalid("presence receiver connection failed"))?,
        _=stopped(&mut stop)=>return Ok(()),
    };
    bridge
        .mute_sidechannel_events()
        .await
        .map_err(|_| invalid("presence receiver subscription failed"))?;
    bridge
        .clear_sidechannel_filter()
        .await
        .map_err(|_| invalid("presence receiver filter failed"))?;
    let selected = channels.iter().collect::<Vec<_>>();
    for batch in selected.chunks(128) {
        bridge
            .join_many(batch.iter().copied())
            .await
            .map_err(|_| invalid("presence receiver join failed"))?;
        bridge
            .subscribe(batch.iter().copied())
            .await
            .map_err(|_| invalid("presence receiver subscription failed"))?;
    }
    let mut health = ReceiverHealth {
        connected: true,
        ..ReceiverHealth::default()
    };
    updates.send_replace(health.clone());
    let result = loop {
        let event = tokio::select! {
            biased;
            _=stopped(&mut stop)=>break Ok(()),
            r=bridge.next_sidechannel_message(Duration::from_secs(1))=>match r {
                Ok(v)=>v,Err(BridgeError::Timeout)=>continue,Err(_)=>break Err(invalid("presence receiver disconnected")),
            }
        };
        let topic = event
            .get("channel")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_owned();
        if !channels.contains(&topic) {
            health.rejected = health.rejected.saturating_add(1);
            updates.send_replace(health.clone());
            continue;
        }
        let raw = event
            .get("message")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let c = catalog.clone();
        let t = table.clone();
        let n = network.clone();
        // One outstanding disk/signature task: a peer cannot create unbounded
        // blocking tasks by flooding the ephemeral transport.
        let received = tokio::task::spawn_blocking(move || {
            let now = unix_ms()?;
            let signed = Signed::parse(&serde_json::to_vec(&raw)?, &n, now)?;
            require(
                channel(&n, &signed.body.market)? == topic,
                "presence market channel differs",
            )?;
            let registered = Registered::read(
                &c.read()?,
                &signed.body.market,
                &signed.body.provider,
                &signed.body.slot,
                now,
            )?;
            t.receive(signed, &registered, unix_ms()?)
        })
        .await;
        match received {
            Ok(Ok(())) => health.accepted = health.accepted.saturating_add(1),
            Ok(Err(_)) => health.rejected = health.rejected.saturating_add(1),
            Err(_) => break Err(invalid("presence receiver worker failed")),
        }
        updates.send_replace(health.clone());
    };
    health.connected = false;
    updates.send_replace(health);
    result
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}
