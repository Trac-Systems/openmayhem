//! Buyer-owned hold recovery. This database contains signed financial intentions,
//! never provider execution authority. Expiry does NOT close the provider journal,
//! free its capacity or make an unknown job safe to repeat.
mod approvals;
mod reservations;
pub mod runner;
use super::*;
use crate::attempts;
use approvals::Acknowledgment;
pub use approvals::{SignedAcknowledgment, VerifiedReceipt};
use mayhem_proto::proxy::finance::{ProxyExpiryBody, ProxyHoldExpiry, ProxyReservationExpiry};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition, TableHandle};
use reservations::ReservationIntent;
pub use reservations::ReservationStatus;
use std::{
    ops::Bound,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_buyer_recovery_meta_v1");
const RECORDS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("proxy_buyer_recovery_records_v1");
const PENDING: TableDefinition<&str, u8> = TableDefinition::new("proxy_buyer_recovery_pending_v1");
const CLOSED: TableDefinition<&str, &str> = TableDefinition::new("proxy_buyer_recovery_closed_v1");
const RECORD_BOUND: usize = 64 * 1024;
const PAGE_BOUND: usize = 64;

/// These are financial outcomes only; none is a backend-capacity capability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FinancialOutcome {
    ExpiredUnknown { expiry: ProxyReservationExpiry },
    Paid { receipt: ProxyUsageReceipt },
    Waived { closure: ProxyReservationClosure },
}
impl Observation {
    fn buyer_binding(&self, identity: &attempts::Identity) -> Result<()> {
        let t = &self.accepted().authorization.terms;
        require(
            self.started.elapsed() <= FRESHNESS
                && identity.network_id == t.network_id
                && identity.msb_bootstrap.as_str() == t.msb_bootstrap
                && identity.subnet_bootstrap.as_str() == t.subnet_bootstrap
                && identity.controller_pubkey.as_str() == t.buyer_pubkey
                && self.wire.requester == t.buyer_pubkey,
            "buyer recovery identity or freshness differs",
        )?;
        self.accepted()
            .authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| invalid("accepted signatures rejected"))
    }
    fn financial_outcome(&self) -> Result<Option<FinancialOutcome>> {
        require(
            self.started.elapsed() <= FRESHNESS,
            "financial observation expired",
        )?;
        if !self.wire.resolution.is_null() {
            let closure: ProxyReservationClosure =
                serde_json::from_value(self.wire.resolution["closure"].clone())?;
            require(self.confirms_waiver(&closure)?, "canonical waiver missing")?;
            return Ok(Some(FinancialOutcome::Waived { closure }));
        }
        if let Some(receipt) = self.receipt_head()? {
            if receipt.body.final_receipt {
                require(
                    self.confirms_receipt(&receipt)?,
                    "canonical receipt missing",
                )?;
                return Ok(Some(FinancialOutcome::Paid { receipt }));
            }
        }
        let record = &self.wire.expiry;
        if record.is_null() {
            return Ok(None);
        }
        let expiry: ProxyReservationExpiry = serde_json::from_value(record["expiry"].clone())?;
        let a = self.accepted();
        let t = &a.authorization.terms;
        expiry
            .verify(
                t,
                &a.settlement_policy,
                self.wire.context.epoch,
                crate::receipts::verify_signature,
            )
            .map_err(|_| invalid("canonical expiry rejected"))?;
        let r = &self.wire.reservation;
        let b = &self.wire.billing;
        require(
            record["type"] == "proxy_reservation_expiry"
                && record["accepted_terms"] == a.accepted_terms
                && record["recorded_at"]
                    == format!(
                        "proxy/expire/{}",
                        expiry
                            .body
                            .digest()
                            .map_err(|_| invalid("invalid expiry"))?
                    )
                && record["result"]["ok"] == true
                && record["result"]["op"] == "proxyExpireReservation"
                && record["result"]["execution"] == "unknown"
                && record["result"]["retry_safe"] == false
                && record["result"]["released_au"] == t.max_spend_au.to_string()
                && record["result"]["retained_au"] == "0"
                && r["status"] == "closed"
                && !r["closed_at"].is_null()
                && r["accepted_terms"] == a.accepted_terms
                && r["reservation_id"] == t.reservation_id
                && r["user"] == t.buyer_pubkey
                && r["provider"] == t.offer.provider_pubkey
                && r["rail"] == serde_json::to_value(t.rail)?
                && b["type"] == "proxy_billing_anchor"
                && b["billing_id"] == t.billing_id
                && b["latest_accepted_terms"] == a.accepted_terms
                && b["latest_attempt"] == t.billing_attempt
                && b["user"] == t.buyer_pubkey
                && b["rail"] == serde_json::to_value(t.rail)?
                && b["active_reservation_id"].is_null()
                && b["reserved_au"] == "0"
                && b["spent_au"] == t.prior_spend_au.to_string()
                && b["retry_blocked"] == true,
            "canonical expiry financial state differs",
        )?;
        Ok(Some(FinancialOutcome::ExpiredUnknown { expiry }))
    }
    fn expiry_body(&self, at_ms: u64) -> Result<ProxyExpiryBody> {
        require(
            self.financial_outcome()?.is_none(),
            "financial outcome already resolved",
        )?;
        let a = self.accepted();
        require(
            self.expiry_eligible()?,
            "accepted expiry policy or canonical deadline not satisfied",
        )?;
        self.reserved_binding()?;
        let body = ProxyExpiryBody {
            schema_version: 1,
            lane: mayhem_proto::proxy::ProxyLane::Proxy,
            accepted_terms: a.accepted_terms.clone(),
            observed_epoch: self.wire.context.epoch,
            at_ms,
        };
        body.validate()
            .map_err(|_| invalid("invalid expiry draft"))?;
        Ok(body)
    }
    fn expiry_eligible(&self) -> Result<bool> {
        let a = self.accepted();
        let t = &a.authorization.terms;
        Ok(a.settlement_policy.hold_expiry
            == Some(ProxyHoldExpiry::ReleaseUnfinalizedAndBlockRetry)
            && self.wire.context.epoch
                > t.reservation_expires_after_epoch
                    .checked_add(t.reservation_receipt_grace_epochs)
                    .ok_or_else(|| invalid("invalid expiry deadline"))?)
    }
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub max_records: u64,
    pub closed_retention_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    schema: u32,
    identity: attempts::Identity,
    records: u64,
}
fn decode_meta(bytes: &[u8]) -> Result<Meta> {
    require(bytes.len() <= 4096, "recovery metadata exceeds bound")?;
    Ok(serde_json::from_slice(bytes)?)
}
/// Retained evidence for the authenticated buyer's startup/recovery worker.
/// Reading it does not renew canonical freshness or authorize publication.
pub struct RecoveryStatus {
    pub reservation: Option<ReservationStatus>,
    pub authorization: ProxySpendAuthorization,
    pub policy: ProxySettlementPolicy,
    pub draft: Option<ProxyExpiryBody>,
    pub signed: Option<ProxyReservationExpiry>,
    pub confirmed: Option<FinancialOutcome>,
    pub outcome_approved: bool,
    pub outcome_signed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    authorization: ProxySpendAuthorization,
    policy: ProxySettlementPolicy,
    draft: Option<ProxyExpiryBody>,
    signed: Option<ProxyReservationExpiry>,
    confirmed: Option<FinancialOutcome>,
    proof: Option<Proof>,
    confirmed_epoch: Option<u64>,
    closed_at: Option<u64>,
    prune_after: Option<u64>,
    #[serde(default)]
    acknowledgment: Option<Acknowledgment>,
    #[serde(default)]
    reservation: Option<ReservationIntent>,
}
impl Record {
    fn expired_unadmitted(&self) -> bool {
        self.reservation
            .as_ref()
            .is_some_and(|v| v.unadmitted.is_some())
    }
    fn key(&self) -> Result<String> {
        self.authorization
            .terms
            .digest()
            .map_err(|_| invalid("invalid recovery authorization"))
    }
    fn validate(&self, key: &str, identity: &attempts::Identity) -> Result<()> {
        self.authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| invalid("invalid recovery signatures"))?;
        let t = &self.authorization.terms;
        require(
            self.key()? == key
                && t.network_id == identity.network_id
                && t.msb_bootstrap == identity.msb_bootstrap.as_str()
                && t.subnet_bootstrap == identity.subnet_bootstrap.as_str()
                && t.buyer_pubkey == identity.controller_pubkey.as_str()
                && self
                    .policy
                    .digest()
                    .map_err(|_| invalid("invalid recovery policy"))?
                    == t.settlement_policy_hash,
            "recovery authorization differs",
        )?;
        if let Some(a) = &self.acknowledgment {
            a.validate(&self.authorization, &self.policy)?;
        }
        if let Some(intent) = &self.reservation {
            intent.validate()?;
            if let Some(absent) = &intent.unadmitted {
                absent.validate_for(t, identity.controller_pubkey.as_str())?;
                require(
                    intent.proof.is_none() && self.proof.is_none() && self.confirmed.is_none(),
                    "non-admission conflicts with reserved funds",
                )?;
            }
            require(
                intent.proof.is_some()
                    || (self.acknowledgment.is_none()
                        && self.draft.is_none()
                        && self.signed.is_none()
                        && self.confirmed.is_none()),
                "unconfirmed reservation has a financial outcome intent",
            )?;
        }
        if let Some(d) = &self.draft {
            d.validate().map_err(|_| invalid("invalid saved expiry"))?;
            require(
                d.accepted_terms == key
                    && self.policy.hold_expiry
                        == Some(ProxyHoldExpiry::ReleaseUnfinalizedAndBlockRetry)
                    && d.observed_epoch
                        > t.reservation_expires_after_epoch + t.reservation_receipt_grace_epochs,
                "saved expiry differs",
            )?;
        }
        if let Some(s) = &self.signed {
            require(
                self.draft.as_ref() == Some(&s.body),
                "signed expiry differs from draft",
            )?;
            s.verify(
                t,
                &self.policy,
                s.body.observed_epoch,
                crate::receipts::verify_signature,
            )
            .map_err(|_| invalid("saved expiry signature rejected"))?;
        }
        require(
            (self.confirmed.is_some() || self.expired_unadmitted()) == self.closed_at.is_some()
                && self.confirmed.is_some() == self.proof.is_some()
                && self.confirmed.is_some() == self.confirmed_epoch.is_some()
                && self.closed_at.is_some() == self.prune_after.is_some()
                && self
                    .closed_at
                    .zip(self.prune_after)
                    .is_some_and(|(at, until)| until > at)
                    == (self.confirmed.is_some() || self.expired_unadmitted()),
            "incomplete recovery confirmation",
        )?;
        if let Some(proof) = &self.proof {
            proof.validate()?;
        }
        match &self.confirmed {
            Some(FinancialOutcome::ExpiredUnknown { expiry }) => expiry.verify(
                t,
                &self.policy,
                self.confirmed_epoch.unwrap(),
                crate::receipts::verify_signature,
            ),
            Some(FinancialOutcome::Paid { receipt }) => {
                receipt.verify(t, &self.policy, None, crate::receipts::verify_signature)
            }
            Some(FinancialOutcome::Waived { closure }) => {
                closure.verify(t, crate::receipts::verify_signature)
            }
            None => Ok(()),
        }
        .map_err(|_| invalid("saved financial confirmation rejected"))
    }
}
pub struct Store {
    database: redb::Database,
    identity: attempts::Identity,
    limits: Limits,
    failed: AtomicBool,
    write_lock: Mutex<()>,
}
struct Transaction<'a> {
    tx: redb::WriteTransaction,
    _guard: MutexGuard<'a, ()>,
}
impl std::ops::Deref for Transaction<'_> {
    type Target = redb::WriteTransaction;
    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}
