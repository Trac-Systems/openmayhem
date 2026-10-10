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

pub mod probes;

use crate::attempts::{existing_private_file, private_file, Digest, Identity};
use crate::financial::negotiation::BuyerOffer;
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    TableHandle,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ops::Bound,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_meta_v1");
const GROUPS: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_groups_v1");
const ROUTES: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_routes_v1");
const LEASES: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_leases_v1");
const BY_GROUP: TableDefinition<&str, &str> = TableDefinition::new("capacity_group_leases_v1");
const BY_CONSTRAINT: TableDefinition<&str, &str> =
    TableDefinition::new("capacity_constraint_leases_v1");
const BY_WORK: TableDefinition<&str, &str> = TableDefinition::new("capacity_work_leases_v1");
const SIGNING: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_signing_intents_v1");
const MAX_RECORD_BYTES: usize = 4096;
const MAX_SIGNING_BYTES: usize = 32768;
const MAX_PAGE: usize = 64;
const MAX_CONSTRAINTS: usize = 16;

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
    #[error("operator recovery-probe allowance is missing or exhausted")]
    ProbeBudget,
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
    #[serde(default)]
    constraint_leases: u64,
    #[serde(default)]
    probe_budgets: u64,
    #[serde(default)]
    probes: u64,
    #[serde(default)]
    probe_groups: u64,
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
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
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
    /// A live guard never falls back to a previously persisted Ready observation.
    #[serde(default)]
    live: Option<u64>,
}
impl Gate {
    fn empty(revision: u64) -> Self {
        Self {
            revision,
            fence: 0,
            expires_ms: 0,
            allowance: 0,
            state: Readiness::Checking,
            live: None,
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupMode {
    #[default]
    Observed,
    /// A physical/operator allocation ceiling, not model-health evidence.
    /// Every route still independently requires fresh readiness.
    Allocation,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    id: Digest,
    ceiling: u32,
    occupied: u32,
    routes: u64,
    gate: Gate,
    #[serde(default)]
    mode: GroupMode,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteState {
    config: Route,
    occupied: u32,
    gate: Gate,
    #[serde(default)]
    constraints: Vec<Digest>,
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

/// Trusted local observation only. Implementations must perform bounded memory
/// reads, never network/storage I/O or a scan of request history. The authority
/// consults this on admission/signing/dispatch, not on generated-token delivery.
/// Errors and missing/expired observations close admission; no cached fallback.
pub trait ReadinessSource: Send + Sync {
    /// Monotonically changes with evidence. Shared sources return the same revision;
    /// the parent rejects an inconsistent group/route pair without spinning.
    fn revision(&self) -> Result<u64>;
    fn evidence(&self) -> Result<Evidence>;
}

struct LiveSource {
    generation: u64,
    source: Arc<dyn ReadinessSource>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Proxy proposal only: provider signing has not begun and dispatch is forbidden.
    /// Older Reserved records remain ambiguous and never acquire this phase.
    Proposed,
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
/// Original buyer-signed terms retained atomically with the signing fence.
/// Historical recovery data, never fresh capacity, funding or dispatch authority.
/// Private construction and no serialization prevent accepting a connector claim.
pub struct SigningIntent {
    lease: Lease,
    buyer: BuyerOffer,
}
impl SigningIntent {
    pub fn lease(&self) -> &Lease {
        &self.lease
    }
    pub fn buyer(&self) -> &BuyerOffer {
        &self.buyer
    }
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
#[derive(Clone, Debug)]
pub struct GroupStatus {
    pub mode: GroupMode,
    pub ceiling: u32,
    pub occupied: u32,
    pub routes: u64,
}
pub struct Page {
    pub leases: Vec<Lease>,
    pub next_after: Option<Digest>,
}

pub struct Authority {
    identity: Identity,
    nonce: Digest,
    database: Database,
    limits: Limits,
    fence: u64,
    started: Instant,
    failed: AtomicBool,
    writer: Mutex<()>,
    live: Mutex<BTreeMap<Scope, LiveSource>>,
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
        Self::open_inner(path.as_ref(), identity, limits, false)
    }
    /// Resume an established authority without creating/resetting its accounting store.
    pub fn open_existing(
        path: impl AsRef<Path>,
        identity: Identity,
        limits: Limits,
    ) -> Result<Self> {
        Self::open_inner(path.as_ref(), identity, limits, true)
    }
    fn open_inner(path: &Path, identity: Identity, limits: Limits, existing: bool) -> Result<Self> {
        identity.validate().map_err(|_| Error::Invalid)?;
        limits.validate()?;
        let file = (if existing {
            existing_private_file(path)
        } else {
            private_file(path)
        })
        .map_err(|_| Error::File)?;
        let mut builder = Database::builder();
        builder.set_cache_size(8 * 1024 * 1024);
        let database = db(crate::storage::create(&builder, file))?;
        let mut tx = db(database.begin_write())?;
        db(tx.set_durability(Durability::Immediate))?;
        let names = db(tx.list_tables())?
            .map(|v| v.name().to_owned())
            .collect::<Vec<_>>();
        let mut t = db(tx.open_table(META))?;
        let previous = db(t.get("state"))?
            .map(|v| decode::<Meta>(v.value()))
            .transpose()?;
        let mut m = if let Some(mut m) = previous {
            if m.identity != identity {
                return Err(Error::Identity);
            }
            require(
                matches!(m.schema, 1 | 2 | 3 | 4 | 5)
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
            if m.schema == 1 {
                require(!names.iter().any(|n| n == SIGNING.name()))?;
                db(tx.open_table(SIGNING))?;
                m.schema = 2;
            } else {
                require(names.iter().any(|n| n == SIGNING.name()))?;
            }
            if m.schema < 4 {
                require(
                    !names.iter().any(|n| n == BY_CONSTRAINT.name()) && m.constraint_leases == 0,
                )?;
                db(tx.open_table(BY_CONSTRAINT))?;
            } else {
                require(names.iter().any(|n| n == BY_CONSTRAINT.name()))?;
            }
            probes::upgrade(&tx, &m, &names)?;
            require(
                db(db(tx.open_table(GROUPS))?.len())? == m.groups
                    && db(db(tx.open_table(ROUTES))?.len())? == m.routes
                    && db(db(tx.open_table(LEASES))?.len())? == m.leases
                    && db(db(tx.open_table(BY_GROUP))?.len())? == m.leases
                    && db(db(tx.open_table(BY_WORK))?.len())? == m.leases
                    && db(db(tx.open_table(BY_CONSTRAINT))?.len())? == m.constraint_leases
                    && db(db(tx.open_table(SIGNING))?.len())? <= m.leases,
            )?;
            m
        } else {
            require(!existing && names.is_empty())?;
            db(tx.open_table(GROUPS))?;
            db(tx.open_table(ROUTES))?;
            db(tx.open_table(LEASES))?;
            db(tx.open_table(BY_GROUP))?;
            db(tx.open_table(BY_CONSTRAINT))?;
            db(tx.open_table(BY_WORK))?;
            db(tx.open_table(SIGNING))?;
            probes::create_tables(&tx)?;
            Meta {
                schema: 5,
                identity,
                fence: 0,
                sequence: 0,
                groups: 0,
                routes: 0,
                leases: 0,
                constraint_leases: 0,
                probe_budgets: 0,
                probes: 0,
                probe_groups: 0,
            }
        };
        // Added gate/group/constraint fields default safely in historical rows;
        // no history rewrite or lease migration. Older binaries reject schema5.
        m.schema = 5;
        m.fence = m.fence.checked_add(1).ok_or(Error::Invalid)?;
        db(t.insert("state", encode(&m)?.as_slice()))?;
        drop(t);
        db(tx.commit())?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
        let nonce =
            Digest::new(blake3::hash(&nonce).to_hex().to_string()).map_err(|_| Error::Invalid)?;
        Ok(Self {
            identity: m.identity,
            nonce,
            database,
            limits,
            fence: m.fence,
            started: Instant::now(),
            failed: AtomicBool::new(false),
            writer: Mutex::new(()),
            live: Mutex::new(BTreeMap::new()),
        })
    }
    pub(crate) fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Monotonic across restarts of this locked authority. Presence receivers
    /// also bind the boot nonce, so a second, freshly created store cannot
    /// impersonate the same generation or add another allocation for an offer.
    pub(crate) fn presence_fence(&self) -> (u64, Digest) {
        (self.fence, self.nonce.clone())
    }

    /// Attach once from trusted startup, after configuring the exact scope.
    /// No model request is issued and no active/uncertain lease is modified.
    /// On restart the saved live requirement stays closed until reattached.
    pub fn bind_live(&self, scope: Scope, source: Arc<dyn ReadinessSource>) -> Result<()> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let generation = sequence(&mut m)?;
        let mut live = self.live.lock().map_err(|_| Error::Storage)?;
        if let Some(old) = live.get(&scope) {
            return if Arc::ptr_eq(&old.source, &source) {
                Ok(())
            } else {
                Err(Error::InUse)
            };
        }
        edit_gate(&tx, &scope, |gate| {
            gate.live = Some(generation);
            gate.revision = generation;
            gate.fence = self.fence;
            gate.expires_ms = 0;
            gate.allowance = 0;
            gate.state = Readiness::Checking;
            Ok(())
        })?;
        save_meta(&tx, &m)?;
        live.insert(scope, LiveSource { generation, source });
        drop(live);
        self.commit(tx)
    }

    fn live_source(&self, scope: Scope, gate: &Gate) -> Result<Option<Arc<dyn ReadinessSource>>> {
        let Some(generation) = gate.live else {
            return Ok(None);
        };
        if gate.fence != self.fence {
            return Err(Error::Checking);
        }
        let sources = self.live.lock().map_err(|_| Error::Storage)?;
        let bound = sources.get(&scope).ok_or(Error::Checking)?;
        if bound.generation != generation {
            return Err(Error::Checking);
        }
        Ok(Some(bound.source.clone()))
    }
    fn allowance(
        &self,
        gate: &Gate,
        source: Option<&dyn ReadinessSource>,
        now: u64,
        ceiling: u32,
    ) -> Result<u32> {
        let Some(source) = source else {
            return gate.allowance(self.fence, now, ceiling);
        };
        let evidence = source.evidence()?;
        if evidence.valid_for.is_zero() || evidence.valid_for > self.limits.max_evidence_age {
            return Err(Error::Invalid);
        }
        if evidence.age >= evidence.valid_for {
            return Err(Error::Checking);
        }
        match evidence.state {
            Readiness::Ready if evidence.allowance > 0 => Ok(evidence.allowance.min(ceiling)),
            Readiness::Ready => Err(Error::Invalid),
            Readiness::Busy => Err(Error::Busy),
            Readiness::Unavailable => Err(Error::Unavailable),
            Readiness::Checking => Err(Error::Checking),
        }
    }
    fn allowances(&self, groups: &[Group], route: &RouteState, now: u64) -> Result<Allowances> {
        self.allowances_with_recovery(groups, route, now, None)
    }
    fn allowances_with_recovery(
        &self,
        groups: &[Group],
        route: &RouteState,
        now: u64,
        recovery_group: Option<&Digest>,
    ) -> Result<Allowances> {
        require(!groups.is_empty() && groups.len() <= MAX_CONSTRAINTS + 1)?;
        let mut sources = Vec::with_capacity(groups.len() + 1);
        for group in groups {
            sources.push(if group.mode == GroupMode::Allocation {
                None
            } else {
                self.live_source(Scope::Group(group.id.clone()), &group.gate)?
            });
        }
        sources.push(self.live_source(Scope::Route(route.config.id.clone()), &route.gate)?);
        let revisions = sources
            .iter()
            .map(|s| s.as_ref().map(|s| s.revision()).transpose())
            .collect::<Result<Vec<_>>>()?;
        let mut free = u32::MAX;
        let mut fits = true;
        for (group, source) in groups.iter().zip(&sources) {
            let allowed = if group.mode == GroupMode::Allocation {
                group.ceiling
            } else {
                recovery_allowance(
                    self.allowance(&group.gate, source.as_deref(), now, group.ceiling),
                    recovery_group == Some(&group.id),
                    group.ceiling,
                )?
            };
            free = free.min(allowed.saturating_sub(group.occupied));
            fits &= group.occupied <= allowed;
        }
        let allowed = recovery_allowance(
            self.allowance(
                &route.gate,
                sources.last().and_then(|s| s.as_deref()),
                now,
                route.config.max_concurrency,
            ),
            recovery_group.is_some(),
            route.config.max_concurrency,
        )?;
        free = free.min(allowed.saturating_sub(route.occupied));
        fits &= route.occupied <= allowed;
        for (source, before) in sources.iter().zip(revisions) {
            if before != source.as_ref().map(|s| s.revision()).transpose()? {
                return Err(Error::Checking);
            }
        }
        Ok(Allowances { free, fits })
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
        self.configure_group_mode(id, ceiling, GroupMode::Observed)
    }
    /// Physical allocation only. This never supplies Ready evidence for a route.
    pub fn configure_allocation_group(&self, id: Digest, ceiling: u32) -> Result<()> {
        self.configure_group_mode(id, ceiling, GroupMode::Allocation)
    }
    fn configure_group_mode(&self, id: Digest, ceiling: u32, mode: GroupMode) -> Result<()> {
        require(ceiling > 0)?;
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut groups = db(tx.open_table(GROUPS))?;
        let old = db(groups.get(id.as_str()))?
            .map(|v| decode::<Group>(v.value()))
            .transpose()?;
        if old
            .as_ref()
            .is_some_and(|g| g.ceiling == ceiling && g.mode == mode)
        {
            return Ok(());
        }
        let g = if let Some(mut g) = old {
            if g.mode != mode && (g.occupied > 0 || g.routes > 0) {
                return Err(Error::InUse);
            }
            g.mode = mode;
            g.ceiling = ceiling;
            let revision = sequence(&mut m)?;
            let live = g.gate.live.map(|_| revision);
            g.gate = Gate::empty(revision);
            g.gate.live = live;
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
                mode,
            }
        };
        db(groups.insert(id.as_str(), encode(&g)?.as_slice()))?;
        save_meta(&tx, &m)?;
        drop(groups);
        self.live
            .lock()
            .map_err(|_| Error::Storage)?
            .remove(&Scope::Group(id));
        self.commit(tx)
    }
    /// An occupied alias cannot move to another backend or change lane. Existing
    /// allocations survive smaller limits; new admissions wait until they fit.
    pub fn configure_route(&self, route: Route) -> Result<()> {
        self.configure_route_inner(route, None)
    }
    /// Additional overlapping limits (for example a credential pool over a shared
    /// physical backend). Each relevant group counts this work exactly once.
    pub fn configure_route_with_constraints(
        &self,
        route: Route,
        constraints: Vec<Digest>,
    ) -> Result<()> {
        self.configure_route_inner(route, Some(constraints))
    }
    fn configure_route_inner(&self, route: Route, constraints: Option<Vec<Digest>>) -> Result<()> {
        require(route.max_concurrency > 0)?;
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut groups = db(tx.open_table(GROUPS))?;
        let mut routes = db(tx.open_table(ROUTES))?;
        let old = db(routes.get(route.id.as_str()))?
            .map(|v| decode::<RouteState>(v.value()))
            .transpose()?;
        // Legacy ceiling updates preserve existing constraints, never remove them.
        let mut constraints = constraints.unwrap_or_else(|| {
            old.as_ref()
                .map(|r| r.constraints.clone())
                .unwrap_or_default()
        });
        constraints.sort();
        validate_constraints(&route, &constraints)?;
        if old
            .as_ref()
            .is_some_and(|r| r.config == route && r.constraints == constraints)
        {
            return Ok(());
        }
        if old.as_ref().is_some_and(|r| {
            r.occupied > 0
                && (r.config.group != route.group
                    || r.config.lane != route.lane
                    || r.constraints != constraints)
        }) {
            return Err(Error::InUse);
        }
        let previous = old.as_ref().map(group_ids).transpose()?.unwrap_or_default();
        let required_live = old.as_ref().is_some_and(|r| r.gate.live.is_some());
        let occupied = old.as_ref().map(|r| r.occupied).unwrap_or(0);
        if old.is_none() {
            if m.routes >= self.limits.max_routes {
                return Err(Error::Quota);
            }
            m.routes += 1;
        }
        let revision = sequence(&mut m)?;
        let mut gate = Gate::empty(revision);
        gate.live = required_live.then_some(revision);
        let state = RouteState {
            config: route,
            occupied,
            gate,
            constraints,
        };
        let current = group_ids(&state)?;
        for id in &previous {
            if !current.contains(id) {
                let mut group: Group = read(&groups, id.as_str())?;
                group.routes = group.routes.checked_sub(1).ok_or(Error::Invalid)?;
                db(groups.insert(id.as_str(), encode(&group)?.as_slice()))?;
            }
        }
        for id in &current {
            let mut group: Group = read(&groups, id.as_str())?;
            if !previous.contains(id) {
                group.routes = group.routes.checked_add(1).ok_or(Error::Invalid)?;
                db(groups.insert(id.as_str(), encode(&group)?.as_slice()))?;
            }
        }
        db(routes.insert(state.config.id.as_str(), encode(&state)?.as_slice()))?;
        save_meta(&tx, &m)?;
        drop(routes);
        drop(groups);
        self.live
            .lock()
            .map_err(|_| Error::Storage)?
            .remove(&Scope::Route(state.config.id));
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
        for id in group_ids(&r)? {
            let mut g: Group = read(&groups, id.as_str())?;
            g.routes = g.routes.checked_sub(1).ok_or(Error::Invalid)?;
            db(groups.insert(g.id.as_str(), encode(&g)?.as_slice()))?;
        }
        m.routes = m.routes.checked_sub(1).ok_or(Error::Invalid)?;
        db(routes.remove(route.as_str()))?;
        save_meta(&tx, &m)?;
        drop(groups);
        drop(routes);
        self.live
            .lock()
            .map_err(|_| Error::Storage)?
            .remove(&Scope::Route(route.clone()));
        self.commit(tx)
    }
    pub fn remove_group(&self, group: &Digest) -> Result<()> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut groups = db(tx.open_table(GROUPS))?;
        let g: Group = read(&groups, group.as_str())?;
        if g.occupied > 0 || g.routes > 0 || probes::has_budget(&tx, group)? {
            return Err(Error::InUse);
        }
        m.groups = m.groups.checked_sub(1).ok_or(Error::Invalid)?;
        db(groups.remove(group.as_str()))?;
        save_meta(&tx, &m)?;
        drop(groups);
        self.live
            .lock()
            .map_err(|_| Error::Storage)?
            .remove(&Scope::Group(group.clone()));
        self.commit(tx)
    }
    pub fn begin_observation(&self, scope: Scope) -> Result<ObservationTicket> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let revision = sequence(&mut m)?;
        edit_gate(&tx, &scope, |gate| {
            if gate.live.is_some() {
                return Err(Error::InUse);
            }
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
            if gate.revision != ticket.revision || gate.live.is_some() {
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
        let allocation_groups = load_groups(&db(tx.open_table(GROUPS))?, &r)?;
        let m: Meta = read(&db(tx.open_table(META))?, "state")?;
        let remaining = self
            .limits
            .max_leases
            .saturating_sub(m.leases)
            .saturating_sub(m.probes)
            .min(u64::from(u32::MAX)) as u32;
        let availability = self
            .allowances(&allocation_groups, &r, self.now()?)
            .map(|a| a.free.min(remaining));
        let (available, state) = match availability {
            Ok(n) if n > 0 => (n, Readiness::Ready),
            Ok(_) | Err(Error::Busy) => (0, Readiness::Busy),
            Err(Error::Checking) => (0, Readiness::Checking),
            Err(Error::Unavailable) => (0, Readiness::Unavailable),
            Err(e) => return Err(e),
        };
        Ok(Status {
            group_occupied: allocation_groups[0].occupied,
            route_occupied: r.occupied,
            available,
            state,
        })
    }
    /// Allocation counters only, never a claim that a model is Ready.
    pub fn group_status(&self, group: &Digest) -> Result<GroupStatus> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let g: Group = read(&db(tx.open_table(GROUPS))?, group.as_str())?;
        Ok(GroupStatus {
            mode: g.mode,
            ceiling: g.ceiling,
            occupied: g.occupied,
            routes: g.routes,
        })
    }
    /// Atomic shared admission. Request hashes do not form idempotency identities;
    /// the invocation does, and must be scoped to the authenticated caller. Each
    /// sequential attempt receives a fresh lease only after the previous one closes.
    pub fn reserve(&self, route: &Digest, work: Work) -> Result<Reservation> {
        self.reserve_phase(route, work, Phase::Reserved)
    }
    pub(crate) fn reserve_proposal(&self, route: &Digest, work: Work) -> Result<Reservation> {
        self.reserve_phase(route, work, Phase::Proposed)
    }
    fn reserve_phase(&self, route: &Digest, work: Work, phase: Phase) -> Result<Reservation> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let mut work_index = db(tx.open_table(BY_WORK))?;
        if db(work_index.get(work.key().as_str()))?.is_some() {
            return Err(Error::ExistingWork);
        }
        if m.leases.saturating_add(m.probes) >= self.limits.max_leases {
            return Err(Error::Quota);
        }
        let mut groups = db(tx.open_table(GROUPS))?;
        let mut routes = db(tx.open_table(ROUTES))?;
        let mut r: RouteState = read(&routes, route.as_str())?;
        require(phase != Phase::Proposed || r.config.lane == Lane::Proxy)?;
        let mut allocation_groups = load_groups(&groups, &r)?;
        if self.allowances(&allocation_groups, &r, self.now()?)?.free == 0 {
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
            group: allocation_groups[0].id.clone(),
            route: route.clone(),
            work,
            controller_fence: self.fence,
            phase,
        };
        r.occupied = r.occupied.checked_add(1).ok_or(Error::Invalid)?;
        for group in &mut allocation_groups {
            group.occupied = group.occupied.checked_add(1).ok_or(Error::Invalid)?;
            db(groups.insert(group.id.as_str(), encode(group)?.as_slice()))?;
        }
        m.leases += 1;
        db(routes.insert(route.as_str(), encode(&r)?.as_slice()))?;
        db(leases.insert(lease.id.as_str(), encode(&lease)?.as_slice()))?;
        db(work_index.insert(lease.work.key().as_str(), lease.id.as_str()))?;
        db(db(tx.open_table(BY_GROUP))?.insert(group_key(&lease).as_str(), lease.id.as_str()))?;
        {
            let mut index = db(tx.open_table(BY_CONSTRAINT))?;
            for group in &r.constraints {
                db(index.insert(constraint_key(group, &lease.id).as_str(), lease.id.as_str()))?;
                m.constraint_leases = m.constraint_leases.checked_add(1).ok_or(Error::Invalid)?;
            }
        }
        save_meta(&tx, &m)?;
        drop(work_index);
        drop(leases);
        drop(groups);
        drop(routes);
        self.commit(tx)?;
        Ok(Reservation { lease })
    }

    /// Fresh read for provider countersigning. Does not dispatch or renew a lease;
    /// actual dispatch repeats the same checks under its write transaction.
    pub(crate) fn check_reserved(&self, expected: &Lease) -> Result<Route> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let lease: Lease = read(&db(tx.open_table(LEASES))?, expected.id.as_str())?;
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, lease.route.as_str())?;
        let allocation_groups = load_groups(&db(tx.open_table(GROUPS))?, &r)?;
        ready_reserved(
            &lease,
            expected,
            self.fence,
            &allocation_groups[0],
            &r,
            self.allowances(&allocation_groups, &r, self.now()?)?,
        )?;
        Ok(r.config)
    }

    /// Durable fence BEFORE any provider spend signature can be produced. A
    /// failed commit never returns permission to sign. Older ordinary reservations
    /// remain protected; replay returns the same already-fenced allocation.
    pub(crate) fn begin_signing(&self, expected: &Lease, buyer: &BuyerOffer) -> Result<Lease> {
        let tx = self.write()?;
        let mut leases = db(tx.open_table(LEASES))?;
        let mut lease: Lease = read(&leases, expected.id.as_str())?;
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, lease.route.as_str())?;
        let allocation_groups = load_groups(&db(tx.open_table(GROUPS))?, &r)?;
        ready_reserved(
            &lease,
            expected,
            self.fence,
            &allocation_groups[0],
            &r,
            self.allowances(&allocation_groups, &r, self.now()?)?,
        )?;
        require(r.config.lane == Lane::Proxy)?;
        validate_signing_intent(buyer, &lease, &self.identity)?;
        let bytes = serde_json::to_vec(buyer).map_err(|_| Error::Invalid)?;
        require(bytes.len() <= MAX_SIGNING_BYTES)?;
        let mut signing = db(tx.open_table(SIGNING))?;
        if let Some(saved) = db(signing.get(lease.id.as_str()))? {
            require(
                decode_signing(saved.value(), &lease, &self.identity)? == *buyer
                    && lease.phase == Phase::Reserved,
            )?;
            return Ok(lease);
        }
        db(signing.insert(lease.id.as_str(), bytes.as_slice()))?;
        lease.phase = Phase::Reserved;
        db(leases.insert(lease.id.as_str(), encode(&lease)?.as_slice()))?;
        drop(signing);
        drop(leases);
        self.commit(tx)?;
        Ok(lease)
    }

    /// Indexed, durability-fenced read; survives failure before the execution
    /// journal was prepared. No inference, history read or health renewal.
    pub fn signing_intent(&self, id: &Digest) -> Result<Option<SigningIntent>> {
        let _guard = self.writer.lock().map_err(|_| Error::Storage)?;
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let table = db(tx.open_table(SIGNING))?;
        let Some(saved) = db(table.get(id.as_str()))? else {
            return Ok(None);
        };
        let lease: Lease = read(&db(tx.open_table(LEASES))?, id.as_str())?;
        let route: RouteState = read(&db(tx.open_table(ROUTES))?, lease.route.as_str())?;
        require(
            route.config.lane == Lane::Proxy
                && route.config.group == lease.group
                && lease.phase != Phase::Proposed,
        )?;
        let buyer = decode_signing(saved.value(), &lease, &self.identity)?;
        Ok(Some(SigningIntent {
            lease: effective(lease, self.fence),
            buyer,
        }))
    }

    /// The journal has durably forbidden dispatch and retained canonical absence.
    /// Replays after a crash between release and journal closure are harmless.
    pub(crate) fn release_retired(
        &self,
        retired: &crate::attempts::retirement::Retirement,
    ) -> Result<bool> {
        retired
            .validate(&self.identity)
            .map_err(|_| Error::Binding)?;
        let expected = retired.lease();
        let tx = self.write()?;
        let leases = db(tx.open_table(LEASES))?;
        let Some(raw) = db(leases.get(expected.id.as_str()))? else {
            return Ok(false);
        };
        let actual: Lease = decode(raw.value())?;
        require(
            actual.id == expected.id
                && actual.group == expected.group
                && actual.route == expected.route
                && actual.work == expected.work
                && actual.controller_fence == expected.controller_fence,
        )?;
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, actual.route.as_str())?;
        require(r.config.lane == Lane::Proxy && r.config.group == actual.group)?;
        let signing = db(tx.open_table(SIGNING))?;
        let saved = db(signing.get(actual.id.as_str()))?.ok_or(Error::Binding)?;
        require(decode_signing(saved.value(), &actual, &self.identity)? == *retired.buyer())?;
        drop(saved);
        drop(signing);
        drop(raw);
        drop(leases);
        release(&tx, &actual)?;
        self.commit(tx)?;
        Ok(true)
    }

