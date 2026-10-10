//! Exact provider-signed setup publication through the existing canonical gate.
//! A local signature or imported permit is never a canonical admission claim.
use super::*;
use crate::discovery::Proof;
use ed25519_dalek::{Signer, SigningKey};
use mayhem_proto::proxy::{ProxyAdmissionPermit, PROXY_MAX_SAFE_INTEGER};
use std::time::Duration;

const MAX_OPERATIONS: usize = 17;
const MAX_RESPONSE: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionPermit {
    pub permit: ProxyAdmissionPermit,
    pub issuer_signature: String,
}
impl AdmissionPermit {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = private_file(path, 8192).map_err(|_| Error::Protection)?;
        let value: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        value.verify()?;
        Ok(value)
    }
    fn verify(&self) -> Result<()> {
        let bytes = self.permit.signing_bytes().map_err(|_| Error::Invalid)?;
        require(crate::receipts::verify_signature(
            &self.issuer_signature,
            &bytes,
            &self.permit.issuer_pubkey,
        ))
    }
    fn matches(&self, operation: &ProxyOperation) -> Result<()> {
        self.verify()?;
        let p = &self.permit;
        require(
            matches!(
                operation.action,
                ProxyAction::CreateMarket { .. } | ProxyAction::JoinMarket { .. }
            ) && p.network_id == operation.network_id
                && p.msb_bootstrap == operation.msb_bootstrap
                && p.subnet_bootstrap == operation.subnet_bootstrap
                && p.contract_version == operation.contract_version
                && p.provider_pubkey == operation.provider_pubkey
                && p.initial_operation_digest == operation.digest().map_err(|_| Error::Invalid)?,
        )
    }
}

