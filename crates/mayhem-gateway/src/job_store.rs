use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const JOB_SCHEMA_VERSION: u32 = 1;
const JOB_FILE_MAGIC: &[u8; 8] = b"MYHMJOB1";
const JOB_NONCE_BYTES: usize = 12;
const JOB_CONTEXT_LENGTH_BYTES: usize = 2;
const JOB_MAX_KEY_CONTEXT_BYTES: usize = 512;
const JOB_FILE_SUFFIX: &str = "mjob";
const JOB_KEY_DOMAIN: &[u8] = b"mayhem-gateway-job-vault-key-v1";
const JOB_RECORD_KEY_DOMAIN: &[u8] = b"mayhem-gateway-job-record-key-v1";
const JOB_ID_DOMAIN: &[u8] = b"mayhem-gateway-job-id-v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GatewayJobStatus {
    ReconciliationPending,
    Completed,
    Cancelled,
    Failed,
}

impl GatewayJobStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReconciliationPending => "reconciliation_pending",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct GatewayJobArtifact {
    pub(crate) id: String,
    pub(crate) content_type: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) blake3: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewayJobArtifactSummary {
    pub(crate) id: String,
    pub(crate) content_type: String,
    pub(crate) bytes: usize,
    pub(crate) blake3: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct GatewayJobErrorInfo {
    pub(crate) code: String,
    pub(crate) category: String,
    pub(crate) retryable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredGatewayJob {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) endpoint_family: String,
    pub(crate) model: String,
    pub(crate) owner_token_id: Option<String>,
    pub(crate) request_fingerprint: String,
    pub(crate) status: GatewayJobStatus,
    pub(crate) created_at: u64,
    pub(crate) finished_at: u64,
    pub(crate) expires_at: u64,
    pub(crate) result: Option<Value>,
    pub(crate) artifacts: Vec<GatewayJobArtifact>,
    pub(crate) receipt: Option<Value>,
    pub(crate) error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error_info: Option<GatewayJobErrorInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredGatewayJobSummary {
    pub(crate) id: String,
    pub(crate) endpoint_family: String,
    pub(crate) model: String,
    pub(crate) status: GatewayJobStatus,
    pub(crate) created_at: u64,
    pub(crate) finished_at: u64,
    pub(crate) expires_at: u64,
    pub(crate) result_metadata: Option<Value>,
    pub(crate) artifacts: Vec<GatewayJobArtifactSummary>,
    pub(crate) receipt: Option<Value>,
    pub(crate) error: Option<String>,
    pub(crate) error_info: Option<GatewayJobErrorInfo>,
}

impl From<&StoredGatewayJob> for StoredGatewayJobSummary {
    fn from(job: &StoredGatewayJob) -> Self {
        Self {
            id: job.id.clone(),
            endpoint_family: job.endpoint_family.clone(),
            model: job.model.clone(),
            status: job.status,
            created_at: job.created_at,
            finished_at: job.finished_at,
            expires_at: job.expires_at,
            result_metadata: summarize_result_metadata(job.result.as_ref()),
            artifacts: job
                .artifacts
                .iter()
                .map(|artifact| GatewayJobArtifactSummary {
                    id: artifact.id.clone(),
                    content_type: artifact.content_type.clone(),
                    bytes: artifact.bytes.len(),
                    blake3: artifact.blake3.clone(),
                })
                .collect(),
            receipt: job.receipt.clone(),
            error: job.error.clone(),
            error_info: job.error_info.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ActiveGatewayJob {
    id: String,
    endpoint_family: String,
    model: String,
    owner_token_id: Option<String>,
    request_fingerprint: String,
    created_at: u64,
    recovery: Option<StoredGatewayJob>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BeginGatewayJob {
    Started,
    InProgress,
    Existing(StoredGatewayJob),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GatewayJobLookup {
    InProgress {
        id: String,
        endpoint_family: String,
        model: String,
        owner_token_id: Option<String>,
        created_at: u64,
        receipt: Option<Value>,
    },
    Terminal(StoredGatewayJob),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GatewayJobListEntry {
    InProgress {
        id: String,
        endpoint_family: String,
        model: String,
        created_at: u64,
    },
    Terminal(StoredGatewayJobSummary),
}

#[derive(Debug)]
pub(crate) struct GatewayJobStore {
    key: [u8; 32],
    directory: Option<PathBuf>,
    records: BTreeMap<String, StoredGatewayJob>,
    sealed_sizes: BTreeMap<String, usize>,
    active: BTreeMap<String, ActiveGatewayJob>,
    reconciling: BTreeSet<String>,
    total_bytes: usize,
    max_jobs: usize,
    max_bytes: usize,
    ttl_seconds: u64,
}

impl GatewayJobStore {
    pub(crate) fn in_memory(
        wallet_seed: [u8; 32],
        max_jobs: usize,
        max_bytes: usize,
        ttl_seconds: u64,
    ) -> Self {
        Self {
            key: derive_job_store_key(wallet_seed),
            directory: None,
            records: BTreeMap::new(),
            sealed_sizes: BTreeMap::new(),
            active: BTreeMap::new(),
            reconciling: BTreeSet::new(),
            total_bytes: 0,
            max_jobs: max_jobs.max(1),
            max_bytes: max_bytes.max(1),
            ttl_seconds: ttl_seconds.max(1),
        }
    }

    pub(crate) fn durable(
        wallet_seed: [u8; 32],
        directory: PathBuf,
        max_jobs: usize,
        max_bytes: usize,
        ttl_seconds: u64,
        now: u64,
    ) -> Result<Self, String> {
        create_private_directory(&directory)?;
        let mut store = Self {
            directory: Some(directory),
            ..Self::in_memory(wallet_seed, max_jobs, max_bytes, ttl_seconds)
        };
        store.load(now)?;
        Ok(store)
    }

    pub(crate) fn begin(
        &mut self,
        id: String,
        endpoint_family: String,
        model: String,
        owner_token_id: Option<String>,
        request_fingerprint: String,
        now: u64,
    ) -> Result<BeginGatewayJob, String> {
        self.purge_expired(now)?;
        if let Some(existing) = self.records.get(&id) {
            validate_job_identity(
                existing,
                &endpoint_family,
                &model,
                owner_token_id.as_deref(),
                &request_fingerprint,
            )?;
            return Ok(BeginGatewayJob::Existing(existing.clone()));
        }
        if let Some(active) = self.active.get(&id) {
            validate_active_job_identity(
                active,
                &endpoint_family,
                &model,
                owner_token_id.as_deref(),
                &request_fingerprint,
            )?;
            return Ok(BeginGatewayJob::InProgress);
        }
        self.make_room_for_job()?;
        self.active.insert(
            id.clone(),
            ActiveGatewayJob {
                id,
                endpoint_family,
                model,
                owner_token_id,
                request_fingerprint,
                created_at: now,
                recovery: None,
            },
        );
        Ok(BeginGatewayJob::Started)
    }

    // Persist a conservative terminal fallback before inference starts. While this
    // process owns the job it remains active; reopening the vault never dispatches it.
    pub(crate) fn protect_active(&mut self, id: &str, now: u64) -> Result<(), String> {
        let active = self
            .active
            .get(id)
            .ok_or_else(|| format!("job {id} is not active"))?;
        if active.recovery.is_some() {
            return Ok(());
        }
        let recovery = StoredGatewayJob {
            schema_version: JOB_SCHEMA_VERSION,
            id: active.id.clone(),
            endpoint_family: active.endpoint_family.clone(),
            model: active.model.clone(),
            owner_token_id: active.owner_token_id.clone(),
            request_fingerprint: active.request_fingerprint.clone(),
            status: GatewayJobStatus::Failed,
            created_at: active.created_at,
            finished_at: now,
            expires_at: now.saturating_add(self.ttl_seconds),
            result: None,
            artifacts: Vec::new(),
            receipt: None,
            error: Some("gateway execution interrupted; billing outcome is unknown without a signed receipt; do not redispatch this request".to_owned()),
            error_info: Some(GatewayJobErrorInfo {
                code: "gateway_execution_interrupted".to_owned(),
                category: "execution_unknown".to_owned(),
                retryable: false,
            }),
        };
        self.persist_active_recovery(recovery)
    }

    pub(crate) fn active_recovery(&self, id: &str) -> Option<&StoredGatewayJob> {
        self.active.get(id)?.recovery.as_ref()
    }

    pub(crate) fn update_active_receipt(
        &mut self,
        id: &str,
        receipt: Value,
        now: u64,
    ) -> Result<(), String> {
        let Some(mut recovery) = self.active_recovery(id).cloned() else {
            return Ok(());
        };
        recovery.status = GatewayJobStatus::ReconciliationPending;
        recovery.receipt = Some(receipt);
        recovery.finished_at = now;
        recovery.error = Some(
            "stream interrupted before terminal receipt; checkpoint reconciliation is pending"
                .to_owned(),
        );
        recovery.error_info = None;
        self.persist_active_recovery(recovery)
    }

    fn persist_active_recovery(&mut self, recovery: StoredGatewayJob) -> Result<(), String> {
        let id = recovery.id.clone();
        let sealed = seal_job(&self.key, &recovery)?;
        self.make_room_for_bytes(sealed.len(), Some(&id))?;
        self.persist(&id, &sealed, self.sealed_sizes.contains_key(&id))?;
        let previous = self
            .sealed_sizes
            .insert(id.clone(), sealed.len())
            .unwrap_or(0);
        self.total_bytes = self
            .total_bytes
            .saturating_sub(previous)
            .saturating_add(sealed.len());
        self.active
            .get_mut(&id)
            .expect("active recovery owner exists")
            .recovery = Some(recovery);
        Ok(())
    }

    fn make_room_for_job(&mut self) -> Result<(), String> {
        while self
            .records_counted_for_job_limit()
            .saturating_add(self.active.len())
            >= self.max_jobs
        {
            let Some(evict) = self
                .records
                .values()
                .filter(|job| !self.reconciling.contains(&job.id))
                .min_by_key(|job| (job.finished_at, job.id.clone()))
                .map(|job| job.id.clone())
            else {
                return Err(format!(
                    "gateway job vault has {} active job(s) at its configured live-job limit of {}; {} receipt reconciliation job(s) are retained separately",
                    self.active.len(),
                    self.max_jobs,
                    self.reconciling.len()
                ));
            };
            self.records.remove(&evict);
            self.remove_sealed_record(&evict)?;
        }
        Ok(())
    }

    pub(crate) fn complete(
        &mut self,
        id: &str,
        status: GatewayJobStatus,
        result: Option<Value>,
        artifacts: Vec<GatewayJobArtifact>,
        receipt: Option<Value>,
        error: Option<String>,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        self.complete_with_error_info(id, status, result, artifacts, receipt, error, None, now)
    }

    pub(crate) fn complete_with_error_info(
        &mut self,
        id: &str,
        status: GatewayJobStatus,
        result: Option<Value>,
        artifacts: Vec<GatewayJobArtifact>,
        receipt: Option<Value>,
        error: Option<String>,
        error_info: Option<GatewayJobErrorInfo>,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        if let Some(existing) = self.records.get(id) {
            if existing.status == status
                && existing.result == result
                && existing.artifacts == artifacts
                && existing.receipt == receipt
                && existing.error == error
                && existing.error_info == error_info
            {
                let existing = existing.clone();
                if status == GatewayJobStatus::ReconciliationPending {
                    self.reconciling.insert(id.to_owned());
                }
                return Ok(existing);
            }
            return Err(format!("job {id} already has a different terminal result"));
        }
        let active = self
            .active
            .get(id)
            .cloned()
            .ok_or_else(|| format!("job {id} is not active"))?;
        let mut job = StoredGatewayJob {
            schema_version: JOB_SCHEMA_VERSION,
            id: active.id,
            endpoint_family: active.endpoint_family,
            model: active.model,
            owner_token_id: active.owner_token_id,
            request_fingerprint: active.request_fingerprint,
            status,
            created_at: active.created_at,
            finished_at: now,
            expires_at: now.saturating_add(self.ttl_seconds),
            result,
            artifacts,
            receipt,
            error,
            error_info,
        };
        if matches!(
            status,
            GatewayJobStatus::Failed | GatewayJobStatus::Cancelled
        ) {
            if let Some(recovery) = active.recovery.filter(|job| job.receipt.is_some()) {
                job.status = recovery.status;
                job.receipt = recovery.receipt;
            }
        }
        let sealed = seal_job(&self.key, &job)?;
        if sealed.len() > self.max_bytes {
            return Err(format!(
                "job {} needs {} encrypted bytes, above the {}-byte job-vault limit",
                job.id,
                sealed.len(),
                self.max_bytes
            ));
        }
        self.make_room_for_bytes(sealed.len(), Some(id))?;
        self.persist(&job.id, &sealed, self.sealed_sizes.contains_key(id))?;
        self.active.remove(id);
        let previous = self
            .sealed_sizes
            .insert(job.id.clone(), sealed.len())
            .unwrap_or(0);
        self.total_bytes = self
            .total_bytes
            .saturating_sub(previous)
            .saturating_add(sealed.len());
        self.records.insert(job.id.clone(), job.clone());
        if job.status == GatewayJobStatus::ReconciliationPending {
            self.reconciling.insert(job.id.clone());
        }
        self.enforce_bounds(Some(&job.id))?;
        Ok(job)
    }

    pub(crate) fn finish_reconciliation(
        &mut self,
        id: &str,
        status: GatewayJobStatus,
        error: Option<String>,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        if !matches!(
            status,
            GatewayJobStatus::Completed | GatewayJobStatus::Cancelled
        ) {
            return Err(format!(
                "job {id} reconciliation cannot finish as {}",
                status.as_str()
            ));
        }
        let existing = self
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| format!("job {id} has no durable reconciliation state"))?;
        if existing.status == status && existing.error == error {
            self.reconciling.remove(id);
            return Ok(existing);
        }
        if existing.status != GatewayJobStatus::ReconciliationPending {
            return Err(format!(
                "job {id} cannot finish reconciliation from {}",
                existing.status.as_str()
            ));
        }
        let mut job = existing;
        job.status = status;
        job.finished_at = now;
        job.expires_at = now.saturating_add(self.ttl_seconds);
        job.error = error;
        let sealed = seal_job(&self.key, &job)?;
        if sealed.len() > self.max_bytes {
            return Err(format!(
                "job {} needs {} encrypted bytes, above the {}-byte job-vault limit",
                job.id,
                sealed.len(),
                self.max_bytes
            ));
        }
        self.make_room_for_bytes(sealed.len(), Some(id))?;
        self.persist_replace(&job.id, &sealed)?;
        let previous_size = self
            .sealed_sizes
            .insert(job.id.clone(), sealed.len())
            .unwrap_or(0);
        self.total_bytes = self
            .total_bytes
            .saturating_sub(previous_size)
            .saturating_add(sealed.len());
        self.reconciling.remove(id);
        self.records.insert(job.id.clone(), job.clone());
        self.enforce_bounds(Some(&job.id))?;
        Ok(job)
    }

    pub(crate) fn update_reconciliation_receipt(
        &mut self,
        id: &str,
        receipt: Value,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let existing = self
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| format!("job {id} has no durable reconciliation state"))?;
        if existing.status != GatewayJobStatus::ReconciliationPending {
            return Err(format!(
                "job {id} cannot update reconciliation from {}",
                existing.status.as_str()
            ));
        }
        if existing.receipt.as_ref() == Some(&receipt) {
            return Ok(existing);
        }
        let mut job = existing;
        job.receipt = Some(receipt);
        job.finished_at = now;
        let sealed = seal_job(&self.key, &job)?;
        if sealed.len() > self.max_bytes {
            return Err(format!(
                "job {} needs {} encrypted bytes, above the {}-byte job-vault limit",
                job.id,
                sealed.len(),
                self.max_bytes
            ));
        }
        self.make_room_for_bytes(sealed.len(), Some(id))?;
        self.persist_replace(&job.id, &sealed)?;
        let previous_size = self
            .sealed_sizes
            .insert(job.id.clone(), sealed.len())
            .unwrap_or(0);
        self.total_bytes = self
            .total_bytes
            .saturating_sub(previous_size)
            .saturating_add(sealed.len());
        self.records.insert(job.id.clone(), job.clone());
        self.enforce_bounds(Some(&job.id))?;
        Ok(job)
    }

    pub(crate) fn get(&mut self, id: &str, now: u64) -> Result<Option<StoredGatewayJob>, String> {
        self.purge_expired(now)?;
        Ok(self.records.get(id).cloned())
    }

    pub(crate) fn lookup(
        &mut self,
        id: &str,
        now: u64,
    ) -> Result<Option<GatewayJobLookup>, String> {
        self.purge_expired(now)?;
        Ok(self.lookup_read_only(id, now))
    }

    pub(crate) fn lookup_read_only(&self, id: &str, now: u64) -> Option<GatewayJobLookup> {
        if let Some(job) = self.records.get(id) {
            return (job.status == GatewayJobStatus::ReconciliationPending || job.expires_at > now)
                .then(|| GatewayJobLookup::Terminal(job.clone()));
        }
        self.active.get(id).map(|job| GatewayJobLookup::InProgress {
            id: job.id.clone(),
            endpoint_family: job.endpoint_family.clone(),
            model: job.model.clone(),
            owner_token_id: job.owner_token_id.clone(),
            created_at: job.created_at,
            receipt: job.recovery.as_ref().and_then(|job| job.receipt.clone()),
        })
    }

    pub(crate) fn pending_reconciliations(
        &mut self,
        now: u64,
    ) -> Result<Vec<StoredGatewayJob>, String> {
        self.purge_expired(now)?;
        Ok(self
            .records
            .values()
            .filter(|job| job.status == GatewayJobStatus::ReconciliationPending)
            .cloned()
            .collect())
    }

    pub(crate) fn is_active(&self, id: &str) -> bool {
        self.active.contains_key(id)
    }

    pub(crate) fn list_summaries_for_owner(
        &mut self,
        owner_token_id: Option<&str>,
        now: u64,
    ) -> Result<Vec<StoredGatewayJobSummary>, String> {
        self.purge_expired(now)?;
        Ok(self
            .records
            .values()
            .filter(|job| job.owner_token_id.as_deref() == owner_token_id)
            .map(StoredGatewayJobSummary::from)
            .collect())
    }

    pub(crate) fn list_entries_for_owner(
        &mut self,
        owner_token_id: Option<&str>,
        now: u64,
    ) -> Result<Vec<GatewayJobListEntry>, String> {
        self.purge_expired(now)?;
        let mut entries = self
            .records
            .values()
            .filter(|job| job.owner_token_id.as_deref() == owner_token_id)
            .map(StoredGatewayJobSummary::from)
            .map(GatewayJobListEntry::Terminal)
            .collect::<Vec<_>>();
        entries.extend(
            self.active
                .values()
                .filter(|job| job.owner_token_id.as_deref() == owner_token_id)
                .map(|job| GatewayJobListEntry::InProgress {
                    id: job.id.clone(),
                    endpoint_family: job.endpoint_family.clone(),
                    model: job.model.clone(),
                    created_at: job.created_at,
                }),
        );
        Ok(entries)
    }

    pub(crate) fn remove(
        &mut self,
        id: &str,
        owner_token_id: Option<&str>,
        now: u64,
    ) -> Result<Option<StoredGatewayJob>, String> {
        self.purge_expired(now)?;
        let Some(job) = self.records.get(id) else {
            return Ok(None);
        };
        if job.owner_token_id.as_deref() != owner_token_id {
            return Ok(None);
        }
        if self.reconciling.contains(id) {
            return Err(format!(
                "job {id} cannot be removed while receipt reconciliation is pending"
            ));
        }
        let job = self.records.remove(id).expect("checked job exists");
        self.remove_sealed_record(id)?;
        Ok(Some(job))
    }

    fn load(&mut self, now: u64) -> Result<(), String> {
        let Some(directory) = self.directory.as_ref() else {
            return Ok(());
        };
        let entries = fs::read_dir(directory)
            .map_err(|err| format!("reading gateway job vault {}: {err}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|err| format!("reading gateway job-vault entry: {err}"))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some(JOB_FILE_SUFFIX) {
                continue;
            }
            let sealed = fs::read(&path)
                .map_err(|err| format!("reading encrypted job {}: {err}", path.display()))?;
            let id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    format!(
                        "job vault contains a non-UTF-8 filename: {}",
                        path.display()
                    )
                })?;
            validate_job_id(id)?;
            let job = open_job(&self.key, id, &sealed)?;
            if job.schema_version != JOB_SCHEMA_VERSION || job.id != id {
                return Err(format!("encrypted job {id} has invalid identity or schema"));
            }
            if job.status == GatewayJobStatus::ReconciliationPending {
                self.reconciling.insert(id.to_owned());
            }
            self.total_bytes = self.total_bytes.saturating_add(sealed.len());
            self.sealed_sizes.insert(id.to_owned(), sealed.len());
            self.records.insert(id.to_owned(), job);
        }
        self.purge_expired(now)?;
        self.enforce_bounds(None)
    }

    fn purge_expired(&mut self, now: u64) -> Result<(), String> {
        let expired = self
            .records
            .values()
            .filter(|job| {
                job.status != GatewayJobStatus::ReconciliationPending && job.expires_at <= now
            })
            .map(|job| job.id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            self.records.remove(&id);
            self.reconciling.remove(&id);
            self.remove_sealed_record(&id)?;
        }
        Ok(())
    }

    fn enforce_bounds(&mut self, preserve: Option<&str>) -> Result<(), String> {
        while self.records_counted_for_job_limit() > self.max_jobs
            || self.total_bytes > self.max_bytes
        {
            let Some(evict) = self
                .records
                .values()
                .filter(|job| Some(job.id.as_str()) != preserve)
                .filter(|job| !self.reconciling.contains(&job.id))
                .min_by_key(|job| (job.finished_at, job.id.clone()))
                .map(|job| job.id.clone())
            else {
                return Err("gateway job vault cannot satisfy its configured bounds".to_owned());
            };
            self.records.remove(&evict);
            self.remove_sealed_record(&evict)?;
        }
        Ok(())
    }

    fn records_counted_for_job_limit(&self) -> usize {
        self.records
            .values()
            .filter(|job| !self.reconciling.contains(&job.id))
            .count()
    }

    fn make_room_for_bytes(
        &mut self,
        incoming_bytes: usize,
        replacing: Option<&str>,
    ) -> Result<(), String> {
        let replaced_bytes = replacing
            .and_then(|id| self.sealed_sizes.get(id))
            .copied()
            .unwrap_or(0);
        while self
            .total_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(incoming_bytes)
            > self.max_bytes
        {
            let Some(evict) = self
                .records
                .values()
                .filter(|job| Some(job.id.as_str()) != replacing)
                .filter(|job| !self.reconciling.contains(&job.id))
                .min_by_key(|job| (job.finished_at, job.id.clone()))
                .map(|job| job.id.clone())
            else {
                return Err(format!(
                    "gateway job vault cannot fit {incoming_bytes} encrypted bytes without evicting an active or reconciliation-pending job"
                ));
            };
            self.records.remove(&evict);
            self.remove_sealed_record(&evict)?;
        }
        Ok(())
    }

    fn persist_replace(&self, id: &str, sealed: &[u8]) -> Result<(), String> {
        self.persist(id, sealed, true)
    }

    fn persist(&self, id: &str, sealed: &[u8], replace: bool) -> Result<(), String> {
        let Some(directory) = self.directory.as_ref() else {
            return Ok(());
        };
        validate_job_id(id)?;
        let destination = job_path(directory, id);
        if !replace && destination.exists() {
            return Err(format!("encrypted job file already exists for {id}"));
        }
        if replace && !destination.exists() {
            return Err(format!(
                "encrypted reconciliation-pending job file is missing for {id}"
            ));
        }
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random)
            .map_err(|err| format!("generating atomic job filename entropy: {err}"))?;
        let temporary = directory.join(format!(".{id}.{}.tmp", hex::encode(random)));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|err| format!("creating encrypted job {}: {err}", temporary.display()))?;
        let write_result = (|| {
            file.write_all(sealed)
                .map_err(|err| format!("writing encrypted job {}: {err}", temporary.display()))?;
            file.sync_all()
                .map_err(|err| format!("syncing encrypted job {}: {err}", temporary.display()))?;
            fs::rename(&temporary, &destination).map_err(|err| {
                format!(
                    "publishing encrypted job {} as {}: {err}",
                    temporary.display(),
                    destination.display()
                )
            })?;
            sync_directory(directory)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result
    }

    fn remove_sealed_record(&mut self, id: &str) -> Result<(), String> {
        if let Some(size) = self.sealed_sizes.remove(id) {
            self.total_bytes = self.total_bytes.saturating_sub(size);
        }
        let Some(directory) = self.directory.as_ref() else {
            return Ok(());
        };
        let path = job_path(directory, id);
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(directory),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("removing encrypted job {}: {err}", path.display())),
        }
    }
}

fn summarize_result_metadata(result: Option<&Value>) -> Option<Value> {
    let result = result?.as_object()?;
    let mut metadata = serde_json::Map::new();
    for key in ["prompt", "size", "seconds", "usage"] {
        if let Some(value) = result.get(key) {
            metadata.insert(key.to_owned(), value.clone());
        }
    }
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(crate) fn gateway_job_id(
    wallet_seed: [u8; 32],
    owner_token_id: Option<&str>,
    endpoint_family: &str,
    idempotency_key: Option<&str>,
) -> Result<String, String> {
    let mut hasher = blake3::Hasher::new_keyed(&derive_job_store_key(wallet_seed));
    hasher.update(JOB_ID_DOMAIN);
    match idempotency_key {
        Some(idempotency_key) => {
            validate_idempotency_key(idempotency_key)?;
            hasher.update(b"idempotent\0");
            hasher.update(owner_token_id.unwrap_or("local").as_bytes());
            hasher.update(b"\0");
            hasher.update(endpoint_family.as_bytes());
            hasher.update(b"\0");
            hasher.update(idempotency_key.as_bytes());
        }
        None => {
            let mut random = [0_u8; 32];
            getrandom::fill(&mut random)
                .map_err(|err| format!("generating gateway job id: {err}"))?;
            hasher.update(b"random\0");
            hasher.update(&random);
        }
    }
    Ok(format!("job_{}", hasher.finalize().to_hex()))
}

pub(crate) fn validate_idempotency_key(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        return Err("Idempotency-Key must contain 1-255 printable ASCII bytes".to_owned());
    }
    Ok(())
}

fn validate_job_identity(
    job: &StoredGatewayJob,
    endpoint_family: &str,
    model: &str,
    owner_token_id: Option<&str>,
    request_fingerprint: &str,
) -> Result<(), String> {
    if job.endpoint_family != endpoint_family
        || job.model != model
        || job.owner_token_id.as_deref() != owner_token_id
        || job.request_fingerprint != request_fingerprint
    {
        return Err(format!(
            "Idempotency-Key for job {} was already used with a different request",
            job.id
        ));
    }
    Ok(())
}

fn validate_active_job_identity(
    job: &ActiveGatewayJob,
    endpoint_family: &str,
    model: &str,
    owner_token_id: Option<&str>,
    request_fingerprint: &str,
) -> Result<(), String> {
    if job.endpoint_family != endpoint_family
        || job.model != model
        || job.owner_token_id.as_deref() != owner_token_id
        || job.request_fingerprint != request_fingerprint
    {
        return Err(format!(
            "Idempotency-Key for job {} is already running a different request",
            job.id
        ));
    }
    Ok(())
}

fn derive_job_store_key(wallet_seed: [u8; 32]) -> [u8; 32] {
    *blake3::keyed_hash(&wallet_seed, JOB_KEY_DOMAIN).as_bytes()
}

fn job_key_context(job: &StoredGatewayJob) -> String {
    let session_id = job
        .receipt
        .as_ref()
        .and_then(|receipt| {
            receipt
                .pointer("/body/session_id")
                .or_else(|| receipt.get("session_id"))
        })
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty());
    match session_id {
        Some(session_id) => format!("session:{session_id}"),
        None => format!("job:{}", job.id),
    }
}

fn derive_job_record_key(master_key: &[u8; 32], id: &str, context: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(master_key);
    hasher.update(JOB_RECORD_KEY_DOMAIN);
    hasher.update(b"\0");
    hasher.update(id.as_bytes());
    hasher.update(b"\0");
    hasher.update(context.as_bytes());
    *hasher.finalize().as_bytes()
}

fn validate_key_context(context: &str) -> Result<(), String> {
    if context.is_empty()
        || context.len() > JOB_MAX_KEY_CONTEXT_BYTES
        || !context
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_graphic())
    {
        return Err("gateway job encryption context is invalid".to_owned());
    }
    Ok(())
}

