//! Exact-attempt retention of the initial canonical financial observation.
//! This table shares journal durability, ownership, quotas and indexed pruning.
use super::*;
use crate::financial::{Observation, Retained};
const FINANCE: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_accepted_finance_v1");
const MAX_BYTES: usize = 64 * 1024;

pub(super) fn initialize(tx: &redb::WriteTransaction, create: bool) -> Result<()> {
    let exists = storage(tx.list_tables())?.any(|t| t.name() == FINANCE.name());
    require(exists != create)?;
    let table = storage(tx.open_table(FINANCE))?;
    require(storage(table.len())? <= storage(storage(tx.open_table(RECORDS))?.len())?)
}
fn decode_retained(bytes: &[u8], record: &Record) -> Result<Retained> {
    require(bytes.len() <= MAX_BYTES)?;
    let value: Retained = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    value
        .validate_for(&record.binding)
        .map_err(|_| Error::Conflict)?;
    Ok(value)
}
pub(super) fn read(tx: &redb::ReadTransaction, record: &Record) -> Result<Option<Retained>> {
    let table = storage(tx.open_table(FINANCE))?;
    storage(table.get(record.key().as_str()))?
        .map(|v| decode_retained(v.value(), record))
        .transpose()
}
pub(super) fn prune(tx: &redb::WriteTransaction, key: &str, meta: &mut Meta) -> Result<()> {
    let mut table = storage(tx.open_table(FINANCE))?;
    if let Some(v) = storage(table.remove(key))? {
        meta.payload_bytes = meta
            .payload_bytes
            .checked_sub(v.value().len() as u64)
            .ok_or(Error::Invalid)?;
    }
    Ok(())
}
impl Journal {
    /// A retained observation is prerequisite evidence, not a backend lease or
    /// permission to send. The dispatch journal still fences every actual POST.
    pub fn retain_financial_acceptance(
        &self,
        invocation: &Digest,
        attempt: u64,
        observation: &Observation,
    ) -> Result<()> {
        let value = Retained::capture(observation).map_err(|_| Error::Conflict)?;
        let tx = self.transaction()?;
        let record = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        if record.attempt != attempt {
            return Err(Error::Stale);
        }
        value
            .validate_for(&record.binding)
            .map_err(|_| Error::Conflict)?;
        let mut meta = metadata(&tx)?;
        let context = value.context();
        let terms = &value.accepted().authorization.terms;
        require(
            context.network_id == meta.identity.network_id
                && context.msb_bootstrap == meta.identity.msb_bootstrap.as_str()
                && context.subnet_bootstrap == meta.identity.subnet_bootstrap.as_str()
                && terms.offer.provider_pubkey == meta.identity.controller_pubkey.as_str(),
        )?;
        let mut table = storage(tx.open_table(FINANCE))?;
        if let Some(saved) = storage(table.get(record.key().as_str()))? {
            let saved = decode_retained(saved.value(), &record)?;
            // Fresh proof may advance. Keep the original terms/proof unchanged.
            require(
                saved.accepted().authorization == value.accepted().authorization
                    && saved.accepted().settlement_policy == value.accepted().settlement_policy,
            )?;
            return Ok(());
        }
        require(record.phase == Phase::Prepared)?;
        let bytes = serde_json::to_vec(&value).map_err(|_| Error::Invalid)?;
        require(bytes.len() <= MAX_BYTES)?;
        meta.payload_bytes = meta
            .payload_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Capacity)?;
        if meta.payload_bytes > self.limits.max_payload_bytes {
            return Err(Error::Capacity);
        }
        storage(table.insert(record.key().as_str(), bytes.as_slice()))?;
        save_meta(&tx, &meta)?;
        drop(table);
        self.commit(tx)
    }
    pub fn financial_acceptance(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<Retained>> {
        let tx = storage(self.database.begin_read())?;
        let records = storage(tx.open_table(RECORDS))?;
        if storage(records.get(record_key(invocation, attempt).as_str()))?.is_none() {
            return Err(Error::NotFound);
        }
        read(&tx, &read_record(&records, invocation, attempt)?)
    }
}
