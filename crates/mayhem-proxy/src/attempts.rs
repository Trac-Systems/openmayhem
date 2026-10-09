//! Trusted parent's local dispatch journal; never a connector or ledger authority.
//!
//! Commit dispatch intent BEFORE POST, and first-delivery intent BEFORE delivery.
//! Losing an acknowledgment leaves uncertainty, not permission for a fresh POST.
//! Only independently checked evidence may resolve an attempt or acknowledge its
//! financial/capacity closure. Hashes here bind that evidence; they do not verify it.
//! Private bounded request/result payloads live in separate owned tables. They
//! are not diagnostic logs. No credentials or connector-authored monetary values.

mod acceptance;
mod finance;
mod non_execution;
mod outcomes;
mod payloads;
mod provider;
pub(crate) mod retirement;
pub(crate) use acceptance::validate_offer_binding;
pub use acceptance::{AcceptanceSnapshot, OwnedAcceptance};
pub use non_execution::NonExecutionEvidence;
pub use outcomes::{TerminalDraft, WaiverDraft};
pub(crate) use payloads::RecoveryHeader;
pub use payloads::{OwnedRequest, OwnedResult, Recovery, ResultCommitment};
pub use provider::SignedProviderAcceptance;

use std::{
    fmt,
    ops::Bound,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard,
    },
};

use crate::connector::failure::{
    safe_parameter, safe_upstream_code, Code, Execution, Failure, Scope, Stage,
};
use mayhem_proto::proxy::{ProxyEndpoint, ProxyRail};
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    TableHandle,
};
use serde::{Deserialize, Deserializer, Serialize};

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_attempt_meta_v1");
const REQUESTS: TableDefinition<&str, u64> = TableDefinition::new("proxy_attempt_requests_v1");
const RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_attempt_records_v1");
const UNFINISHED: TableDefinition<&str, u64> = TableDefinition::new("proxy_attempt_unfinished_v1");
const EXPIRY: TableDefinition<&str, &str> = TableDefinition::new("proxy_attempt_expiry_v1");
const MAX_RECORD_BYTES: usize = 8192;
const MAX_PAGE: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid proxy attempt data")]
    Invalid,
    #[error("proxy attempt journal belongs to another network or controller")]
    Identity,
    #[error("idempotency key already belongs to a different request")]
    Conflict,
    #[error("proxy attempt changed; read its current state")]
    Stale,
    #[error("proxy attempt transition is not allowed")]
    Transition,
    #[error("proxy attempt was not found")]
    NotFound,
    #[error("proxy attempt journal is full; no new dispatch is allowed")]
    Capacity,
    #[error("proxy attempt storage failed; dispatch must remain stopped")]
    Storage,
    #[error("proxy attempt journal requires a private regular file")]
    File,
    #[error("proxy attempt journal file protection is not supported on this platform")]
    UnsupportedFileProtection,
}
pub type Result<T> = std::result::Result<T, Error>;

fn storage<T, E>(r: std::result::Result<T, E>) -> Result<T> {
    r.map_err(|_| Error::Storage)
}
fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Digest(String);

impl Digest {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        require(
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        )?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn hash(domain: &'static str, parts: &[&[u8]]) -> Self {
        let mut h = blake3::Hasher::new_derive_key(domain);
        for p in parts {
            h.update(&(p.len() as u64).to_le_bytes());
            h.update(p);
        }
        Self(h.finalize().to_hex().to_string())
    }
}
impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// Caller is the authenticated principal, never a user-supplied display name.
/// Supply a fresh opaque key per invocation and reuse it only for that invocation.
/// Identical prompt text alone is deliberately not the idempotency identity.
pub fn invocation_key(
    caller: &Digest,
    endpoint: ProxyEndpoint,
    opaque_key: &[u8],
) -> Result<Digest> {
    require(!opaque_key.is_empty() && opaque_key.len() <= 256)?;
    let endpoint = serde_json::to_vec(&endpoint).map_err(|_| Error::Invalid)?;
    Ok(Digest::hash(
        "mayhem/proxy/local-invocation/v1",
        &[caller.0.as_bytes(), &endpoint, opaque_key],
    ))
}

/// Stable identity deliberately excludes a release/contract version. Historical
/// attempts retain their accepted contract version and survive an ordinary update.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub network_id: String,
    pub msb_bootstrap: Digest,
    pub subnet_bootstrap: Digest,
    pub controller_pubkey: Digest,
}
impl Identity {
    pub(crate) fn validate(&self) -> Result<()> {
        require(
            !self.network_id.is_empty()
                && self.network_id.len() <= 128
                && self
                    .network_id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b)),
        )
    }
}

