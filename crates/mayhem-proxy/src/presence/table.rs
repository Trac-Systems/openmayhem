//! One durable anti-replay watermark per admitted offer, never per token or
//! request. Restart deliberately restores NO live availability. Watermarks cannot
//! be aged out: an older controller may continue signing new wall-clock times.
use super::*;
use crate::db;
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use std::{collections::BTreeMap, path::Path, sync::Mutex};
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_presence_identity_v1");
const HIGH: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_presence_watermark_v1");
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mark {
    membership: u64,
    fence: u64,
    boot: Digest,
    sequence: u64,
    conflict: bool,
}
struct Live {
    signed: Signed,
    received_ms: u64,
    received: Instant,
}
pub struct Table {
    database: Database,
    network: discovery::Identity,
    max_routes: u64,
    live: Mutex<BTreeMap<String, Live>>,
}
impl Table {
    /// max_routes is a local control-memory/disk quota, not a public catalog page
    /// limit. Exhaustion rejects new telemetry, never evicts anti-replay evidence.
    pub fn open(path: &Path, network: discovery::Identity, max_routes: u64) -> Result<Self> {
        network.validate()?;
        require(max_routes > 0, "presence storage quota is required")?;
        let file = crate::attempts::private_file(path)
            .map_err(|_| invalid("presence requires protected storage"))?;
        let mut builder = Database::builder();
        builder.set_cache_size(8 * 1024 * 1024);
        let database = db(builder.create_file(file))?;
        let mut tx = db(database.begin_write())?;
        db(tx.set_durability(Durability::Immediate))?;
        {
            let mut meta = db(tx.open_table(META))?;
            let stored = db(meta.get("network"))?
                .map(|v| serde_json::from_slice::<discovery::Identity>(v.value()))
                .transpose()?;
            if let Some(value) = stored {
                require(
                    value == network,
                    "presence store belongs to another network",
                )?;
            } else {
                db(meta.insert("network", serde_json::to_vec(&network)?.as_slice()))?;
            }
            require(
                db(db(tx.open_table(HIGH))?.len())? <= max_routes,
                "presence store exceeds configured quota",
            )?;
        }
        db(tx.commit())?;
        Ok(Self {
            database,
            network,
            max_routes,
            live: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn network(&self) -> &discovery::Identity {
        &self.network
    }
    pub fn receive(&self, signed: Signed, registered: &Registered, now_ms: u64) -> Result<()> {
        let received = Instant::now();
        signed.verify(&self.network, now_ms)?;
        registered.check(&signed.body, now_ms)?;
        let body = &signed.body;
        let key = body.key();
        let mut live = self
            .live
            .lock()
            .map_err(|_| invalid("presence receiver lock failed"))?;
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(Durability::Immediate))?;
        let mut conflict = false;
        {
            let mut table = db(tx.open_table(HIGH))?;
            let prior = db(table.get(key.as_str()))?
                .map(|v| serde_json::from_slice::<Mark>(v.value()))
                .transpose()?;
            if let Some(prior) = prior {
                require(
                    body.membership_revision >= prior.membership,
                    "old proxy membership presence",
                )?;
                if body.membership_revision == prior.membership {
                    require(body.fence >= prior.fence, "retired proxy controller")?;
                    if body.fence == prior.fence {
                        require(
                            !prior.conflict,
                            "proxy controller conflict requires a new fence",
                        )?;
                        conflict = body.boot != prior.boot;
                        if !conflict {
                            require(body.sequence > prior.sequence, "replayed proxy presence")?;
                        }
                    }
                }
            } else {
                require(
                    db(table.len())? < self.max_routes,
                    "presence storage quota exhausted",
                )?;
            }
            let mark = Mark {
                membership: body.membership_revision,
                fence: body.fence,
                boot: body.boot.clone(),
                sequence: body.sequence,
                conflict,
            };
            db(table.insert(key.as_str(), serde_json::to_vec(&mark)?.as_slice()))?;
        }
        db(tx.commit())?;
        if conflict {
            live.remove(&key);
            return Err(invalid("conflicting proxy control instances"));
        }
        live.insert(
            key,
            Live {
                signed,
                received_ms: now_ms,
                received,
            },
        );
        Ok(())
    }
    /// Used by routing AND catalog/UI. A caller supplies the latest registration,
    /// preventing cached availability from bypassing fee/offer/policy revocation.
    pub fn status(
        &self,
        registered: &Registered,
        now_ms: u64,
        min_tok_s: Option<u32>,
    ) -> Result<Eligibility> {
        if now_ms < registered.observed_ms
            || now_ms >= registered.expires_ms
            || Instant::now() >= registered.deadline
        {
            return Ok(Eligibility::CatalogUnavailable);
        }
        let key = format!(
            "{}/{}/{}",
            registered.offer.market_id,
            registered.offer.provider_pubkey,
            registered.offer.slot_id().map_err(invalid)?
        );
        let live = self
            .live
            .lock()
            .map_err(|_| invalid("presence receiver lock failed"))?;
        if let Some(live) = live.get(&key) {
            let signed = &live.signed;
            if registered.check(&signed.body, now_ms).is_err() {
                return Ok(Eligibility::CatalogUnavailable);
            }
            // A wall-clock correction must never extend a cached lease. Original
            // signed evidence and elapsed monotonic time both constrain it.
            let effective_now = now_ms.max(
                live.received_ms
                    .saturating_add(live.received.elapsed().as_millis() as u64),
            );
            return Ok(eligibility(
                &signed.body,
                &registered.offer,
                effective_now,
                min_tok_s,
            ));
        }
        let tx = db(self.database.begin_read())?;
        let table = db(tx.open_table(HIGH))?;
        if let Some(mark) = db(table.get(key.as_str()))? {
            let mark: Mark = serde_json::from_slice(mark.value())?;
            if mark.conflict && mark.membership == registered.member.revision {
                return Ok(Eligibility::ControllerConflict);
            }
        }
        Ok(Eligibility::HeartbeatMissing)
    }
}
