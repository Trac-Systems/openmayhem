//! Durable provider countersignature, sharing execution journal quotas and expiry.
//! Signature exposure follows request/snapshot retention and an immediate commit.
use super::*;
use crate::{
    financial::{provider::Approval, terms_binding},
    signing::Authority,
};
use mayhem_proto::proxy::finance::{ProxySettlementPolicy, ProxySpendAuthorization};

const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_provider_acceptance_v1");
const MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedProviderAcceptance {
    pub authorization: ProxySpendAuthorization,
    pub policy: ProxySettlementPolicy,
}
impl SignedProviderAcceptance {
    fn validate(&self, record: &Record, identity: &Identity) -> Result<()> {
        let t = &self.authorization.terms;
        self.authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| Error::Invalid)?;
        require(
            terms_binding(t).map_err(|_| Error::Invalid)? == record.binding
                && self.policy.digest().map_err(|_| Error::Invalid)? == t.settlement_policy_hash
                && identity.network_id == t.network_id
                && identity.msb_bootstrap.as_str() == t.msb_bootstrap
                && identity.subnet_bootstrap.as_str() == t.subnet_bootstrap
                && identity.controller_pubkey.as_str() == t.offer.provider_pubkey
                && crate::exchange::invocation_for_terms(t).map_err(|_| Error::Invalid)?
                    == record.invocation,
        )
    }
}
pub(super) fn initialize(tx: &redb::WriteTransaction, create: bool) -> Result<()> {
    let exists = storage(tx.list_tables())?.any(|t| t.name() == TABLE.name());
    require(exists != create)?;
    let table = storage(tx.open_table(TABLE))?;
    require(storage(table.len())? <= storage(storage(tx.open_table(RECORDS))?.len())?)
}
pub(super) fn prune(tx: &redb::WriteTransaction, key: &str, meta: &mut Meta) -> Result<()> {
    let mut table = storage(tx.open_table(TABLE))?;
    if let Some(v) = storage(table.remove(key))? {
        meta.payload_bytes = meta
            .payload_bytes
            .checked_sub(v.value().len() as u64)
            .ok_or(Error::Invalid)?;
    }
    Ok(())
}
fn decode_signed(
    bytes: &[u8],
    record: &Record,
    identity: &Identity,
) -> Result<SignedProviderAcceptance> {
    require(bytes.len() <= MAX_BYTES)?;
    let value: SignedProviderAcceptance =
        serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    value.validate(record, identity)?;
    Ok(value)
}
impl Journal {
    /// Blocking trusted-parent operation. Use ProviderNegotiation's bounded
    /// executor from async code. Crashes before the final commit expose no sig;
    /// crashes after it recover the same signature through provider_acceptance.
    pub(crate) fn sign_provider_acceptance(
        &self,
        approval: &Approval,
        signer: &Authority,
        now_ms: u64,
    ) -> Result<SignedProviderAcceptance> {
        approval.signing_fenced().map_err(|_| Error::Conflict)?;
        require(
            self.identity()? == *approval.identity() && signer.identity() == approval.identity(),
        )?;
        let record = self.prepare(
            approval.invocation().clone(),
            approval.binding().clone(),
            now_ms,
        )?;
        require(record.binding == *approval.binding())?;
        self.retain_acceptance(&record.invocation, record.attempt, approval.snapshot())?;
        self.retain_request(
            &record.invocation,
            record.attempt,
            approval.request(),
            approval.response_bytes(),
        )?;
        self.reserve_outcome(&record.invocation, record.attempt)?;
        let tx = self.transaction()?;
        let current = current(&tx, &record.invocation)?.ok_or(Error::NotFound)?;
        require(current.attempt == record.attempt && current.binding == record.binding)?;
        let mut meta = metadata(&tx)?;
        let mut table = storage(tx.open_table(TABLE))?;
        if let Some(v) = storage(table.get(record.key().as_str()))? {
            let saved = decode_signed(v.value(), &record, &meta.identity)?;
            require(
                saved.authorization.terms == *approval.terms()
                    && saved.authorization.buyer_sig == approval.buyer().buyer_sig
                    && saved.policy == *approval.policy(),
            )?;
            return Ok(saved);
        }
        require(current.phase == Phase::Prepared && !current.cancellation_requested)?;
        approval.recheck().map_err(|_| Error::Conflict)?;
        let signed = SignedProviderAcceptance {
            authorization: ProxySpendAuthorization {
                terms: approval.terms().clone(),
                buyer_sig: approval.buyer().buyer_sig.clone(),
                provider_sig: signer
                    .provider_spend(approval)
                    .map_err(|_| Error::Identity)?,
            },
            policy: approval.policy().clone(),
        };
        signed.validate(&record, &meta.identity)?;
        let bytes = serde_json::to_vec(&signed).map_err(|_| Error::Invalid)?;
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
        approval.recheck().map_err(|_| Error::Conflict)?;
        self.commit(tx)?;
        Ok(signed)
    }

    /// Historical signature, NOT fresh capacity or permission for a new POST.
    pub fn provider_acceptance(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<SignedProviderAcceptance>> {
        // A reader must not expose a signature while its commit is still awaiting
        // fsync, or after that acknowledgment failed. Reopen resolves the outcome.
        let _guard = self.write_lock.lock().map_err(|_| Error::Storage)?;
        if self.commit_failed.load(Ordering::Acquire) {
            return Err(Error::Storage);
        }
        let tx = storage(self.database.begin_read())?;
        let record = read_record(&storage(tx.open_table(RECORDS))?, invocation, attempt)?;
        let meta: Meta = decode(
            storage(storage(tx.open_table(META))?.get("state"))?
                .ok_or(Error::Invalid)?
                .value(),
        )?;
        let table = storage(tx.open_table(TABLE))?;
        storage(table.get(record.key().as_str()))?
            .map(|v| decode_signed(v.value(), &record, &meta.identity))
            .transpose()
    }
}