fn seal_job(master_key: &[u8; 32], job: &StoredGatewayJob) -> Result<Vec<u8>, String> {
    let plaintext = serde_json::to_vec(job)
        .map_err(|err| format!("serializing gateway job {}: {err}", job.id))?;
    let context = job_key_context(job);
    validate_key_context(&context)?;
    let context_length = u16::try_from(context.len())
        .map_err(|_| "gateway job encryption context is too long".to_owned())?;
    let mut nonce = [0_u8; JOB_NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|err| format!("generating gateway job nonce: {err}"))?;
    let record_key = derive_job_record_key(master_key, &job.id, &context);
    let cipher = Aes256Gcm::new_from_slice(&record_key)
        .map_err(|err| format!("initializing gateway job cipher: {err}"))?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: job_aad(&job.id, &context).as_bytes(),
            },
        )
        .map_err(|_| format!("encrypting gateway job {} failed", job.id))?;
    let mut sealed = Vec::with_capacity(
        JOB_FILE_MAGIC.len()
            + JOB_CONTEXT_LENGTH_BYTES
            + context.len()
            + nonce.len()
            + ciphertext.len(),
    );
    sealed.extend_from_slice(JOB_FILE_MAGIC);
    sealed.extend_from_slice(&context_length.to_be_bytes());
    sealed.extend_from_slice(context.as_bytes());
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&ciphertext);
    Ok(sealed)
}

