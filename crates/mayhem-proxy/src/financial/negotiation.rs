//! Buyer-owned durable pre-signing negotiation. No signature leaves this parent
//! until its exact request/terms and signature are committed. No upstream POST.
use super::*;
use crate::{attempts, buyer::Snapshot, signing::Authority};
use quote::{PreparedPurchase, RetainedPurchase};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition, TableHandle};
use std::{
    ops::Bound,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_negotiation_meta_v1");
const RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_negotiation_records_v1");
const PENDING: TableDefinition<&str, u8> = TableDefinition::new("proxy_negotiation_pending_v1");
const CLOSED: TableDefinition<&str, &str> = TableDefinition::new("proxy_negotiation_closed_v1");
const MAX_PAGE: usize = 64;
const MAX_RECORD_BYTES: usize = 300 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuyerOffer {
    pub terms: mayhem_proto::proxy::finance::ProxySpendTerms,
    pub buyer_sig: String,
}
impl BuyerOffer {
    pub fn verify(&self) -> Result<()> {
        let bytes = self.terms.buyer_signing_bytes().map_err(invalid)?;
        require(
            crate::receipts::verify_signature(&self.buyer_sig, &bytes, &self.terms.buyer_pubkey),
            "invalid buyer offer signature",
        )
    }
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub max_records: u64,
    pub max_payload_bytes: u64,
    pub max_record_bytes: usize,
    pub closed_retention_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    schema: u32,
    identity: attempts::Identity,
    records: u64,
    bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    allocated_bytes: u64,
    purchase: RetainedPurchase,
    commitment: Digest,
    buyer_sig: String,
    provider_sig: Option<String>,
    at_ms: u64,
    proof: Option<Proof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unadmitted: Option<intent::RetainedAbsence>,
    closed_at: Option<u64>,
    prune_after: Option<u64>,
}
fn key(terms: &mayhem_proto::proxy::finance::ProxySpendTerms) -> Result<Digest> {
    terms.validate().map_err(invalid)?;
    Ok(Digest::hash(
        "mayhem/proxy/buyer-purchase-slot/v1",
        &[
            terms.network_id.as_bytes(),
            terms.msb_bootstrap.as_bytes(),
            terms.subnet_bootstrap.as_bytes(),
            terms.buyer_pubkey.as_bytes(),
            terms.billing_id.as_bytes(),
            &terms.billing_attempt.to_le_bytes(),
        ],
    ))
}
impl Record {
    fn offer(&self) -> BuyerOffer {
        BuyerOffer {
            terms: self.purchase.terms.clone(),
            buyer_sig: self.buyer_sig.clone(),
        }
    }
    fn authorization(&self) -> Option<ProxySpendAuthorization> {
        self.provider_sig.as_ref().map(|s| ProxySpendAuthorization {
            terms: self.purchase.terms.clone(),
            buyer_sig: self.buyer_sig.clone(),
            provider_sig: s.clone(),
        })
    }
    fn validate(&self, expected: &Digest, identity: &attempts::Identity) -> Result<()> {
        let t = &self.purchase.terms;
        require(
            key(t)? == *expected
                && self.purchase.commitment()? == self.commitment
                && identity.network_id == t.network_id
                && identity.msb_bootstrap.as_str() == t.msb_bootstrap
                && identity.subnet_bootstrap.as_str() == t.subnet_bootstrap
                && identity.controller_pubkey.as_str() == t.buyer_pubkey
                && self.at_ms <= PROXY_MAX_SAFE_INTEGER
                && self.closed_at.is_some() == self.prune_after.is_some(),
            "negotiation record binding differs",
        )?;
        self.offer().verify()?;
        if let Some(auth) = self.authorization() {
            auth.verify(crate::receipts::verify_signature)
                .map_err(invalid)?;
        }
        if let Some(proof) = &self.proof {
            proof.validate()?;
            require(
                self.provider_sig.is_some(),
                "unsigned purchase cannot be confirmed",
            )?;
        }
        if let Some(absence) = &self.unadmitted {
            absence.validate_for(t, identity.controller_pubkey.as_str())?;
            require(
                self.proof.is_none() && self.closed_at.is_some(),
                "invalid non-admission closure",
            )?;
        }
        if let Some(at) = self.closed_at {
            require(
                (self.proof.is_some() || self.unadmitted.is_some())
                    && at >= self.at_ms
                    && self
                        .prune_after
                        .is_some_and(|p| p >= at && p <= PROXY_MAX_SAFE_INTEGER),
                "invalid negotiation closure",
            )?;
        }
        Ok(())
    }
}
/// Recovery contains private owned prompt/adapter evidence; deliberately not
/// Debug or serializable. Only the explicit offer/authorization are wire data.
pub struct SavedPurchase {
    key: Digest,
    record: Record,
}
impl SavedPurchase {
    pub fn key(&self) -> &Digest {
        &self.key
    }
    pub fn offer(&self) -> BuyerOffer {
        self.record.offer()
    }
    pub fn authorization(&self) -> Option<ProxySpendAuthorization> {
        self.record.authorization()
    }
    pub fn policy(&self) -> &ProxySettlementPolicy {
        &self.record.purchase.policy
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.record.purchase.snapshot
    }
    pub fn request(&self) -> &[u8] {
        self.record.purchase.request.as_bytes()
    }
    pub fn confirmed(&self) -> bool {
        self.record.proof.is_some()
    }
    pub fn closed(&self) -> bool {
        self.record.closed_at.is_some()
    }
}

/// Blocking private database methods belong on a bounded storage executor.
/// `BuyerNegotiation` below supplies that executor for production call sites.
pub struct Store {
    database: redb::Database,
    identity: attempts::Identity,
    limits: Limits,
    failed: AtomicBool,
    lock: Mutex<()>,
}
impl Store {
    pub fn open(
        path: impl AsRef<Path>,
        identity: attempts::Identity,
        limits: Limits,
    ) -> Result<Self> {
        identity
            .validate()
            .map_err(|_| invalid("invalid negotiation identity"))?;
        require(
            limits.max_records > 0
                && limits.max_records <= 1_000_000
                && limits.max_record_bytes > 0
                && limits.max_record_bytes <= MAX_RECORD_BYTES
                && limits.max_payload_bytes >= limits.max_record_bytes as u64
                && limits.closed_retention_ms > 0
                && limits.closed_retention_ms <= PROXY_MAX_SAFE_INTEGER,
            "invalid negotiation limits",
        )?;
        let file = attempts::private_file(path.as_ref())
            .map_err(|_| invalid("negotiation needs private supported storage"))?;
        let mut builder = redb::Database::builder();
        builder.set_cache_size(4 * 1024 * 1024);
        let database = crate::db(builder.create_file(file))?;
        Self::initialize(database, identity, limits)
    }
    fn initialize(
        database: redb::Database,
        identity: attempts::Identity,
        limits: Limits,
    ) -> Result<Self> {
        let mut tx = crate::db(database.begin_write())?;
        crate::db(tx.set_durability(redb::Durability::Immediate))?;
        let names = crate::db(tx.list_tables())?
            .map(|t| t.name().to_owned())
            .collect::<Vec<_>>();
        let mut meta = crate::db(tx.open_table(META))?;
        let old = crate::db(meta.get("state"))?
            .map(|v| decode_meta(v.value()))
            .transpose()?;
        if let Some(m) = old {
            require(
                m.schema == 1
                    && m.identity == identity
                    && [RECORDS.name(), PENDING.name(), CLOSED.name()]
                        .iter()
                        .all(|n| names.iter().any(|v| v == n)),
                "negotiation identity or tables differ",
            )?;
            require(
                crate::db(crate::db(tx.open_table(RECORDS))?.len())? == m.records
                    && crate::db(crate::db(tx.open_table(PENDING))?.len())?
                        .checked_add(crate::db(crate::db(tx.open_table(CLOSED))?.len())?)
                        == Some(m.records),
                "negotiation index count differs",
            )?;
        } else {
            require(names.is_empty(), "unknown negotiation store")?;
            crate::db(tx.open_table(RECORDS))?;
            crate::db(tx.open_table(PENDING))?;
            crate::db(tx.open_table(CLOSED))?;
            crate::db(
                meta.insert(
                    "state",
                    serde_json::to_vec(&Meta {
                        schema: 1,
                        identity: identity.clone(),
                        records: 0,
                        bytes: 0,
                    })?
                    .as_slice(),
                ),
            )?;
        }
        drop(meta);
        crate::db(tx.commit())?;
        Ok(Self {
            database,
            identity,
            limits,
            failed: AtomicBool::new(false),
            lock: Mutex::new(()),
        })
    }
    fn write(&self) -> Result<redb::WriteTransaction> {
        require(
            !self.failed.load(Ordering::Acquire),
            "negotiation storage needs recovery after failed commit",
        )?;
        let mut tx = crate::db(self.database.begin_write())?;
        crate::db(tx.set_durability(redb::Durability::Immediate))?;
        Ok(tx)
    }
    fn commit(&self, tx: redb::WriteTransaction) -> Result<()> {
        if let Err(e) = tx.commit() {
            self.failed.store(true, Ordering::Release);
            return crate::db(Err::<(), _>(e));
        }
        Ok(())
    }
    fn decode(&self, k: &Digest, bytes: &[u8]) -> Result<Record> {
        require(
            bytes.len() <= MAX_RECORD_BYTES,
            "negotiation record exceeds bound",
        )?;
        let r: Record = serde_json::from_slice(bytes)?;
        require(
            bytes.len() as u64 <= r.allocated_bytes && r.allocated_bytes <= MAX_RECORD_BYTES as u64,
            "invalid negotiation allocation",
        )?;
        r.validate(k, &self.identity)?;
        Ok(r)
    }
    fn replace(&self, tx: &redb::WriteTransaction, k: &Digest, r: &Record) -> Result<()> {
        r.validate(k, &self.identity)?;
        let bytes = serde_json::to_vec(r)?;
        require(
            bytes.len() <= MAX_RECORD_BYTES,
            "negotiation record exceeds bound",
        )?;
        let mut records = crate::db(tx.open_table(RECORDS))?;
        let previous = crate::db(records.get(k.as_str()))?
            .map(|v| self.decode(k, v.value()).map(|r| r.allocated_bytes))
            .transpose()?;
        require(
            bytes.len() as u64 <= r.allocated_bytes
                && r.allocated_bytes <= MAX_RECORD_BYTES as u64
                && previous.is_none_or(|n| n == r.allocated_bytes),
            "negotiation allocation differs",
        )?;
        let mut meta = metadata(tx)?;
        meta.bytes = meta
            .bytes
            .checked_sub(previous.unwrap_or(0))
            .and_then(|n| n.checked_add(r.allocated_bytes))
            .ok_or_else(|| invalid("negotiation quota overflow"))?;
        if previous.is_none() {
            meta.records = meta
                .records
                .checked_add(1)
                .ok_or_else(|| invalid("negotiation quota overflow"))?;
        }
        if previous.is_none() {
            require(
                meta.records <= self.limits.max_records
                    && meta.bytes <= self.limits.max_payload_bytes
                    && r.allocated_bytes <= self.limits.max_record_bytes as u64,
                "negotiation storage quota exhausted",
            )?;
        }
        crate::db(records.insert(k.as_str(), bytes.as_slice()))?;
        crate::db(
            crate::db(tx.open_table(META))?.insert("state", serde_json::to_vec(&meta)?.as_slice()),
        )?;
        Ok(())
    }
    pub fn recover(&self, k: &Digest) -> Result<Option<SavedPurchase>> {
        // Signatures cannot escape through a concurrent read before durable
        // acknowledgment, including the interval before a failed commit latches.
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        require(
            !self.failed.load(Ordering::Acquire),
            "negotiation storage needs recovery after failed commit",
        )?;
        let tx = crate::db(self.database.begin_read())?;
        let table = crate::db(tx.open_table(RECORDS))?;
        let r = crate::db(table.get(k.as_str()))?
            .map(|v| self.decode(k, v.value()))
            .transpose()?;
        Ok(r.map(|record| SavedPurchase {
            key: k.clone(),
            record,
        }))
    }
    fn sign(
        &self,
        p: PreparedPurchase,
        q: quote::Observation,
        signer: &Authority,
        at_ms: u64,
    ) -> Result<SavedPurchase> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        require(
            signer.identity() == &self.identity && at_ms <= PROXY_MAX_SAFE_INTEGER,
            "negotiation signing identity differs",
        )?;
        q.recheck_purchase(&p)?;
        let purchase = p.retained()?;
        let k = key(&purchase.terms)?;
        let commitment = purchase.commitment()?;
        let tx = self.write()?;
        let old = {
            let table = crate::db(tx.open_table(RECORDS))?;
            let v = crate::db(table.get(k.as_str()))?
                .map(|v| self.decode(&k, v.value()))
                .transpose()?;
            v
        };
        if let Some(record) = old {
            require(
                record.commitment == commitment,
                "logical purchase already has different signed terms; recover it",
            )?;
            return Ok(SavedPurchase { key: k, record });
        }
        let mut record = Record {
            allocated_bytes: 0,
            purchase,
            commitment,
            buyer_sig: signer.buyer_spend(&p)?,
            provider_sig: None,
            at_ms,
            proof: None,
            unadmitted: None,
            closed_at: None,
            prune_after: None,
        };
        // Countersignature/proof/closure must fit even when the store is full.
        // Only these bounded fields can grow; no arbitrary outcome body is added.
        record.allocated_bytes = (serde_json::to_vec(&record)?.len() as u64)
            .checked_add(4096)
            .ok_or_else(|| invalid("negotiation allocation overflow"))?;
        self.replace(&tx, &k, &record)?;
        crate::db(crate::db(tx.open_table(PENDING))?.insert(k.as_str(), 0))?;
        q.recheck_purchase(&p)?;
        self.commit(tx)?; // No signed bytes can leave before this succeeds.
        Ok(SavedPurchase { key: k, record })
    }
    fn countersignature(&self, offer: ProxySpendAuthorization) -> Result<SavedPurchase> {
        offer
            .verify(crate::receipts::verify_signature)
            .map_err(invalid)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        let k = key(&offer.terms)?;
        let tx = self.write()?;
        let mut record = {
            let table = crate::db(tx.open_table(RECORDS))?;
            let value = crate::db(table.get(k.as_str()))?
                .ok_or_else(|| invalid("unknown buyer negotiation"))?;
            self.decode(&k, value.value())?
        };
        require(
            record.unadmitted.is_none()
                && record.purchase.terms == offer.terms
                && record.buyer_sig == offer.buyer_sig
                && record
                    .provider_sig
                    .as_ref()
                    .is_none_or(|s| s == &offer.provider_sig),
            "provider changed the original signed purchase",
        )?;
        record.provider_sig = Some(offer.provider_sig);
        self.replace(&tx, &k, &record)?;
        self.commit(tx)?;
        Ok(SavedPurchase { key: k, record })
    }
    fn observe(&self, observation: Observation, at_ms: u64) -> Result<SavedPurchase> {
        require(
            observation.started.elapsed() <= FRESHNESS && at_ms <= PROXY_MAX_SAFE_INTEGER,
            "stale negotiation observation",
        )?;
        let auth = &observation.accepted().authorization;
        let k = key(&auth.terms)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        let tx = self.write()?;
        let mut record = {
            let table = crate::db(tx.open_table(RECORDS))?;
            let value =
                crate::db(table.get(k.as_str()))?.ok_or_else(|| invalid("unknown negotiation"))?;
            self.decode(&k, value.value())?
        };
        require(
            record.unadmitted.is_none()
                && observation.wire.requester == self.identity.controller_pubkey.as_str()
                && record.authorization().as_ref() == Some(auth)
                && record
                    .proof
                    .as_ref()
                    .is_none_or(|p| observation.proof().follows(p)),
            "canonical negotiation proof differs",
        )?;
        let closed = observation.is_closed()
            || observation
                .receipt_head()?
                .is_some_and(|r| r.body.final_receipt);
        require(
            record.closed_at.is_none() || closed,
            "closed negotiation cannot reopen",
        )?;
        if closed && record.closed_at.is_none() {
            require(at_ms >= record.at_ms, "negotiation clock moved backwards")?;
            let deadline = at_ms
                .checked_add(self.limits.closed_retention_ms)
                .filter(|v| *v <= PROXY_MAX_SAFE_INTEGER)
                .ok_or_else(|| invalid("retention overflow"))?;
            record.closed_at = Some(at_ms);
            record.prune_after = Some(deadline);
            crate::db(crate::db(tx.open_table(PENDING))?.remove(k.as_str()))?;
            crate::db(crate::db(tx.open_table(CLOSED))?.insert(
                format!("{deadline:020}:{}", k.as_str()).as_str(),
                k.as_str(),
            ))?;
        }
        record.proof = Some(observation.proof().clone());
        self.replace(&tx, &k, &record)?;
        require(
            observation.started.elapsed() <= FRESHNESS,
            "negotiation observation expired during storage",
        )?;
        self.commit(tx)?;
        Ok(SavedPurchase { key: k, record })
    }
    fn expire_unadmitted(
        &self,
        k: Digest,
        observation: intent::Observation,
        at_ms: u64,
    ) -> Result<SavedPurchase> {
        observation.fresh()?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        let tx = self.write()?;
        let mut record = {
            let table = crate::db(tx.open_table(RECORDS))?;
            let value =
                crate::db(table.get(k.as_str()))?.ok_or_else(|| invalid("unknown negotiation"))?;
            self.decode(&k, value.value())?
        };
        require(
            record.proof.is_none(),
            "admitted purchase cannot become unadmitted",
        )?;
        let absence =
            observation.absent(&record.offer(), self.identity.controller_pubkey.as_str())?;
        if record.unadmitted.is_some() {
            return Ok(SavedPurchase { key: k, record });
        }
        require(
            record.closed_at.is_none() && at_ms >= record.at_ms && at_ms <= PROXY_MAX_SAFE_INTEGER,
            "invalid non-admission closure time",
        )?;
        let deadline = at_ms
            .checked_add(self.limits.closed_retention_ms)
            .filter(|n| *n <= PROXY_MAX_SAFE_INTEGER)
            .ok_or_else(|| invalid("retention overflow"))?;
        record.unadmitted = Some(absence);
        record.closed_at = Some(at_ms);
        record.prune_after = Some(deadline);
        crate::db(crate::db(tx.open_table(PENDING))?.remove(k.as_str()))?;
        crate::db(crate::db(tx.open_table(CLOSED))?.insert(
            format!("{deadline:020}:{}", k.as_str()).as_str(),
            k.as_str(),
        ))?;
        self.replace(&tx, &k, &record)?;
        observation.fresh()?;
        self.commit(tx)?;
        Ok(SavedPurchase { key: k, record })
    }
    pub fn pending(&self, after: Option<&Digest>, limit: usize) -> Result<Vec<Digest>> {
        require(
            limit > 0 && limit <= MAX_PAGE,
            "invalid negotiation page bound",
        )?;
        let tx = crate::db(self.database.begin_read())?;
        let table = crate::db(tx.open_table(PENDING))?;
        let start = after.map_or(Bound::Unbounded, |v| Bound::Excluded(v.as_str()));
        crate::db(table.range::<&str>((start, Bound::Unbounded)))?
            .take(limit)
            .map(|row| {
                let (k, _) = crate::db(row)?;
                Digest::new(k.value()).map_err(|_| invalid("invalid negotiation index"))
            })
            .collect()
    }
    fn prune(&self, now: u64, limit: usize) -> Result<usize> {
        require(
            limit > 0 && limit <= MAX_PAGE && now <= PROXY_MAX_SAFE_INTEGER,
            "invalid negotiation pruning bound",
        )?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        let tx = self.write()?;
        let mut closed = crate::db(tx.open_table(CLOSED))?;
        let mut removals = Vec::new();
        for entry in crate::db(closed.range(..=format!("{now:020}:z").as_str()))?.take(limit) {
            let (index, k) = crate::db(entry)?;
            removals.push((
                index.value().to_owned(),
                Digest::new(k.value()).map_err(|_| invalid("invalid closed index"))?,
            ));
        }
        let mut records = crate::db(tx.open_table(RECORDS))?;
        let mut meta = metadata(&tx)?;
        for (index, k) in &removals {
            let value = crate::db(records.get(k.as_str()))?
                .ok_or_else(|| invalid("closed negotiation missing"))?;
            let record = self.decode(k, value.value())?;
            require(
                record.prune_after.is_some_and(|at| at <= now)
                    && record
                        .prune_after
                        .map(|at| format!("{at:020}:{}", k.as_str()))
                        .as_ref()
                        == Some(index),
                "closed negotiation index differs",
            )?;
            meta.bytes = meta
                .bytes
                .checked_sub(record.allocated_bytes)
                .ok_or_else(|| invalid("negotiation byte count differs"))?;
            meta.records = meta
                .records
                .checked_sub(1)
                .ok_or_else(|| invalid("negotiation count differs"))?;
            drop(value);
            crate::db(records.remove(k.as_str()))?;
            crate::db(closed.remove(index.as_str()))?;
        }
        crate::db(
            crate::db(tx.open_table(META))?.insert("state", serde_json::to_vec(&meta)?.as_slice()),
        )?;
        drop(records);
        drop(closed);
        self.commit(tx)?;
        Ok(removals.len())
    }
}
fn decode_meta(bytes: &[u8]) -> Result<Meta> {
    require(bytes.len() <= 4096, "negotiation metadata exceeds bound")?;
    Ok(serde_json::from_slice(bytes)?)
}
fn metadata(tx: &redb::WriteTransaction) -> Result<Meta> {
    let table = crate::db(tx.open_table(META))?;
    let value =
        crate::db(table.get("state"))?.ok_or_else(|| invalid("negotiation metadata missing"))?;
    decode_meta(value.value())
}