/// Commitments resolve to retained, verified parent records. A connector cannot
/// provide these to authorize itself. `accepted_terms` includes the exact offer,
/// limits, rail backing and cumulative exposure; the journal cannot price a retry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub request_hash: Digest,
    pub endpoint: ProxyEndpoint,
    pub contract_version: u32,
    pub provider_pubkey: Digest,
    pub market_id: Digest,
    pub offer_digest: Digest,
    pub endpoint_contract: Digest,
    pub metering_policy: Digest,
    pub accepted_terms: Digest,
    pub reservation: Digest,
    pub capacity_lease: Digest,
    pub connection_digest: Digest,
    pub connection_revision: u64,
    pub recipe_digest: Digest,
    pub rail: ProxyRail,
}
impl Binding {
    fn validate(&self) -> Result<()> {
        require(self.contract_version > 0 && self.connection_revision > 0)
    }
    fn same_request(&self, other: &Self) -> bool {
        self.request_hash == other.request_hash && self.endpoint == other.endpoint
    }
}

/// Private opaque ID; never a URL or public diagnostic. Poll adapters must encode
/// it into an approved endpoint, not follow it as a destination.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RemoteId(String);
impl RemoteId {
    pub fn new(s: impl Into<String>) -> Result<Self> {
        let s = s.into();
        require(!s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))?;
        Ok(Self(s))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for RemoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteId([redacted])")
    }
}
impl<'de> Deserialize<'de> for RemoteId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    Dispatched,
    Resolved,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedResult {
    /// Retained owned result, validated against the endpoint contract, including
    /// response shape. Not arbitrary JSON supplied by a connector.
    pub result: Digest,
    pub usage_evidence: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Resolution {
    NotExecuted {
        evidence: Digest,
    },
    Completed {
        verified: VerifiedResult,
    },
    Cancelled {
        evidence: Digest,
        partial: Option<VerifiedResult>,
    },
    Failed {
        evidence: Digest,
        partial: Option<VerifiedResult>,
    },
}

/// Latest bounded diagnostic, attached to the invocation/attempt on disk. No
/// vendor text. Only strictly validated parent nonexecution evidence can resolve
/// execution; a failure observation alone never resolves customer money.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureSnapshot {
    pub code: Code,
    pub scope: Scope,
    pub stage: Stage,
    pub execution: Execution,
    pub upstream_status: Option<u16>,
    pub upstream_code: Option<String>,
    pub parameter: Option<String>,
    pub retry_after_ms: Option<u64>,
}
impl From<&Failure> for FailureSnapshot {
    fn from(f: &Failure) -> Self {
        Self {
            code: f.code,
            scope: f.scope,
            stage: f.stage,
            execution: f.execution,
            upstream_status: f.upstream_status,
            upstream_code: f.upstream_code.map(str::to_owned),
            parameter: f.parameter.map(str::to_owned),
            retry_after_ms: f.retry_after_ms,
        }
    }
}
impl FailureSnapshot {
    pub fn to_failure(&self) -> Result<Failure> {
        require(
            self.upstream_status
                .is_none_or(|s| (100..=599).contains(&s))
                && self
                    .upstream_code
                    .as_deref()
                    .is_none_or(|s| safe_upstream_code(s).is_some())
                && self
                    .parameter
                    .as_deref()
                    .is_none_or(|s| safe_parameter(s).is_some()),
        )?;
        let failure = Failure {
            code: self.code,
            scope: self.scope,
            stage: self.stage,
            execution: self.execution,
            upstream_status: self.upstream_status,
            upstream_code: self.upstream_code.as_deref().and_then(safe_upstream_code),
            parameter: self.parameter.as_deref().and_then(safe_parameter),
            retry_after_ms: self.retry_after_ms,
        };
        require(failure.execution != Execution::Rejected || failure.verified_rejection())?;
        Ok(failure)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub invocation: Digest,
    /// Unique local attempt ID, not a per-request attempt counter. Never reused,
    /// even after retention expiry or a change to the configured retention period.
    pub attempt: u64,
    pub generation: u64,
    pub binding: Binding,
    pub phase: Phase,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub remote_id: Option<RemoteId>,
    /// Delivery intent is conservative across a crash before the actual send.
    pub output_may_have_been_delivered: bool,
    pub cancellation_requested: bool,
    pub last_failure: Option<FailureSnapshot>,
    pub resolution: Option<Resolution>,
    /// Financial/capacity reconciliation acknowledgment, NOT a connector finish.
    pub closure: Option<Digest>,
    pub expires_at_ms: Option<u64>,
}
impl Record {
    /// Only the journal's pre-dispatch cancellation transition emits this evidence.
    /// A transport timeout or a generic financial closure cannot substitute for it.
    pub(crate) fn unsent_cancellation_evidence(&self) -> Option<Digest> {
        if !self.cancellation_requested || !matches!(self.phase, Phase::Resolved | Phase::Closed) {
            return None;
        }
        let expected = unsent_commitment(&self.invocation, self.attempt);
        match &self.resolution {
            Some(Resolution::NotExecuted { evidence }) if evidence == &expected => Some(expected),
            _ => None,
        }
    }