    pub(crate) fn route_group(&self, route: &Digest) -> Result<Digest> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, route.as_str())?;
        Ok(r.config.group)
    }

    /// Only a prior-controller Proposed record proves signing and dispatch never
    /// began. No ledger read, elapsed-time guess or missing journal inference.
    /// Legacy Reserved, native and dispatched/uncertain records are not reclaimed.
    pub(crate) fn reclaim_old_proposal(&self, expected: &Lease, route: &Digest) -> Result<bool> {
        let tx = self.write()?;
        let leases = db(tx.open_table(LEASES))?;
        let Some(raw) = db(leases.get(expected.id.as_str()))? else {
            return Ok(false);
        };
        let actual: Lease = decode(raw.value())?;
        if effective(actual.clone(), self.fence) != *expected || actual.route != *route {
            return Err(Error::Binding);
        }
        if actual.controller_fence >= self.fence || actual.phase != Phase::Proposed {
            return Ok(false);
        }
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, route.as_str())?;
        require(r.config.lane == Lane::Proxy && r.config.group == actual.group)?;
        drop(raw);
        drop(leases);
        release(&tx, &actual)?;
        self.commit(tx)?;
        Ok(true)
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
        let r: RouteState = read(&db(tx.open_table(ROUTES))?, lease.route.as_str())?;
        let allocation_groups = load_groups(&db(tx.open_table(GROUPS))?, &r)?;
        ready_reserved(
            &lease,
            &reservation.lease,
            self.fence,
            &allocation_groups[0],
            &r,
            self.allowances(&allocation_groups, &r, self.now()?)?,
        )?;
        require(lease.phase == Phase::Reserved)?;
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

    /// Only the still-held, unsigned, never-dispatched capability cancels without external
    /// evidence. Dropping it is intentionally NOT a cancellation or a free slot.
    pub fn cancel_reserved(&self, reservation: Reservation) -> Result<()> {
        self.cancel_reserved_ref(&reservation)
    }
    // Controller retains the capability until durability succeeds, so an I/O
    // error cannot erase its pending cleanup entry. No public forged capability.
    pub(crate) fn cancel_reserved_ref(&self, reservation: &Reservation) -> Result<()> {
        if reservation.lease.controller_fence != self.fence {
            return Err(Error::Stale);
        }
        let tx = self.write()?;
        let actual: Lease = read(&db(tx.open_table(LEASES))?, reservation.lease.id.as_str())?;
        if actual != reservation.lease || !matches!(actual.phase, Phase::Reserved | Phase::Proposed)
        {
            return Err(Error::Binding);
        }
        if db(db(tx.open_table(SIGNING))?.get(actual.id.as_str()))?.is_some() {
            return Err(Error::InUse);
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
        let primary = db(tx.open_table(BY_GROUP))?;
        let additional = db(tx.open_table(BY_CONSTRAINT))?;
        let routes = db(tx.open_table(ROUTES))?;
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
        let mut found = BTreeMap::new();
        for (index, extra) in [(&primary, false), (&additional, true)] {
            for row in
                db(index.range::<&str>((start_bound.clone(), Bound::Excluded(end.as_str()))))?
                    .take(limit + 1)
            {
                let (key, id) = db(row)?;
                let lease: Lease = read(&leases, id.value())?;
                require(
                    lease.id.as_str() == id.value()
                        && key.value() == constraint_key(group, &lease.id),
                )?;
                if extra {
                    let route: RouteState = read(&routes, lease.route.as_str())?;
                    validate_constraints(&route.config, &route.constraints)?;
                    require(
                        route.config.group == lease.group
                            && lease.group != *group
                            && route.constraints.contains(group),
                    )?;
                } else {
                    require(lease.group == *group)?;
                }
                require(
                    found
                        .insert(lease.id.clone(), effective(lease, self.fence))
                        .is_none(),
                )?;
            }
        }
        let mut rows = found.into_values().collect::<Vec<_>>();
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
fn ready_reserved(
    lease: &Lease,
    expected: &Lease,
    fence: u64,
    g: &Group,
    r: &RouteState,
    allowances: Allowances,
) -> Result<()> {
    if expected.controller_fence != fence {
        return Err(Error::Stale);
    }
    let mut comparable = expected.clone();
    if comparable.phase == Phase::Proposed && lease.phase == Phase::Reserved {
        comparable.phase = Phase::Reserved;
    }
    if lease != &comparable
        || !matches!(lease.phase, Phase::Reserved | Phase::Proposed)
        || lease.group != g.id
        || lease.route != r.config.id
        || r.config.group != g.id
    {
        return Err(Error::Binding);
    }
    // Reservations already count in each applicable constraint exactly once.
    if !allowances.fits {
        return Err(Error::Busy);
    }
    Ok(())
}

struct Allowances {
    free: u32,
    fits: bool,
}
fn recovery_allowance(value: Result<u32>, recovering: bool, ceiling: u32) -> Result<u32> {
    match value {
        // Only an excluded gate in the explicitly recovered scope is bypassed.
        // Healthy observed allowances and unrelated failed groups remain binding.
        Err(Error::Busy | Error::Unavailable | Error::Checking) if recovering => Ok(ceiling),
        value => value,
    }
}
fn validate_constraints(route: &Route, constraints: &[Digest]) -> Result<()> {
    require(
        constraints.len() <= MAX_CONSTRAINTS
            && !constraints.contains(&route.group)
            && constraints.windows(2).all(|w| w[0] < w[1]),
    )
}
fn group_ids(route: &RouteState) -> Result<Vec<Digest>> {
    validate_constraints(&route.config, &route.constraints)?;
    let mut ids = Vec::with_capacity(route.constraints.len() + 1);
    ids.push(route.config.group.clone());
    ids.extend(route.constraints.iter().cloned());
    Ok(ids)
}
fn load_groups(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    route: &RouteState,
) -> Result<Vec<Group>> {
    group_ids(route)?
        .iter()
        .map(|id| {
            let group: Group = read(table, id.as_str())?;
            require(group.id == *id)?;
            Ok(group)
        })
        .collect()
}
fn constraint_key(group: &Digest, lease: &Digest) -> String {
    format!("{}/{}", group.as_str(), lease.as_str())
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
            require(g.mode == GroupMode::Observed)?;
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
    let mut r: RouteState = read(&routes, l.route.as_str())?;
    require(r.config.group == l.group)?;
    for mut group in load_groups(&groups, &r)? {
        group.occupied = group.occupied.checked_sub(1).ok_or(Error::Invalid)?;
        db(groups.insert(group.id.as_str(), encode(&group)?.as_slice()))?;
    }
    {
        let mut index = db(tx.open_table(BY_CONSTRAINT))?;
        for group in &r.constraints {
            let key = constraint_key(group, &l.id);
            require(db(index.get(key.as_str()))?.is_some_and(|v| v.value() == l.id.as_str()))?;
            db(index.remove(key.as_str()))?;
            m.constraint_leases = m.constraint_leases.checked_sub(1).ok_or(Error::Invalid)?;
        }
    }
    r.occupied = r.occupied.checked_sub(1).ok_or(Error::Invalid)?;
    m.leases = m.leases.checked_sub(1).ok_or(Error::Invalid)?;
    db(routes.insert(r.config.id.as_str(), encode(&r)?.as_slice()))?;
    db(db(tx.open_table(LEASES))?.remove(l.id.as_str()))?;
    db(db(tx.open_table(BY_GROUP))?.remove(group_key(l).as_str()))?;
    db(db(tx.open_table(BY_WORK))?.remove(l.work.key().as_str()))?;
    db(db(tx.open_table(SIGNING))?.remove(l.id.as_str()))?;
    save_meta(tx, &m)
}

fn validate_signing_intent(buyer: &BuyerOffer, lease: &Lease, id: &Identity) -> Result<()> {
    buyer.verify().map_err(|_| Error::Invalid)?;
    let t = &buyer.terms;
    require(
        t.capacity_lease == lease.id.as_str()
            && t.request_hash == lease.work.request_hash.as_str()
            && t.network_id == id.network_id
            && t.msb_bootstrap == id.msb_bootstrap.as_str()
            && t.subnet_bootstrap == id.subnet_bootstrap.as_str()
            && t.offer.provider_pubkey == id.controller_pubkey.as_str()
            && crate::exchange::invocation_for_terms(t).map_err(|_| Error::Invalid)?
                == lease.work.invocation,
    )
}
fn decode_signing(bytes: &[u8], lease: &Lease, id: &Identity) -> Result<BuyerOffer> {
    require(bytes.len() <= MAX_SIGNING_BYTES)?;
    let buyer: BuyerOffer = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    validate_signing_intent(&buyer, lease, id)?;
    Ok(buyer)
}