pub struct BuyerNegotiation {
    store: Arc<Store>,
    client: Arc<Client>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl BuyerNegotiation {
    pub fn new(store: Arc<Store>, client: Arc<Client>, workers: usize) -> Result<Self> {
        let id = client.identity();
        require(
            workers > 0
                && workers <= 64
                && id.network_id == store.identity.network_id
                && id.msb_bootstrap == store.identity.msb_bootstrap.as_str()
                && id.subnet_bootstrap == store.identity.subnet_bootstrap.as_str()
                && client.requester() == store.identity.controller_pubkey.as_str(),
            "negotiation client identity or storage concurrency differs",
        )?;
        Ok(Self {
            store,
            client,
            slots: Arc::new(tokio::sync::Semaphore::new(workers)),
        })
    }
    async fn run<T: Send + 'static>(
        &self,
        body: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("negotiation storage is busy"))?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            body(&store)
        })
        .await
        .map_err(|_| Error::Task)?
    }
    pub async fn sign(
        &self,
        purchase: PreparedPurchase,
        observation: quote::Observation,
        signer: Arc<Authority>,
        at_ms: u64,
    ) -> Result<SavedPurchase> {
        self.run(move |s| s.sign(purchase, observation, &signer, at_ms))
            .await
    }
    pub async fn recover(&self, key: Digest) -> Result<Option<SavedPurchase>> {
        self.run(move |s| s.recover(&key)).await
    }
    /// Direct lookup after a lost signing reply, before creating another quote.
    pub async fn lookup(&self, billing_id: Digest, attempt: u64) -> Result<Option<SavedPurchase>> {
        require(
            attempt > 0 && attempt <= PROXY_MAX_SAFE_INTEGER,
            "invalid purchase attempt",
        )?;
        let id = &self.store.identity;
        let k = Digest::hash(
            "mayhem/proxy/buyer-purchase-slot/v1",
            &[
                id.network_id.as_bytes(),
                id.msb_bootstrap.as_str().as_bytes(),
                id.subnet_bootstrap.as_str().as_bytes(),
                id.controller_pubkey.as_str().as_bytes(),
                billing_id.as_str().as_bytes(),
                &attempt.to_le_bytes(),
            ],
        );
        self.recover(k).await
    }
    pub async fn retain_provider_acceptance(
        &self,
        authorization: ProxySpendAuthorization,
    ) -> Result<SavedPurchase> {
        self.run(move |s| s.countersignature(authorization)).await
    }
    pub async fn publish(
        &self,
        key: Digest,
        recovery: &recovery::BuyerRecovery,
        at_ms: u64,
    ) -> Result<SavedPurchase> {
        let saved = self
            .recover(key)
            .await?
            .ok_or_else(|| invalid("negotiation not found"))?;
        require(
            saved.record.unadmitted.is_none(),
            "intention expired without admission; create a fresh purchase",
        )?;
        let auth = saved
            .authorization()
            .ok_or_else(|| invalid("provider has not accepted purchase"))?;
        let observed =
            if saved.confirmed() || auth.terms.contract_version != mayhem_proto::CONTRACT_VERSION {
                // A previous release may already have admitted this hold. Recover
                // authoritative history; never try to submit a new obsolete contract.
                let observed = self.client.observe(&auth).await?;
                recovery.retain_observation(observed, at_ms).await?
            } else {
                let k = recovery
                    .retain_reservation(auth, saved.policy().clone(), saved.record.at_ms)
                    .await?;
                recovery.publish_reservation(k, at_ms).await?
            };
        self.run(move |s| s.observe(observed, at_ms)).await
    }
    pub async fn refresh(&self, key: Digest, at_ms: u64) -> Result<SavedPurchase> {
        let saved = self
            .recover(key.clone())
            .await?
            .ok_or_else(|| invalid("negotiation not found"))?;
        if saved.record.unadmitted.is_some() {
            return Ok(saved);
        }
        let auth = if saved.confirmed() {
            saved
                .authorization()
                .ok_or_else(|| invalid("confirmed purchase lacks acceptance"))?
        } else {
            let observation = self.client.intent_state(&saved.offer()).await?;
            match observation.status()? {
                intent::Status::Open => return Ok(saved),
                intent::Status::Expired => {
                    return self
                        .run(move |s| s.expire_unadmitted(key, observation, at_ms))
                        .await
                }
                intent::Status::Admitted => {
                    let auth = observation
                        .authorization()?
                        .ok_or_else(|| invalid("admitted purchase lacks authorization"))?
                        .clone();
                    // The canonical copy recovers a lost countersignature ACK.
                    self.retain_provider_acceptance(auth.clone()).await?;
                    auth
                }
            }
        };
        let observed = self.client.observe(&auth).await?;
        self.run(move |s| s.observe(observed, at_ms)).await
    }
    pub async fn pending(&self, after: Option<Digest>, limit: usize) -> Result<Vec<Digest>> {
        self.run(move |s| s.pending(after.as_ref(), limit)).await
    }
    pub async fn prune(&self, now: u64, limit: usize) -> Result<usize> {
        self.run(move |s| s.prune(now, limit)).await
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use redb::{backends::FileBackend, StorageBackend};
    use std::{io, sync::atomic::AtomicU8};
    #[derive(Debug)]
    struct Fault {
        file: FileBackend,
        mode: Arc<AtomicU8>,
    }
    impl StorageBackend for Fault {
        fn len(&self) -> io::Result<u64> {
            self.file.len()
        }
        fn read(&self, n: u64, b: &mut [u8]) -> io::Result<()> {
            self.file.read(n, b)
        }
        fn write(&self, n: u64, b: &[u8]) -> io::Result<()> {
            self.file.write(n, b)
        }
        fn set_len(&self, n: u64) -> io::Result<()> {
            self.file.set_len(n)
        }
        fn sync_data(&self) -> io::Result<()> {
            match self.mode.load(Ordering::Acquire) {
                1 => Err(io::Error::other("injected persistence failure")),
                2 => {
                    self.file.sync_data()?;
                    Err(io::Error::other("injected persistence acknowledgment loss"))
                }
                _ => self.file.sync_data(),
            }
        }
        fn close(&self) -> io::Result<()> {
            self.file.close()
        }
    }
    #[test]
    fn negotiation_commit_failure_fences_following_signatures_and_pruning() {
        for mode in [1, 2] {
            let control = Arc::new(AtomicU8::new(0));
            let database = redb::Database::builder()
                .create_with_backend(Fault {
                    file: FileBackend::new(tempfile::tempfile().unwrap()).unwrap(),
                    mode: control.clone(),
                })
                .unwrap();
            let digest = |n| Digest::new(format!("{n:064x}")).unwrap();
            let store = Store::initialize(
                database,
                attempts::Identity {
                    network_id: "918".into(),
                    msb_bootstrap: digest(1),
                    subnet_bootstrap: digest(2),
                    controller_pubkey: digest(3),
                },
                Limits {
                    max_records: 2,
                    max_payload_bytes: 128 * 1024,
                    max_record_bytes: 64 * 1024,
                    closed_retention_ms: 1000,
                },
            )
            .unwrap();
            let tx = store.write().unwrap();
            {
                let mut meta = tx.open_table(META).unwrap();
                meta.insert("test-intent", b"committed-before-exposure".as_slice())
                    .unwrap();
            }
            control.store(mode, Ordering::Release);
            assert!(store.commit(tx).is_err());
            control.store(0, Ordering::Release);
            assert!(store.failed.load(Ordering::Acquire));
            assert!(store.write().is_err());
            assert!(store.prune(99999, 64).is_err());
            assert!(store.recover(&digest(1)).is_err());
        }
    }
}
