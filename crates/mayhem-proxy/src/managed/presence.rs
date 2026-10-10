//! Independent control task: explicit-key canonical reads and coalesced signed
//! heartbeats. No inference POSTs, ledger writes, history scans or per-token work.
use super::*;
use crate::presence::{self, Publisher, Signed};
use std::{collections::BTreeSet, time::Instant};
use tokio::task::JoinSet;
pub(super) struct Entry {
    pub route: Digest,
    pub monitor: Monitor,
    pub query: financial::offer::Query,
}
#[derive(Clone, Default, Serialize)]
pub struct Health {
    pub connected: bool,
    pub published: u64,
    pub canonical_failures: u64,
    pub reconnects: u64,
}
pub(super) struct Runner {
    pub entries: Vec<Entry>,
    pub financial: Arc<financial::Client>,
    pub publisher: Arc<std::sync::Mutex<Publisher>>,
    pub bridge: mayhem_bridge::ScBridgeConfig,
    pub network: crate::discovery::Identity,
    pub concurrency: usize,
    pub health_ttl_ms: u64,
}
impl Runner {
    pub async fn run(
        self,
        mut stop: watch::Receiver<bool>,
        updates: watch::Sender<Health>,
    ) -> Result<()> {
        let mut delay = Duration::from_millis(500);
        loop {
            if *stop.borrow() {
                return Ok(());
            }
            match self.session(stop.clone(), updates.clone()).await {
                Ok(()) => return Ok(()),
                Err(Error::Transport) => {
                    let mut health = updates.borrow().clone();
                    health.connected = false;
                    health.reconnects = health.reconnects.saturating_add(1);
                    updates.send_replace(health);
                    // Presence failure must not terminate useful paid work.
                    // Reconnect only this control channel; sequence/fence remain
                    // owned by the same publisher throughout reconnection.
                    tokio::select! {_=stopped(&mut stop)=>return Ok(()),_=tokio::time::sleep(delay)=>{}}
                    delay = (delay * 2).min(Duration::from_secs(5));
                }
                Err(error) => return Err(error),
            }
        }
    }
    async fn session(
        &self,
        mut stop: watch::Receiver<bool>,
        updates: watch::Sender<Health>,
    ) -> Result<()> {
        let mut bridge = tokio::select! {
            r=mayhem_bridge::ScBridgeClient::connect(self.bridge.clone())=>r.map_err(|_|Error::Transport)?,
            _=stopped(&mut stop)=>return Ok(()),
        };
        bridge
            .mute_sidechannel_events()
            .await
            .map_err(|_| Error::Transport)?;
        let channels = self
            .entries
            .iter()
            .map(|e| {
                Digest::new(&e.query.offer.market_id)
                    .map_err(|_| Error::Configuration)
                    .and_then(|market| {
                        presence::channel(&self.network, &market).map_err(|_| Error::Configuration)
                    })
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let channels = channels.into_iter().collect::<Vec<_>>();
        for batch in channels.chunks(128) {
            bridge
                .join_many(batch)
                .await
                .map_err(|_| Error::Transport)?;
        }
        let publisher = self.publisher.clone();
        let mut pending: JoinSet<(usize, crate::Result<financial::offer::Observation>)> =
            JoinSet::new();
        let mut active = BTreeSet::new();
        let mut observed: BTreeMap<usize, Arc<financial::offer::Observation>> = BTreeMap::new();
        let mut last: BTreeMap<usize, (Instant, Signed)> = BTreeMap::new();
        let mut due = vec![Instant::now(); self.entries.len()];
        let mut rails = vec![0usize; self.entries.len()];
        let mut queries = self.entries.iter().map(|e| e.query.clone()).collect::<Vec<_>>();
        let mut cursor = 0;
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut health = Health {
            connected: true,
            ..updates.borrow().clone()
        };
        updates.send_replace(health.clone());
        let result = loop {
            tokio::select! {
                biased;
                _=stopped(&mut stop)=>break Ok(()),
                r=pending.join_next(),if !pending.is_empty()=>{
                    let Some(Ok((i,r)))=r else {break Err(Error::Task)};
                    active.remove(&i);
                    due[i]=Instant::now()+Duration::from_secs(5);
                    match r.and_then(|view| { let offer=view.offer()?.clone(); Ok((view,offer)) }) {Ok((view,offer))=>{
                        queries[i].offer=offer;
                        observed.insert(i,Arc::new(view));
                    },Err(_)=>{
                        observed.remove(&i);health.canonical_failures=health.canonical_failures.saturating_add(1);
                        rails[i]=(rails[i]+1)%self.entries[i].query.offer.accepted_rails.len();
                        if rails[i]!=0 {due[i]=Instant::now();}
                    }}
                },
                _=timer.tick()=>{
                    for _ in 0..self.entries.len() {
                        if pending.len()>=self.concurrency {break}
                        let i=cursor;cursor=(cursor+1)%self.entries.len();
                        if active.contains(&i)||Instant::now()<due[i] {continue}
                        active.insert(i);let mut query=queries[i].clone();let client=self.financial.clone();
                        query.rail=query.offer.accepted_rails[rails[i]];
                        pending.spawn(async move{(i,client.current_rate_state(&query).await)});
                    }
                    // Only configured offer slots. Capacity storage and Ed25519
                    // signing run outside the async inference executor.
                    for (i,entry) in self.entries.iter().enumerate() {
                        if *stop.borrow() {break}
                        let canonical=observed.get(&i).cloned();let owner=publisher.clone();
                        let route=entry.route.clone();let monitor=entry.monitor.clone();let ttl=self.health_ttl_ms;
                        let previous=last.get(&i).map(|(_,s)|s.clone());
                        let refresh_due=last.get(&i).is_none_or(|(at,_)|at.elapsed()>=Duration::from_millis(presence::HEARTBEAT_MS));
                        let message=tokio::task::spawn_blocking(move||{
                            let mut p=owner.lock().map_err(|_|crate::invalid("presence signer lock failed"))?;
                            if let Some(canonical)=canonical {
                                if let Ok(s)=p.issue(&canonical,&route,&monitor,ttl,previous.as_ref(),refresh_due) {return Ok(s)}
                            }
                            previous.filter(|s|s.body.state!=presence::State::Unavailable)
                                .map(|s|p.withdraw(&s,false)).transpose()
                        }).await.map_err(|_|Error::Task)? .map_err(|_|Error::Setup)?;
                        let Some(message)=message else {continue};
                        let send=last.get(&i).is_none_or(|(at,old)| at.elapsed()>=Duration::from_millis(presence::HEARTBEAT_MS)
                            || old.body.state!=message.body.state || old.body.reason!=message.body.reason
                            || old.body.offer!=message.body.offer || old.body.membership!=message.body.membership
                            || old.body.free_slots!=message.body.free_slots || old.body.allowance!=message.body.allowance);
                        if send {
                            let channel=presence::channel(&self.network,&message.body.market).map_err(|_|Error::Configuration)?;
                            // Sending can be cancelled at shutdown; it cannot
                            // cancel an upstream generation or release its lease.
                            tokio::select! {
                                r=bridge.send(&channel,&message)=>{r.map_err(|_|Error::Transport)?;},
                                _=stopped(&mut stop)=>break,
                            }
                            last.insert(i,(Instant::now(),message));health.published=health.published.saturating_add(1);
                        }
                    }
                }
            }
            updates.send_replace(health.clone());
        };
        // Best effort with one total transport bound, not N unbounded waits.
        // If disconnected, prior eligibility still expires at its signed bound.
        let withdrawals = async {
            for (_, (_, old)) in last {
                let message = publisher
                    .lock()
                    .map_err(|_| Error::Task)?
                    .withdraw(&old, true)
                    .map_err(|_| Error::Setup)?;
                let channel = presence::channel(&self.network, &message.body.market)
                    .map_err(|_| Error::Configuration)?;
                bridge
                    .send(&channel, &message)
                    .await
                    .map_err(|_| Error::Transport)?;
            }
            Ok::<(), Error>(())
        };
        let _ = tokio::time::timeout(Duration::from_secs(2), withdrawals).await;
        pending.abort_all();
        while pending.join_next().await.is_some() {}
        health.connected = false;
        updates.send_replace(health);
        result
    }
}
