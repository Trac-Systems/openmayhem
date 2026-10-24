//! Explicit operator recovery of physical capacity, never financial outcomes.
use super::*;
use serde::Deserialize;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryConfirmation {
    pub schema_version: u32,
    pub probe_id: Digest,
    pub connection_digest: Digest,
    pub evidence_digest: Digest,
    /// The operator checked the original upstream; a local timeout alone is not proof.
    pub upstream_stopped: bool,
}
#[derive(Serialize)]
pub struct RecoveryResolution {
    pub probe_id: Digest,
    pub released: bool,
    pub evidence_digest: Digest,
}
#[derive(Serialize)]
pub struct RecoveryStatus {
    pub group: Digest,
    pub connection_digest: Digest,
    pub probe: Option<capacity::probes::Probe>,
    pub budget: Option<capacity::probes::BudgetStatus>,
}

/// Exact operator attestation for an old customer attempt whose HTTP terminal
/// evidence was not retained. This is capacity recovery, never a refund or retry.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecoveryConfirmation {
    pub schema_version: u32,
    pub route: Digest,
    pub invocation: Digest,
    pub attempt: u64,
    pub capacity_lease: Digest,
    pub connection_digest: Digest,
    pub evidence_digest: Digest,
    pub upstream_stopped: bool,
}
#[derive(Serialize)]
pub struct ExecutionRecoveryResolution {
    pub invocation: Digest,
    pub attempt: u64,
    pub released: bool,
    pub evidence_digest: Digest,
}
impl Prepared {
    /// Exclusive capacity/journal locks require the controller to be stopped.
    /// No POST, ledger write, signature, outcome rewriting or history scan.
    pub fn resolve_stopped_execution(
        self,
        signer: &Authority,
        confirmation: ExecutionRecoveryConfirmation,
    ) -> Result<ExecutionRecoveryResolution> {
        if signer.identity() != &self.identity {
            return Err(Error::Identity);
        }
        require(confirmation.schema_version == 1 && confirmation.upstream_stopped)?;
        let route = self
            .routes
            .iter()
            .find(|r| r.spec.id == confirmation.route)
            .ok_or(Error::Configuration)?;
        let connection = self
            .connections
            .get(&route.spec.connection)
            .ok_or(Error::Configuration)?;
        require(connection.http.fingerprint() == &confirmation.connection_digest)?;
        let authority = self.recovery_capacity()?;
        let path = self
            .config
            .state_dir
            .join(format!("route-{}.redb", route.spec.id.as_str()));
        require(path.is_file() && !path.is_symlink())?;
        let journal = Journal::open(
            &path,
            self.identity.clone(),
            self.config.limits.journal.clone(),
        )
        .map_err(|_| Error::Setup)?;
        let record = journal
            .get_attempt(&confirmation.invocation, confirmation.attempt)
            .map_err(|_| Error::Setup)?
            .ok_or(Error::Configuration)?;
        require(
            record.phase == attempts::Phase::Dispatched
                && record.last_failure.is_some()
                && record.binding.connection_digest == confirmation.connection_digest
                && record.binding.capacity_lease == confirmation.capacity_lease
                && record.binding.recipe_digest == *route.adapter.recipe_hash(),
        )?;
        let evidence = Digest::hash(
            "mayhem/proxy/operator-execution-ended/v1",
            &[&serde_json::to_vec(&confirmation).map_err(|_| Error::Configuration)?],
        );
        let lease = authority
            .lease(&confirmation.capacity_lease)
            .map_err(|_| Error::Setup)?;
        if let Some(lease) = &lease {
            require(
                lease.phase == capacity::Phase::Uncertain
                    && lease.route == confirmation.route
                    && lease.work.invocation == record.invocation
                    && lease.work.request_hash == record.binding.request_hash,
            )?;
        } else {
            require(record.execution_ended.as_ref() == Some(&evidence))?;
        }
        journal
            .observe_attempt(
                &record.invocation,
                record.attempt,
                attempts::Event::ExecutionEnded(evidence.clone()),
                crate::supervisor::unix_ms(),
            )
            .map_err(|_| Error::Setup)?;
        let released = if let Some(lease) = lease {
            authority
                .complete(capacity::VerifiedCompletion {
                    lease,
                    evidence: evidence.clone(),
                })
                .map_err(|_| Error::Setup)?
        } else {
            false
        };
        Ok(ExecutionRecoveryResolution {
            invocation: record.invocation,
            attempt: record.attempt,
            released,
            evidence_digest: evidence,
        })
    }
    /// Inspect only configured groups while the controller is stopped. Opening
    /// the authority retains its normal restart fence; this is not a live reader.
    pub fn recovery_status(self, signer: &Authority) -> Result<Vec<RecoveryStatus>> {
        if signer.identity() != &self.identity {
            return Err(Error::Identity);
        }
        let authority = self.recovery_capacity()?;
        self.connections
            .iter()
            .map(|(group, connection)| {
                Ok(RecoveryStatus {
                    group: group.clone(),
                    connection_digest: connection.http.fingerprint().clone(),
                    probe: authority.probe_for_group(group).map_err(|_| Error::Setup)?,
                    budget: authority.probe_budget(group).map_err(|_| Error::Setup)?,
                })
            })
            .collect()
    }

    fn recovery_capacity(&self) -> Result<capacity::Authority> {
        capacity::Authority::open_existing(
            self.config.state_dir.join("capacity.redb"),
            self.identity.clone(),
            capacity::Limits {
                max_groups: self.config.limits.max_groups,
                max_routes: self.config.limits.max_routes,
                max_leases: self.config.limits.max_leases,
                max_evidence_age: Duration::from_millis(self.config.health.evidence_ttl_ms),
            },
        )
        .map_err(|_| Error::Setup)
    }
    /// Requires the controller to be stopped: the existing authority's exclusive
    /// file lock is retained. No stores are created, no inference or ledger write
    /// occurs, and consumed monitoring allowance is never refunded.
    pub fn resolve_recovery_probe(
        self,
        signer: &Authority,
        confirmation: RecoveryConfirmation,
    ) -> Result<RecoveryResolution> {
        if signer.identity() != &self.identity {
            return Err(Error::Identity);
        }
        require(confirmation.schema_version == 1 && confirmation.upstream_stopped)?;
        let (group, _) = self
            .connections
            .iter()
            .find(|(_, c)| c.http.fingerprint() == &confirmation.connection_digest)
            .ok_or(Error::Configuration)?;
        let authority = self.recovery_capacity()?;
        let probe = authority.probe_for_group(group).map_err(|_| Error::Setup)?;
        let evidence = Digest::hash(
            "mayhem/proxy/operator-probe-resolution/v1",
            &[&serde_json::to_vec(&confirmation).map_err(|_| Error::Configuration)?],
        );
        let released = if let Some(probe) = probe {
            require(
                probe.id == confirmation.probe_id
                    && probe.specification.connection_digest == confirmation.connection_digest
                    && self.routes.iter().any(|r| {
                        r.spec.id == probe.specification.route && &r.spec.connection == group
                    })
                    && probe.phase == capacity::probes::ProbePhase::Uncertain,
            )?;
            authority
                .complete_probe(capacity::probes::VerifiedCompletion {
                    probe,
                    evidence: evidence.clone(),
                })
                .map_err(|_| Error::Setup)?
        } else {
            false
        };
        Ok(RecoveryResolution {
            probe_id: confirmation.probe_id,
            released,
            evidence_digest: evidence,
        })
    }
}
