//! A private-to-this-process, durable public catalog cache. This is not a ledger
//! replica, capacity reservation, receipt store, or authorization decision. Reads
//! see one complete snapshot; a refresh never exposes a half-applied catalog.

use std::{path::Path, sync::Arc};

use redb::{
    Database, ReadTransaction, ReadableDatabase, ReadableTable, ReadableTableMetadata,
    TableDefinition,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::discovery::{
    Context, DiscoveryClient, Entry, Identity, Mode, Page, Proof, Query, CATALOG_PREFIX,
    MAX_PAGE_ENTRIES, MAX_PAGE_ENTRY_BYTES,
};
use crate::{db, invalid, require, Error, Result};

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_catalog_metadata_v1");
const CURRENT: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_catalog_current_v1");
const STAGED: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_catalog_staged_v1");
const STATE_KEY: &str = "state";
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_STATE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Committed {
    pub proof: Proof,
    pub context: Context,
    pub checkpoint: String,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    proof: Proof,
    context: Context,
    base_proof: Option<Proof>,
    mode: Mode,
    last_key: String,
    next_cursor: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema_version: u32,
    identity: Identity,
    generation: u64,
    invalidated: bool,
    committed: Option<Committed>,
    pending: Option<Pending>,
}

impl State {
    fn query(&self) -> Query {
        let mut query = Query::catalog();
        if let Some(pending) = &self.pending {
            query.cursor = Some(pending.next_cursor.clone());
        } else if !self.invalidated {
            query.since = self.committed.as_ref().map(|c| c.checkpoint.clone());
        }
        query
    }

    fn validate(&self, identity: &Identity) -> Result<()> {
        if self.identity != *identity {
            return Err(Error::Identity);
        }
        require(self.schema_version == 1, "unsupported catalog cache schema")?;
        if let Some(c) = &self.committed {
            c.proof.validate()?;
            require(
                c.context.identity() == *identity
                    && crate::discovery::safe_integer(c.context.epoch)
                    && crate::discovery::token(&c.checkpoint),
                "invalid cached checkpoint",
            )?;
        }
        if let Some(p) = &self.pending {
            p.proof.validate()?;
            require(
                p.context.identity() == *identity
                    && crate::discovery::safe_integer(p.context.epoch)
                    && p.last_key.starts_with(CATALOG_PREFIX)
                    && p.last_key.len() <= 256
                    && crate::discovery::token(&p.next_cursor),
                "invalid cached traversal",
            )?;
            match p.mode {
                Mode::Snapshot => require(
                    p.base_proof.is_none() && (self.invalidated || self.committed.is_none()),
                    "invalid pending snapshot",
                )?,
                Mode::Changes => require(
                    !self.invalidated
                        && self.committed.as_ref().is_some_and(|c| {
                            p.base_proof.as_ref() == Some(&c.proof) && p.proof.follows(&c.proof)
                        }),
                    "invalid pending delta",
                )?,
            }
        }
        self.query().validate()
    }

    fn advance(&mut self) -> Result<()> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("catalog generation exhausted"))?;
        Ok(())
    }
}

fn decode_state(bytes: &[u8], identity: &Identity) -> Result<State> {
    require(
        bytes.len() <= MAX_STATE_BYTES,
        "catalog metadata exceeds bound",
    )?;
    let state: State = serde_json::from_slice(bytes)?;
    state.validate(identity)?;
    Ok(state)
}

fn save_state(tx: &redb::WriteTransaction, state: &State) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    require(
        bytes.len() <= MAX_STATE_BYTES,
        "catalog metadata exceeds bound",
    )?;
    let mut meta = db(tx.open_table(META))?;
    db(meta.insert(STATE_KEY, bytes.as_slice()))?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshTicket {
    generation: u64,
    query: Query,
}