/// Public reviewable plan. No connection path, upstream mapping or secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationPlan {
    pub schema_version: u32,
    pub draft_id: Digest,
    pub operations: Vec<ProxyOperation>,
    pub plan_digest: Digest,
}
impl PublicationPlan {
    fn digest(&self) -> Result<Digest> {
        let bytes = mayhem_proto::stable_json_bytes(
            &serde_json::json!({"schema_version":self.schema_version,
            "draft_id":self.draft_id,"operations":self.operations}),
        )
        .map_err(|_| Error::Invalid)?;
        Ok(Digest::hash(
            "mayhem/proxy/setup-publication-plan/v1",
            &[&bytes],
        ))
    }
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && (1..=MAX_OPERATIONS).contains(&self.operations.len())
                && self.plan_digest == self.digest()?,
        )?;
        let first = &self.operations[0];
        for (index, operation) in self.operations.iter().enumerate() {
            operation.validate().map_err(|_| Error::Invalid)?;
            require(
                serde_json::to_vec(operation)
                    .map_err(|_| Error::Invalid)?
                    .len()
                    <= 65536
                    && operation.network_id == first.network_id
                    && operation.msb_bootstrap == first.msb_bootstrap
                    && operation.subnet_bootstrap == first.subnet_bootstrap
                    && operation.contract_version == first.contract_version
                    && operation.provider_pubkey == first.provider_pubkey
                    && first.sequence.checked_add(index as u64) == Some(operation.sequence)
                    && (matches!(operation.action, ProxyAction::SetOffer { .. })
                        || index == 0
                            && matches!(
                                operation.action,
                                ProxyAction::CreateMarket { .. } | ProxyAction::JoinMarket { .. }
                            )),
            )?;
        }
        Ok(())
    }
    /// Uses the explicitly unlocked existing provider wallet. Only the reviewed
    /// typed registry operations are signed; no key is serialized or retained.
    pub fn authorize(
        self,
        key: &SigningKey,
        admission: Option<AdmissionPermit>,
    ) -> Result<PublicationAuthorization> {
        self.validate()?;
        require(hex(&key.verifying_key().to_bytes()) == self.operations[0].provider_pubkey)?;
        let signatures = self
            .operations
            .iter()
            .map(|operation| {
                let bytes = operation.signing_bytes().map_err(|_| Error::Invalid)?;
                Ok(hex(&key.sign(&bytes).to_bytes()))
            })
            .collect::<Result<_>>()?;
        let value = PublicationAuthorization {
            plan: self,
            provider_signatures: signatures,
            admission,
        };
        value.validate()?;
        Ok(value)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationAuthorization {
    pub plan: PublicationPlan,
    pub provider_signatures: Vec<String>,
    pub admission: Option<AdmissionPermit>,
}
impl PublicationAuthorization {
    fn validate(&self) -> Result<()> {
        self.plan.validate()?;
        require(self.provider_signatures.len() == self.plan.operations.len())?;
        for (operation, signature) in self.plan.operations.iter().zip(&self.provider_signatures) {
            require(crate::receipts::verify_signature(
                signature,
                &operation.signing_bytes().map_err(|_| Error::Invalid)?,
                &operation.provider_pubkey,
            ))?;
        }
        if let Some(permit) = &self.admission {
            permit.matches(&self.plan.operations[0])?;
        }
        for index in 0..self.plan.operations.len() {
            require(
                serde_json::to_vec(&self.envelope(index))
                    .map_err(|_| Error::Invalid)?
                    .len()
                    <= 65536,
            )?;
        }
        Ok(())
    }
    fn envelope(&self, index: usize) -> serde_json::Value {
        serde_json::json!({"op":"proxy_registry","intent":self.plan.operations[index],
            "provider_signature":self.provider_signatures[index],"admission":if index==0 {self.admission.as_ref()} else {None}})
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    Prepared,
    AdmissionRequired,
    Pending,
    Blocked,
    Complete,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationReason {
    ObservationUnavailable,
    InvalidResponse,
    TimedOut,
    SequenceConflict,
    RegistryDisabled,
    Revoked,
    ConfigurationChanged,
    PermitPolicyMismatch,
    AwaitingCanonicalResult,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    authorization: PublicationAuthorization,
    binding: Digest,
    peer: String,
    state: PublicationState,
    confirmed: usize,
    high_water: Option<(Proof, u64)>,
    evidence: Option<AdmissionEvidence>,
    reason: Option<PublicationReason>,
}
impl Attempt {
    pub(super) fn check_identity(&self, record: &Record) -> Result<()> {
        let p = &self.authorization.plan;
        let o = &p.operations[0];
        require(
            p.draft_id == record.id
                && o.provider_pubkey == record.input.provider_pubkey.as_str()
                && o.network_id == record.input.network.network_id
                && o.msb_bootstrap == record.input.network.msb_bootstrap
                && o.subnet_bootstrap == record.input.network.subnet_bootstrap
                && o.contract_version == record.input.network.contract_version,
        )
    }
    pub(super) fn validate(&self) -> Result<()> {
        self.authorization.validate()?;
        require(
            self.confirmed <= self.authorization.plan.operations.len()
                && (self.state == PublicationState::Complete)
                    == (self.confirmed == self.authorization.plan.operations.len()),
        )?;
        if let Some(evidence) = &self.evidence {
            evidence.validate()?;
            require(
                self.high_water
                    .as_ref()
                    .is_some_and(|(p, e)| p == &evidence.proof && *e == evidence.context.epoch),
            )?;
        }
        if let Some((proof, epoch)) = &self.high_water {
            proof.validate().map_err(|_| Error::Invalid)?;
            require(*epoch <= PROXY_MAX_SAFE_INTEGER)?;
        }
        Ok(())
    }
    pub(super) fn complete(&self) -> bool {
        self.state == PublicationState::Complete
    }
    pub(super) fn status(&self, current: bool) -> &'static str {
        if self.complete() && !current {
            return "previous_publication_confirmed";
        }
        match self.state {
            PublicationState::Prepared => "prepared",
            PublicationState::AdmissionRequired => "admission_required",
            PublicationState::Pending => "pending",
            PublicationState::Blocked => "blocked",
            PublicationState::Complete => "canonical_operations_confirmed",
        }
    }
    pub(super) fn report(&self, record: &Record) -> Result<PublicationReport> {
        Ok(PublicationReport {
            state: self.state,
            plan_digest: self.authorization.plan.plan_digest.clone(),
            total_operations: self.authorization.plan.operations.len(),
            confirmed_operations: self.confirmed,
            pending_operation_digest: self
                .authorization
                .plan
                .operations
                .get(self.confirmed)
                .map(|o| o.digest().map_err(|_| Error::Invalid))
                .transpose()?,
            for_current_configuration: self.binding == record.binding()?
                && record
                    .input
                    .connection()
                    .is_ok_and(|c| c == record.connection),
            canonical_observation: self.evidence.clone(),
            reason: self.reason,
            payment_status: "not_collected_by_setup",
            authorizes_serving: false,
        })
    }
}
#[derive(Serialize)]
pub struct PublicationReport {
    pub state: PublicationState,
    pub plan_digest: Digest,
    pub total_operations: usize,
    pub confirmed_operations: usize,
    pub pending_operation_digest: Option<String>,
    pub for_current_configuration: bool,
    pub canonical_observation: Option<AdmissionEvidence>,
    pub reason: Option<PublicationReason>,
    pub payment_status: &'static str,
    pub authorizes_serving: bool,
}

impl Record {
    pub(super) fn publication_plan(&self, offers_only: bool) -> Result<PublicationPlan> {
        require(self.checked.as_ref() == Some(&self.binding()?))?;
        if self.input.connection()? != self.connection {
            return Err(Error::ConnectionChanged);
        }
        let mut operations = Vec::new();
        if !offers_only {
            operations.push(self.input.operation());
        }
        for offer in &self.input.offers {
            let mut operation = self.input.operation();
            operation.sequence = self
                .input
                .sequence
                .checked_add(operations.len() as u64)
                .ok_or(Error::Invalid)?;
            operation.action = ProxyAction::SetOffer {
                offer: offer.clone(),
            };
            operations.push(operation);
        }
        let mut plan = PublicationPlan {
            schema_version: 1,
            draft_id: self.id.clone(),
            operations,
            plan_digest: Digest::hash("pending", &[]),
        };
        plan.plan_digest = plan.digest()?;
        plan.validate()?;
        Ok(plan)
    }
    fn save_publication(
        &mut self,
        guard: &store::Guard,
        state: PublicationState,
        reason: Option<PublicationReason>,
    ) -> Result<()> {
        let attempt = self.publication.as_mut().ok_or(Error::Invalid)?;
        attempt.state = state;
        attempt.reason = reason;
        self.next(self.revision)?;
        guard.write(self)
    }
}
impl Store {
    /// Pure local review. False includes create/join; true changes only the
    /// saved offers on an already admitted membership. Neither guesses sequence.
    pub fn publication_plan(
        &self,
        expected_revision: u64,
        offers_only: bool,
    ) -> Result<PublicationPlan> {
        let record = store::Guard::open(&self.directory)?
            .read()?
            .ok_or(Error::Missing)?;
        require(record.revision == expected_revision).map_err(|_| Error::Conflict)?;
        if let Some(a) = &record.publication {
            if !a.complete() {
                return Ok(a.authorization.plan.clone());
            }
        }
        record.publication_plan(offers_only)
    }
    /// Persist every exact signature/permit before any I/O. The canonical writer
    /// rechecks active issuer, entitlement, epoch, quotas and all offer policies.
    pub async fn publish(
        &self,
        expected_revision: u64,
        peer_rpc: &str,
        timeout_ms: u64,
        authorization: PublicationAuthorization,
    ) -> Result<Review> {
        authorization.validate()?;
        let (client, base) = admission::peer(peer_rpc, timeout_ms)?;
        let guard = store::Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        if record.revision != expected_revision {
            return Err(Error::Conflict);
        }
        require(
            record
                .revision
                .checked_add((MAX_OPERATIONS * 2 + 3) as u64)
                .is_some_and(|v| v <= PROXY_MAX_SAFE_INTEGER),
        )?;
        let binding = record.binding()?;
        let offers_only = matches!(
            authorization.plan.operations[0].action,
            ProxyAction::SetOffer { .. }
        );
        if let Some(a) = record
            .publication
            .as_mut()
            .filter(|a| !a.complete() || a.binding == binding)
        {
            require(
                a.authorization.plan == authorization.plan
                    && a.authorization.provider_signatures == authorization.provider_signatures
                    && a.peer == base.as_str(),
            )?;
            if a.authorization.admission != authorization.admission {
                require(
                    a.state == PublicationState::AdmissionRequired
                        && a.authorization.admission.is_none()
                        && authorization.admission.is_some(),
                )?;
                a.authorization.admission = authorization.admission;
            }
            if a.complete() {
                return record.review();
            }
        } else {
            require(record.publication_plan(offers_only)? == authorization.plan)?;
            let high_water = record
                .publication
                .as_ref()
                .and_then(|a| a.high_water.clone())
                .or_else(|| record.admission.as_ref().and_then(|a| a.high_water()));
            record.publication = Some(Attempt {
                authorization,
                binding,
                peer: base.as_str().into(),
                state: PublicationState::Prepared,
                confirmed: 0,
                high_water,
                evidence: None,
                reason: None,
            });
        }
        record.save_publication(&guard, PublicationState::Prepared, None)?;
        drive(&guard, &mut record, &client, &base, timeout_ms).await?;
        record.review()
    }
    /// Explicitly resume only the retained signed plan. No wallet, new permit,
    /// invoice, sequence, envelope or writer nonce can be manufactured here.
    pub async fn recover_publication(
        &self,
        expected_revision: u64,
        peer_rpc: &str,
        timeout_ms: u64,
    ) -> Result<Review> {
        let (client, base) = admission::peer(peer_rpc, timeout_ms)?;
        let guard = store::Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        if record.revision != expected_revision {
            return Err(Error::Conflict);
        }
        let a = record.publication.as_ref().ok_or(Error::Missing)?;
        require(a.peer == base.as_str())?;
        if a.complete() {
            return record.review();
        }
        drive(&guard, &mut record, &client, &base, timeout_ms).await?;
        record.review()
    }
}
async fn drive(
    guard: &store::Guard,
    record: &mut Record,
    client: &reqwest::Client,
    base: &url::Url,
    timeout_ms: u64,
) -> Result<()> {
    require(
        record
            .revision
            .checked_add((MAX_OPERATIONS * 2 + 2) as u64)
            .is_some_and(|v| v <= PROXY_MAX_SAFE_INTEGER),
    )?;
    // The whole call has one deadline, not a new allowance per operation.
    match tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        advance(guard, record, client, base),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => record.save_publication(
            guard,
            PublicationState::Pending,
            Some(PublicationReason::TimedOut),
        ),
    }
}
async fn observation(
    record: &mut Record,
    client: &reqwest::Client,
    base: &url::Url,
    operation: &ProxyOperation,
) -> std::result::Result<AdmissionEvidence, PublicationReason> {
    let mut random = [0; 32];
    getrandom::fill(&mut random).map_err(|_| PublicationReason::ObservationUnavailable)?;
    let nonce = Digest::hash("mayhem/proxy/setup-publication-read/v1", &[&random]);
    let digest = Digest::new(
        operation
            .digest()
            .map_err(|_| PublicationReason::InvalidResponse)?,
    )
    .map_err(|_| PublicationReason::InvalidResponse)?;
    let evidence = admission::observe(
        client,
        base,
        &record.input.network,
        &record.input.provider_pubkey,
        &digest,
        &nonce,
    )
    .await
    .map_err(|e| {
        if e == AdmissionState::InvalidResponse {
            PublicationReason::InvalidResponse
        } else {
            PublicationReason::ObservationUnavailable
        }
    })?;
    let a = record
        .publication
        .as_mut()
        .ok_or(PublicationReason::InvalidResponse)?;
    if a.high_water
        .as_ref()
        .is_some_and(|(p, e)| !evidence.proof.follows(p) || evidence.context.epoch < *e)
    {
        return Err(PublicationReason::InvalidResponse);
    }
    a.high_water = Some((evidence.proof.clone(), evidence.context.epoch));
    a.evidence = Some(evidence.clone());
    Ok(evidence)
}
fn applied(evidence: &AdmissionEvidence, operation: &ProxyOperation) -> bool {
    evidence.provider.as_ref().is_some_and(|p| {
        p.sequence == operation.sequence
            && operation
                .digest()
                .is_ok_and(|d| p.operation_digest.as_str() == d)
    })
}
async fn advance(
    guard: &store::Guard,
    record: &mut Record,
    client: &reqwest::Client,
    base: &url::Url,
) -> Result<()> {
    for _ in 0..MAX_OPERATIONS {
        let a = record.publication.as_ref().ok_or(Error::Invalid)?;
        if a.complete() {
            return Ok(());
        }
        let index = a.confirmed;
        let operation = a.authorization.plan.operations[index].clone();
        let evidence = match observation(record, client, base, &operation).await {
            Ok(v) => v,
            Err(reason) => {
                return record.save_publication(guard, PublicationState::Pending, Some(reason))
            }
        };
        if !applied(&evidence, &operation) {
            let reason = if evidence.provider_revoked || evidence.admission_revoked {
                Some(PublicationReason::Revoked)
            } else if !evidence.registry_enabled {
                Some(PublicationReason::RegistryDisabled)
            } else if evidence
                .provider
                .as_ref()
                .map_or(Some(1), |p| p.sequence.checked_add(1))
                != Some(operation.sequence)
            {
                Some(PublicationReason::SequenceConflict)
            } else if record.publication.as_ref().unwrap().binding != record.binding()?
                || record.checked.as_ref() != Some(&record.binding()?)
                || !record
                    .input
                    .connection()
                    .is_ok_and(|c| c == record.connection)
            {
                Some(PublicationReason::ConfigurationChanged)
            } else {
                None
            };
            if let Some(reason) = reason {
                return record.save_publication(guard, PublicationState::Blocked, Some(reason));
            }
            let a = record.publication.as_ref().unwrap();
            if evidence.provider.is_none() && a.authorization.admission.is_none() {
                return record.save_publication(guard, PublicationState::AdmissionRequired, None);
            }
            if index == 0 {
                if let Some(permit) = &a.authorization.admission {
                    let p = &permit.permit;
                    if evidence.provider.is_some()
                        || p.fee_policy_hash != evidence.fee_policy_hash.as_str()
                        || evidence.context.epoch < p.valid_from_epoch
                        || evidence.context.epoch > p.expires_after_epoch
                    {
                        return record.save_publication(
                            guard,
                            PublicationState::Blocked,
                            Some(PublicationReason::PermitPolicyMismatch),
                        );
                    }
                }
            }
            let key = format!(
                "proxy/registry/{}/{}/{}",
                operation.provider_pubkey,
                operation.sequence,
                operation.digest().map_err(|_| Error::Invalid)?
            );
            let envelope = a.authorization.envelope(index);
            record.save_publication(
                guard,
                PublicationState::Pending,
                Some(PublicationReason::AwaitingCanonicalResult),
            )?;
            // Durable persistence can take time. Recheck the private connection
            // immediately before dispatch; drift cannot activate stale claims.
            if !record
                .input
                .connection()
                .is_ok_and(|c| c == record.connection)
            {
                return record.save_publication(
                    guard,
                    PublicationState::Blocked,
                    Some(PublicationReason::ConfigurationChanged),
                );
            }
            // ACKs and errors cannot declare success or safely replace an intent.
            // The writer's existing durable journal owns its original nonce.
            let sent = client
                .post(base.join("contract/feature").map_err(|_| Error::Invalid)?)
                .json(&serde_json::json!({"feature":"mayhem","key":key,"value":envelope}))
                .send()
                .await;
            if let Ok(response) = sent {
                let _ = admission::bounded_response(response, MAX_RESPONSE).await;
            }
            let after = match observation(record, client, base, &operation).await {
                Ok(v) => v,
                Err(reason) => {
                    return record.save_publication(guard, PublicationState::Pending, Some(reason))
                }
            };
            if !applied(&after, &operation) {
                return record.save_publication(
                    guard,
                    PublicationState::Pending,
                    Some(PublicationReason::AwaitingCanonicalResult),
                );
            }
        }
        let a = record.publication.as_mut().unwrap();
        a.confirmed += 1;
        let state = if a.confirmed == a.authorization.plan.operations.len() {
            PublicationState::Complete
        } else {
            PublicationState::Prepared
        };
        record.save_publication(guard, state, None)?;
    }
    Ok(())
}