    fn validate(&self) -> Result<()> {
        self.binding.validate()?;
        require(
            self.attempt > 0 && self.generation > 0 && self.created_at_ms <= self.updated_at_ms,
        )?;
        if let Some(f) = &self.last_failure {
            if f.to_failure()?.known_non_execution()
                && matches!(self.phase, Phase::Resolved | Phase::Closed)
            {
                require(self.failure_non_execution().is_some())?;
            }
        }
        require(match self.phase {
            Phase::Prepared => {
                self.remote_id.is_none()
                    && !self.output_may_have_been_delivered
                    && self.resolution.is_none()
                    && !self.cancellation_requested
                    && self.closure.is_none()
                    && self.expires_at_ms.is_none()
            }
            Phase::Dispatched => {
                self.resolution.is_none() && self.closure.is_none() && self.expires_at_ms.is_none()
            }
            Phase::Resolved => {
                self.resolution.is_some() && self.closure.is_none() && self.expires_at_ms.is_none()
            }
            Phase::Closed => {
                self.resolution.is_some()
                    && self.closure.is_some()
                    && self.expires_at_ms.is_some_and(|t| t >= self.updated_at_ms)
            }
        })?;
        require(
            !matches!(self.resolution, Some(Resolution::NotExecuted { .. }))
                || !self.output_may_have_been_delivered,
        )
    }
    fn key(&self) -> String {
        record_key(&self.invocation, self.attempt)
    }
}
fn record_key(invocation: &Digest, attempt: u64) -> String {
    format!("{}:{attempt:020}", invocation.0)
}
pub(crate) fn unsent_commitment(invocation: &Digest, attempt: u64) -> Digest {
    Digest::hash(
        "mayhem/proxy/local-cancel-before-dispatch/v1",
        &[invocation.0.as_bytes(), &attempt.to_le_bytes()],
    )
}
fn expiry_key(record: &Record) -> String {
    format!(
        "{:020}:{}",
        record.expires_at_ms.expect("closed record"),
        record.key()
    )
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Retained attempts including completed ones, not a lifetime request limit.
    pub max_records: u64,
    pub max_unfinished: u64,
    /// Explicit caller policy; no implicit commercial retention promise.
    pub closed_retention_ms: u64,
    /// Logical bytes held for private request/results, including reserved result
    /// space before dispatch. Filesystem pages and journal overhead are additional.
    pub max_payload_bytes: u64,
}
impl Limits {
    fn validate(&self) -> Result<()> {
        require(
            self.max_records > 0
                && self.max_unfinished > 0
                && self.max_unfinished <= self.max_records
                && self.closed_retention_ms > 0,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    schema: u32,
    identity: Identity,
    records: u64,
    unfinished: u64,
    generation: u64,
    #[serde(default)]
    payload_bytes: u64,
}

/// Not Clone/Serialize: only a successful fresh dispatch transition issues one.
/// Future worker dispatch must consume this ticket, not reconstruct it from reads.
#[derive(Debug)]
pub struct DispatchTicket {
    record: Record,
}
impl DispatchTicket {
    pub fn record(&self) -> &Record {
        &self.record
    }
}

pub enum Event {
    Accepted(RemoteId),
    FirstOutput,
    CancelRequested,
    Failure(FailureSnapshot),
    Resolve(Resolution),
    /// Supplied only after the parent reconciles money AND backend capacity.
    Close(Digest),
}

#[derive(Debug)]
pub struct Page {
    pub records: Vec<Record>,
    pub next_after: Option<Digest>,
}

pub struct Journal {
    database: Database,
    limits: Limits,
    commit_failed: AtomicBool,
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

impl Journal {
    /// Requires an existing private parent directory. All IO is blocking; a Core
    /// async caller must use its bounded storage executor. Never open on a token.
    /// redb owns an exclusive process lock. No auto-reset on corruption/mismatch.
    pub fn open(path: impl AsRef<Path>, identity: Identity, limits: Limits) -> Result<Self> {
        identity.validate()?;
        limits.validate()?;
        let file = private_file(path.as_ref())?;
        let mut builder = Database::builder();
        builder.set_cache_size(8 * 1024 * 1024);
        let database = storage(builder.create_file(file))?;
        Self::initialize(database, identity, limits)
    }

    fn initialize(database: Database, identity: Identity, limits: Limits) -> Result<Self> {
        let mut tx = storage(database.begin_write())?;
        storage(tx.set_durability(Durability::Immediate))?;
        let names = storage(tx.list_tables())?
            .map(|t| t.name().to_owned())
            .collect::<Vec<_>>();
        let mut meta_table = storage(tx.open_table(META))?;
        let meta = storage(meta_table.get("state"))?
            .map(|v| decode::<Meta>(v.value()))
            .transpose()?;
        if let Some(mut meta) = meta {
            if meta.identity != identity {
                return Err(Error::Identity);
            }
            require(matches!(meta.schema, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8))?;
            require(
                [
                    REQUESTS.name(),
                    RECORDS.name(),
                    UNFINISHED.name(),
                    EXPIRY.name(),
                ]
                .iter()
                .all(|n| names.iter().any(|s| s == n)),
            )?;
            require(
                storage(tx.open_table(RECORDS))?
                    .len()
                    .map_err(|_| Error::Storage)?
                    == meta.records,
            )?;
            require(
                storage(tx.open_table(UNFINISHED))?
                    .len()
                    .map_err(|_| Error::Storage)?
                    == meta.unfinished,
            )?;
            require(
                storage(tx.open_table(EXPIRY))?
                    .len()
                    .map_err(|_| Error::Storage)?
                    == meta
                        .records
                        .checked_sub(meta.unfinished)
                        .ok_or(Error::Invalid)?,
            )?;
            payloads::initialize(&tx, meta.schema == 1)?;
            acceptance::initialize(&tx, meta.schema < 3)?;
            finance::initialize(&tx, meta.schema < 4)?;
            outcomes::initialize(&tx, meta.schema < 5)?;
            provider::initialize(&tx, meta.schema < 7)?;
            retirement::initialize(&tx, meta.schema < 8)?;
            if meta.schema == 1 {
                require(meta.payload_bytes == 0)?;
            }
            if meta.schema < 8 {
                meta.schema = 8;
                storage(meta_table.insert("state", encode(&meta)?.as_slice()))?;
            }
        } else {
            require(names.is_empty())?;
            storage(tx.open_table(REQUESTS))?;
            storage(tx.open_table(RECORDS))?;
            storage(tx.open_table(UNFINISHED))?;
            storage(tx.open_table(EXPIRY))?;
            payloads::initialize(&tx, true)?;
            acceptance::initialize(&tx, true)?;
            finance::initialize(&tx, true)?;
            outcomes::initialize(&tx, true)?;
            provider::initialize(&tx, true)?;
            retirement::initialize(&tx, true)?;
            let meta = Meta {
                schema: 8,
                identity,
                records: 0,
                unfinished: 0,
                generation: 0,
                payload_bytes: 0,
            };
            storage(meta_table.insert("state", encode(&meta)?.as_slice()))?;
        }
        drop(meta_table);
        storage(tx.commit())?;
        Ok(Self {
            database,
            limits,
            commit_failed: AtomicBool::new(false),
            write_lock: Mutex::new(()),
        })
    }

    fn transaction(&self) -> Result<Transaction<'_>> {
        // Retain this guard until an uncertain commit has latched the failure.
        // redb alone releases its writer lock before commit() returns to us.
        let guard = self.write_lock.lock().map_err(|_| Error::Storage)?;
        if self.commit_failed.load(Ordering::Acquire) {
            return Err(Error::Storage);
        }
        let mut tx = storage(self.database.begin_write())?;
        storage(tx.set_durability(Durability::Immediate))?;
        Ok(Transaction { tx, _guard: guard })
    }

    fn commit(&self, transaction: Transaction<'_>) -> Result<()> {
        let Transaction { tx, _guard } = transaction;
        if tx.commit().is_err() {
            self.commit_failed.store(true, Ordering::Release);
            return Err(Error::Storage);
        }
        Ok(())
    }

    pub(crate) fn identity(&self) -> Result<Identity> {
        let tx = storage(self.database.begin_read())?;
        let table = storage(tx.open_table(META))?;
        let raw = storage(table.get("state"))?.ok_or(Error::Invalid)?;
        Ok(decode::<Meta>(raw.value())?.identity)
    }

    /// First acceptance only. Same-key replay returns the pinned attempt even if
    /// current price/config changed. Changed request body/endpoint is a conflict.
    pub fn prepare(&self, invocation: Digest, binding: Binding, now_ms: u64) -> Result<Record> {
        binding.validate()?;
        let tx = self.transaction()?;
        let existing = current(&tx, &invocation)?;
        if let Some(old) = existing {
            if !old.binding.same_request(&binding) {
                return Err(Error::Conflict);
            }
            return Ok(old);
        }
        let record = self.insert(&tx, invocation, binding, now_ms)?;
        self.commit(tx)?;
        Ok(record)
    }

    /// Only a fully reconciled known-nonexecution permits replacement here.
    /// Upstream deduplicated resume is the SAME attempt and needs no new ticket.
    /// Parent must separately authorize the new provider/offer/cumulative budget.
    pub fn replace_not_executed(
        &self,
        invocation: &Digest,
        generation: u64,
        binding: Binding,
        now_ms: u64,
    ) -> Result<Record> {
        binding.validate()?;
        let tx = self.transaction()?;
        let old = checked_current(&tx, invocation, generation, now_ms)?;
        if !old.binding.same_request(&binding) {
            return Err(Error::Conflict);
        }
        if old.phase != Phase::Closed
            || old.cancellation_requested
            || !matches!(old.resolution, Some(Resolution::NotExecuted { .. }))
        {
            return Err(Error::Transition);
        }
        let record = self.insert(&tx, invocation.clone(), binding, now_ms)?;
        self.commit(tx)?;
        Ok(record)
    }

    fn insert(
        &self,
        tx: &redb::WriteTransaction,
        invocation: Digest,
        binding: Binding,
        now_ms: u64,
    ) -> Result<Record> {
        let mut meta = metadata(tx)?;
        if meta.records >= self.limits.max_records || meta.unfinished >= self.limits.max_unfinished
        {
            return Err(Error::Capacity);
        }
        meta.generation = meta.generation.checked_add(1).ok_or(Error::Invalid)?;
        let attempt = meta.generation;
        let record = Record {
            invocation,
            attempt,
            generation: meta.generation,
            binding,
            phase: Phase::Prepared,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            remote_id: None,
            output_may_have_been_delivered: false,
            cancellation_requested: false,
            last_failure: None,
            resolution: None,
            closure: None,
            expires_at_ms: None,
        };
        let mut requests = storage(tx.open_table(REQUESTS))?;
        storage(requests.insert(record.invocation.as_str(), attempt))?;
        let mut unfinished = storage(tx.open_table(UNFINISHED))?;
        require(storage(unfinished.insert(record.invocation.as_str(), attempt))?.is_none())?;
        meta.records = meta.records.checked_add(1).ok_or(Error::Invalid)?;
        meta.unfinished = meta.unfinished.checked_add(1).ok_or(Error::Invalid)?;
        save_meta(tx, &meta)?;
        save(tx, &record)?;
        Ok(record)
    }

    /// Commit-before-send fence. A second worker/stale restart cannot get another
    /// fresh dispatch ticket for this attempt, even when no remote ID is known.
    pub fn begin_dispatch(
        &self,
        invocation: &Digest,
        generation: u64,
        now_ms: u64,
    ) -> Result<DispatchTicket> {
        let tx = self.transaction()?;
        let mut record = checked_current(&tx, invocation, generation, now_ms)?;
        if record.phase != Phase::Prepared {
            return Err(Error::Transition);
        }
        record.phase = Phase::Dispatched;
        bump(&tx, &mut record, now_ms)?;
        save(&tx, &record)?;
        self.commit(tx)?;
        Ok(DispatchTicket { record })
    }

    /// Meaningful lifecycle transitions only, no token/chunk journal. HTTP status
    /// or EOF is not a Resolution. Unknown failures remain Dispatched/recoverable.
    pub fn advance(
        &self,
        invocation: &Digest,
        generation: u64,
        event: Event,
        now_ms: u64,
    ) -> Result<Record> {
        let tx = self.transaction()?;
        let mut r = checked_current(&tx, invocation, generation, now_ms)?;
        match event {
            Event::Failure(failure)
                if r.phase == Phase::Resolved
                    && r.last_failure.as_ref() == Some(&failure)
                    && r.failure_non_execution().is_some() =>
            {
                return Ok(r)
            }
            Event::Failure(failure) if r.phase == Phase::Dispatched => {
                if failure.execution == Execution::NotDispatched
                    && (r.remote_id.is_some() || r.output_may_have_been_delivered)
                {
                    return Err(Error::Transition);
                }
                if r.last_failure.as_ref() == Some(&failure) {
                    return Ok(r);
                }
                non_execution::resolve_failure(&tx, &mut r, failure)?;
            }
            Event::Accepted(id) if r.phase == Phase::Dispatched => {
                if r.remote_id.as_ref().is_some_and(|old| old != &id) {
                    return Err(Error::Conflict);
                }
                if r.remote_id.is_some() {
                    return Ok(r);
                }
                r.remote_id = Some(id);
            }
            Event::FirstOutput if r.phase == Phase::Dispatched && !r.cancellation_requested => {
                if r.output_may_have_been_delivered {
                    return Ok(r);
                }
                r.output_may_have_been_delivered = true;
            }
            Event::CancelRequested if r.phase != Phase::Closed => {
                // Once known terminal evidence has fixed the outcome/waiver,
                // cancellation cannot retroactively relabel finished work.
                if r.phase == Phase::Resolved && outcomes::has_intent(&tx, &r.key())? {
                    return Ok(r);
                }
                if r.cancellation_requested {
                    return Ok(r);
                }
                r.cancellation_requested = true;
                if r.phase == Phase::Prepared {
                    r.phase = Phase::Resolved;
                    r.resolution = Some(Resolution::NotExecuted {
                        evidence: unsent_commitment(&r.invocation, r.attempt),
                    });
                }
            }
            Event::Resolve(resolution) if r.phase == Phase::Dispatched => {
                payloads::verify_resolution(&tx, &r.key(), &resolution)?;
                if matches!(resolution, Resolution::NotExecuted { .. })
                    && r.output_may_have_been_delivered
                {
                    return Err(Error::Transition);
                }
                r.phase = Phase::Resolved;
                r.resolution = Some(resolution);
            }
            Event::Close(ack) if r.phase == Phase::Resolved => {
                r.phase = Phase::Closed;
                r.closure = Some(ack);
                r.expires_at_ms = Some(
                    now_ms
                        .checked_add(self.limits.closed_retention_ms)
                        .ok_or(Error::Invalid)?,
                );
                let mut unfinished = storage(tx.open_table(UNFINISHED))?;
                require(
                    storage(unfinished.remove(invocation.as_str()))?
                        .is_some_and(|v| v.value() == r.attempt),
                )?;
                let mut meta = metadata(&tx)?;
                meta.unfinished = meta.unfinished.checked_sub(1).ok_or(Error::Invalid)?;
                save_meta(&tx, &meta)?;
                storage(tx.open_table(EXPIRY))?
                    .insert(expiry_key(&r).as_str(), r.key().as_str())
                    .map_err(|_| Error::Storage)?;
            }
            _ => return Err(Error::Transition),
        }
        bump(&tx, &mut r, now_ms)?;
        save(&tx, &r)?;
        self.commit(tx)?;
        Ok(r)
    }

    pub fn get(&self, invocation: &Digest) -> Result<Option<Record>> {
        let tx = storage(self.database.begin_read())?;
        let requests = storage(tx.open_table(REQUESTS))?;
        let Some(attempt) = storage(requests.get(invocation.as_str()))? else {
            return Ok(None);
        };
        let records = storage(tx.open_table(RECORDS))?;
        Ok(Some(read_record(&records, invocation, attempt.value())?))
    }

    pub fn get_attempt(&self, invocation: &Digest, attempt: u64) -> Result<Option<Record>> {
        require(attempt > 0)?;
        let tx = storage(self.database.begin_read())?;
        let records = storage(tx.open_table(RECORDS))?;
        if storage(records.get(record_key(invocation, attempt).as_str()))?.is_none() {
            return Ok(None);
        }
        Ok(Some(read_record(&records, invocation, attempt)?))
    }

    /// Only unfinished entries, including resolved results awaiting financial ACK.
    /// Cursor is a seek key, not a held transaction/snapshot. Periodic recovery must
    /// wrap to None to include newly inserted keys before a previous cursor.
    pub fn recovery_page(&self, after: Option<&Digest>, limit: usize) -> Result<Page> {
        require((1..=MAX_PAGE).contains(&limit))?;
        let tx = storage(self.database.begin_read())?;
        let unfinished = storage(tx.open_table(UNFINISHED))?;
        let records = storage(tx.open_table(RECORDS))?;
        let range = (
            after.map_or(Bound::Unbounded, |d| Bound::Excluded(d.as_str())),
            Bound::<&str>::Unbounded,
        );
        let mut iter = storage(unfinished.range::<&str>(range))?;
        let mut page = Vec::with_capacity(limit);
        for _ in 0..limit {
            let Some(item) = iter.next() else {
                break;
            };
            let (key, attempt) = storage(item)?;
            page.push(read_record(
                &records,
                &Digest::new(key.value())?,
                attempt.value(),
            )?);
        }
        let more = iter
            .next()
            .transpose()
            .map_err(|_| Error::Storage)?
            .is_some();
        let next_after = more.then(|| page.last().expect("nonempty page").invocation.clone());
        Ok(Page {
            records: page,
            next_after,
        })
    }

    /// Delete only explicitly closed entries past their stored retention. Never
    /// expire unknown work or release a hold because a clock/retention limit passed.
    /// At most 64 entries and their direct indexes per transaction; no history scan.
    /// Logical record quotas bound retained payload; filesystem quota/headroom is a
    /// separate operator control because database pages/metadata add overhead.
    pub fn prune_closed(&self, now_ms: u64, limit: usize) -> Result<usize> {
        require((1..=MAX_PAGE).contains(&limit))?;
        let tx = self.transaction()?;
        let mut expiry = storage(tx.open_table(EXPIRY))?;
        let end = format!("{now_ms:020}:~");
        let due = storage(expiry.range(..=end.as_str()))?
            .take(limit)
            .map(|v| {
                let (k, v) = storage(v)?;
                Ok((k.value().to_owned(), v.value().to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        if due.is_empty() {
            return Ok(0);
        }
        let mut records = storage(tx.open_table(RECORDS))?;
        let mut requests = storage(tx.open_table(REQUESTS))?;
        let mut meta = metadata(&tx)?;
        for (key, record_key) in &due {
            let record: Record = decode(
                storage(records.get(record_key.as_str()))?
                    .ok_or(Error::Invalid)?
                    .value(),
            )?;
            record.validate()?;
            require(
                record.phase == Phase::Closed
                    && record.key() == *record_key
                    && expiry_key(&record) == *key
                    && record.expires_at_ms.is_some_and(|t| t <= now_ms),
            )?;
            let is_current = storage(requests.get(record.invocation.as_str()))?
                .is_some_and(|a| a.value() == record.attempt);
            if is_current {
                storage(requests.remove(record.invocation.as_str()))?;
            }
            payloads::prune(&tx, record_key, &mut meta)?;
            finance::prune(&tx, record_key, &mut meta)?;
            outcomes::prune(&tx, record_key, &mut meta)?;
            provider::prune(&tx, record_key, &mut meta)?;
            retirement::prune(&tx, record_key, &mut meta)?;
            storage(records.remove(record_key.as_str()))?;
            storage(expiry.remove(key.as_str()))?;
            meta.records = meta.records.checked_sub(1).ok_or(Error::Invalid)?;
        }
        save_meta(&tx, &meta)?;
        drop(expiry);
        drop(records);
        drop(requests);
        self.commit(tx)?;
        Ok(due.len())
    }
}

fn checked_current(
    tx: &redb::WriteTransaction,
    invocation: &Digest,
    generation: u64,
    now_ms: u64,
) -> Result<Record> {
    let r = current(tx, invocation)?.ok_or(Error::NotFound)?;
    if r.generation != generation {
        return Err(Error::Stale);
    }
    require(now_ms >= r.updated_at_ms)?;
    Ok(r)
}
fn current(tx: &redb::WriteTransaction, invocation: &Digest) -> Result<Option<Record>> {
    let requests = storage(tx.open_table(REQUESTS))?;
    let Some(attempt) = storage(requests.get(invocation.as_str()))? else {
        return Ok(None);
    };
    let records = storage(tx.open_table(RECORDS))?;
    Ok(Some(read_record(&records, invocation, attempt.value())?))
}
fn read_record(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    invocation: &Digest,
    attempt: u64,
) -> Result<Record> {
    let r: Record = decode(
        storage(table.get(record_key(invocation, attempt).as_str()))?
            .ok_or(Error::Invalid)?
            .value(),
    )?;
    r.validate()?;
    require(r.invocation == *invocation && r.attempt == attempt)?;
    Ok(r)
}
fn bump(tx: &redb::WriteTransaction, r: &mut Record, now_ms: u64) -> Result<()> {
    // Global high-water mark survives pruning/key reuse too. A delayed event can
    // never acquire authority over a later invocation with the same opaque key.
    let mut meta = metadata(tx)?;
    require(meta.generation >= r.generation)?;
    meta.generation = meta.generation.checked_add(1).ok_or(Error::Invalid)?;
    r.generation = meta.generation;
    r.updated_at_ms = now_ms;
    save_meta(tx, &meta)?;
    Ok(())
}
fn save(tx: &redb::WriteTransaction, r: &Record) -> Result<()> {
    r.validate()?;
    storage(storage(tx.open_table(RECORDS))?.insert(r.key().as_str(), encode(r)?.as_slice()))?;
    Ok(())
}
fn metadata(tx: &redb::WriteTransaction) -> Result<Meta> {
    let table = storage(tx.open_table(META))?;
    let value = storage(table.get("state"))?.ok_or(Error::Invalid)?;
    decode(value.value())
}
fn save_meta(tx: &redb::WriteTransaction, meta: &Meta) -> Result<()> {
    storage(storage(tx.open_table(META))?.insert("state", encode(meta)?.as_slice()))?;
    Ok(())
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Invalid)?;
    require(bytes.len() <= MAX_RECORD_BYTES)?;
    Ok(bytes)
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    require(bytes.len() <= MAX_RECORD_BYTES)?;
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid)
}

#[cfg(unix)]
pub(crate) fn private_file(path: &Path) -> Result<std::fs::File> {
    use rustix::fs::{fstat, open, FileType, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;
    let parent = std::fs::metadata(path.parent().ok_or(Error::File)?).map_err(|_| Error::File)?;
    let uid = rustix::process::geteuid().as_raw();
    if !parent.is_dir() || parent.uid() != uid || parent.mode() & 0o077 != 0 {
        return Err(Error::File);
    }
    let fd = open(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| Error::File)?;
    let s = fstat(&fd).map_err(|_| Error::File)?;
    if FileType::from_raw_mode(s.st_mode) != FileType::RegularFile
        || s.st_uid != uid
        || s.st_mode & 0o077 != 0
        || s.st_nlink != 1
    {
        return Err(Error::File);
    }
    Ok(fd.into())
}
#[cfg(not(unix))]
pub(crate) fn private_file(_: &Path) -> Result<std::fs::File> {
    Err(Error::UnsupportedFileProtection)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use redb::{backends::FileBackend, StorageBackend};
    use std::{
        io,
        sync::{
            atomic::{AtomicU64, AtomicU8},
            Arc,
        },
    };

    #[derive(Debug)]
    struct SyncFault {
        inner: FileBackend,
        mode: Arc<AtomicU8>,
        syncs: Arc<AtomicU64>,
    }
    impl StorageBackend for SyncFault {
        fn len(&self) -> io::Result<u64> {
            self.inner.len()
        }
        fn read(&self, offset: u64, out: &mut [u8]) -> io::Result<()> {
            self.inner.read(offset, out)
        }
        fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
            self.inner.write(offset, data)
        }
        fn set_len(&self, n: u64) -> io::Result<()> {
            self.inner.set_len(n)
        }
        fn sync_data(&self) -> io::Result<()> {
            let mode = self.mode.load(Ordering::Acquire);
            self.syncs.fetch_add(1, Ordering::AcqRel);
            if mode == 1 {
                return Err(io::Error::other("injected sync failure"));
            }
            self.inner.sync_data()?;
            if mode == 2 {
                return Err(io::Error::other("injected lost sync acknowledgment"));
            }
            Ok(())
        }
        fn close(&self) -> io::Result<()> {
            self.inner.close()
        }
    }
    fn d(n: u64) -> Digest {
        Digest::new(format!("{n:064x}")).unwrap()
    }
    fn binding() -> Binding {
        Binding {
            request_hash: d(1),
            endpoint: ProxyEndpoint::Chat,
            contract_version: 30,
            provider_pubkey: d(2),
            market_id: d(3),
            offer_digest: d(4),
            endpoint_contract: d(5),
            metering_policy: d(6),
            accepted_terms: d(7),
            reservation: d(8),
            capacity_lease: d(9),
            connection_digest: d(10),
            connection_revision: 1,
            recipe_digest: d(11),
            rail: ProxyRail::Fiat,
        }
    }

    #[test]
    fn failed_durable_commit_never_issues_a_ticket_and_fences_further_writes() {
        for mode in [1, 2] {
            let file = tempfile::tempfile().unwrap();
            let fault = Arc::new(AtomicU8::new(0));
            let syncs = Arc::new(AtomicU64::new(0));
            let backend = SyncFault {
                inner: FileBackend::new(file).unwrap(),
                mode: fault.clone(),
                syncs: syncs.clone(),
            };
            let database = Database::builder().create_with_backend(backend).unwrap();
            let j = Journal::initialize(
                database,
                Identity {
                    network_id: "918".into(),
                    msb_bootstrap: d(100),
                    subnet_bootstrap: d(101),
                    controller_pubkey: d(102),
                },
                Limits {
                    max_records: 10,
                    max_unfinished: 10,
                    closed_retention_ms: 1000,
                    max_payload_bytes: 128 * 1024 * 1024,
                },
            )
            .unwrap();
            let r = j.prepare(d(200), binding(), 100).unwrap();
            let before = syncs.load(Ordering::Acquire);
            j.prepare(d(200), binding(), 100).unwrap();
            assert_eq!(
                before,
                syncs.load(Ordering::Acquire),
                "replay does not fsync"
            );
            fault.store(mode, Ordering::Release);
            assert!(matches!(
                j.begin_dispatch(&r.invocation, r.generation, 101),
                Err(Error::Storage)
            ));
            fault.store(0, Ordering::Release);
            assert!(j.commit_failed.load(Ordering::Acquire));
            assert!(matches!(
                j.provider_acceptance(&r.invocation, r.attempt),
                Err(Error::Storage)
            ));
            assert!(matches!(
                j.prepare(d(201), binding(), 102),
                Err(Error::Storage)
            ));
            assert!(matches!(
                j.begin_dispatch(&r.invocation, r.generation, 102),
                Err(Error::Storage)
            ));
        }
    }
}