impl RefreshTicket {
    pub fn query(&self) -> &Query {
        &self.query
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub generation: u64,
    pub invalidated: bool,
    pub refresh_in_progress: bool,
    pub committed: Option<Committed>,
}

impl Status {
    /// Freshness of discovery only. Paid admission, offer validity and live
    /// capacity remain separate checks and cannot be inferred from this result.
    pub fn discovery_is_fresh(&self, now_ms: u64, max_age_ms: u64) -> bool {
        !self.invalidated
            && max_age_ms > 0
            && self.committed.as_ref().is_some_and(|c| {
                now_ms
                    .checked_sub(c.completed_at_ms)
                    .is_some_and(|age| age <= max_age_ms)
            })
    }
}

impl From<&State> for Status {
    fn from(state: &State) -> Self {
        Self {
            generation: state.generation,
            invalidated: state.invalidated,
            refresh_in_progress: state.pending.is_some(),
            committed: state.committed.clone(),
        }
    }
}

pub struct Catalog {
    database: Database,
    identity: Identity,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Catalog {
    /// redb holds an exclusive process lock. Share an Arc within this process;
    /// other processes use the local control RPC, not a second writer. Corrupt
    /// files and wrong-network caches fail closed; neither is silently reset.
    pub fn open(path: impl AsRef<Path>, identity: Identity) -> Result<Self> {
        identity.validate()?;
        let mut builder = Database::builder();
        builder.set_cache_size(CACHE_BYTES);
        let database = db(builder.create(path))?;
        let tx = db(database.begin_write())?;
        let existing = {
            let meta = db(tx.open_table(META))?;
            let value = db(meta.get(STATE_KEY))?;
            value
                .map(|v| decode_state(v.value(), &identity))
                .transpose()?
        };
        if let Some(state) = existing {
            // Check the bounded metadata/table boundary, without reading every
            // catalog record during startup. Missing tables are corruption.
            let tables = db(tx.list_tables())?.collect::<Vec<_>>();
            use redb::TableHandle;
            require(
                tables.iter().any(|t| t.name() == CURRENT.name()),
                "catalog table is missing",
            )?;
            let has_staged = tables.iter().any(|t| t.name() == STAGED.name());
            require(
                has_staged == state.pending.is_some(),
                "catalog staging state is inconsistent",
            )?;
            if let Some(pending) = state.pending {
                let staged = db(tx.open_table(STAGED))?;
                let last = db(staged.last())?;
                require(
                    last.as_ref()
                        .is_some_and(|(key, _)| key.value() == pending.last_key),
                    "catalog staged boundary is inconsistent",
                )?;
            }
        } else {
            // Only a new, empty DB can be initialized. Never adopt unrelated data.
            use redb::TableHandle;
            require(
                db(tx.list_tables())?.all(|t| t.name() == META.name()),
                "unrecognized catalog database",
            )?;
            db(tx.open_table(CURRENT))?;
            save_state(
                &tx,
                &State {
                    schema_version: 1,
                    identity: identity.clone(),
                    generation: 0,
                    invalidated: true,
                    committed: None,
                    pending: None,
                },
            )?;
        }
        db(tx.commit())?;
        Ok(Self {
            database,
            identity,
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub fn read(&self) -> Result<CatalogRead> {
        let tx = db(self.database.begin_read())?;
        let state = {
            let meta = db(tx.open_table(META))?;
            let value =
                db(meta.get(STATE_KEY))?.ok_or_else(|| invalid("catalog metadata is missing"))?;
            decode_state(value.value(), &self.identity)?
        };
        Ok(CatalogRead { tx, state })
    }

    pub fn refresh_ticket(&self) -> Result<RefreshTicket> {
        let read = self.read()?;
        Ok(RefreshTicket {
            generation: read.state.generation,
            query: read.state.query(),
        })
    }

    fn state_for_ticket(
        &self,
        tx: &redb::WriteTransaction,
        ticket: &RefreshTicket,
    ) -> Result<State> {
        let meta = db(tx.open_table(META))?;
        let value =
            db(meta.get(STATE_KEY))?.ok_or_else(|| invalid("catalog metadata is missing"))?;
        let state = decode_state(value.value(), &self.identity)?;
        if state.generation != ticket.generation || state.query() != ticket.query {
            return Err(Error::StaleRefresh);
        }
        Ok(state)
    }

    pub fn apply(
        &self,
        ticket: &RefreshTicket,
        page: &Page,
        completed_at_ms: u64,
    ) -> Result<Status> {
        page.validate(&self.identity)?;
        let tx = db(self.database.begin_write())?;
        let mut state = self.state_for_ticket(&tx, ticket)?;
        if let Some(pending) = &state.pending {
            require(
                page.proof == pending.proof
                    && page.context == pending.context
                    && page.base_proof == pending.base_proof
                    && page.mode == pending.mode,
                "snapshot changed during pagination",
            )?;
            require(
                page.entries
                    .first()
                    .is_none_or(|entry| entry.key > pending.last_key),
                "pagination did not advance",
            )?;
            require(
                page.next_cursor.as_ref() != Some(&pending.next_cursor),
                "discovery cursor did not advance",
            )?;
        } else if ticket.query.since.is_some() {
            require(
                page.mode == Mode::Changes
                    && state
                        .committed
                        .as_ref()
                        .is_some_and(|c| page.base_proof.as_ref() == Some(&c.proof)),
                "delta does not match committed checkpoint",
            )?;
        } else {
            require(
                page.mode == Mode::Snapshot && page.base_proof.is_none(),
                "initial refresh must be a full snapshot",
            )?;
        }
        // A new traversal starts from an empty staging table. Writes and cursor
        // persist together in one crash-safe transaction after full validation.
        if page.mode == Mode::Snapshot || page.truncated {
            let mut staged = db(tx.open_table(STAGED))?;
            if state.pending.is_none() {
                require(db(staged.is_empty())?, "orphaned catalog staging data")?;
            }
            for entry in &page.entries {
                let bytes = serde_json::to_vec(&entry.value)?;
                db(staged.insert(entry.key.as_str(), bytes.as_slice()))?;
            }
        }
        if page.truncated {
            state.pending = Some(Pending {
                proof: page.proof.clone(),
                context: page.context.clone(),
                base_proof: page.base_proof.clone(),
                mode: page.mode,
                last_key: page
                    .entries
                    .last()
                    .ok_or_else(|| invalid("empty continuation page"))?
                    .key
                    .clone(),
                next_cursor: page
                    .next_cursor
                    .clone()
                    .ok_or_else(|| invalid("missing continuation cursor"))?,
            });
        } else {
            match page.mode {
                Mode::Snapshot => {
                    db(tx.delete_table(CURRENT))?;
                    db(tx.rename_table(STAGED, CURRENT))?;
                }
                Mode::Changes => {
                    // Stream only the changed rows from disk; no history scan,
                    // whole-catalog clone or materialized delta vector.
                    {
                        let mut current = db(tx.open_table(CURRENT))?;
                        if state.pending.is_some() {
                            let staged = db(tx.open_table(STAGED))?;
                            for entry in db(staged.iter())? {
                                let (key, value) = db(entry)?;
                                if value.value() == b"null" {
                                    db(current.remove(key.value()))?;
                                } else {
                                    db(current.insert(key.value(), value.value()))?;
                                }
                            }
                        }
                        // The final page already commits atomically here. Do not
                        // write it twice or mutate a table being retired in this
                        // same transaction (unsupported by the pinned redb).
                        for entry in &page.entries {
                            if entry.value.is_null() {
                                db(current.remove(entry.key.as_str()))?;
                            } else {
                                let bytes = serde_json::to_vec(&entry.value)?;
                                db(current.insert(entry.key.as_str(), bytes.as_slice()))?;
                            }
                        }
                    }
                    db(tx.delete_table(STAGED))?;
                }
            }
            state.committed = Some(Committed {
                proof: page.proof.clone(),
                context: page.context.clone(),
                checkpoint: page
                    .checkpoint
                    .clone()
                    .ok_or_else(|| invalid("missing completed checkpoint"))?,
                completed_at_ms,
            });
            state.pending = None;
            state.invalidated = false;
        }
        state.advance()?;
        save_state(&tx, &state)?;
        db(tx.commit())?;
        Ok(Status::from(&state))
    }

    /// Cursor expiration or canonical view replacement starts a fresh snapshot.
    /// Retain the old complete catalog for diagnosis/browsing, mark it unusable as
    /// fresh discovery, and fence responses already in flight. No ledger writes.
    pub fn invalidate(&self, ticket: &RefreshTicket) -> Result<()> {
        let tx = db(self.database.begin_write())?;
        let mut state = self.state_for_ticket(&tx, ticket)?;
        db(tx.delete_table(STAGED))?;
        state.pending = None;
        state.invalidated = true;
        state.advance()?;
        save_state(&tx, &state)?;
        db(tx.commit())
    }

    /// One bounded supervisor step. No internal retry loop and no disk work on
    /// the async inference executor. Callers provide scheduling/backoff/shutdown.
    pub async fn refresh_page(
        self: &Arc<Self>,
        client: &DiscoveryClient,
        completed_at_ms: u64,
    ) -> Result<RefreshOutcome> {
        // At most one supervisor HTTP/read/write step per cache, with no queue
        // of duplicate refresh tasks accumulating behind a slow peer.
        let refresh = self
            .refresh_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| Error::RefreshBusy)?;
        let catalog = self.clone();
        // Carry ownership into each blocking operation. Cancelling this async
        // caller cannot unlock and pile up detached disk tasks still in progress.
        let (ticket, refresh) = tokio::task::spawn_blocking(move || {
            catalog.refresh_ticket().map(|ticket| (ticket, refresh))
        })
        .await
        .map_err(|_| Error::Task)??;
        match client.page(ticket.query()).await {
            Ok(page) => {
                let catalog = self.clone();
                let status = tokio::task::spawn_blocking(move || {
                    let _refresh = refresh;
                    catalog.apply(&ticket, &page, completed_at_ms)
                })
                .await
                .map_err(|_| Error::Task)??;
                Ok(if status.refresh_in_progress {
                    RefreshOutcome::Staged(status)
                } else {
                    RefreshOutcome::Committed(status)
                })
            }
            Err(Error::Http { status: 409, code })
                if matches!(
                    code.as_str(),
                    "proxy_cursor_expired" | "proxy_cursor_invalidated"
                ) =>
            {
                let catalog = self.clone();
                tokio::task::spawn_blocking(move || {
                    let _refresh = refresh;
                    catalog.invalidate(&ticket)
                })
                .await
                .map_err(|_| Error::Task)??;
                Ok(RefreshOutcome::Invalidated)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug)]
pub enum RefreshOutcome {
    Staged(Status),
    Committed(Status),
    Invalidated,
}

/// The metadata and rows share a single MVCC read snapshot, even if a refresh
/// commits concurrently. Drop promptly; retaining old readers retains disk pages.
pub struct CatalogRead {
    tx: ReadTransaction,
    state: State,
}

#[derive(Debug)]
pub struct ReadPage {
    pub entries: Vec<Entry>,
    pub next_after: Option<String>,
}

impl CatalogRead {
    pub fn status(&self) -> Status {
        Status::from(&self.state)
    }

    pub fn get(&self, key: &str) -> Result<Option<Value>> {
        require(
            key.starts_with(CATALOG_PREFIX) && key.len() <= 256,
            "invalid catalog lookup key",
        )?;
        let table = db(self.tx.open_table(CURRENT))?;
        let value = db(table.get(key))?;
        value
            .map(|value| serde_json::from_slice(value.value()).map_err(Error::from))
            .transpose()
    }

    /// Indexed public-key prefix paging, bounded by both rows and returned bytes.
    /// A returned cursor belongs to this read snapshot, not a newer generation.
    pub fn page(&self, prefix: &str, after: Option<&str>, limit: usize) -> Result<ReadPage> {
        require(
            prefix.starts_with(CATALOG_PREFIX)
                && prefix.len() <= 256
                && prefix.is_ascii()
                && limit > 0
                && limit <= MAX_PAGE_ENTRIES
                && after.is_none_or(|key| key.starts_with(prefix) && key.len() <= 256),
            "invalid catalog read page",
        )?;
        use std::ops::Bound::{Excluded, Included};
        let upper = format!("{prefix}\u{7f}");
        let lower = after.map(Excluded).unwrap_or(Included(prefix));
        let table = db(self.tx.open_table(CURRENT))?;
        let range = db(table.range::<&str>((lower, Excluded(upper.as_str()))))?;
        let mut entries: Vec<Entry> = Vec::new();
        let mut bytes = 0;
        for entry in range.take(limit + 1) {
            let (key, value) = db(entry)?;
            let size = key.value().len() + value.value().len() + 32;
            if entries.len() == limit || bytes + size > MAX_PAGE_ENTRY_BYTES {
                require(
                    !entries.is_empty(),
                    "stored catalog record exceeds read bound",
                )?;
                return Ok(ReadPage {
                    next_after: entries.last().map(|entry| entry.key.clone()),
                    entries,
                });
            }
            bytes += size;
            entries.push(Entry {
                key: key.value().to_owned(),
                value: serde_json::from_slice(value.value())?,
            });
        }
        Ok(ReadPage {
            entries,
            next_after: None,
        })
    }
}