fn open_job(master_key: &[u8; 32], id: &str, sealed: &[u8]) -> Result<StoredGatewayJob, String> {
    let minimum = JOB_FILE_MAGIC.len() + JOB_CONTEXT_LENGTH_BYTES + JOB_NONCE_BYTES + 1;
    if sealed.len() < minimum || &sealed[..JOB_FILE_MAGIC.len()] != JOB_FILE_MAGIC {
        return Err(format!("encrypted job {id} has an invalid header"));
    }
    let length_start = JOB_FILE_MAGIC.len();
    let length_end = length_start + JOB_CONTEXT_LENGTH_BYTES;
    let context_length = usize::from(u16::from_be_bytes(
        sealed[length_start..length_end]
            .try_into()
            .map_err(|_| format!("encrypted job {id} has an invalid context length"))?,
    ));
    let context_end = length_end.saturating_add(context_length);
    let nonce_start = context_end;
    let nonce_end = nonce_start + JOB_NONCE_BYTES;
    if context_length == 0
        || context_length > JOB_MAX_KEY_CONTEXT_BYTES
        || nonce_end >= sealed.len()
    {
        return Err(format!("encrypted job {id} has an invalid context"));
    }
    let context = std::str::from_utf8(&sealed[length_end..context_end])
        .map_err(|_| format!("encrypted job {id} has a non-UTF-8 context"))?;
    validate_key_context(context)?;
    let record_key = derive_job_record_key(master_key, id, context);
    let cipher = Aes256Gcm::new_from_slice(&record_key)
        .map_err(|err| format!("initializing gateway job cipher: {err}"))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&sealed[nonce_start..nonce_end]),
            Payload {
                msg: &sealed[nonce_end..],
                aad: job_aad(id, context).as_bytes(),
            },
        )
        .map_err(|_| format!("authenticating encrypted gateway job {id} failed"))?;
    serde_json::from_slice(&plaintext).map_err(|err| format!("decoding gateway job {id}: {err}"))
}

