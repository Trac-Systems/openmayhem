//! Immutable private execution/offer snapshots, retained before dispatch. This
//! checks binding consistency, NOT canonical admission, signatures or funding.
//! Storage is bounded and shares the journal's commit/expiry/identity rules.

use super::*;
use crate::{
    endpoint::{Adapter, AdapterSnapshot},
    metering::Policy,
};
use mayhem_proto::proxy::ProxyOffer;

const ACCEPTANCE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("proxy_accepted_snapshots_v1");
const MAX_SNAPSHOT: usize = 256 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceSnapshot {
    pub adapter: AdapterSnapshot,
    pub offer: ProxyOffer,
}
impl fmt::Debug for AcceptanceSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcceptanceSnapshot").finish_non_exhaustive()
    }
}
impl AcceptanceSnapshot {
    pub fn validate_for(&self, binding: &Binding) -> Result<()> {
        let adapter = Adapter::restore(self.adapter.clone()).map_err(|_| Error::Invalid)?;
        if adapter.endpoint() != binding.endpoint
            || adapter.contract_hash() != &binding.endpoint_contract
            || adapter.recipe_hash() != &binding.recipe_digest
        {
            return Err(Error::Conflict);
        }
        validate_offer_binding(&self.offer, binding)
    }
}
pub(crate) fn validate_offer_binding(offer: &ProxyOffer, binding: &Binding) -> Result<()> {
    let policy =
        Policy::resolve(binding.endpoint, &binding.metering_policy).map_err(|_| Error::Invalid)?;
    if offer.digest().map_err(|_| Error::Invalid)? != binding.offer_digest.as_str()
        || offer.endpoint != binding.endpoint
        || offer.market_id != binding.market_id.as_str()
        || offer.provider_pubkey != binding.provider_pubkey.as_str()
        || offer.metering_policy_hash != binding.metering_policy.as_str()
        || !offer.accepted_rails.contains(&binding.rail)
        || !offer
            .rates
            .iter()
            .map(|r| &r.unit)
            .eq(policy.contract().units.iter())
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedAcceptance {
    pub snapshot: AcceptanceSnapshot,
    pub digest: Digest,
}
impl OwnedAcceptance {
    pub fn validate_for(&self, record: &Record) -> Result<()> {
        self.snapshot.validate_for(&record.binding)?;
        if commitment(record, &self.snapshot)? != self.digest {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
fn snapshot_bytes(snapshot: &AcceptanceSnapshot) -> Result<Vec<u8>> {
    let value = serde_json::to_value(snapshot).map_err(|_| Error::Invalid)?;
    let bytes = mayhem_proto::stable_json_bytes(&value).map_err(|_| Error::Invalid)?;
    require(bytes.len() <= MAX_SNAPSHOT)?;
    Ok(bytes)
}
fn commitment(record: &Record, snapshot: &AcceptanceSnapshot) -> Result<Digest> {
    Ok(Digest::hash(
        "mayhem/proxy/accepted-snapshot/v1",
        &[
            record.key().as_bytes(),
            &encode(&record.binding)?,
            &snapshot_bytes(snapshot)?,
        ],
    ))
}
pub(super) fn initialize(tx: &redb::WriteTransaction, create: bool) -> Result<()> {
    let exists = storage(tx.list_tables())?.any(|t| t.name() == ACCEPTANCE.name());
    require(exists != create)?;
    let table = storage(tx.open_table(ACCEPTANCE))?;
    require(storage(table.len())? <= storage(storage(tx.open_table(RECORDS))?.len())?)
}
fn decode_owned(bytes: &[u8], record: &Record) -> Result<OwnedAcceptance> {
    require(bytes.len() <= MAX_SNAPSHOT + 1024)?;
    let owned: OwnedAcceptance = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    owned.validate_for(record)?;
    Ok(owned)
}
pub(super) fn read(tx: &redb::ReadTransaction, record: &Record) -> Result<Option<OwnedAcceptance>> {
    let table = storage(tx.open_table(ACCEPTANCE))?;
    storage(table.get(record.key().as_str()))?
        .map(|v| decode_owned(v.value(), record))
        .transpose()
}
pub(super) fn prune(tx: &redb::WriteTransaction, key: &str, meta: &mut Meta) -> Result<()> {
    let mut table = storage(tx.open_table(ACCEPTANCE))?;
    if let Some(value) = storage(table.remove(key))? {
        meta.payload_bytes = meta
            .payload_bytes
            .checked_sub(value.value().len() as u64)
            .ok_or(Error::Invalid)?;
    }
    Ok(())
}
impl Journal {
    /// Trusted admission supplies the snapshot matching already authenticated
    /// terms. Immutable even after provider reconfiguration/repricing/withdrawal.
    pub fn retain_acceptance(
        &self,
        invocation: &Digest,
        attempt: u64,
        snapshot: &AcceptanceSnapshot,
    ) -> Result<Digest> {
        let tx = self.transaction()?;
        let record = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        if record.attempt != attempt {
            return Err(Error::Stale);
        }
        snapshot.validate_for(&record.binding)?;
        let digest = commitment(&record, snapshot)?;
        let mut table = storage(tx.open_table(ACCEPTANCE))?;
        if let Some(value) = storage(table.get(record.key().as_str()))? {
            let existing = decode_owned(value.value(), &record)?;
            if existing.digest != digest {
                return Err(Error::Conflict);
            }
            return Ok(existing.digest);
        }
        if record.phase != Phase::Prepared {
            return Err(Error::Transition);
        }
        let owned = OwnedAcceptance {
            snapshot: snapshot.clone(),
            digest: digest.clone(),
        };
        let bytes = serde_json::to_vec(&owned).map_err(|_| Error::Invalid)?;
        require(bytes.len() <= MAX_SNAPSHOT + 1024)?;
        let mut meta = metadata(&tx)?;
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
        self.commit(tx)?;
        Ok(digest)
    }
    pub fn accepted_snapshot(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<OwnedAcceptance>> {
        let tx = storage(self.database.begin_read())?;
        let records = storage(tx.open_table(RECORDS))?;
        if storage(records.get(record_key(invocation, attempt).as_str()))?.is_none() {
            return Err(Error::NotFound);
        }
        let record = read_record(&records, invocation, attempt)?;
        read(&tx, &record)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use mayhem_proto::{
        endpoint_family_contract_template,
        proxy::{ProxyLane, ProxyRate},
    };
    use std::os::unix::fs::PermissionsExt;
    const BODY: &[u8] = br#"{"model":"m","messages":[{"role":"user","content":"hello"}]}"#;
    fn d(n: u64) -> Digest {
        Digest::new(format!("{n:064x}")).unwrap()
    }
    fn identity() -> Identity {
        Identity {
            network_id: "918".into(),
            msb_bootstrap: d(1),
            subnet_bootstrap: d(2),
            controller_pubkey: d(3),
        }
    }
    fn limits() -> Limits {
        Limits {
            max_records: 100,
            max_unfinished: 100,
            closed_retention_ms: 1000,
            max_payload_bytes: 1024 * 1024,
        }
    }
    fn fixture() -> (Binding, AcceptanceSnapshot) {
        let adapter = Adapter::new(
            ProxyEndpoint::Chat,
            endpoint_family_contract_template(mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS)
                .unwrap(),
            "private-upstream-model".into(),
            crate::endpoint::Limits {
                request_bytes: 1024 * 1024,
                response_bytes: 1024 * 1024,
                choices: 16,
                tools: 32,
                questions: 32,
                decision_options: 64,
            },
        )
        .unwrap();
        let request = adapter.prepare_json(BODY).unwrap();
        let policy = Policy::ObservableTextV1;
        let offer = ProxyOffer {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            market_id: d(6).as_str().into(),
            provider_pubkey: d(5).as_str().into(),
            membership_revision: 1,
            revision: 1,
            endpoint: ProxyEndpoint::Chat,
            ctx_bracket: "ctx_1024".into(),
            outcome_class: String::new(),
            metering_policy_hash: policy.hash().as_str().into(),
            rates: policy
                .contract()
                .units
                .into_iter()
                .map(|unit| ProxyRate {
                    unit,
                    per_unit_au: 1,
                    granularity: 1,
                })
                .collect(),
            per_request_au: 1,
            min_session_au: 0,
            accepted_rails: vec![ProxyRail::Fiat],
        };
        let binding = Binding {
            request_hash: request.request_hash().clone(),
            endpoint: ProxyEndpoint::Chat,
            contract_version: 30,
            provider_pubkey: d(5),
            market_id: d(6),
            offer_digest: Digest::new(offer.digest().unwrap()).unwrap(),
            endpoint_contract: adapter.contract_hash().clone(),
            metering_policy: policy.hash(),
            accepted_terms: d(9),
            reservation: d(10),
            capacity_lease: d(11),
            connection_digest: d(12),
            connection_revision: 1,
            recipe_digest: adapter.recipe_hash().clone(),
            rail: ProxyRail::Fiat,
        };
        (
            binding,
            AcceptanceSnapshot {
                adapter: adapter.snapshot(),
                offer,
            },
        )
    }
    fn dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        d
    }

    #[test]
    fn snapshot_alone_has_bounded_atomic_retention_and_closed_expiry() {
        let dir = dir();
        let path = dir.path().join("journal");
        let (binding, snapshot) = fixture();
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let r = j.prepare(d(100), binding, 100).unwrap();
        let digest = j
            .retain_acceptance(&r.invocation, r.attempt, &snapshot)
            .unwrap();
        let allocated = j.allocated_payload_bytes().unwrap();
        assert!(allocated > 0 && allocated < MAX_SNAPSHOT as u64);
        assert_eq!(
            j.retain_acceptance(&r.invocation, r.attempt, &snapshot)
                .unwrap(),
            digest
        );
        assert_eq!(j.allocated_payload_bytes().unwrap(), allocated);
        let recovered = j.recover(&r.invocation, r.attempt).unwrap();
        assert!(recovered.request.is_none() && recovered.result.is_none());
        assert_eq!(recovered.acceptance.unwrap().digest, digest);
        assert_eq!(j.prune_closed(u64::MAX, 64).unwrap(), 0);
        let r = j
            .advance(&r.invocation, r.generation, Event::CancelRequested, 101)
            .unwrap();
        let r = j
            .advance(&r.invocation, r.generation, Event::Close(d(99)), 102)
            .unwrap();
        assert_eq!(j.prune_closed(r.expires_at_ms.unwrap(), 64).unwrap(), 1);
        assert_eq!(j.allocated_payload_bytes().unwrap(), 0);
        assert!(matches!(
            j.accepted_snapshot(&r.invocation, r.attempt),
            Err(Error::NotFound)
        ));
        drop(j);
        let j = Journal::open(
            &path,
            identity(),
            Limits {
                max_payload_bytes: 1,
                ..limits()
            },
        )
        .unwrap();
        let r = j.prepare(d(101), fixture().0, 1000).unwrap();
        assert!(matches!(
            j.retain_acceptance(&r.invocation, r.attempt, &snapshot),
            Err(Error::Capacity)
        ));
        assert!(j
            .accepted_snapshot(&r.invocation, r.attempt)
            .unwrap()
            .is_none());
        assert_eq!(j.allocated_payload_bytes().unwrap(), 0);
    }

    #[test]
    fn schema_two_migration_preserves_owned_request_and_unknown_dispatch_without_fabricating_snapshot(
    ) {
        let dir = dir();
        let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let r = j.prepare(d(100), fixture().0, 100).unwrap();
        let request = j
            .retain_request(&r.invocation, r.attempt, BODY, 4096)
            .unwrap();
        let r = j
            .begin_dispatch(&r.invocation, r.generation, 101)
            .unwrap()
            .record()
            .clone();
        let allocated = j.allocated_payload_bytes().unwrap();
        let tx = j.transaction().unwrap();
        tx.delete_table(ACCEPTANCE).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_accepted_finance_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_outcomes_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_acceptance_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_retirement_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_jobs_v1")).unwrap();
        let mut meta = metadata(&tx).unwrap();
        meta.schema = 2;
        save_meta(&tx, &meta).unwrap();
        j.commit(tx).unwrap();
        drop(j);
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let saved = j.recover(&r.invocation, r.attempt).unwrap();
        assert_eq!(saved.request.unwrap().digest, request);
        assert!(saved.acceptance.is_none() && saved.result.is_none());
        assert_eq!(saved.record, r);
        assert_eq!(j.allocated_payload_bytes().unwrap(), allocated);
        assert!(j.begin_dispatch(&r.invocation, r.generation, 102).is_err());
        assert!(matches!(
            j.retain_acceptance(&r.invocation, r.attempt, &fixture().1),
            Err(Error::Transition)
        ));
    }

    #[test]
    fn schema_three_upgrade_keeps_owned_offer_and_uncertain_dispatch_without_inventing_finance() {
        let dir=dir();let path=dir.path().join("journal");
        let j=Journal::open(&path,identity(),limits()).unwrap();
        let (binding,snapshot)=fixture();
        let r=j.prepare(d(100),binding,100).unwrap();
        j.retain_acceptance(&r.invocation,r.attempt,&snapshot).unwrap();
        j.retain_request(&r.invocation,r.attempt,BODY,4096).unwrap();
        let r=j.begin_dispatch(&r.invocation,r.generation,101).unwrap().record().clone();
        let allocated=j.allocated_payload_bytes().unwrap();
        let tx=j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_accepted_finance_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_outcomes_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_acceptance_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_retirement_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_jobs_v1")).unwrap();
        let mut meta=metadata(&tx).unwrap();meta.schema=3;save_meta(&tx,&meta).unwrap();j.commit(tx).unwrap();drop(j);
        let j=Journal::open(&path,identity(),limits()).unwrap();
        let saved=j.recover(&r.invocation,r.attempt).unwrap();
        assert_eq!(saved.record,r);assert!(saved.acceptance.is_some());assert!(saved.financial.is_none());
        assert_eq!(j.allocated_payload_bytes().unwrap(),allocated);
        assert!(j.begin_dispatch(&r.invocation,r.generation,102).is_err());
    }

    #[test]
    fn schema_four_upgrade_keeps_unknown_execution_without_inventing_a_receipt() {
        let dir = dir();
        let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let (binding, snapshot) = fixture();
        let r = j.prepare(d(100), binding, 100).unwrap();
        j.retain_acceptance(&r.invocation, r.attempt, &snapshot).unwrap();
        j.retain_request(&r.invocation, r.attempt, BODY, 4096).unwrap();
        let r = j.begin_dispatch(&r.invocation, r.generation, 101).unwrap().record().clone();
        let allocated = j.allocated_payload_bytes().unwrap();
        let tx = j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_outcomes_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_acceptance_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_retirement_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_jobs_v1")).unwrap();
        let mut meta = metadata(&tx).unwrap();
        meta.schema = 4;
        save_meta(&tx, &meta).unwrap();
        j.commit(tx).unwrap();
        drop(j);
        let j = Journal::open(&path, identity(), limits()).unwrap();
        assert_eq!(j.get(&r.invocation).unwrap().unwrap(), r);
        assert_eq!(j.allocated_payload_bytes().unwrap(), allocated);
        assert!(j.terminal_draft(&r.invocation, r.attempt).unwrap().is_none());
        assert!(j.terminal_receipt(&r.invocation, r.attempt).unwrap().is_none());
        assert!(j.begin_dispatch(&r.invocation, r.generation, 102).is_err());
    }

    #[test]
    fn schema_six_upgrade_preserves_unknown_execution_without_inventing_provider_acceptance() {
        let dir = dir(); let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let (binding, snapshot) = fixture();
        let r = j.prepare(d(100), binding, 100).unwrap();
        j.retain_acceptance(&r.invocation, r.attempt, &snapshot).unwrap();
        j.retain_request(&r.invocation, r.attempt, BODY, 4096).unwrap();
        let r = j.begin_dispatch(&r.invocation, r.generation, 101).unwrap().record().clone();
        let allocated = j.allocated_payload_bytes().unwrap();
        let tx = j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_acceptance_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_retirement_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_jobs_v1")).unwrap();
        let mut meta = metadata(&tx).unwrap(); meta.schema = 6; save_meta(&tx, &meta).unwrap();
        j.commit(tx).unwrap(); drop(j);
        let j = Journal::open(&path, identity(), limits()).unwrap();
        assert_eq!(j.get(&r.invocation).unwrap().unwrap(), r);
        assert_eq!(j.allocated_payload_bytes().unwrap(), allocated);
        assert!(j.provider_acceptance(&r.invocation, r.attempt).unwrap().is_none());
        assert!(j.begin_dispatch(&r.invocation, r.generation, 102).is_err());
        let tx = j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_acceptance_v1")).unwrap();
        j.commit(tx).unwrap(); drop(j);
        assert!(Journal::open(&path, identity(), limits()).is_err(), "current schema must not silently rebuild a missing signature table");
    }

    #[test]
    fn schema_seven_upgrade_preserves_dispatch_without_fabricating_retirement() {
        let dir = dir(); let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let r = j.prepare(d(100), fixture().0, 100).unwrap();
        let r = j.begin_dispatch(&r.invocation, r.generation, 101).unwrap().record().clone();
        let tx = j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_retirement_v1")).unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_jobs_v1")).unwrap();
        let mut meta = metadata(&tx).unwrap(); meta.schema = 7; save_meta(&tx, &meta).unwrap();
        j.commit(tx).unwrap(); drop(j);
        let j = Journal::open(&path, identity(), limits()).unwrap();
        assert_eq!(j.get(&r.invocation).unwrap().unwrap(), r);
        assert!(j.provider_retirement(&r.invocation, r.attempt).unwrap().is_none());
        assert_eq!(j.recovery_page(None, 64).unwrap().records.len(), 1);
        let tx = j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_provider_retirement_v1")).unwrap();
        j.commit(tx).unwrap(); drop(j);
        assert!(Journal::open(&path, identity(), limits()).is_err());
    }

    #[test]
    fn corruption_missing_table_and_binding_changes_fail_closed_without_disclosure() {
        for missing in [false, true] {
            let dir = dir();
            let path = dir.path().join("journal");
            let j = Journal::open(&path, identity(), limits()).unwrap();
            let (binding, snapshot) = fixture();
            let r = j.prepare(d(100), binding, 100).unwrap();
            j.retain_acceptance(&r.invocation, r.attempt, &snapshot)
                .unwrap();
            for edit in 0..4 {
                let mut wrong = snapshot.clone();
                match edit {
                    0 => wrong.adapter.upstream_model = "different".into(),
                    1 => wrong.adapter.version = 2,
                    2 => wrong.offer.revision += 1,
                    _ => wrong.offer.accepted_rails = vec![ProxyRail::Tap],
                }
                assert!(j
                    .retain_acceptance(&r.invocation, r.attempt, &wrong)
                    .is_err());
            }
            let saved = j
                .accepted_snapshot(&r.invocation, r.attempt)
                .unwrap()
                .unwrap();
            assert!(!format!("{saved:?}").contains("private-upstream-model"));
            let tx = j.transaction().unwrap();
            if missing {
                tx.delete_table(ACCEPTANCE).unwrap();
            } else {
                tx.open_table(ACCEPTANCE)
                    .unwrap()
                    .insert(r.key().as_str(), b"corrupt".as_slice())
                    .unwrap();
            }
            j.commit(tx).unwrap();
            assert_eq!(j.get(&r.invocation).unwrap().unwrap(), r);
            if !missing {
                assert!(j.recover(&r.invocation, r.attempt).is_err());
            }
            drop(j);
            if missing {
                assert!(Journal::open(&path, identity(), limits()).is_err());
            }
        }
    }
}
