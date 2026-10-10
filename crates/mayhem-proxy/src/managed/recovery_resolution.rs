//! Explicit operator recovery of monitoring work, never customer execution.
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
impl Prepared {
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
