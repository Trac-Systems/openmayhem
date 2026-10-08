//! Trusted local parent's shared capacity authority. This is an admission allocation,
//! not knowledge of an opaque upstream's global GPU slots or outside consumers.
//!
//! One private database is shared by aliases and native/proxy routes to the same
//! backend. redb excludes a second owner; each open advances the controller fence.
//! Outstanding work survives restart and is NEVER released by elapsed time, a lost
//! socket, a health observation, or dropping a Rust handle. All calls are blocking;
//! the supervised parent must use its bounded storage executor, not a token callback.
//!
//! The parent authenticates backend identity, observations and completion evidence.
//! Neither serialized records nor arbitrary connector messages grant these powers.
//! Cross-host enforcement requires one shared scheduler/authority; separate database
//! files cannot establish a global capacity guarantee.

use crate::attempts::{private_file, Digest, Identity};
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    TableHandle,
};
use serde::{Deserialize, Serialize};
use std::{
    ops::Bound,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_meta_v1");
const GROUPS: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_groups_v1");
const ROUTES: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_routes_v1");
const LEASES: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_leases_v1");
const BY_GROUP: TableDefinition<&str, &str> = TableDefinition::new("capacity_group_leases_v1");
const BY_WORK: TableDefinition<&str, &str> = TableDefinition::new("capacity_work_leases_v1");
const MAX_RECORD_BYTES: usize = 4096;
const MAX_PAGE: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid capacity configuration or evidence")]
    Invalid,
    #[error("capacity authority identity does not match")]
    Identity,
    #[error("capacity storage is unavailable; new dispatch must stop")]
    Storage,
    #[error("capacity authority requires a private supported file")]
    File,
    #[error("capacity configuration still has outstanding work or references")]
    InUse,
    #[error("capacity record not found")]
    NotFound,
    #[error("capacity observation or controller is stale")]
    Stale,
    #[error("capacity allocation is busy")]
    Busy,
    #[error("capacity readiness needs fresh evidence")]
    Checking,
    #[error("upstream is unavailable in this scope")]
    Unavailable,
    #[error("capacity storage quota reached")]
    Quota,
    #[error("this work already owns a capacity lease; recover it")]
    ExistingWork,
    #[error("capacity evidence does not match this lease")]
    Binding,
}
pub type Result<T> = std::result::Result<T, Error>;
fn db<T, E>(v: std::result::Result<T, E>) -> Result<T> {
    v.map_err(|_| Error::Storage)
}
fn require(v: bool) -> Result<()> {
    if v {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}
fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let b = serde_json::to_vec(v).map_err(|_| Error::Invalid)?;
    require(b.len() <= MAX_RECORD_BYTES)?;
    Ok(b)
}
fn decode<T: serde::de::DeserializeOwned>(v: &[u8]) -> Result<T> {
    require(v.len() <= MAX_RECORD_BYTES)?;
    serde_json::from_slice(v).map_err(|_| Error::Invalid)
}
fn read<T: serde::de::DeserializeOwned>(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &str,
) -> Result<T> {
    decode(db(table.get(key))?.ok_or(Error::NotFound)?.value())
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_groups: u64,
    pub max_routes: u64,
    /// Active/uncertain records only. Closed leases are removed; original attempt
    /// evidence belongs to the owned execution journal, not this admission index.
    pub max_leases: u64,
    pub max_evidence_age: Duration,
}
impl Limits {
    fn validate(&self) -> Result<()> {
        require(
            self.max_groups > 0
                && self.max_groups <= 100_000
                && self.max_routes > 0
                && self.max_routes <= 1_000_000
                && self.max_leases > 0
                && self.max_leases <= 1_000_000
                && !self.max_evidence_age.is_zero()
                && self.max_evidence_age <= Duration::from_secs(3600),
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    schema: u32,
    identity: Identity,
    fence: u64,
    sequence: u64,
    groups: u64,
    routes: u64,
    leases: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Native,
    Proxy,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub id: Digest,
    pub group: Digest,
    pub lane: Lane,
    pub max_concurrency: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Scope {
    Group(Digest),
    Route(Digest),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    Busy,
    Unavailable,
    Checking,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    revision: u64,
    fence: u64,
    expires_ms: u64,
    allowance: u32,
    state: Readiness,
}
impl Gate {
    fn empty(revision: u64) -> Self {
        Self {
            revision,
            fence: 0,
            expires_ms: 0,
            allowance: 0,
            state: Readiness::Checking,
        }
    }
    fn allowance(&self, fence: u64, now: u64, ceiling: u32) -> Result<u32> {
        if self.fence != fence || now >= self.expires_ms {
            return Err(Error::Checking);
        }
        match self.state {
            Readiness::Ready => Ok(self.allowance.min(ceiling)),
            Readiness::Busy => Err(Error::Busy),
            Readiness::Unavailable => Err(Error::Unavailable),
            Readiness::Checking => Err(Error::Checking),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    id: Digest,
    ceiling: u32,
    occupied: u32,
    routes: u64,
    gate: Gate,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteState {
    config: Route,
    occupied: u32,
    gate: Gate,
}

/// Issued by this authority before starting an asynchronous observation. Newer
/// observations/configuration fence older replies, including after delete/re-add.
/// Deliberately neither Clone nor Deserialize.
pub struct ObservationTicket {
    authority: Digest,
    fence: u64,
    scope: Scope,
    revision: u64,
}
/// A trusted parent's measurement, with its original age. `allowance` is a TOTAL
/// admission allowance, not an upstream free-slot count to subtract again. External
/// telemetry must first reconcile overlapping known work in its connector adapter.
pub struct Evidence {
    pub state: Readiness,
    pub allowance: u32,
    pub age: Duration,
    pub valid_for: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Reserved,
    Dispatched,
    Uncertain,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    pub invocation: Digest,
    pub request_hash: Digest,
}
impl Work {
    fn key(&self) -> String {
        self.invocation.as_str().to_owned()
    }
}
/// Recovery data, never a dispatch capability. A record from an earlier controller
/// has effective phase Uncertain even if its last durable phase was only Reserved.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub id: Digest,
    pub group: Digest,
    pub route: Digest,
    pub work: Work,
    pub controller_fence: u64,
    pub phase: Phase,
}
pub struct Reservation {
    lease: Lease,
}
impl Reservation {
    pub fn lease(&self) -> &Lease {
        &self.lease
    }
}
pub struct Dispatch {
    lease: Lease,
}
impl Dispatch {
    pub fn lease(&self) -> &Lease {
        &self.lease
    }
}
/// Evidence has already been verified by the trusted parent against retained
/// request/upstream outcome. A financial expiry or timeout is NEVER such evidence.
#[derive(Clone, Debug)]
pub struct VerifiedCompletion {
    pub lease: Lease,
    pub evidence: Digest,
}
#[derive(Clone, Debug)]
pub struct Status {
    pub group_occupied: u32,
    pub route_occupied: u32,
    pub available: u32,
    pub state: Readiness,
}
pub struct Page {
    pub leases: Vec<Lease>,
    pub next_after: Option<Digest>,
}

pub struct Authority {
    nonce: Digest,
    database: Database,
    limits: Limits,
    fence: u64,
    started: Instant,
    failed: AtomicBool,
    writer: Mutex<()>,
}
struct Tx<'a> {
    tx: redb::WriteTransaction,
    _guard: MutexGuard<'a, ()>,
}
impl std::ops::Deref for Tx<'_> {
    type Target = redb::WriteTransaction;
    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}
impl Authority {
    pub fn open(path: impl AsRef<Path>, identity: Identity, limits: Limits) -> Result<Self> {
        identity.validate().map_err(|_| Error::Invalid)?;
        limits.validate()?;
        let file = private_file(path.as_ref()).map_err(|_| Error::File)?;
        let mut builder = Database::builder();
        builder.set_cache_size(8 * 1024 * 1024);
        let database = db(builder.create_file(file))?;
        let mut tx = db(database.begin_write())?;
        db(tx.set_durability(Durability::Immediate))?;
        let names = db(tx.list_tables())?
            .map(|v| v.name().to_owned())
            .collect::<Vec<_>>();
        let mut t = db(tx.open_table(META))?;
        let previous = db(t.get("state"))?
            .map(|v| decode::<Meta>(v.value()))
            .transpose()?;
        let mut m = if let Some(m) = previous {
            if m.identity != identity {
                return Err(Error::Identity);
            }
            require(
                m.schema == 1
                    && [
                        GROUPS.name(),
                        ROUTES.name(),
                        LEASES.name(),
                        BY_GROUP.name(),
                        BY_WORK.name(),
                    ]
                    .iter()
                    .all(|n| names.iter().any(|s| s == n)),
            )?;
            require(
                db(db(tx.open_table(GROUPS))?.len())? == m.groups
                    && db(db(tx.open_table(ROUTES))?.len())? == m.routes
                    && db(db(tx.open_table(LEASES))?.len())? == m.leases
                    && db(db(tx.open_table(BY_GROUP))?.len())? == m.leases
                    && db(db(tx.open_table(BY_WORK))?.len())? == m.leases,
            )?;
            m
        } else {
            require(names.is_empty())?;
            db(tx.open_table(GROUPS))?;
            db(tx.open_table(ROUTES))?;
            db(tx.open_table(LEASES))?;
            db(tx.open_table(BY_GROUP))?;
            db(tx.open_table(BY_WORK))?;
            Meta {
                schema: 1,
                identity,
                fence: 0,
                sequence: 0,
                groups: 0,
                routes: 0,
                leases: 0,
            }
        };
        m.fence = m.fence.checked_add(1).ok_or(Error::Invalid)?;
        db(t.insert("state", encode(&m)?.as_slice()))?;
        drop(t);
        db(tx.commit())?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
        let nonce =
            Digest::new(blake3::hash(&nonce).to_hex().to_string()).map_err(|_| Error::Invalid)?;
        Ok(Self {
            nonce,
            database,
            limits,
            fence: m.fence,
            started: Instant::now(),
            failed: AtomicBool::new(false),
            writer: Mutex::new(()),
        })
    }
    fn now(&self) -> Result<u64> {
        u64::try_from(self.started.elapsed().as_millis()).map_err(|_| Error::Invalid)
    }
    fn healthy(&self) -> Result<()> {
        if self.failed.load(Ordering::Acquire) {
            Err(Error::Storage)
        } else {
            Ok(())
        }
    }
    fn write(&self) -> Result<Tx<'_>> {
        let guard = self.writer.lock().map_err(|_| Error::Storage)?;
        self.healthy()?;
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(Durability::Immediate))?;
        Ok(Tx { tx, _guard: guard })
    }
    fn commit(&self, t: Tx<'_>) -> Result<()> {
        let Tx { tx, _guard } = t;
        if tx.commit().is_err() {
            self.failed.store(true, Ordering::Release);
            return Err(Error::Storage);
        }
        Ok(())
    }
    /// Changing a ceiling invalidates its health allowance, but NEVER drops work.
    pub fn configure_group(&self, id: Digest, ceiling: u32) -> Result<()> {
        require(ceiling > 0)?;
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut groups = db(tx.open_table(GROUPS))?;
        let old = db(groups.get(id.as_str()))?
            .map(|v| decode::<Group>(v.value()))
            .transpose()?;
        if old.as_ref().is_some_and(|g| g.ceiling == ceiling) {
            return Ok(());
        }
        let g = if let Some(mut g) = old {
            g.ceiling = ceiling;
            g.gate = Gate::empty(sequence(&mut m)?);
            g
        } else {
            if m.groups >= self.limits.max_groups {
                return Err(Error::Quota);
            }
            m.groups += 1;
            Group {
                id: id.clone(),
                ceiling,
                occupied: 0,
                routes: 0,
                gate: Gate::empty(sequence(&mut m)?),
            }
        };
        db(groups.insert(id.as_str(), encode(&g)?.as_slice()))?;
        save_meta(&tx, &m)?;
        drop(groups);
        self.commit(tx)
    }
    /// An occupied alias cannot move to another backend or change lane. Existing
    /// allocations survive smaller limits; new admissions wait until they fit.
    pub fn configure_route(&self, route: Route) -> Result<()> {
        require(route.max_concurrency > 0)?;
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut groups = db(tx.open_table(GROUPS))?;
        let mut target: Group = read(&groups, route.group.as_str())?;
        let mut routes = db(tx.open_table(ROUTES))?;
        let old = db(routes.get(route.id.as_str()))?
            .map(|v| decode::<RouteState>(v.value()))
            .transpose()?;
        if old.as_ref().is_some_and(|r| r.config == route) {
            return Ok(());
        }
        let occupied = if let Some(old) = old {
            if old.occupied > 0
                && (old.config.group != route.group || old.config.lane != route.lane)
            {
                return Err(Error::InUse);
            }
            if old.config.group != route.group {
                let mut previous: Group = read(&groups, old.config.group.as_str())?;
                previous.routes = previous.routes.checked_sub(1).ok_or(Error::Invalid)?;
                target.routes = target.routes.checked_add(1).ok_or(Error::Invalid)?;
                db(groups.insert(previous.id.as_str(), encode(&previous)?.as_slice()))?;
            }
            old.occupied
        } else {
            if m.routes >= self.limits.max_routes {
                return Err(Error::Quota);
            }
            m.routes += 1;
            target.routes = target.routes.checked_add(1).ok_or(Error::Invalid)?;
            0
        };
        let state = RouteState {
            config: route,
            occupied,
            gate: Gate::empty(sequence(&mut m)?),
        };
        db(routes.insert(state.config.id.as_str(), encode(&state)?.as_slice()))?;
        db(groups.insert(target.id.as_str(), encode(&target)?.as_slice()))?;
        save_meta(&tx, &m)?;
        drop(routes);
        drop(groups);
        self.commit(tx)
    }
    pub fn remove_route(&self, route: &Digest) -> Result<()> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut routes = db(tx.open_table(ROUTES))?;
        let r: RouteState = read(&routes, route.as_str())?;
        if r.occupied > 0 {
            return Err(Error::InUse);
        }
        let mut groups = db(tx.open_table(GROUPS))?;
        let mut g: Group = read(&groups, r.config.group.as_str())?;
        g.routes = g.routes.checked_sub(1).ok_or(Error::Invalid)?;
        m.routes = m.routes.checked_sub(1).ok_or(Error::Invalid)?;
        db(groups.insert(g.id.as_str(), encode(&g)?.as_slice()))?;
        db(routes.remove(route.as_str()))?;
        save_meta(&tx, &m)?;
        drop(groups);
        drop(routes);
        self.commit(tx)
    }
    pub fn remove_group(&self, group: &Digest) -> Result<()> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut groups = db(tx.open_table(GROUPS))?;
        let g: Group = read(&groups, group.as_str())?;
        if g.occupied > 0 || g.routes > 0 {
            return Err(Error::InUse);
        }
        m.groups = m.groups.checked_sub(1).ok_or(Error::Invalid)?;
        db(groups.remove(group.as_str()))?;
        save_meta(&tx, &m)?;
        drop(groups);
        self.commit(tx)
    }
    pub fn begin_observation(&self, scope: Scope) -> Result<ObservationTicket> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let revision = sequence(&mut m)?;
        edit_gate(&tx, &scope, |gate| {
            gate.revision = revision;
            Ok(())
        })?;
        save_meta(&tx, &m)?;
        self.commit(tx)?;
        Ok(ObservationTicket {
            authority: self.nonce.clone(),
            fence: self.fence,
            scope,
            revision,
        })
    }
    pub fn observe(&self, ticket: ObservationTicket, evidence: Evidence) -> Result<()> {
        if ticket.fence != self.fence || ticket.authority != self.nonce {
            return Err(Error::Stale);
        }
        require(
            !evidence.valid_for.is_zero()
                && evidence.valid_for <= self.limits.max_evidence_age
                && evidence.age < evidence.valid_for
                && (evidence.state != Readiness::Ready || evidence.allowance > 0),
        )?;
        let remaining = u64::try_from((evidence.valid_for - evidence.age).as_millis())
            .map_err(|_| Error::Invalid)?;
        require(remaining > 0)?;
        let expires_ms = self.now()?.checked_add(remaining).ok_or(Error::Invalid)?;
        let tx = self.write()?;
        edit_gate(&tx, &ticket.scope, |gate| {
            if gate.revision != ticket.revision {
                return Err(Error::Stale);
            }
            // Consume this ticket even if its value is repeated. Only a new actual
            // observation can refresh it; reusing a heartbeat is not evidence.
            gate.revision = 0;
            gate.fence = self.fence;
            gate.expires_ms = expires_ms;
            gate.allowance = evidence.allowance;
            gate.state = evidence.state;
            Ok(())
        })?;
        self.commit(tx)
    }
    pub fn status(&self, route: &Digest) -> Result<Status> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, route.as_str())?;
        let g: Group = read(&db(tx.open_table(GROUPS))?, r.config.group.as_str())?;
        let m: Meta = read(&db(tx.open_table(META))?, "state")?;
        let remaining = self
            .limits
            .max_leases
            .saturating_sub(m.leases)
            .min(u64::from(u32::MAX)) as u32;
        let availability = free(&g, &r, self.fence, self.now()?).map(|n| n.min(remaining));
        let (available, state) = match availability {
            Ok(n) if n > 0 => (n, Readiness::Ready),
            Ok(_) | Err(Error::Busy) => (0, Readiness::Busy),
            Err(Error::Checking) => (0, Readiness::Checking),
            Err(Error::Unavailable) => (0, Readiness::Unavailable),
            Err(e) => return Err(e),
        };
        Ok(Status {
            group_occupied: g.occupied,
            route_occupied: r.occupied,
            available,
            state,
        })
    }
    /// Atomic shared admission. Request hashes do not form idempotency identities;
    /// the invocation does, and must be scoped to the authenticated caller. Each
    /// sequential attempt receives a fresh lease only after the previous one closes.
    pub fn reserve(&self, route: &Digest, work: Work) -> Result<Reservation> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut work_index = db(tx.open_table(BY_WORK))?;
        if db(work_index.get(work.key().as_str()))?.is_some() {
            return Err(Error::ExistingWork);
        }
        if m.leases >= self.limits.max_leases {
            return Err(Error::Quota);
        }
        let mut groups = db(tx.open_table(GROUPS))?;
        let mut routes = db(tx.open_table(ROUTES))?;
        let mut r: RouteState = read(&routes, route.as_str())?;
        let mut g: Group = read(&groups, r.config.group.as_str())?;
        if free(&g, &r, self.fence, self.now()?)? == 0 {
            return Err(Error::Busy);
        }
        let mut entropy = [0u8; 32];
        getrandom::fill(&mut entropy).map_err(|_| Error::Storage)?;
        let id =
            Digest::new(blake3::hash(&entropy).to_hex().to_string()).map_err(|_| Error::Invalid)?;
        let mut leases = db(tx.open_table(LEASES))?;
        if db(leases.get(id.as_str()))?.is_some() {
            return Err(Error::Invalid);
        }
        let lease = Lease {
            id,
            group: g.id.clone(),
            route: route.clone(),
            work,
            controller_fence: self.fence,
            phase: Phase::Reserved,
        };
        r.occupied = r.occupied.checked_add(1).ok_or(Error::Invalid)?;
        g.occupied = g.occupied.checked_add(1).ok_or(Error::Invalid)?;
        m.leases += 1;
        db(routes.insert(route.as_str(), encode(&r)?.as_slice()))?;
        db(groups.insert(g.id.as_str(), encode(&g)?.as_slice()))?;
        db(leases.insert(lease.id.as_str(), encode(&lease)?.as_slice()))?;
        db(work_index.insert(lease.work.key().as_str(), lease.id.as_str()))?;
        db(db(tx.open_table(BY_GROUP))?.insert(group_key(&lease).as_str(), lease.id.as_str()))?;
        save_meta(&tx, &m)?;
        drop(work_index);
        drop(leases);
        drop(groups);
        drop(routes);
        self.commit(tx)?;
        Ok(Reservation { lease })
    }
    /// Commit BEFORE the executor's own dispatch fence/POST. Failure between those
    /// steps retains an occupied slot for reconciliation; it never admits a duplicate.
    /// Readiness is rechecked to close the observation-to-send race within this authority.
    pub fn dispatch(&self, reservation: &Reservation) -> Result<Dispatch> {
        if reservation.lease.controller_fence != self.fence {
            return Err(Error::Stale);
        }
        let tx = self.write()?;
        let mut leases = db(tx.open_table(LEASES))?;
        let mut lease: Lease = read(&leases, reservation.lease.id.as_str())?;
        if lease != reservation.lease || lease.phase != Phase::Reserved {
            return Err(Error::Binding);
        }
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, lease.route.as_str())?;
        let g: Group = read(&db(tx.open_table(GROUPS))?, lease.group.as_str())?;
        let now = self.now()?;
        let group_allowance = g.gate.allowance(self.fence, now, g.ceiling)?;
        let route_allowance = r
            .gate
            .allowance(self.fence, now, r.config.max_concurrency)?;
        // Existing reservations already count. Do not subtract this request twice.
        if g.occupied > group_allowance || r.occupied > route_allowance {
            return Err(Error::Busy);
        }
        lease.phase = Phase::Dispatched;
        db(leases.insert(lease.id.as_str(), encode(&lease)?.as_slice()))?;
        drop(leases);
        self.commit(tx)?;
        Ok(Dispatch { lease })
    }
    /// Trusted executor lookup for a previously accepted lease. The exact work and
    /// configured route must match; a prior-controller reservation cannot dispatch.
    /// The durable phase transition still has one winner across competing callers.
    pub fn dispatch_accepted(&self, id: &Digest, work: &Work, route: &Digest) -> Result<Dispatch> {
        let lease = self.lease(id)?.ok_or(Error::NotFound)?;
        if lease.work != *work || lease.route != *route {
            return Err(Error::Binding);
        }
        self.dispatch(&Reservation { lease })
    }

    /// Only the still-held, never-dispatched capability can cancel without external
    /// evidence. Dropping it is intentionally NOT a cancellation or a free slot.
    pub fn cancel_reserved(&self, reservation: Reservation) -> Result<()> {
        if reservation.lease.controller_fence != self.fence {
            return Err(Error::Stale);
        }
        let tx = self.write()?;
        let actual: Lease = read(&db(tx.open_table(LEASES))?, reservation.lease.id.as_str())?;
        if actual != reservation.lease || actual.phase != Phase::Reserved {
            return Err(Error::Binding);
        }
        release(&tx, &actual)?;
        self.commit(tx)
    }
    /// A disconnect/timeout remains occupied. Caller retains the lease ID for exact
    /// reconciliation; a fresh health response cannot close this attempt.
    pub fn uncertain(&self, dispatch: Dispatch) -> Result<Lease> {
        if dispatch.lease.controller_fence != self.fence {
            return Err(Error::Stale);
        }
        let tx = self.write()?;
        let mut leases = db(tx.open_table(LEASES))?;
        let mut actual: Lease = read(&leases, dispatch.lease.id.as_str())?;
        if actual != dispatch.lease {
            return Err(Error::Binding);
        }
        actual.phase = Phase::Uncertain;
        db(leases.insert(actual.id.as_str(), encode(&actual)?.as_slice()))?;
        drop(leases);
        self.commit(tx)?;
        Ok(actual)
    }
    /// Trusted completion/cancellation/nonexecution proof only. Replay is harmless.
    /// This releases capacity, NOT a financial hold or authorization for a fresh POST.
    pub fn complete(&self, proof: VerifiedCompletion) -> Result<bool> {
        let tx = self.write()?;
        let leases = db(tx.open_table(LEASES))?;
        let Some(v) = db(leases.get(proof.lease.id.as_str()))? else {
            return Ok(false);
        };
        let actual: Lease = decode(v.value())?;
        if effective(actual.clone(), self.fence) != proof.lease {
            return Err(Error::Binding);
        }
        // The proof commitment is intentionally not an authority on its own. The
        // paid controller must retain the verified evidence in its attempt journal.
        let _evidence = proof.evidence;
        drop(v);
        drop(leases);
        release(&tx, &actual)?;
        self.commit(tx)?;
        Ok(true)
    }
    pub fn lease(&self, id: &Digest) -> Result<Option<Lease>> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let t = db(tx.open_table(LEASES))?;
        db(t.get(id.as_str()))?
            .map(|v| decode(v.value()).map(|l| effective(l, self.fence)))
            .transpose()
    }
    pub fn work_lease(&self, work: &Work) -> Result<Option<Lease>> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let index = db(tx.open_table(BY_WORK))?;
        let Some(id) = db(index.get(work.key().as_str()))? else {
            return Ok(None);
        };
        let lease: Lease = read(&db(tx.open_table(LEASES))?, id.value())?;
        if lease.work != *work {
            return Err(Error::Binding);
        }
        Ok(Some(effective(lease, self.fence)))
    }
    pub fn recover_group(
        &self,
        group: &Digest,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Page> {
        require(limit > 0 && limit <= MAX_PAGE)?;
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let index = db(tx.open_table(BY_GROUP))?;
        let leases = db(tx.open_table(LEASES))?;
        let prefix = format!("{}/", group.as_str());
        let end = format!("{}0", group.as_str());
        let start = after
            .map(|id| format!("{}{}", prefix, id.as_str()))
            .unwrap_or_else(|| prefix.clone());
        let start_bound = if after.is_some() {
            Bound::Excluded(start.as_str())
        } else {
            Bound::Included(start.as_str())
        };
        let mut rows = Vec::new();
        for row in
            db(index.range::<&str>((start_bound, Bound::Excluded(end.as_str()))))?.take(limit + 1)
        {
            let (_, id) = db(row)?;
            let l: Lease = read(&leases, id.value())?;
            require(l.group == *group)?;
            rows.push(effective(l, self.fence));
        }
        let more = rows.len() > limit;
        rows.truncate(limit);
        let next_after = if more {
            rows.last().map(|l| l.id.clone())
        } else {
            None
        };
        Ok(Page {
            leases: rows,
            next_after,
        })
    }
}
fn free(g: &Group, r: &RouteState, fence: u64, now: u64) -> Result<u32> {
    let group = g
        .gate
        .allowance(fence, now, g.ceiling)?
        .saturating_sub(g.occupied);
    let route = r
        .gate
        .allowance(fence, now, r.config.max_concurrency)?
        .saturating_sub(r.occupied);
    Ok(group.min(route))
}
fn effective(mut l: Lease, fence: u64) -> Lease {
    if l.controller_fence != fence {
        l.phase = Phase::Uncertain;
    }
    l
}
fn group_key(l: &Lease) -> String {
    format!("{}/{}", l.group.as_str(), l.id.as_str())
}
fn meta(tx: &redb::WriteTransaction) -> Result<Meta> {
    read(&db(tx.open_table(META))?, "state")
}
fn save_meta(tx: &redb::WriteTransaction, m: &Meta) -> Result<()> {
    db(db(tx.open_table(META))?.insert("state", encode(m)?.as_slice()))?;
    Ok(())
}
fn sequence(m: &mut Meta) -> Result<u64> {
    m.sequence = m.sequence.checked_add(1).ok_or(Error::Invalid)?;
    Ok(m.sequence)
}
fn edit_gate(
    tx: &redb::WriteTransaction,
    scope: &Scope,
    edit: impl FnOnce(&mut Gate) -> Result<()>,
) -> Result<()> {
    match scope {
        Scope::Group(id) => {
            let mut t = db(tx.open_table(GROUPS))?;
            let mut g: Group = read(&t, id.as_str())?;
            edit(&mut g.gate)?;
            db(t.insert(id.as_str(), encode(&g)?.as_slice()))?;
        }
        Scope::Route(id) => {
            let mut t = db(tx.open_table(ROUTES))?;
            let mut r: RouteState = read(&t, id.as_str())?;
            edit(&mut r.gate)?;
            db(t.insert(id.as_str(), encode(&r)?.as_slice()))?;
        }
    }
    Ok(())
}
fn release(tx: &redb::WriteTransaction, l: &Lease) -> Result<()> {
    let mut m = meta(tx)?;
    let mut groups = db(tx.open_table(GROUPS))?;
    let mut routes = db(tx.open_table(ROUTES))?;
    let mut g: Group = read(&groups, l.group.as_str())?;
    let mut r: RouteState = read(&routes, l.route.as_str())?;
    require(r.config.group == l.group)?;
    g.occupied = g.occupied.checked_sub(1).ok_or(Error::Invalid)?;
    r.occupied = r.occupied.checked_sub(1).ok_or(Error::Invalid)?;
    m.leases = m.leases.checked_sub(1).ok_or(Error::Invalid)?;
    db(groups.insert(g.id.as_str(), encode(&g)?.as_slice()))?;
    db(routes.insert(r.config.id.as_str(), encode(&r)?.as_slice()))?;
    db(db(tx.open_table(LEASES))?.remove(l.id.as_str()))?;
    db(db(tx.open_table(BY_GROUP))?.remove(group_key(l).as_str()))?;
    db(db(tx.open_table(BY_WORK))?.remove(l.work.key().as_str()))?;
    save_meta(tx, &m)
}
