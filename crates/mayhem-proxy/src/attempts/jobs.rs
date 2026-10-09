//! Durable asynchronous control fencing. No dispatch, retry, or financial authority.
use super::*;
const JOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_attempt_jobs_v1");
const RESERVE: u64 = 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    generation: u64,
    until: u64,
    next: u64,
    cancel_sent: bool,
    stopped: bool,
}
#[derive(Clone)]
pub(crate) struct Lease {
    record: Record,
    generation: u64,
    pub(crate) cancel_sent: bool,
}
impl Lease {
    pub(crate) fn record(&self) -> &Record {
        &self.record
    }
}
pub(super) fn initialize(tx: &redb::WriteTransaction, create: bool) -> Result<()> {
    let exists = storage(tx.list_tables())?.any(|t| t.name() == JOBS.name());
    require(exists != create)?;
    let table = storage(tx.open_table(JOBS))?;
    require(storage(table.len())? <= storage(storage(tx.open_table(RECORDS))?.len())?)
}
fn read(tx: &redb::WriteTransaction, key: &str) -> Result<Option<State>> {
    storage(storage(tx.open_table(JOBS))?.get(key))?
        .map(|v| {
            require(v.value().len() <= RESERVE as usize)?;
            decode(v.value())
        })
        .transpose()
}
fn write(tx: &redb::WriteTransaction, key: &str, state: &State) -> Result<()> {
    let bytes = encode(state)?;
    require(bytes.len() <= RESERVE as usize)?;
    storage(storage(tx.open_table(JOBS))?.insert(key, bytes.as_slice()))?;
    Ok(())
}
pub(super) fn check(tx: &redb::WriteTransaction, lease: &Lease, now: u64) -> Result<()> {
    let r = current(tx, &lease.record.invocation)?.ok_or(Error::NotFound)?;
    let state = read(tx, &r.key())?.ok_or(Error::NotFound)?;
    if r.attempt != lease.record.attempt
        || r.phase != Phase::Dispatched
        || state.generation != lease.generation
        || now >= state.until
    {
        return Err(Error::Stale);
    }
    Ok(())
}
pub(super) fn prune(tx: &redb::WriteTransaction, key: &str, meta: &mut Meta) -> Result<()> {
    if storage(storage(tx.open_table(JOBS))?.remove(key))?.is_some() {
        meta.payload_bytes = meta
            .payload_bytes
            .checked_sub(RESERVE)
            .ok_or(Error::Invalid)?;
    }
    Ok(())
}
impl Journal {
    pub(crate) fn has_job(&self, key: &Digest, attempt: u64) -> Result<bool> {
        let tx = storage(self.database.begin_read())?;
        let table = storage(tx.open_table(JOBS))?;
        Ok(storage(table.get(record_key(key, attempt).as_str()))?.is_some())
    }

    pub(crate) fn reserve_job(&self, key: &Digest, attempt: u64) -> Result<()> {
        let tx = self.transaction()?;
        let r = current(&tx, key)?.ok_or(Error::NotFound)?;
        if r.attempt != attempt {
            return Err(Error::Stale);
        }
        if read(&tx, &r.key())?.is_some() {
            return Ok(());
        }
        if r.phase != Phase::Prepared {
            return Err(Error::Transition);
        }
        let mut meta = metadata(&tx)?;
        meta.payload_bytes = meta
            .payload_bytes
            .checked_add(RESERVE)
            .ok_or(Error::Capacity)?;
        if meta.payload_bytes > self.limits.max_payload_bytes {
            return Err(Error::Capacity);
        }
        write(
            &tx,
            &r.key(),
            &State {
                generation: 0,
                until: 0,
                next: 0,
                cancel_sent: false,
                stopped: false,
            },
        )?;
        save_meta(&tx, &meta)?;
        self.commit(tx)
    }
    pub(crate) fn claim_job(
        &self,
        key: &Digest,
        attempt: u64,
        now: u64,
        duration: u64,
    ) -> Result<Option<Lease>> {
        require((100..=65_000).contains(&duration))?;
        let tx = self.transaction()?;
        let r = current(&tx, key)?.ok_or(Error::NotFound)?;
        if r.attempt != attempt {
            return Err(Error::Stale);
        }
        if !matches!(r.phase, Phase::Prepared | Phase::Dispatched) {
            return Ok(None);
        }
        let mut state = read(&tx, &r.key())?.ok_or(Error::NotFound)?;
        if state.stopped || now < state.until || now < state.next {
            return Ok(None);
        }
        state.generation = state.generation.checked_add(1).ok_or(Error::Invalid)?;
        state.until = now.checked_add(duration).ok_or(Error::Invalid)?;
        let lease = Lease {
            record: r.clone(),
            generation: state.generation,
            cancel_sent: state.cancel_sent,
        };
        write(&tx, &r.key(), &state)?;
        self.commit(tx)?;
        Ok(Some(lease))
    }
    pub(crate) fn mark_job_cancel(&self, lease: &Lease, now: u64) -> Result<()> {
        let tx = self.transaction()?;
        check(&tx, lease, now)?;
        let key = lease.record.key();
        let mut s = read(&tx, &key)?.ok_or(Error::NotFound)?;
        s.cancel_sent = true;
        write(&tx, &key, &s)?;
        self.commit(tx)
    }
    pub(crate) fn finish_job_step(
        &self,
        lease: &Lease,
        now: u64,
        next: u64,
        stopped: bool,
    ) -> Result<()> {
        let tx = self.transaction()?;
        check(&tx, lease, now)?;
        let key = lease.record.key();
        let mut s = read(&tx, &key)?.ok_or(Error::NotFound)?;
        s.until = 0;
        s.next = next;
        s.stopped = stopped;
        write(&tx, &key, &s)?;
        self.commit(tx)
    }
    pub(crate) fn accept_job(&self, lease: &Lease, id: RemoteId, now: u64) -> Result<Record> {
        let tx = self.transaction()?;
        check(&tx, lease, now)?;
        let mut r = current(&tx, &lease.record.invocation)?.ok_or(Error::NotFound)?;
        if r.remote_id.as_ref().is_some_and(|old| old != &id) {
            return Err(Error::Conflict);
        }
        r.remote_id = Some(id);
        bump(&tx, &mut r, now)?;
        save(&tx, &r)?;
        self.commit(tx)?;
        Ok(r)
    }
}
