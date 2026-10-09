use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Unchecked,
    StructurallyValid,
    RecheckRequired,
}

/// Exact unsigned public declaration to bind a later invoice/challenge. This
/// is not proof of identity, payment, unused entitlement or publication success.
#[derive(Serialize)]
pub struct AdmissionHandoff {
    pub initial_operation: ProxyOperation,
    pub initial_operation_digest: String,
}
#[derive(Serialize)]
pub struct Review {
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub kind: &'static str,
    pub draft_id: Digest,
    pub revision: u64,
    pub state: State,
    pub claim_status: &'static str,
    pub probe_status: &'static str,
    pub probe: Option<ProbeReport>,
    pub admission_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admission: Option<AdmissionReport>,
    pub publication_status: &'static str,
    pub serving_status: &'static str,
    pub network: Identity,
    pub provider_pubkey: Digest,
    pub market: ProxyMarketDescriptor,
    pub membership: ProxyMembership,
    pub offers: Vec<ProxyOffer>,
    pub contract: mayhem_proto::EndpointFamilyContract,
    pub settlement_policy: ProxySettlementPolicy,
    pub settlement_policy_hash: String,
    pub admission_handoff: Option<AdmissionHandoff>,
}
impl Record {
    pub(super) fn review(&self) -> Result<Review> {
        self.validate()?;
        let current = self.input.connection();
        let state = if !current.is_ok_and(|c| c == self.connection) {
            State::RecheckRequired
        } else if self.checked.is_some() {
            State::StructurallyValid
        } else {
            State::Unchecked
        };
        let operation = self.input.operation();
        let probe = self.probe.as_ref().map(|p| p.report(self)).transpose()?;
        let admission = self
            .admission
            .as_ref()
            .map(|a| a.report(self))
            .transpose()?;
        let handoff = if state == State::StructurallyValid {
            Some(AdmissionHandoff {
                initial_operation_digest: operation.digest().map_err(|_| Error::Invalid)?,
                initial_operation: operation,
            })
        } else {
            None
        };
        Ok(Review {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            kind: "provider_setup_review",
            draft_id: self.id.clone(),
            revision: self.revision,
            state,
            claim_status: "operator_declared",
            probe_status: probe.as_ref().map_or("not_run", |p| p.status_name()),
            probe,
            admission_status: admission.as_ref().map_or("not_checked", |a| a.status),
            admission,
            publication_status: "not_submitted",
            serving_status: "not_started",
            network: self.input.network.clone(),
            provider_pubkey: self.input.provider_pubkey.clone(),
            market: self.input.market.clone(),
            membership: self.input.membership.clone(),
            offers: self.input.offers.clone(),
            contract: self.input.adapter.contract.clone(),
            settlement_policy: self.input.settlement_policy.clone(),
            settlement_policy_hash: self
                .input
                .settlement_policy
                .digest()
                .map_err(|_| Error::Invalid)?,
            admission_handoff: handoff,
        })
    }
}
