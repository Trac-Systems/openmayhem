//! Local API-key budget authority. This is not canonical money or execution
//! authority. Closed rows are retained deduplication fences; capacity exhaustion
//! stops new admissions, never settlement/recovery. No request-history scans.
use super::{GatewayTokenBudgetPeriod as Period, GatewayTokenRecord, MoneyAu};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

const ACCOUNTS: TableDefinition<&str, &[u8]> = TableDefinition::new("gateway_key_accounts_v1");
const RESERVATIONS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("gateway_key_reservations_v1");
const BILLING: TableDefinition<&str, &[u8]> = TableDefinition::new("gateway_native_billing_v1");
const PENDING: TableDefinition<&str, u8> = TableDefinition::new("gateway_key_pending_v1");
const META: TableDefinition<&str, u64> = TableDefinition::new("gateway_key_budget_meta_v1");
const RECORD_BYTES: usize = 8192;
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_tokens: u64,
    pub max_reservations: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Invalid,
    Unavailable,
    Conflict,
    Cap,
    Full,
}
pub(super) type Result<T> = std::result::Result<T, Error>;
fn db<T>(v: std::result::Result<T, impl std::fmt::Debug>) -> Result<T> {
    v.map_err(|_| Error::Unavailable)
}
fn require(v: bool, e: Error) -> Result<()> {
    if v {
        Ok(())
    } else {
        Err(e)
    }
}
fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let b = serde_json::to_vec(v).map_err(|_| Error::Invalid)?;
    require(b.len() <= RECORD_BYTES, Error::Invalid)?;
    Ok(b)
}
fn decode<T: serde::de::DeserializeOwned>(v: &[u8]) -> Result<T> {
    require(v.len() <= RECORD_BYTES, Error::Invalid)?;
    serde_json::from_slice(v).map_err(|_| Error::Invalid)
}
fn id(v: &str) -> Result<()> {
    require(
        !v.is_empty() && v.len() <= 512 && !v.contains('\0'),
        Error::Invalid,
    )
}
fn key(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new_derive_key("mayhem/gateway/key-budget/v1");
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize().to_hex().to_string()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Lane {
    Native,
    Proxy,
}
impl Lane {
    fn tag(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Proxy => "proxy",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    token_hash: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    total: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    period: MoneyAu,
    started: u64,
    budget_period: Option<Period>,
    #[serde(with = "mayhem_proto::decimal_u128")]
    reserved: MoneyAu,
}
impl Account {
    fn initial(t: &GatewayTokenRecord) -> Self {
        Self {
            token_hash: t.token_hash.clone(),
            total: t.spent_total_au,
            period: t.spent_period_au,
            started: t.period_started_at.unwrap_or(t.created_at),
            budget_period: t.budget_period,
            reserved: 0,
        }
    }
    fn roll(&mut self, now: u64) {
        if self
            .budget_period
            .and_then(Period::window_seconds)
            .is_some_and(|w| now.saturating_sub(self.started) >= w)
        {
            self.period = 0;
            self.started = now;
        }
    }
    fn configured(&mut self, t: &GatewayTokenRecord, now: u64) -> Result<()> {
        require(self.token_hash == t.token_hash, Error::Conflict)?;
        self.budget_period = t.budget_period;
        self.roll(now);
        Ok(())
    }
    fn spent(&self) -> MoneyAu {
        if matches!(self.budget_period, Some(Period::Day | Period::Month)) {
            self.period
        } else {
            self.total
        }
    }
    fn project(&self, t: &mut GatewayTokenRecord) {
        t.spent_total_au = self.total;
        t.spent_period_au = self.period;
        t.period_started_at = Some(self.started);
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    token_id: String,
    lane: Lane,
    request_id: String,
    fingerprint: String,
    terms: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    maximum: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    charged: MoneyAu,
    closed: bool,
    released: bool,
    proof: Option<String>,
    sequence: u64,
    billing: Option<String>,
    #[serde(with = "mayhem_proto::decimal_u128")]
    cumulative: MoneyAu,
}
#[derive(Serialize, Deserialize)]
struct Billing {
    #[serde(with = "mayhem_proto::decimal_u128")]
    cumulative: MoneyAu,
}
#[derive(Clone, Debug)]
pub(super) struct Pending {
    pub token_id: String,
    pub lane: Lane,
    pub request_id: String,
    pub fingerprint: String,
    pub terms: String,
    pub maximum: MoneyAu,
    pub charged: MoneyAu,
}
pub(super) struct NativeReceipt<'a> {
    pub buyer: &'a str,
    pub maximum: MoneyAu,
    pub billing_id: &'a str,
    pub prior: MoneyAu,
    pub sequence: u64,
    pub cumulative: MoneyAu,
    pub terminal: bool,
    pub proof: &'a str,
}
pub(super) struct Authority {
    database: redb::Database,
    limits: Limits,
    failed: AtomicBool,
    lock: Mutex<()>,
}
impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyBudgetAuthority")
            .field("failed", &self.failed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}
impl Authority {
    pub(super) fn create(
        path: &Path,
        limits: Limits,
        tokens: &[GatewayTokenRecord],
    ) -> Result<Self> {
        Self::load(path, limits, tokens, true)
    }
    pub(super) fn open(path: &Path, limits: Limits, tokens: &[GatewayTokenRecord]) -> Result<Self> {
        Self::load(path, limits, tokens, false)
    }
    fn load(
        path: &Path,
        limits: Limits,
        tokens: &[GatewayTokenRecord],
        create: bool,
    ) -> Result<Self> {
        require(
            limits.max_tokens > 0
                && limits.max_tokens <= 1_000_000
                && limits.max_reservations > 0
                && limits.max_reservations <= 1_000_000
                && tokens.len() as u64 <= limits.max_tokens,
            Error::Invalid,
        )?;
        let file = private_file(path, create)?;
        require(create || db(file.metadata())?.len() > 0, Error::Invalid)?;
        let mut builder = redb::Database::builder();
        builder.set_cache_size(4 * 1024 * 1024);
        let result = Self {
            database: db(builder.create_file(file))?,
            limits,
            failed: AtomicBool::new(false),
            lock: Mutex::new(()),
        };
        if !create {
            let tx = db(result.database.begin_read())?;
            let meta = db(tx.open_table(META))?;
            require(
                db(meta.get("schema"))?.is_some_and(|v| v.value() == 1),
                Error::Invalid,
            )?;
            db(tx.open_table(ACCOUNTS))?;
            db(tx.open_table(RESERVATIONS))?;
            db(tx.open_table(BILLING))?;
            db(tx.open_table(PENDING))?;
            drop(meta);
            drop(tx);
            return Ok(result);
        }
        result.write(|tx| {
            let mut meta = db(tx.open_table(META))?;
            if let Some(version) = db(meta.get("schema"))? {
                require(version.value() == 1, Error::Invalid)?;
            }
            db(meta.insert("schema", 1))?;
            let mut accounts = db(tx.open_table(ACCOUNTS))?;
            for t in tokens {
                id(&t.token_id)?;
                id(&t.token_hash)?;
                let old = db(accounts.get(t.token_id.as_str()))?
                    .map(|v| decode::<Account>(v.value()))
                    .transpose()?;
                match old {
                    Some(a) => require(a.token_hash == t.token_hash, Error::Conflict)?,
                    None => {
                        require(db(accounts.len())? < limits.max_tokens, Error::Full)?;
                        db(accounts.insert(
                            t.token_id.as_str(),
                            encode(&Account::initial(t))?.as_slice(),
                        ))?;
                    }
                }
            }
            db(tx.open_table(RESERVATIONS))?;
            db(tx.open_table(BILLING))?;
            db(tx.open_table(PENDING))?;
            Ok(())
        })?;
        db(std::fs::File::open(path.parent().ok_or(Error::Invalid)?).and_then(|d| d.sync_all()))?;
        Ok(result)
    }
    fn write<T>(&self, f: impl FnOnce(&redb::WriteTransaction) -> Result<T>) -> Result<T> {
        let _guard = self.lock.lock().map_err(|_| Error::Unavailable)?;
        require(!self.failed.load(Ordering::Acquire), Error::Unavailable)?;
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(redb::Durability::Immediate))?;
        let value = f(&tx)?;
        if tx.commit().is_err() {
            self.failed.store(true, Ordering::Release);
            return Err(Error::Unavailable);
        }
        Ok(value)
    }
    pub(super) fn contains(&self, token_id: &str, lane: Lane, request_id: &str) -> Result<bool> {
        let tx = db(self.database.begin_read())?;
        let table = db(tx.open_table(RESERVATIONS))?;
        Ok(db(table.get(key(&[token_id, lane.tag(), request_id]).as_str()))?.is_some())
    }
    pub(super) fn project(&self, t: &mut GatewayTokenRecord, now: u64) -> Result<()> {
        require(!self.failed.load(Ordering::Acquire), Error::Unavailable)?;
        let tx = db(self.database.begin_read())?;
        let table = db(tx.open_table(ACCOUNTS))?;
        if let Some(a) = db(table.get(t.token_id.as_str()))? {
            let mut a: Account = decode(a.value())?;
            a.configured(t, now)?;
            a.project(t);
        }
        Ok(())
    }
    pub(super) fn sync_config(&self, t: &GatewayTokenRecord, now: u64) -> Result<()> {
        id(&t.token_id)?;
        id(&t.token_hash)?;
        self.write(|tx| {
            let mut table = db(tx.open_table(ACCOUNTS))?;
            let old = db(table.get(t.token_id.as_str()))?
                .map(|v| decode::<Account>(v.value()))
                .transpose()?;
            if old.is_none() {
                require(db(table.len())? < self.limits.max_tokens, Error::Full)?;
            }
            let mut a = old.unwrap_or_else(|| Account::initial(t));
            a.configured(t, now)?;
            db(table.insert(t.token_id.as_str(), encode(&a)?.as_slice()))?;
            Ok(())
        })
    }
    pub(super) fn reserve(
        &self,
        t: &GatewayTokenRecord,
        lane: Lane,
        request_id: &str,
        fingerprint: &str,
        terms: &str,
        maximum: MoneyAu,
        now: u64,
    ) -> Result<()> {
        for v in [&t.token_id, &t.token_hash, request_id, fingerprint, terms] {
            id(v)?;
        }
        require(t.is_active(now), Error::Invalid)?;
        let k = key(&[&t.token_id, lane.tag(), request_id]);
        self.write(|tx| {
            let mut rows = db(tx.open_table(RESERVATIONS))?;
            if let Some(old) = db(rows.get(k.as_str()))? {
                let r: Reservation = decode(old.value())?;
                let accounts = db(tx.open_table(ACCOUNTS))?;
                let account: Account = decode(
                    db(accounts.get(t.token_id.as_str()))?
                        .ok_or(Error::Conflict)?
                        .value(),
                )?;
                require(account.token_hash == t.token_hash, Error::Conflict)?;
                return require(
                    lane == Lane::Proxy
                        && !r.closed
                        && r.fingerprint == fingerprint
                        && r.terms == terms
                        && r.maximum == maximum,
                    Error::Conflict,
                );
            }
            require(db(rows.len())? < self.limits.max_reservations, Error::Full)?;
            let mut accounts = db(tx.open_table(ACCOUNTS))?;
            let existing = db(accounts.get(t.token_id.as_str()))?
                .map(|v| decode::<Account>(v.value()))
                .transpose()?;
            if existing.is_none() {
                require(db(accounts.len())? < self.limits.max_tokens, Error::Full)?;
            }
            let mut a = existing.unwrap_or_else(|| Account::initial(t));
            a.configured(t, now)?;
            let reserved = a.reserved.checked_add(maximum).ok_or(Error::Cap)?;
            let exposure = a.spent().checked_add(reserved).ok_or(Error::Cap)?;
            require(t.budget_au.is_none_or(|cap| exposure <= cap), Error::Cap)?;
            a.reserved = reserved;
            let r = Reservation {
                token_id: t.token_id.clone(),
                lane,
                request_id: request_id.into(),
                fingerprint: fingerprint.into(),
                terms: terms.into(),
                maximum,
                charged: 0,
                closed: false,
                released: false,
                proof: None,
                sequence: 0,
                billing: None,
                cumulative: 0,
            };
            db(accounts.insert(t.token_id.as_str(), encode(&a)?.as_slice()))?;
            db(rows.insert(k.as_str(), encode(&r)?.as_slice()))?;
            db(tx.open_table(PENDING))?
                .insert(k.as_str(), 1)
                .map_err(|_| Error::Unavailable)?;
            Ok(())
        })
    }
    pub(super) fn settle_proxy(
        &self,
        token_id: &str,
        request_id: &str,
        fingerprint: &str,
        terms: &str,
        cumulative: MoneyAu,
        terminal: bool,
        proof: &str,
        now: u64,
    ) -> Result<MoneyAu> {
        id(proof)?;
        self.settle(token_id, Lane::Proxy, request_id, now, |r, _| {
            require(
                r.fingerprint == fingerprint && r.terms == terms,
                Error::Conflict,
            )?;
            if r.closed {
                require(
                    r.charged == cumulative && r.proof.as_deref() == Some(proof) && terminal,
                    Error::Conflict,
                )?;
                return Ok((0, true));
            }
            require(
                cumulative >= r.charged && cumulative <= r.maximum,
                Error::Conflict,
            )?;
            if cumulative == r.charged && r.proof.is_some() && !terminal {
                require(r.proof.as_deref() == Some(proof), Error::Conflict)?;
            }
            let delta = cumulative - r.charged;
            r.proof = Some(proof.into());
            Ok((delta, terminal))
        })
    }
    pub(super) fn settle_native(
        &self,
        token_id: &str,
        request_id: &str,
        value: NativeReceipt<'_>,
        now: u64,
    ) -> Result<MoneyAu> {
        id(value.billing_id)?;
        id(value.proof)?;
        let billing_key = key(&[token_id, value.billing_id]);
        self.settle(token_id, Lane::Native, request_id, now, |r, tx| {
            require(
                r.fingerprint == value.buyer && r.maximum == value.maximum,
                Error::Conflict,
            )?;
            if let Some(old) = &r.billing {
                require(old == value.billing_id, Error::Conflict)?;
            }
            if r.proof.is_some() && value.sequence == r.sequence {
                require(
                    r.proof.as_deref() == Some(value.proof)
                        && r.cumulative == value.cumulative
                        && r.closed == value.terminal,
                    Error::Conflict,
                )?;
                return Ok((0, r.closed));
            }
            require(
                !r.closed && (r.proof.is_none() || value.sequence > r.sequence),
                Error::Conflict,
            )?;
            let mut billings = db(tx.open_table(BILLING))?;
            let previous = db(billings.get(billing_key.as_str()))?
                .map(|v| decode::<Billing>(v.value()))
                .transpose()?
                .map(|b| b.cumulative)
                .unwrap_or(value.prior);
            require(
                value.prior <= previous && value.cumulative >= previous,
                Error::Conflict,
            )?;
            let delta = value.cumulative - previous;
            require(delta <= r.maximum - r.charged, Error::Cap)?;
            r.billing = Some(value.billing_id.into());
            r.sequence = value.sequence;
            r.cumulative = value.cumulative;
            r.proof = Some(value.proof.into());
            db(billings.insert(
                billing_key.as_str(),
                encode(&Billing {
                    cumulative: value.cumulative,
                })?
                .as_slice(),
            ))?;
            Ok((delta, value.terminal))
        })
    }
    fn settle(
        &self,
        token_id: &str,
        lane: Lane,
        request_id: &str,
        now: u64,
        update: impl FnOnce(&mut Reservation, &redb::WriteTransaction) -> Result<(MoneyAu, bool)>,
    ) -> Result<MoneyAu> {
        let k = key(&[token_id, lane.tag(), request_id]);
        self.write(|tx| {
            let mut rows = db(tx.open_table(RESERVATIONS))?;
            let mut r: Reservation =
                decode(db(rows.get(k.as_str()))?.ok_or(Error::Conflict)?.value())?;
            let was_closed = r.closed;
            let remaining = if r.closed || r.released {
                0
            } else {
                r.maximum - r.charged
            };
            let (delta, terminal) = update(&mut r, tx)?;
            require(delta <= r.maximum - r.charged, Error::Cap)?;
            let mut accounts = db(tx.open_table(ACCOUNTS))?;
            let mut a: Account =
                decode(db(accounts.get(token_id))?.ok_or(Error::Conflict)?.value())?;
            a.roll(now);
            a.total = a.total.checked_add(delta).ok_or(Error::Cap)?;
            a.period = a.period.checked_add(delta).ok_or(Error::Cap)?;
            a.reserved = a
                .reserved
                .checked_sub(if terminal || r.released {
                    remaining
                } else {
                    delta.min(remaining)
                })
                .ok_or(Error::Conflict)?;
            r.charged = r.charged.checked_add(delta).ok_or(Error::Cap)?;
            r.closed = terminal || was_closed;
            db(accounts.insert(token_id, encode(&a)?.as_slice()))?;
            db(rows.insert(k.as_str(), encode(&r)?.as_slice()))?;
            if r.closed || r.released {
                db(tx.open_table(PENDING))?
                    .remove(k.as_str())
                    .map_err(|_| Error::Unavailable)?;
            }
            Ok(delta)
        })
    }
    /// Only an owner holding the controller's durable unsigned-intent fence may
    /// call this. It is not release-on-error: it closes the exact authorization
    /// and prevents the same request from ever reserving again, even when budget
    /// admission originally failed before a reservation was written.
    pub(super) fn fence_proxy_non_admission(
        &self,
        token_id: &str,
        request_id: &str,
        fingerprint: &str,
        terms: &str,
        maximum: MoneyAu,
        proof: &str,
        now: u64,
    ) -> Result<()> {
        for v in [token_id, request_id, fingerprint, terms, proof] {
            id(v)?;
        }
        let k = key(&[token_id, Lane::Proxy.tag(), request_id]);
        self.write(|tx| {
            let mut rows = db(tx.open_table(RESERVATIONS))?;
            let old = db(rows.get(k.as_str()))?
                .map(|v| decode::<Reservation>(v.value()))
                .transpose()?;
            let mut r = if let Some(r) = old {
                require(
                    r.token_id == token_id
                        && r.lane == Lane::Proxy
                        && r.request_id == request_id
                        && r.fingerprint == fingerprint
                        && r.terms == terms
                        && r.maximum == maximum
                        && r.charged == 0,
                    Error::Conflict,
                )?;
                if r.closed {
                    return require(
                        r.released && r.proof.as_deref() == Some(proof),
                        Error::Conflict,
                    );
                }
                require(!r.released && r.proof.is_none(), Error::Conflict)?;
                let mut accounts = db(tx.open_table(ACCOUNTS))?;
                let mut a: Account =
                    decode(db(accounts.get(token_id))?.ok_or(Error::Conflict)?.value())?;
                a.roll(now);
                a.reserved = a.reserved.checked_sub(maximum).ok_or(Error::Conflict)?;
                db(accounts.insert(token_id, encode(&a)?.as_slice()))?;
                r
            } else {
                require(db(rows.len())? < self.limits.max_reservations, Error::Full)?;
                Reservation {
                    token_id: token_id.into(),
                    lane: Lane::Proxy,
                    request_id: request_id.into(),
                    fingerprint: fingerprint.into(),
                    terms: terms.into(),
                    maximum,
                    charged: 0,
                    closed: false,
                    released: false,
                    proof: None,
                    sequence: 0,
                    billing: None,
                    cumulative: 0,
                }
            };
            r.closed = true;
            r.released = true;
            r.proof = Some(proof.into());
            db(rows.insert(k.as_str(), encode(&r)?.as_slice()))?;
            db(tx.open_table(PENDING))?
                .remove(k.as_str())
                .map_err(|_| Error::Unavailable)?;
            Ok(())
        })
    }
    // Native owners retain their established explicit/Drop release behavior.
    // Proxy callers instead need canonical settlement or a durable signing fence.
    pub(super) fn release_native(&self, token_id: &str, request_id: &str, now: u64) -> Result<()> {
        self.settle(token_id, Lane::Native, request_id, now, |r, _| {
            r.released = true;
            Ok((0, r.closed))
        })
        .map(|_| ())
    }
    pub(super) fn pending(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Pending)>> {
        use std::ops::Bound;
        require((1..=64).contains(&limit), Error::Invalid)?;
        let tx = db(self.database.begin_read())?;
        let index = db(tx.open_table(PENDING))?;
        let rows = db(tx.open_table(RESERVATIONS))?;
        let start = after.map(Bound::Excluded).unwrap_or(Bound::Unbounded);
        let range = db(index.range::<&str>((start, Bound::Unbounded)))?;
        let mut output = Vec::new();
        for row in range.take(limit) {
            let (k, _) = db(row)?;
            let r: Reservation = decode(db(rows.get(k.value()))?.ok_or(Error::Invalid)?.value())?;
            output.push((
                k.value().into(),
                Pending {
                    token_id: r.token_id,
                    lane: r.lane,
                    request_id: r.request_id,
                    fingerprint: r.fingerprint,
                    terms: r.terms,
                    maximum: r.maximum,
                    charged: r.charged,
                },
            ));
        }
        Ok(output)
    }
}
#[cfg(unix)]
fn private_file(path: &Path, create: bool) -> Result<std::fs::File> {
    use rustix::fs::{fstat, open, FileType, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;
    let parent =
        std::fs::metadata(path.parent().ok_or(Error::Invalid)?).map_err(|_| Error::Unavailable)?;
    let uid = rustix::process::geteuid().as_raw();
    require(
        parent.is_dir() && parent.uid() == uid && parent.mode() & 0o077 == 0,
        Error::Invalid,
    )?;
    let fd = open(
        path,
        OFlags::RDWR
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC
            | if create {
                OFlags::CREATE | OFlags::EXCL
            } else {
                OFlags::empty()
            },
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| Error::Unavailable)?;
    let s = fstat(&fd).map_err(|_| Error::Unavailable)?;
    require(
        FileType::from_raw_mode(s.st_mode) == FileType::RegularFile
            && s.st_uid == uid
            && s.st_mode & 0o077 == 0
            && s.st_nlink == 1,
        Error::Invalid,
    )?;
    Ok(fd.into())
}
#[cfg(not(unix))]
fn private_file(_: &Path, _: bool) -> Result<std::fs::File> {
    Err(Error::Unavailable)
}
#[cfg(test)]
mod tests;
