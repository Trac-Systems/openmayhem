//! Owned payloads share the journal transaction/lock/identity and expiry index.
//! Request body is retained before POST and result capacity is reserved then.
//! Payloads never enter Debug, logs, discovery or ledger records. A stored result
//! passed endpoint/schema checks. Observed quantities do not imply settlement.

use super::*;
use crate::endpoint::ProtocolReply;
use serde_json::Value;

const PAYLOADS: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_owned_meta_v1");
const INPUT: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_owned_requests_v1");
const OUTPUT: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_owned_results_v1");
const MAX_BODY: usize = 256 * 1024 * 1024;
const RESULT_OVERHEAD: usize = 16 * 1024;

/// Existing private journal commitments remain readable. New receipts commit
/// only to the normalized result the buyer actually receives. The domains are
/// distinct, so changing this selector cannot reinterpret a signed commitment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultCommitment {
    #[default]
    OwnedV1,
    PublicV1,
}
impl ResultCommitment {
    pub(crate) fn compute(
        self,
        invocation: &Digest,
        attempt: u64,
        binding: &Binding,
        reply: &ProtocolReply,
    ) -> Result<Digest> {
        if self == Self::OwnedV1 {
            return result_commitment(
                invocation,
                attempt,
                binding,
                &serde_json::to_vec(reply).map_err(|_| Error::Invalid)?,
            );
        }
        let bytes = serde_json::to_vec(&reply.body).map_err(|_| Error::Invalid)?;
        require(!bytes.is_empty() && bytes.len() <= MAX_BODY)?;
        Ok(Digest::hash(
            "mayhem/proxy/public-result/v1",
            &[
                record_key(invocation, attempt).as_bytes(),
                &encode(binding)?,
                &bytes,
            ],
        ))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    request_digest: Digest,
    request_bytes: u64,
    result_limit: u64,
    result_digest: Option<Digest>,
    result_bytes: u64,
    allocated: u64,
}
impl Payload {
    fn validate(&self) -> Result<()> {
        require(
            self.request_bytes > 0
                && self.request_bytes <= MAX_BODY as u64
                && self.result_limit > 0
                && self.result_limit <= (MAX_BODY + RESULT_OVERHEAD) as u64
                && self.result_bytes <= self.result_limit
                && self.result_digest.is_some() == (self.result_bytes > 0),
        )?;
        require(
            self.allocated
                == self
                    .request_bytes
                    .checked_add(if self.result_digest.is_some() {
                        self.result_bytes
                    } else {
                        self.result_limit
                    })
                    .ok_or(Error::Invalid)?,
        )
    }
}
pub struct OwnedRequest {
    pub body: Vec<u8>,
    pub digest: Digest,
}
impl fmt::Debug for OwnedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedRequest")
            .field("bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}
pub struct OwnedResult {
    pub reply: ProtocolReply,
    pub digest: Digest,
}
impl fmt::Debug for OwnedResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedResult").finish_non_exhaustive()
    }
}
#[derive(Debug)]
pub struct Recovery {
    pub record: Record,
    pub acceptance: Option<OwnedAcceptance>,
    pub financial: Option<crate::financial::Retained>,
    pub request: Option<OwnedRequest>,
    pub result: Option<OwnedResult>,
}
/// Bounded control metadata only. `has_result` is a scheduling hint, never proof
/// authorizing capacity release, billing, or delivery of the retained payload.
pub(crate) struct RecoveryHeader {
    pub record: Record,
    pub financial: Option<crate::financial::Retained>,
    pub has_result: bool,
}
pub(super) fn initialize(tx: &redb::WriteTransaction, create: bool) -> Result<()> {
    let names = storage(tx.list_tables())?
        .map(|n| n.name().to_owned())
        .collect::<Vec<_>>();
    for name in [PAYLOADS.name(), INPUT.name(), OUTPUT.name()] {
        require(names.iter().any(|n| n == name) != create)?;
    }
    let records = storage(tx.open_table(PAYLOADS))?;
    let input = storage(tx.open_table(INPUT))?;
    let output = storage(tx.open_table(OUTPUT))?;
    require(
        storage(records.len())? == storage(input.len())?
            && storage(output.len())? <= storage(records.len())?,
    )
}
fn payload(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &str,
) -> Result<Option<Payload>> {
    let p: Option<Payload> = storage(table.get(key))?
        .map(|v| decode(v.value()))
        .transpose()?;
    if let Some(p) = &p {
        p.validate()?;
    }
    Ok(p)
}
fn request_digest(record: &Record, body: &[u8]) -> Result<Digest> {
    require(!body.is_empty() && body.len() <= MAX_BODY)?;
    let value: Value = serde_json::from_slice(body).map_err(|_| Error::Invalid)?;
    require(value.is_object())?;
    if mayhem_proto::endpoint_request_fingerprint(&value) != record.binding.request_hash.as_str() {
        return Err(Error::Conflict);
    }
    Ok(Digest::hash(
        "mayhem/proxy/owned-request/v1",
        &[record.key().as_bytes(), &encode(&record.binding)?, body],
    ))
}
fn result_digest(record: &Record, bytes: &[u8]) -> Result<Digest> {
    result_commitment(&record.invocation, record.attempt, &record.binding, bytes)
}
pub(crate) fn result_commitment(
    invocation: &Digest,
    attempt: u64,
    binding: &Binding,
    bytes: &[u8],
) -> Result<Digest> {
    require(!bytes.is_empty() && bytes.len() <= MAX_BODY + RESULT_OVERHEAD)?;
    Ok(Digest::hash(
        "mayhem/proxy/owned-result/v1",
        &[
            record_key(invocation, attempt).as_bytes(),
            &encode(binding)?,
            bytes,
        ],
    ))
}
impl Journal {
    pub(crate) fn recovery_header(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<RecoveryHeader> {
        require(attempt > 0)?;
        let tx = storage(self.database.begin_read())?;
        let record = read_record(&storage(tx.open_table(RECORDS))?, invocation, attempt)?;
        let financial = super::finance::read(&tx, &record)?;
        let saved = payload(&storage(tx.open_table(PAYLOADS))?, &record.key())?;
        Ok(RecoveryHeader {
            record,
            financial,
            has_result: saved.is_some_and(|p| p.result_digest.is_some()),
        })
    }
    /// Called by the trusted parent after endpoint validation, before dispatch.
    /// Reserve worst-case result bytes now; a full logical store cannot strand a
    /// successful paid request merely because unrelated jobs consumed its space.
    pub fn retain_request(
        &self,
        invocation: &Digest,
        attempt: u64,
        body: &[u8],
        result_body_limit: usize,
    ) -> Result<Digest> {
        require(result_body_limit > 0 && result_body_limit <= MAX_BODY)?;
        let tx = self.transaction()?;
        let record = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        if record.attempt != attempt {
            return Err(Error::Stale);
        }
        let digest = request_digest(&record, body)?;
        let key = record.key();
        let mut table = storage(tx.open_table(PAYLOADS))?;
        if let Some(p) = payload(&table, &key)? {
            // Body-hash equality is the idempotency rule. Preserve the original
            // exact JSON bytes when a replay only changes whitespace/key order.
            let inputs = storage(tx.open_table(INPUT))?;
            let saved = storage(inputs.get(key.as_str()))?.ok_or(Error::Invalid)?;
            require(
                saved.value().len() as u64 == p.request_bytes
                    && request_digest(&record, saved.value())? == p.request_digest,
            )?;
            return Ok(p.request_digest);
        }
        if record.phase != Phase::Prepared {
            return Err(Error::Transition);
        }
        let result_limit = result_body_limit
            .checked_add(RESULT_OVERHEAD)
            .ok_or(Error::Invalid)? as u64;
        let allocated = (body.len() as u64)
            .checked_add(result_limit)
            .ok_or(Error::Invalid)?;
        let mut meta = metadata(&tx)?;
        let new_total = meta
            .payload_bytes
            .checked_add(allocated)
            .ok_or(Error::Capacity)?;
        if new_total > self.limits.max_payload_bytes {
            return Err(Error::Capacity);
        }
        let p = Payload {
            request_digest: digest.clone(),
            request_bytes: body.len() as u64,
            result_limit,
            result_digest: None,
            result_bytes: 0,
            allocated,
        };
        storage(table.insert(key.as_str(), encode(&p)?.as_slice()))?;
        storage(storage(tx.open_table(INPUT))?.insert(key.as_str(), body))?;
        meta.payload_bytes = new_total;
        save_meta(&tx, &meta)?;
        drop(table);
        self.commit(tx)?;
        Ok(digest)
    }
    /// Crate-private: only the parent validated-result path supplies this value.
    /// Persist even a late cancelled result for reconciliation, never to override
    /// cancellation. This cannot resolve money, create receipts or free capacity.
    pub(crate) fn retain_result(
        &self,
        invocation: &Digest,
        attempt: u64,
        reply: &ProtocolReply,
        now_ms: u64,
    ) -> Result<OwnedResult> {
        self.retain_result_checked(invocation, attempt, reply, now_ms, None)
    }
    pub(crate) fn retain_job_result(
        &self,
        lease: &super::jobs::Lease,
        reply: &ProtocolReply,
        now: u64,
    ) -> Result<OwnedResult> {
        self.retain_result_checked(
            &lease.record().invocation,
            lease.record().attempt,
            reply,
            now,
            Some(lease),
        )
    }
    fn retain_result_checked(
        &self,
        invocation: &Digest,
        attempt: u64,
        reply: &ProtocolReply,
        now_ms: u64,
        lease: Option<&super::jobs::Lease>,
    ) -> Result<OwnedResult> {
        let tx = self.transaction()?;
        if let Some(lease) = lease {
            super::jobs::check(&tx, lease, now_ms)?;
        }
        let mut record = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        if record.attempt != attempt {
            return Err(Error::Stale);
        }
        require(now_ms >= record.updated_at_ms)?;
        let key = record.key();
        let mut table = storage(tx.open_table(PAYLOADS))?;
        let mut p = payload(&table, &key)?.ok_or(Error::NotFound)?;
        let bytes = serde_json::to_vec(reply).map_err(|_| Error::Invalid)?;
        if bytes.len() as u64 > p.result_limit {
            return Err(Error::Capacity);
        }
        let digest = result_digest(&record, &bytes)?;
        if let Some(existing) = &p.result_digest {
            if existing != &digest {
                return Err(Error::Conflict);
            }
            return Ok(OwnedResult {
                reply: serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?,
                digest,
            });
        }
        if record.phase != Phase::Dispatched {
            return Err(Error::Transition);
        }
        require(
            reply.body["id"].as_str() == Some(format!("proxy_{}", invocation.as_str()).as_str()),
        )?;
        if let Some(id) = &reply.upstream_id {
            if record.remote_id.as_ref().is_some_and(|old| old != id) {
                return Err(Error::Conflict);
            }
            record.remote_id = Some(id.clone());
        }
        let mut meta = metadata(&tx)?;
        meta.payload_bytes = meta
            .payload_bytes
            .checked_sub(p.allocated)
            .ok_or(Error::Invalid)?;
        p.result_bytes = bytes.len() as u64;
        p.result_digest = Some(digest.clone());
        p.allocated = p
            .request_bytes
            .checked_add(p.result_bytes)
            .ok_or(Error::Invalid)?;
        meta.payload_bytes = meta
            .payload_bytes
            .checked_add(p.allocated)
            .ok_or(Error::Invalid)?;
        p.validate()?;
        storage(table.insert(key.as_str(), encode(&p)?.as_slice()))?;
        storage(storage(tx.open_table(OUTPUT))?.insert(key.as_str(), bytes.as_slice()))?;
        save_meta(&tx, &meta)?;
        bump(&tx, &mut record, now_ms)?;
        save(&tx, &record)?;
        drop(table);
        self.commit(tx)?;
        Ok(OwnedResult {
            reply: serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?,
            digest,
        })
    }
    /// Exact-key local recovery; no network, fresh dispatch ticket, history scan
    /// or automatic buyer delivery. Caller authenticates ownership and reconciles
    /// the immutable accepted terms before exposing/rebilling any retained result.
    pub fn recover(&self, invocation: &Digest, attempt: u64) -> Result<Recovery> {
        require(attempt > 0)?;
        let tx = storage(self.database.begin_read())?;
        let records = storage(tx.open_table(RECORDS))?;
        if storage(records.get(record_key(invocation, attempt).as_str()))?.is_none() {
            return Err(Error::NotFound);
        }
        let record = read_record(&records, invocation, attempt)?;
        let acceptance = super::acceptance::read(&tx, &record)?;
        let financial = super::finance::read(&tx, &record)?;
        let table = storage(tx.open_table(PAYLOADS))?;
        let Some(p) = payload(&table, &record.key())? else {
            return Ok(Recovery {
                record,
                acceptance,
                financial,
                request: None,
                result: None,
            });
        };
        let input = storage(tx.open_table(INPUT))?;
        let body = storage(input.get(record.key().as_str()))?.ok_or(Error::Invalid)?;
        require(
            body.value().len() as u64 == p.request_bytes
                && request_digest(&record, body.value())? == p.request_digest,
        )?;
        let request = Some(OwnedRequest {
            body: body.value().to_vec(),
            digest: p.request_digest,
        });
        let output = storage(tx.open_table(OUTPUT))?;
        let result = match (p.result_digest, storage(output.get(record.key().as_str()))?) {
            (None, None) => None,
            (Some(digest), Some(bytes)) => {
                require(
                    bytes.value().len() as u64 == p.result_bytes
                        && result_digest(&record, bytes.value())? == digest,
                )?;
                let reply: ProtocolReply =
                    serde_json::from_slice(bytes.value()).map_err(|_| Error::Invalid)?;
                require(reply.upstream_id == record.remote_id)?;
                Some(OwnedResult { reply, digest })
            }
            _ => return Err(Error::Invalid),
        };
        Ok(Recovery {
            record,
            acceptance,
            financial,
            request,
            result,
        })
    }
    pub fn allocated_payload_bytes(&self) -> Result<u64> {
        let tx = storage(self.database.begin_read())?;
        let meta = storage(tx.open_table(META))?;
        Ok(
            decode::<Meta>(storage(meta.get("state"))?.ok_or(Error::Invalid)?.value())?
                .payload_bytes,
        )
    }
}
pub(super) fn prune(tx: &redb::WriteTransaction, key: &str, meta: &mut Meta) -> Result<()> {
    super::acceptance::prune(tx, key, meta)?;
    super::jobs::prune(tx, key, meta)?;
    let mut table = storage(tx.open_table(PAYLOADS))?;
    let Some(p) = payload(&table, key)? else {
        return Ok(());
    };
    let mut input = storage(tx.open_table(INPUT))?;
    require(
        storage(input.remove(key))?.is_some_and(|v| v.value().len() as u64 == p.request_bytes),
    )?;
    let mut output = storage(tx.open_table(OUTPUT))?;
    let removed = storage(output.remove(key))?;
    require(match (removed, p.result_digest) {
        (Some(value), Some(_)) => value.value().len() as u64 == p.result_bytes,
        (None, None) => true,
        _ => false,
    })?;
    storage(table.remove(key))?;
    meta.payload_bytes = meta
        .payload_bytes
        .checked_sub(p.allocated)
        .ok_or(Error::Invalid)?;
    Ok(())
}

pub(super) fn verify_resolution(
    tx: &redb::WriteTransaction,
    key: &str,
    resolution: &Resolution,
) -> Result<()> {
    let table = storage(tx.open_table(PAYLOADS))?;
    let Some(p) = payload(&table, key)? else {
        return Ok(());
    };
    let verified = match resolution {
        Resolution::Completed { verified } => Some(verified),
        Resolution::Cancelled { partial, .. } | Resolution::Failed { partial, .. } => {
            partial.as_ref()
        }
        Resolution::NotExecuted { .. } => {
            if p.result_digest.is_some() {
                return Err(Error::Transition);
            }
            None
        }
    };
    if let Some(v) = verified {
        if p.result_digest.as_ref() != Some(&v.result) {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };
    const BODY: &[u8] =
        br#"{"model":"m","messages":[{"role":"user","content":"private-request-fixture"}]}"#;
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
            max_payload_bytes: 128 * 1024,
        }
    }
    fn binding() -> Binding {
        Binding {
            request_hash: Digest::new(mayhem_proto::endpoint_request_fingerprint(
                &serde_json::from_slice(BODY).unwrap(),
            ))
            .unwrap(),
            endpoint: ProxyEndpoint::Chat,
            contract_version: 30,
            provider_pubkey: d(1),
            market_id: d(2),
            offer_digest: d(3),
            endpoint_contract: d(4),
            metering_policy: d(5),
            accepted_terms: d(6),
            reservation: d(7),
            capacity_lease: d(8),
            connection_digest: d(9),
            connection_revision: 1,
            recipe_digest: d(10),
            rail: ProxyRail::Tap,
        }
    }
    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }
    fn prepare(j: &Journal, n: u64) -> Record {
        let r = j.prepare(d(n), binding(), 100).unwrap();
        j.retain_request(&r.invocation, r.attempt, BODY, 1024)
            .unwrap();
        j.begin_dispatch(&r.invocation, r.generation, 101)
            .unwrap()
            .record()
            .clone()
    }
    fn reply(r: &Record) -> ProtocolReply {
        ProtocolReply {
            body: json!({"id":format!("proxy_{}",r.invocation.as_str()),"model":"m","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"private-result-fixture"},"finish_reason":"stop"}]}),
            reported_usage: None,
            observed_usage: None,
            upstream_id: Some(RemoteId::new("opaque-upstream-id").unwrap()),
        }
    }
    fn complete(j: &Journal, r: &Record, result: Digest) -> Record {
        let current = j.get(&r.invocation).unwrap().unwrap();
        j.advance(
            &r.invocation,
            current.generation,
            Event::Resolve(Resolution::Completed {
                verified: VerifiedResult {
                    result,
                    usage_evidence: d(200),
                },
            }),
            200,
        )
        .unwrap()
    }
    #[test]
    fn owned_results_are_immutable_bound_to_resolution_and_pruned_only_after_closure() {
        let dir = private_dir();
        let j = Journal::open(dir.path().join("journal"), identity(), limits()).unwrap();
        let r = prepare(&j, 100);
        let value = reply(&r);
        let reserved = j.allocated_payload_bytes().unwrap();
        let retained = j
            .retain_result(&r.invocation, r.attempt, &value, 102)
            .unwrap();
        assert!(j.allocated_payload_bytes().unwrap() < reserved);
        let current = j.get(&r.invocation).unwrap().unwrap();
        assert_eq!(
            j.retain_result(&r.invocation, r.attempt, &value, 102)
                .unwrap()
                .digest,
            retained.digest
        );
        assert_eq!(
            j.get(&r.invocation).unwrap().unwrap().generation,
            current.generation
        );
        let mut other = reply(&r);
        other.body["model"] = json!("different-result");
        assert!(matches!(
            j.retain_result(&r.invocation, r.attempt, &other, 103),
            Err(Error::Conflict)
        ));
        assert!(matches!(
            j.advance(
                &r.invocation,
                current.generation,
                Event::Resolve(Resolution::Completed {
                    verified: VerifiedResult {
                        result: d(999),
                        usage_evidence: d(200)
                    }
                }),
                103
            ),
            Err(Error::Conflict)
        ));
        assert!(matches!(
            j.advance(
                &r.invocation,
                current.generation,
                Event::Resolve(Resolution::NotExecuted { evidence: d(999) }),
                103
            ),
            Err(Error::Transition)
        ));
        assert_eq!(j.prune_closed(u64::MAX, 64).unwrap(), 0);
        let r = complete(&j, &r, retained.digest);
        assert_eq!(j.prune_closed(u64::MAX, 64).unwrap(), 0);
        let r = j
            .advance(&r.invocation, r.generation, Event::Close(d(201)), 201)
            .unwrap();
        assert!(j
            .recover(&r.invocation, r.attempt)
            .unwrap()
            .result
            .is_some());
        assert_eq!(j.prune_closed(1200, 64).unwrap(), 0);
        assert_eq!(j.prune_closed(1201, 64).unwrap(), 1);
        assert_eq!(j.allocated_payload_bytes().unwrap(), 0);
        assert!(matches!(
            j.recover(&r.invocation, r.attempt),
            Err(Error::NotFound)
        ));
    }
    #[test]
    fn admission_space_is_reserved_and_lower_new_limits_do_not_block_existing_results() {
        let dir = private_dir();
        let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let r = prepare(&j, 100);
        let whitespace=b"{ \"messages\":[{\"content\":\"private-request-fixture\",\"role\":\"user\"}], \"model\":\"m\" }";
        j.retain_request(&r.invocation, r.attempt, whitespace, 1024)
            .unwrap();
        assert_eq!(
            j.recover(&r.invocation, r.attempt)
                .unwrap()
                .request
                .unwrap()
                .body,
            BODY
        );
        assert!(matches!(
            j.retain_request(&r.invocation, r.attempt, br#"{"model":"changed"}"#, 1024),
            Err(Error::Conflict)
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
        let pending = j.prepare(d(101), binding(), 101).unwrap();
        assert!(matches!(
            j.retain_request(&pending.invocation, pending.attempt, BODY, 1024),
            Err(Error::Capacity)
        ));
        let retained = j
            .retain_result(&r.invocation, r.attempt, &reply(&r), 102)
            .unwrap();
        assert_eq!(
            j.recover(&r.invocation, r.attempt)
                .unwrap()
                .result
                .unwrap()
                .digest,
            retained.digest
        );
    }
    #[test]
    fn async_job_leases_survive_restart_fence_stale_writers_and_preserve_cancel_intent() {
        let dir = private_dir();
        let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let r = j.prepare(d(890), binding(), 100).unwrap();
        j.retain_request(&r.invocation, r.attempt, BODY, 1024)
            .unwrap();
        let before = j.allocated_payload_bytes().unwrap();
        j.reserve_job(&r.invocation, r.attempt).unwrap();
        assert_eq!(j.allocated_payload_bytes().unwrap(), before + 1024);
        j.reserve_job(&r.invocation, r.attempt).unwrap();
        assert_eq!(j.allocated_payload_bytes().unwrap(), before + 1024);
        let old = j
            .claim_job(&r.invocation, r.attempt, 100, 100)
            .unwrap()
            .unwrap();
        assert!(j
            .claim_job(&r.invocation, r.attempt, 101, 100)
            .unwrap()
            .is_none());
        j.begin_dispatch(&r.invocation, r.generation, 101).unwrap();
        j.accept_job(&old, RemoteId::new("opaque-upstream-id").unwrap(), 102)
            .unwrap();
        j.mark_job_cancel(&old, 103).unwrap();
        drop(j);
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let next = j
            .claim_job(&r.invocation, r.attempt, 200, 100)
            .unwrap()
            .unwrap();
        assert!(next.cancel_sent);
        assert!(matches!(
            j.accept_job(&old, RemoteId::new("changed").unwrap(), 201),
            Err(Error::Stale)
        ));
        assert!(matches!(
            j.retain_job_result(&old, &reply(&r), 201),
            Err(Error::Stale)
        ));
        assert!(matches!(
            j.accept_job(&next, RemoteId::new("changed").unwrap(), 201),
            Err(Error::Conflict)
        ));
        let result = j.retain_job_result(&next, &reply(&r), 201).unwrap();
        assert_eq!(
            j.recover(&r.invocation, r.attempt)
                .unwrap()
                .result
                .unwrap()
                .digest,
            result.digest
        );
        assert_eq!(
            j.get(&r.invocation)
                .unwrap()
                .unwrap()
                .remote_id
                .unwrap()
                .as_str(),
            "opaque-upstream-id"
        );
    }
    #[test]
    fn async_job_control_reserve_cannot_bypass_payload_quota() {
        let dir = private_dir();
        let mut l = limits();
        l.max_payload_bytes = 1;
        let j = Journal::open(dir.path().join("journal"), identity(), l).unwrap();
        let r = j.prepare(d(891), binding(), 100).unwrap();
        assert!(matches!(
            j.reserve_job(&r.invocation, r.attempt),
            Err(Error::Capacity)
        ));
        assert_eq!(j.allocated_payload_bytes().unwrap(), 0);
        assert_eq!(
            j.get(&r.invocation).unwrap().unwrap().phase,
            Phase::Prepared
        );
    }
    #[test]
    fn schema_eight_upgrade_never_creates_a_job_or_redispatch_authority() {
        let dir = private_dir();
        let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let r = prepare(&j, 892);
        let allocated = j.allocated_payload_bytes().unwrap();
        let tx = j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_jobs_v1"))
            .unwrap();
        let mut meta = metadata(&tx).unwrap();
        meta.schema = 8;
        save_meta(&tx, &meta).unwrap();
        j.commit(tx).unwrap();
        drop(j);
        let j = Journal::open(&path, identity(), limits()).unwrap();
        assert_eq!(j.get(&r.invocation).unwrap().unwrap(), r);
        assert!(!j.has_job(&r.invocation, r.attempt).unwrap());
        assert_eq!(j.allocated_payload_bytes().unwrap(), allocated);
        assert!(j.begin_dispatch(&r.invocation, r.generation, 102).is_err());
        let tx = j.transaction().unwrap();
        tx.delete_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_jobs_v1"))
            .unwrap();
        j.commit(tx).unwrap();
        drop(j);
        assert!(Journal::open(&path, identity(), limits()).is_err());
    }
    #[test]
    fn old_record_only_journal_migrates_without_erasing_uncertain_attempts() {
        let dir = private_dir();
        let path = dir.path().join("journal");
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let r = j.prepare(d(100), binding(), 100).unwrap();
        let r = j
            .begin_dispatch(&r.invocation, r.generation, 101)
            .unwrap()
            .record()
            .clone();
        let tx = j.transaction().unwrap();
        for name in [
            PAYLOADS.name(),
            INPUT.name(),
            OUTPUT.name(),
            "proxy_accepted_snapshots_v1",
            "proxy_accepted_finance_v1",
            "proxy_attempt_outcomes_v1",
            "proxy_provider_acceptance_v1",
            "proxy_provider_retirement_v1",
            "proxy_attempt_jobs_v1",
        ] {
            tx.delete_table(TableDefinition::<&str, &[u8]>::new(name))
                .unwrap();
        }
        let mut m = metadata(&tx).unwrap();
        m.schema = 1;
        save_meta(&tx, &m).unwrap();
        j.commit(tx).unwrap();
        drop(j);
        let j = Journal::open(&path, identity(), limits()).unwrap();
        let recovered = j.recover(&r.invocation, r.attempt).unwrap();
        assert_eq!(recovered.record, r);
        assert!(recovered.request.is_none() && recovered.result.is_none());
        assert!(j.begin_dispatch(&r.invocation, r.generation, 102).is_err());
        assert!(matches!(
            j.retain_request(&r.invocation, r.attempt, BODY, 1024),
            Err(Error::Transition)
        ));
    }
    #[test]
    fn corrupted_payload_fails_closed_without_deleting_attempt_or_reissuing_dispatch() {
        for output in [false, true] {
            let dir = private_dir();
            let path = dir.path().join("journal");
            let j = Journal::open(&path, identity(), limits()).unwrap();
            let r = prepare(&j, 100);
            j.retain_result(&r.invocation, r.attempt, &reply(&r), 102)
                .unwrap();
            let tx = j.transaction().unwrap();
            tx.open_table(if output { OUTPUT } else { INPUT })
                .unwrap()
                .insert(r.key().as_str(), b"corrupted".as_slice())
                .unwrap();
            j.commit(tx).unwrap();
            // Maintenance sees only a scheduling hint, never validated output.
            let header = j.recovery_header(&r.invocation, r.attempt).unwrap();
            assert!(header.has_result);
            assert!(header.financial.is_none());
            assert_eq!(header.record.phase, Phase::Dispatched);
            assert!(j.recover(&r.invocation, r.attempt).is_err());
            assert_eq!(
                j.get(&r.invocation).unwrap().unwrap().phase,
                Phase::Dispatched
            );
            assert_eq!(j.prune_closed(u64::MAX, 64).unwrap(), 0);
        }
    }
    #[test]
    fn owned_payload_crash_child() {
        let Ok(path) = std::env::var("MAYHEM_PROXY_OWNED_TEST_PATH") else {
            return;
        };
        let j = Journal::open(path, identity(), limits()).unwrap();
        let r = prepare(&j, 100);
        if std::env::var("MAYHEM_PROXY_OWNED_TEST_RESULT").unwrap() == "yes" {
            j.retain_result(&r.invocation, r.attempt, &reply(&r), 102)
                .unwrap();
        }
        println!("PAYLOAD_COMMITTED");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }
    #[test]
    fn sigkill_before_and_after_result_commit_recovers_same_attempt_without_redispatch() {
        for has_result in [false, true] {
            let dir = private_dir();
            let path = dir.path().join("journal");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "attempts::payloads::tests::owned_payload_crash_child",
                    "--nocapture",
                ])
                .env("MAYHEM_PROXY_OWNED_TEST_PATH", &path)
                .env(
                    "MAYHEM_PROXY_OWNED_TEST_RESULT",
                    if has_result { "yes" } else { "no" },
                )
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut reader = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line.contains("PAYLOAD_COMMITTED") {
                    break;
                }
            }
            child.kill().unwrap();
            child.wait().unwrap();
            let j = Journal::open(&path, identity(), limits()).unwrap();
            let r = j.get(&d(100)).unwrap().unwrap();
            let recovered = j.recover(&r.invocation, r.attempt).unwrap();
            assert_eq!(recovered.record.phase, Phase::Dispatched);
            assert_eq!(recovered.request.unwrap().body, BODY);
            assert_eq!(recovered.result.is_some(), has_result);
            assert!(j.begin_dispatch(&r.invocation, r.generation, 103).is_err());
        }
    }
}