impl Store {
    /// Private file, exclusive owner, fixed per-record byte bound and indexed
    /// pending/pruning. Blocking methods run only through the bounded controller.
    pub fn open(
        path: impl AsRef<Path>,
        identity: attempts::Identity,
        limits: Limits,
    ) -> Result<Self> {
        identity
            .validate()
            .map_err(|_| invalid("invalid recovery identity"))?;
        require(
            limits.max_records > 0 && limits.closed_retention_ms > 0,
            "invalid recovery limits",
        )?;
        let file = attempts::private_file(path.as_ref())
            .map_err(|_| invalid("recovery needs a private regular file"))?;
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
        let previous = crate::db(meta.get("state"))?
            .map(|v| decode_meta(v.value()))
            .transpose()?;
        if let Some(m) = previous {
            require(
                m.schema == 1 && m.identity == identity,
                "buyer recovery store belongs to another identity",
            )?;
            require(
                [RECORDS.name(), PENDING.name(), CLOSED.name()]
                    .iter()
                    .all(|n| names.iter().any(|v| v == n)),
                "incomplete recovery store",
            )?;
            require(
                crate::db(crate::db(tx.open_table(RECORDS))?.len())? == m.records
                    && crate::db(crate::db(tx.open_table(PENDING))?.len())?
                        .checked_add(crate::db(crate::db(tx.open_table(CLOSED))?.len())?)
                        == Some(m.records),
                "recovery index count differs",
            )?;
        } else {
            require(names.is_empty(), "unknown recovery store")?;
            crate::db(tx.open_table(RECORDS))?;
            crate::db(tx.open_table(PENDING))?;
            crate::db(tx.open_table(CLOSED))?;
            let bytes = serde_json::to_vec(&Meta {
                schema: 1,
                identity: identity.clone(),
                records: 0,
            })?;
            crate::db(meta.insert("state", bytes.as_slice()))?;
        }
        drop(meta);
        crate::db(tx.commit())?;
        Ok(Self {
            database,
            identity,
            limits,
            failed: AtomicBool::new(false),
            write_lock: Mutex::new(()),
        })
    }
    fn transaction(&self) -> Result<Transaction<'_>> {
        let guard = self
            .write_lock
            .lock()
            .map_err(|_| invalid("recovery writer failed"))?;
        require(
            !self.failed.load(Ordering::Acquire),
            "recovery storage requires reopen after failed commit",
        )?;
        let mut tx = crate::db(self.database.begin_write())?;
        require(
            !self.failed.load(Ordering::Acquire),
            "recovery storage requires reopen after failed commit",
        )?;
        crate::db(tx.set_durability(redb::Durability::Immediate))?;
        Ok(Transaction { tx, _guard: guard })
    }
    fn commit(&self, transaction: Transaction<'_>) -> Result<()> {
        let Transaction { tx, _guard } = transaction;
        if let Err(e) = tx.commit() {
            self.failed.store(true, Ordering::Release);
            return crate::db(Err::<(), _>(e));
        }
        Ok(())
    }
    fn decode(&self, key: &str, bytes: &[u8]) -> Result<Record> {
        require(bytes.len() <= RECORD_BOUND, "recovery record exceeds bound")?;
        let r: Record = serde_json::from_slice(bytes)?;
        r.validate(key, &self.identity)?;
        Ok(r)
    }
    fn get(&self, key: &str) -> Result<Record> {
        Digest::new(key).map_err(|_| invalid("invalid recovery key"))?;
        let tx = crate::db(self.database.begin_read())?;
        let table = crate::db(tx.open_table(RECORDS))?;
        let v = crate::db(table.get(key))?.ok_or_else(|| invalid("recovery record not found"))?;
        self.decode(key, v.value())
    }
    fn save(&self, table: &mut redb::Table<&str, &[u8]>, key: &str, r: &Record) -> Result<()> {
        r.validate(key, &self.identity)?;
        let bytes = serde_json::to_vec(r)?;
        require(bytes.len() <= RECORD_BOUND, "recovery record exceeds bound")?;
        crate::db(table.insert(key, bytes.as_slice()))?;
        Ok(())
    }
    fn observe(&self, o: &Observation, prepare: bool, at_ms: u64) -> Result<Record> {
        o.buyer_binding(&self.identity)?;
        let key = &o.accepted().accepted_terms;
        let tx = self.transaction()?;
        // Recheck after waiting for another writer. Old evidence cannot start a new intent.
        o.buyer_binding(&self.identity)?;
        let outcome = o.financial_outcome()?;
        let mut table = crate::db(tx.open_table(RECORDS))?;
        let existing = crate::db(table.get(key.as_str()))?
            .map(|v| self.decode(key, v.value()))
            .transpose()?;
        let mut r = existing.clone().unwrap_or_else(|| Record {
            authorization: o.accepted().authorization.clone(),
            policy: o.accepted().settlement_policy.clone(),
            draft: None,
            signed: None,
            confirmed: None,
            proof: None,
            confirmed_epoch: None,
            closed_at: None,
            prune_after: None,
            acknowledgment: None,
            reservation: None,
        });
        require(
            !r.expired_unadmitted()
                && r.authorization == o.accepted().authorization
                && r.policy == o.accepted().settlement_policy,
            "original recovery terms changed",
        )?;
        let newly_accepted = r.reservation.as_ref().is_some_and(|v| v.proof.is_none());
        if let Some(intent) = &mut r.reservation {
            if intent.proof.is_none() {
                intent.proof = Some(o.proof().clone());
            }
        }
        if let Some(old) = &r.confirmed {
            if outcome.as_ref() == Some(old) {
                return Ok(r);
            }
            require(
                matches!(
                    (old, &outcome),
                    (
                        FinancialOutcome::ExpiredUnknown { .. },
                        Some(FinancialOutcome::Waived { .. })
                    )
                ),
                "canonical recovery outcome regressed or conflicts",
            )?;
        } else if existing.is_some()
            && !newly_accepted
            && outcome.is_none()
            && (!prepare || r.draft.is_some())
        {
            return Ok(r); // Polling an unchanged hold performs no write/fsync.
        }
        if let Some(outcome) = outcome {
            r.confirmed = Some(outcome);
            r.proof = Some(o.proof().clone());
            r.confirmed_epoch = Some(o.wire.context.epoch);
            r.closed_at.get_or_insert(at_ms);
            if r.prune_after.is_none() {
                r.prune_after = Some(
                    at_ms
                        .checked_add(self.limits.closed_retention_ms)
                        .ok_or_else(|| invalid("recovery retention overflow"))?,
                );
            }
        } else if prepare && r.draft.is_none() {
            r.draft = Some(o.expiry_body(at_ms)?);
        }
        if existing.is_none() {
            let mut mt = crate::db(tx.open_table(META))?;
            let mut m = decode_meta(
                crate::db(mt.get("state"))?
                    .ok_or_else(|| invalid("missing recovery metadata"))?
                    .value(),
            )?;
            require(
                m.records < self.limits.max_records,
                "buyer recovery store is full",
            )?;
            m.records += 1;
            crate::db(mt.insert("state", serde_json::to_vec(&m)?.as_slice()))?;
        }
        if let Some(expiry) = r.prune_after {
            crate::db(crate::db(tx.open_table(PENDING))?.remove(key.as_str()))?;
            crate::db(
                crate::db(tx.open_table(CLOSED))?
                    .insert(format!("{expiry:020}/{key}").as_str(), key.as_str()),
            )?;
        } else {
            crate::db(crate::db(tx.open_table(PENDING))?.insert(key.as_str(), 0))?;
        }
        self.save(&mut table, key, &r)?;
        drop(table);
        self.commit(tx)?;
        Ok(r)
    }
    fn retain(&self, key: &str, signed: ProxyReservationExpiry) -> Result<()> {
        let tx = self.transaction()?;
        let mut table = crate::db(tx.open_table(RECORDS))?;
        let mut r = self.decode(
            key,
            crate::db(table.get(key))?
                .ok_or_else(|| invalid("recovery record not found"))?
                .value(),
        )?;
        require(
            r.draft.as_ref() == Some(&signed.body),
            "expiry signature changed the saved intent",
        )?;
        if let Some(s) = &r.signed {
            return require(s == &signed, "saved expiry signature changed");
        }
        require(r.confirmed.is_none(), "financial outcome already resolved")?;
        r.signed = Some(signed);
        self.save(&mut table, key, &r)?;
        drop(table);
        self.commit(tx)
    }
    fn pending(&self, after: Option<Digest>, limit: usize) -> Result<Vec<Digest>> {
        require(
            (1..=PAGE_BOUND).contains(&limit),
            "invalid recovery page size",
        )?;
        let tx = crate::db(self.database.begin_read())?;
        let table = crate::db(tx.open_table(PENDING))?;
        let lower = after
            .as_ref()
            .map_or(Bound::Unbounded, |v| Bound::Excluded(v.as_str()));
        crate::db(table.range::<&str>((lower, Bound::Unbounded)))?
            .take(limit)
            .map(|v| {
                let (k, _) = crate::db(v)?;
                Digest::new(k.value()).map_err(|_| invalid("invalid recovery index"))
            })
            .collect()
    }
    fn prune(&self, now: u64, limit: usize) -> Result<usize> {
        require(
            (1..=PAGE_BOUND).contains(&limit),
            "invalid recovery prune size",
        )?;
        let tx = self.transaction()?;
        let mut closed = crate::db(tx.open_table(CLOSED))?;
        let upper = format!("{now:020}/~");
        let keys = crate::db(closed.range::<&str>(..=upper.as_str()))?
            .take(limit)
            .map(|v| {
                let (k, v) = crate::db(v)?;
                Ok((k.value().to_owned(), v.value().to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut records = crate::db(tx.open_table(RECORDS))?;
        for (index, key) in &keys {
            let r = self.decode(
                key,
                crate::db(records.get(key.as_str()))?
                    .ok_or_else(|| invalid("missing recovery record"))?
                    .value(),
            )?;
            require(
                (r.confirmed.is_some() || r.expired_unadmitted())
                    && r.prune_after.is_some_and(|deadline| {
                        deadline <= now && index == &format!("{deadline:020}/{key}")
                    })
                    && crate::db(crate::db(tx.open_table(PENDING))?.get(key.as_str()))?.is_none(),
                "invalid recovery prune index",
            )?;
            crate::db(records.remove(key.as_str()))?;
            crate::db(closed.remove(index.as_str()))?;
        }
        let mut mt = crate::db(tx.open_table(META))?;
        let mut m = decode_meta(
            crate::db(mt.get("state"))?
                .ok_or_else(|| invalid("missing recovery metadata"))?
                .value(),
        )?;
        m.records = m
            .records
            .checked_sub(keys.len() as u64)
            .ok_or_else(|| invalid("recovery count underflow"))?;
        crate::db(mt.insert("state", serde_json::to_vec(&m)?.as_slice()))?;
        drop(mt);
        drop(records);
        drop(closed);
        self.commit(tx)?;
        Ok(keys.len())
    }
}

/// Callers authenticate the buyer before selecting this controller. No method
/// exposes signing keys, dispatches inference or changes a provider capacity lease.
pub struct BuyerRecovery {
    store: Arc<Store>,
    client: Arc<Client>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl BuyerRecovery {
    pub fn new(store: Arc<Store>, client: Arc<Client>, storage_concurrency: usize) -> Result<Self> {
        require(
            (1..=64).contains(&storage_concurrency)
                && client.requester() == store.identity.controller_pubkey.as_str()
                && client.identity().network_id == store.identity.network_id
                && client.identity().msb_bootstrap == store.identity.msb_bootstrap.as_str()
                && client.identity().subnet_bootstrap == store.identity.subnet_bootstrap.as_str(),
            "buyer recovery controller identity differs",
        )?;
        Ok(Self {
            store,
            client,
            slots: Arc::new(tokio::sync::Semaphore::new(storage_concurrency)),
        })
    }
    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("buyer recovery storage busy"))?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f(&store)
        })
        .await
        .map_err(|_| Error::Task)?
    }
    /// Track one accepted hold using canonical evidence; also reconcile a race
    /// won by a receipt, waiver, or another authorized buyer expiry controller.
    pub async fn refresh(
        &self,
        authorization: &ProxySpendAuthorization,
        at_ms: u64,
    ) -> Result<Option<FinancialOutcome>> {
        let o = self.client.observe(authorization).await?;
        self.run(move |s| Ok(s.observe(&o, false, at_ms)?.confirmed))
            .await
    }
    /// Adopt already verified canonical history during negotiation recovery,
    /// including accepted terms from an earlier contract version. No new append.
    pub(crate) async fn retain_observation(
        &self,
        observation: Observation,
        at_ms: u64,
    ) -> Result<Observation> {
        self.run(move |s| {
            s.observe(&observation, false, at_ms)?;
            Ok(observation)
        })
        .await
    }
    pub async fn prepare_expiry(
        &self,
        authorization: &ProxySpendAuthorization,
        at_ms: u64,
    ) -> Result<ProxyExpiryBody> {
        let o = self.client.observe(authorization).await?;
        let r = self.run(move |s| s.observe(&o, true, at_ms)).await?;
        require(r.confirmed.is_none(), "financial outcome already resolved")?;
        r.draft.ok_or_else(|| invalid("expiry draft missing"))
    }
    /// Sign the saved original intent and retain it durably before returning.
    /// The wallet stays in the trusted parent; no signing request enters RPC/IPC.
    pub async fn sign_expiry(
        &self,
        signer: &crate::signing::Authority,
        key: Digest,
    ) -> Result<ProxyReservationExpiry> {
        let k = key.clone();
        let saved = self.run(move |s| s.get(k.as_str())).await?;
        require(
            saved.confirmed.is_none(),
            "financial outcome already resolved",
        )?;
        let body = saved.draft.ok_or_else(|| invalid("expiry draft missing"))?;
        let signed = signer.buyer_expiry(&saved.authorization, &saved.policy, body)?;
        self.retain_expiry(key, signed.clone()).await?;
        Ok(signed)
    }
    pub async fn retain_expiry(&self, key: Digest, expiry: ProxyReservationExpiry) -> Result<()> {
        self.run(move |s| s.retain(key.as_str(), expiry)).await
    }
    /// Always recover the same retained intent. ACK is not confirmation. Returning
    /// ExpiredUnknown releases only finance, with retry still blocked on the ledger.
    pub async fn publish_expiry(
        &self,
        key: Digest,
        at_ms: u64,
    ) -> Result<Option<FinancialOutcome>> {
        let k = key.clone();
        let r = self.run(move |s| s.get(k.as_str())).await?;
        let o = self.client.observe(&r.authorization).await?;
        let (fresh, observed) = self
            .run(move |s| Ok((s.observe(&o, false, at_ms)?, o)))
            .await?;
        if fresh.confirmed.is_some() {
            return Ok(fresh.confirmed);
        }
        let signed = r
            .signed
            .ok_or_else(|| invalid("expiry signature missing"))?;
        let submission = self.client.submit_expiry(&observed, &signed).await;
        let o = self.client.observe(&r.authorization).await?;
        let fresh = self.run(move |s| s.observe(&o, false, at_ms)).await?;
        if fresh.confirmed.is_some() {
            return Ok(fresh.confirmed);
        }
        submission?;
        Ok(None)
    }
    pub async fn pending(&self, after: Option<Digest>, limit: usize) -> Result<Vec<Digest>> {
        self.run(move |s| s.pending(after, limit)).await
    }
    pub async fn recover(&self, key: Digest) -> Result<RecoveryStatus> {
        self.run(move |s| {
            let r = s.get(key.as_str())?;
            let outcome_approved = r.acknowledgment.is_some();
            let outcome_signed = r
                .acknowledgment
                .as_ref()
                .is_some_and(|a| a.buyer_sig.is_some());
            Ok(RecoveryStatus {
                reservation: r.reservation.as_ref().map(ReservationIntent::status),
                outcome_approved,
                outcome_signed,
                authorization: r.authorization,
                policy: r.policy,
                draft: r.draft,
                signed: r.signed,
                confirmed: r.confirmed,
            })
        })
        .await
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
    fn expiry_storage_commit_failure_fences_all_following_writers() {
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
                    closed_retention_ms: 1000,
                },
            )
            .unwrap();
            let tx = store.transaction().unwrap();
            {
                let mut meta = tx.open_table(META).unwrap();
                meta.insert("test-durable-intent", b"test".as_slice())
                    .unwrap();
            }
            control.store(mode, Ordering::Release);
            assert!(store.commit(tx).is_err());
            control.store(0, Ordering::Release);
            assert!(store.failed.load(Ordering::Acquire));
            assert!(
                store.transaction().is_err(),
                "a new intent cannot follow uncertain persistence"
            );
            assert!(store.prune(99999, 64).is_err());
        }
    }
}