fn job_aad(id: &str, context: &str) -> String {
    format!("mayhem-gateway-job-vault-v1\0{id}\0{context}")
}

fn validate_job_id(id: &str) -> Result<(), String> {
    if id.len() < 5
        || id.len() > 128
        || !id
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
    {
        return Err("gateway job id is invalid".to_owned());
    }
    Ok(())
}

fn job_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.{JOB_FILE_SUFFIX}"))
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|err| format!("creating gateway job vault {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|err| format!("securing gateway job vault {}: {err}", path.display()))?;
    }
    Ok(())
}

fn sync_directory(directory: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|err| format!("syncing gateway job vault {}: {err}", directory.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_stream_active_checkpoint_is_pinned_and_replacement_bytes_are_exact() {
        let root = tempfile::tempdir().unwrap();
        let seed = [31; 32];
        let mut store =
            GatewayJobStore::durable(seed, root.path().to_owned(), 1, 64 * 1024, 1, 10).unwrap();
        let id = gateway_job_id(
            seed,
            Some("owner"),
            "openai_chat_completions",
            Some("stream"),
        )
        .unwrap();
        store
            .begin(
                id.clone(),
                "openai_chat_completions".to_owned(),
                "model".to_owned(),
                Some("owner".to_owned()),
                "fingerprint".to_owned(),
                10,
            )
            .unwrap();
        store.protect_active(&id, 10).unwrap();
        assert_eq!(
            store.total_bytes,
            fs::metadata(job_path(root.path(), &id)).unwrap().len() as usize
        );
        assert_eq!(
            store.total_bytes,
            store.sealed_sizes.values().sum::<usize>()
        );
        let checkpoint = serde_json::json!({"body": {"session_id": "first-session", "final_receipt": false}, "receipt_ack": {"user_sig": "signed"}});
        for _ in 0..3 {
            store
                .update_active_receipt(&id, checkpoint.clone(), 11)
                .unwrap();
            assert_eq!(
                store.total_bytes,
                fs::metadata(job_path(root.path(), &id)).unwrap().len() as usize
            );
            assert_eq!(
                store.total_bytes,
                store.sealed_sizes.values().sum::<usize>()
            );
        }
        assert!(store
            .begin(
                "job_pressure".to_owned(),
                "chat".to_owned(),
                "model".to_owned(),
                None,
                "other".to_owned(),
                100
            )
            .is_err());
        assert!(matches!(
            store.lookup(&id, 100).unwrap(),
            Some(GatewayJobLookup::InProgress { .. })
        ));
        let before = fs::read(job_path(root.path(), &id)).unwrap();
        store.max_bytes = store.total_bytes;
        assert!(store
            .update_active_receipt(
                &id,
                serde_json::json!({"large": "x".repeat(64 * 1024)}),
                100
            )
            .is_err());
        assert_eq!(fs::read(job_path(root.path(), &id)).unwrap(), before);
        drop(store);
        let mut reopened =
            GatewayJobStore::durable(seed, root.path().to_owned(), 1, 64 * 1024, 1, 100).unwrap();
        let pending = reopened.get(&id, 100).unwrap().unwrap();
        assert_eq!(pending.status, GatewayJobStatus::ReconciliationPending);
        assert_eq!(pending.receipt, Some(checkpoint));
        assert_eq!(
            reopened.total_bytes,
            reopened.sealed_sizes.values().sum::<usize>()
        );
        reopened
            .begin(
                "job_pressure".to_owned(),
                "chat".to_owned(),
                "model".to_owned(),
                None,
                "other".to_owned(),
                100,
            )
            .unwrap();
        finish(&mut reopened, "job_pressure", 100);
        assert!(reopened.get(&id, 1000).unwrap().is_some());
        assert_eq!(
            reopened.total_bytes,
            reopened.sealed_sizes.values().sum::<usize>()
        );
        reopened
            .finish_reconciliation(&id, GatewayJobStatus::Cancelled, None, 1000)
            .unwrap();
        assert_eq!(
            reopened.total_bytes,
            fs::metadata(job_path(root.path(), &id)).unwrap().len() as usize
        );
    }

    #[test]
    fn durable_stream_reservation_restarts_without_rerun_and_read_only_lookup_does_not_purge() {
        let root = tempfile::tempdir().unwrap();
        let mut store =
            GatewayJobStore::durable([32; 32], root.path().to_owned(), 1, 64 * 1024, 10, 1)
                .unwrap();
        store
            .begin(
                "job_stream".to_owned(),
                "chat".to_owned(),
                "model".to_owned(),
                Some("owner".to_owned()),
                "hash".to_owned(),
                1,
            )
            .unwrap();
        store.protect_active("job_stream", 1).unwrap();
        drop(store);
        let mut reopened =
            GatewayJobStore::durable([32; 32], root.path().to_owned(), 1, 64 * 1024, 10, 2)
                .unwrap();
        assert!(matches!(
            reopened
                .begin(
                    "job_stream".to_owned(),
                    "chat".to_owned(),
                    "model".to_owned(),
                    Some("owner".to_owned()),
                    "hash".to_owned(),
                    2
                )
                .unwrap(),
            BeginGatewayJob::Existing(_)
        ));
        assert!(reopened
            .begin(
                "job_stream".to_owned(),
                "chat".to_owned(),
                "model".to_owned(),
                Some("other".to_owned()),
                "hash".to_owned(),
                2
            )
            .is_err());
        let bytes = reopened.total_bytes;
        assert!(reopened.lookup_read_only("job_stream", 12).is_none());
        assert_eq!(reopened.records.len(), 1);
        assert_eq!(reopened.total_bytes, bytes);
        assert!(job_path(root.path(), "job_stream").exists());
    }

    fn finish(store: &mut GatewayJobStore, id: &str, now: u64) -> StoredGatewayJob {
        store
            .complete(
                id,
                GatewayJobStatus::Completed,
                Some(serde_json::json!({"ok": true, "secret": "output"})),
                vec![GatewayJobArtifact {
                    id: "artifact".to_owned(),
                    content_type: "image/png".to_owned(),
                    bytes: b"png-secret".to_vec(),
                    blake3: blake3::hash(b"png-secret").to_hex().to_string(),
                }],
                Some(serde_json::json!({
                    "rail": "tap",
                    "body": {"session_id": "session-encryption-context"}
                })),
                None,
                now,
            )
            .unwrap()
    }

    #[test]
    fn failed_job_error_info_is_optional_durable_and_part_of_terminal_identity() {
        let root = tempfile::tempdir().unwrap();
        let seed = [19_u8; 32];
        let mut store =
            GatewayJobStore::durable(seed, root.path().to_owned(), 8, 1024 * 1024, 60, 10).unwrap();
        let info = GatewayJobErrorInfo {
            code: "provider_model_output_invalid".to_owned(),
            category: "provider_response".to_owned(),
            retryable: false,
        };
        for id in ["legacy", "typed"] {
            store
                .begin(
                    id.to_owned(),
                    "chat".to_owned(),
                    "model".to_owned(),
                    None,
                    id.to_owned(),
                    10,
                )
                .unwrap();
        }
        let message = Some("The provider returned invalid output.".to_owned());
        let legacy = store
            .complete(
                "legacy",
                GatewayJobStatus::Failed,
                None,
                Vec::new(),
                None,
                message.clone(),
                11,
            )
            .unwrap();
        // With None omitted, this is the unchanged version-1 record shape.
        assert!(serde_json::to_value(&legacy)
            .unwrap()
            .get("error_info")
            .is_none());
        let typed = store
            .complete_with_error_info(
                "typed",
                GatewayJobStatus::Failed,
                None,
                Vec::new(),
                None,
                message.clone(),
                Some(info.clone()),
                11,
            )
            .unwrap();
        assert_eq!(
            store
                .complete_with_error_info(
                    "typed",
                    GatewayJobStatus::Failed,
                    None,
                    Vec::new(),
                    None,
                    message.clone(),
                    Some(info.clone()),
                    12
                )
                .unwrap(),
            typed
        );
        assert!(store
            .complete(
                "typed",
                GatewayJobStatus::Failed,
                None,
                Vec::new(),
                None,
                message.clone(),
                12
            )
            .is_err());
        let mut changed_info = info.clone();
        changed_info.retryable = true;
        assert!(store
            .complete_with_error_info(
                "typed",
                GatewayJobStatus::Failed,
                None,
                Vec::new(),
                None,
                message,
                Some(changed_info),
                12
            )
            .is_err());
        drop(store);
        let mut reopened =
            GatewayJobStore::durable(seed, root.path().to_owned(), 8, 1024 * 1024, 60, 12).unwrap();
        assert_eq!(reopened.get("legacy", 12).unwrap().unwrap(), legacy);
        assert_eq!(reopened.get("typed", 12).unwrap().unwrap(), typed);
        let summaries = reopened.list_summaries_for_owner(None, 12).unwrap();
        assert_eq!(
            summaries
                .iter()
                .find(|job| job.id == "typed")
                .unwrap()
                .error_info,
            Some(info)
        );
        assert!(summaries
            .iter()
            .find(|job| job.id == "legacy")
            .unwrap()
            .error_info
            .is_none());
    }

    #[test]
    fn durable_jobs_are_encrypted_restart_safe_owned_and_ttl_purged() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("jobs");
        let seed = [7_u8; 32];
        let id = gateway_job_id(seed, Some("buyer-a"), "image", Some("idem-a")).unwrap();
        let mut store =
            GatewayJobStore::durable(seed, directory.clone(), 1, 1024 * 1024, 60, 10).unwrap();
        assert_eq!(
            store
                .begin(
                    id.clone(),
                    "image".to_owned(),
                    "model".to_owned(),
                    Some("buyer-a".to_owned()),
                    "request-a".to_owned(),
                    10,
                )
                .unwrap(),
            BeginGatewayJob::Started
        );
        finish(&mut store, &id, 11);

        let bytes = fs::read(job_path(&directory, &id)).unwrap();
        assert!(bytes
            .windows(b"session:session-encryption-context".len())
            .any(|part| part == b"session:session-encryption-context"));
        assert!(!bytes.windows(b"output".len()).any(|part| part == b"output"));
        assert!(!bytes
            .windows(b"png-secret".len())
            .any(|part| part == b"png-secret"));

        let mut reopened =
            GatewayJobStore::durable(seed, directory.clone(), 8, 1024 * 1024, 60, 12).unwrap();
        let restored = reopened.get(&id, 12).unwrap().unwrap();
        assert_eq!(restored.result.unwrap()["secret"], "output");
        assert!(reopened
            .list_summaries_for_owner(Some("buyer-b"), 12)
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened
                .list_summaries_for_owner(Some("buyer-a"), 12)
                .unwrap()
                .len(),
            1
        );
        assert!(reopened.get(&id, 71).unwrap().is_none());
        assert!(!job_path(&directory, &id).exists());
    }

    #[test]
    fn reconciliation_pending_payload_survives_restart_and_promotes_in_place() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("jobs");
        let seed = [13_u8; 32];
        let id = gateway_job_id(seed, Some("buyer"), "video", Some("ack-retry")).unwrap();
        let artifact = GatewayJobArtifact {
            id: "artifact".to_owned(),
            content_type: "video/mp4".to_owned(),
            bytes: b"finished-video".to_vec(),
            blake3: blake3::hash(b"finished-video").to_hex().to_string(),
        };
        let result = serde_json::json!({"kind": "video_generation", "frames": 24});
        let receipt = serde_json::json!({
            "body": {"session_id": "session-ack-retry"},
            "receipt_ack": {"seq": 1}
        });
        let mut store =
            GatewayJobStore::durable(seed, directory.clone(), 8, 1024 * 1024, 60, 10).unwrap();
        store
            .begin(
                id.clone(),
                "video".to_owned(),
                "model".to_owned(),
                Some("buyer".to_owned()),
                "request".to_owned(),
                10,
            )
            .unwrap();
        store
            .complete(
                &id,
                GatewayJobStatus::ReconciliationPending,
                Some(result.clone()),
                vec![artifact.clone()],
                Some(receipt.clone()),
                Some("signed receipt acknowledgement is pending".to_owned()),
                11,
            )
            .unwrap();
        drop(store);

        let mut reopened =
            GatewayJobStore::durable(seed, directory.clone(), 1, 1024 * 1024, 60, 12).unwrap();
        let pending = reopened.get(&id, 12).unwrap().unwrap();
        assert_eq!(pending.status, GatewayJobStatus::ReconciliationPending);
        assert_eq!(pending.result, Some(result.clone()));
        assert_eq!(pending.artifacts, vec![artifact.clone()]);
        assert_eq!(pending.receipt, Some(receipt.clone()));
        assert_eq!(
            reopened
                .begin(
                    "job_restart_pressure".to_owned(),
                    "video".to_owned(),
                    "model".to_owned(),
                    Some("buyer".to_owned()),
                    "request-pressure".to_owned(),
                    12,
                )
                .unwrap(),
            BeginGatewayJob::Started
        );
        reopened
            .complete(
                "job_restart_pressure",
                GatewayJobStatus::Completed,
                Some(serde_json::json!({"kind": "video_generation"})),
                Vec::new(),
                None,
                None,
                12,
            )
            .unwrap();
        assert_eq!(
            reopened.get(&id, 12).unwrap().unwrap().artifacts,
            vec![artifact.clone()]
        );
        assert_eq!(
            reopened
                .get("job_restart_pressure", 12)
                .unwrap()
                .unwrap()
                .status,
            GatewayJobStatus::Completed
        );
        let completed = reopened
            .finish_reconciliation(&id, GatewayJobStatus::Completed, None, 13)
            .unwrap();
        assert_eq!(completed.status, GatewayJobStatus::Completed);
        assert_eq!(completed.result, Some(result.clone()));
        assert_eq!(completed.artifacts, vec![artifact.clone()]);
        assert_eq!(completed.receipt, Some(receipt.clone()));
        assert_eq!(
            reopened
                .finish_reconciliation(&id, GatewayJobStatus::Completed, None, 14)
                .unwrap(),
            completed
        );
        drop(reopened);

        let mut reopened =
            GatewayJobStore::durable(seed, directory, 1, 1024 * 1024, 60, 14).unwrap();
        let restored = reopened.get(&id, 14).unwrap().unwrap();
        assert_eq!(restored.status, GatewayJobStatus::Completed);
        assert_eq!(restored.result, Some(result));
        assert_eq!(restored.artifacts, vec![artifact]);
        assert_eq!(restored.receipt, Some(receipt));
    }

    #[test]
    fn reconciliation_pending_payload_survives_ttl_until_it_is_finished() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("jobs");
        let seed = [23_u8; 32];
        let id = gateway_job_id(seed, Some("buyer"), "audio", Some("ack-retry")).unwrap();
        let receipt = serde_json::json!({
            "body": {"session_id": "session-ack-retry"},
            "receipt_ack": {"seq": 1}
        });
        let mut store =
            GatewayJobStore::durable(seed, directory.clone(), 1, 1024 * 1024, 2, 10).unwrap();
        store
            .begin(
                id.clone(),
                "audio".to_owned(),
                "model".to_owned(),
                Some("buyer".to_owned()),
                "request".to_owned(),
                10,
            )
            .unwrap();
        store
            .complete(
                &id,
                GatewayJobStatus::ReconciliationPending,
                Some(serde_json::json!({"kind": "audio_speech"})),
                Vec::new(),
                Some(receipt.clone()),
                Some("signed receipt acknowledgement is pending".to_owned()),
                11,
            )
            .unwrap();
        drop(store);

        let mut reopened =
            GatewayJobStore::durable(seed, directory.clone(), 1, 1024 * 1024, 2, 100).unwrap();
        let pending = reopened.pending_reconciliations(100).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(pending[0].receipt, Some(receipt));
        reopened
            .finish_reconciliation(&id, GatewayJobStatus::Completed, None, 101)
            .unwrap();
        assert!(reopened.pending_reconciliations(101).unwrap().is_empty());
        assert!(reopened.get(&id, 104).unwrap().is_none());
    }

    #[test]
    fn reconciliation_pending_job_is_not_evicted_or_counted_as_live_work() {
        let mut store = GatewayJobStore::in_memory([14_u8; 32], 1, 1024 * 1024, 60);
        store
            .begin(
                "job_pending".to_owned(),
                "image".to_owned(),
                "model".to_owned(),
                None,
                "request-pending".to_owned(),
                1,
            )
            .unwrap();
        store
            .complete(
                "job_pending",
                GatewayJobStatus::ReconciliationPending,
                Some(serde_json::json!({"kind": "image_generation"})),
                Vec::new(),
                Some(serde_json::json!({"receipt_ack": {"seq": 1}})),
                Some("signed receipt acknowledgement is pending".to_owned()),
                2,
            )
            .unwrap();

        assert_eq!(
            store
                .begin(
                    "job_new".to_owned(),
                    "image".to_owned(),
                    "model".to_owned(),
                    None,
                    "request-new".to_owned(),
                    3,
                )
                .unwrap(),
            BeginGatewayJob::Started
        );
        let error = store
            .begin(
                "job_blocked_by_active".to_owned(),
                "image".to_owned(),
                "model".to_owned(),
                None,
                "request-blocked".to_owned(),
                4,
            )
            .expect_err("active live work remains bounded");
        assert!(error.contains("live-job limit"));
        store
            .complete(
                "job_new",
                GatewayJobStatus::Completed,
                Some(serde_json::json!({"kind": "image_generation"})),
                Vec::new(),
                None,
                None,
                4,
            )
            .unwrap();
        assert_eq!(
            store.get("job_pending", 4).unwrap().unwrap().status,
            GatewayJobStatus::ReconciliationPending
        );
        assert_eq!(
            store.get("job_new", 4).unwrap().unwrap().status,
            GatewayJobStatus::Completed
        );
        assert_eq!(
            store.get("job_pending", 5).unwrap().unwrap().status,
            GatewayJobStatus::ReconciliationPending
        );

        store
            .finish_reconciliation("job_pending", GatewayJobStatus::Completed, None, 6)
            .unwrap();
        assert_eq!(
            store.get("job_pending", 6).unwrap().unwrap().status,
            GatewayJobStatus::Completed
        );
        assert!(store.get("job_new", 6).unwrap().is_none());
    }

    #[test]
    fn protected_pending_jobs_keep_the_aggregate_byte_limit_hard() {
        let mut store = GatewayJobStore::in_memory([15_u8; 32], 2, 1024 * 1024, 60);
        store
            .begin(
                "job_large_pending".to_owned(),
                "image".to_owned(),
                "model".to_owned(),
                None,
                "request-large".to_owned(),
                1,
            )
            .unwrap();
        store
            .complete(
                "job_large_pending",
                GatewayJobStatus::ReconciliationPending,
                Some(serde_json::json!({"kind": "image_generation"})),
                vec![GatewayJobArtifact {
                    id: "artifact".to_owned(),
                    content_type: "image/png".to_owned(),
                    bytes: vec![7_u8; 4096],
                    blake3: blake3::hash(&vec![7_u8; 4096]).to_hex().to_string(),
                }],
                Some(serde_json::json!({"receipt_ack": {"seq": 1}})),
                Some("signed receipt acknowledgement is pending".to_owned()),
                2,
            )
            .unwrap();
        let pending_bytes = store.total_bytes;
        store.max_bytes = pending_bytes.saturating_add(64);
        store
            .begin(
                "job_byte_pressure".to_owned(),
                "image".to_owned(),
                "model".to_owned(),
                None,
                "request-pressure".to_owned(),
                3,
            )
            .unwrap();

        let error = store
            .complete(
                "job_byte_pressure",
                GatewayJobStatus::ReconciliationPending,
                Some(serde_json::json!({"kind": "image_generation"})),
                Vec::new(),
                Some(serde_json::json!({"receipt_ack": {"seq": 1}})),
                Some("signed receipt acknowledgement is pending".to_owned()),
                4,
            )
            .expect_err("byte pressure must fail closed");
        assert!(error.contains("cannot fit"));
        assert_eq!(store.total_bytes, pending_bytes);
        assert!(store.total_bytes <= store.max_bytes);
        assert_eq!(
            store
                .get("job_large_pending", 4)
                .unwrap()
                .unwrap()
                .artifacts[0]
                .bytes
                .len(),
            4096
        );
        assert!(store.is_active("job_byte_pressure"));
    }

    #[test]
    fn list_summaries_exclude_result_and_artifact_payloads() {
        let mut store = GatewayJobStore::in_memory([8_u8; 32], 8, 1024 * 1024, 60);
        store
            .begin(
                "job_summary".to_owned(),
                "image".to_owned(),
                "model".to_owned(),
                Some("buyer".to_owned()),
                "request".to_owned(),
                1,
            )
            .unwrap();
        let job = finish(&mut store, "job_summary", 2);
        let summaries = store.list_summaries_for_owner(Some("buyer"), 2).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].artifacts[0].bytes,
            job.artifacts[0].bytes.len()
        );
        assert_eq!(summaries[0].result_metadata, None);

        let entries = store.list_entries_for_owner(Some("buyer"), 2).unwrap();
        let GatewayJobListEntry::Terminal(summary) = &entries[0] else {
            panic!("completed job must be terminal");
        };
        assert_eq!(summary.artifacts[0].bytes, b"png-secret".len());
        assert_eq!(summary.result_metadata, None);
    }

    #[test]
    fn idempotency_reuses_exact_request_and_rejects_conflicts() {
        let seed = [9_u8; 32];
        let id = gateway_job_id(seed, None, "image", Some("same-key")).unwrap();
        assert_eq!(
            id,
            gateway_job_id(seed, None, "image", Some("same-key")).unwrap()
        );
        assert_ne!(
            id,
            gateway_job_id(seed, None, "audio", Some("same-key")).unwrap()
        );
        let mut store = GatewayJobStore::in_memory(seed, 8, 1024 * 1024, 60);
        store
            .begin(
                id.clone(),
                "image".to_owned(),
                "model".to_owned(),
                None,
                "request-a".to_owned(),
                1,
            )
            .unwrap();
        assert_eq!(
            store
                .begin(
                    id.clone(),
                    "image".to_owned(),
                    "model".to_owned(),
                    None,
                    "request-a".to_owned(),
                    1,
                )
                .unwrap(),
            BeginGatewayJob::InProgress
        );
        assert!(store
            .begin(
                id,
                "image".to_owned(),
                "model".to_owned(),
                None,
                "request-b".to_owned(),
                1,
            )
            .unwrap_err()
            .contains("different request"));
    }

    #[test]
    fn wrong_wallet_key_and_ciphertext_tampering_fail_closed() {
        let job = StoredGatewayJob {
            schema_version: JOB_SCHEMA_VERSION,
            id: "job_test".to_owned(),
            endpoint_family: "image".to_owned(),
            model: "model".to_owned(),
            owner_token_id: None,
            request_fingerprint: "request".to_owned(),
            status: GatewayJobStatus::Completed,
            created_at: 1,
            finished_at: 2,
            expires_at: 3,
            result: Some(serde_json::json!({"secret": true})),
            artifacts: Vec::new(),
            receipt: None,
            error: None,
            error_info: None,
        };
        let key = derive_job_store_key([1_u8; 32]);
        let sealed = seal_job(&key, &job).unwrap();
        assert!(open_job(&derive_job_store_key([2_u8; 32]), &job.id, &sealed).is_err());
        let mut tampered_ciphertext = sealed.clone();
        *tampered_ciphertext.last_mut().unwrap() ^= 1;
        assert!(open_job(&key, &job.id, &tampered_ciphertext).is_err());
        let mut tampered_context = sealed;
        tampered_context[JOB_FILE_MAGIC.len() + JOB_CONTEXT_LENGTH_BYTES] ^= 1;
        assert!(open_job(&key, &job.id, &tampered_context).is_err());
    }

    #[test]
    fn active_and_completed_jobs_share_one_hard_count_bound() {
        let mut store = GatewayJobStore::in_memory([4_u8; 32], 2, 1024 * 1024, 60);
        for id in ["job_a", "job_b"] {
            assert_eq!(
                store
                    .begin(
                        id.to_owned(),
                        "image".to_owned(),
                        "model".to_owned(),
                        Some("buyer".to_owned()),
                        id.to_owned(),
                        1,
                    )
                    .unwrap(),
                BeginGatewayJob::Started
            );
        }
        assert!(store
            .begin(
                "job_c".to_owned(),
                "image".to_owned(),
                "model".to_owned(),
                Some("buyer".to_owned()),
                "job_c".to_owned(),
                1,
            )
            .unwrap_err()
            .contains("active job"));

        finish(&mut store, "job_a", 2);
        assert_eq!(
            store
                .begin(
                    "job_c".to_owned(),
                    "image".to_owned(),
                    "model".to_owned(),
                    Some("buyer".to_owned()),
                    "job_c".to_owned(),
                    3,
                )
                .unwrap(),
            BeginGatewayJob::Started
        );
        assert!(store.get("job_a", 3).unwrap().is_none());
        assert_eq!(store.active.len(), 2);
        assert!(store.records.is_empty());
    }
}
