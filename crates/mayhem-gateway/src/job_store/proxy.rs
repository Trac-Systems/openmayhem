//! Private Proxy owner obligations in the existing encrypted job vault. No
//! signing, budget mutation, dispatch, refund or retry authority lives here.
use super::*;
use mayhem_proto::proxy::finance::{
    ProxySettlementPolicy, ProxySpendAuthorization, ProxySpendTerms,
};
use mayhem_proxy::{
    attempts::Digest,
    buyer_controller::{NonAdmission, RequestIdentity, VerifiedOutput},
    discovery::Proof,
    financial::{
        negotiation::SavedPurchase, quote::PreparedPurchase, recovery::FinancialOutcome,
        Observation,
    },
    signing::ProviderReceipt,
};
use std::ops::Bound::{Excluded, Unbounded};

const MAX_PENDING_PAGE: usize = 64;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProxyJobState {
    identity: RequestIdentity,
    terms: Option<ProxySpendTerms>,
    policy: Option<ProxySettlementPolicy>,
    buyer_journal_seen: bool,
    authorization: Option<ProxySpendAuthorization>,
    output_receipt: Option<ProviderReceipt>,
    closure: Option<ProxyClosure>,
    budget_marker: Option<Digest>,
    #[serde(default)]
    not_authorized: bool,
    #[serde(default)]
    non_admission: Option<ProxyNonAdmission>,
}
impl std::fmt::Debug for ProxyJobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyJobState")
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyPhase {
    AwaitingAuthorization,
    AwaitingBuyerJournal,
    PurchaseRetained,
    OutputRetained,
    ClosurePendingBudget,
    Closed,
    NotAuthorized,
    NonAdmissionPendingBudget,
    NotAdmitted,
}

/// Captured only from the controller's opaque durable signing fence. The
/// encrypted owner record preserves it before any budget release takes place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProxyNonAdmission {
    identity: RequestIdentity,
    pub(crate) terms: Digest,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub(crate) maximum: mayhem_proto::MoneyAu,
    pub(crate) marker: Digest,
}
impl ProxyNonAdmission {
    pub(crate) fn capture(proof: &NonAdmission) -> Result<Self, String> {
        Ok(Self {
            identity: proof.identity().clone(),
            terms: Digest::new(proof.terms().digest()?).map_err(err)?,
            maximum: proof.terms().max_spend_au,
            marker: proof.commitment().clone(),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProxyClosure {
    pub(crate) outcome: FinancialOutcome,
    pub(crate) proof: Proof,
}
impl ProxyClosure {
    /// Stable across later canonical observations of the same financial result.
    pub(crate) fn commitment(&self) -> Result<Digest, String> {
        let bytes =
            mayhem_proto::stable_json_bytes(&serde_json::to_value(&self.outcome).map_err(err)?)
                .map_err(err)?;
        let mut hash = blake3::Hasher::new_derive_key("mayhem/gateway/proxy-closure/v1");
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        Digest::new(hash.finalize().to_hex().to_string()).map_err(err)
    }
}

/// Owned only after the controller independently verifies delivered output.
/// Capture before moving the bounded owner write onto its storage executor.
pub(crate) struct ProxyVerifiedOutput {
    identity: RequestIdentity,
    authorization: ProxySpendAuthorization,
    response: Value,
    receipt: ProviderReceipt,
}
impl ProxyVerifiedOutput {
    pub(crate) fn capture(output: &VerifiedOutput<'_>) -> Self {
        Self {
            identity: output.identity().clone(),
            authorization: output.authorization().clone(),
            response: output.response().clone(),
            receipt: output.receipt().clone(),
        }
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn require(ok: bool, message: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}
fn identity_matches(identity: &RequestIdentity, terms: &ProxySpendTerms) -> bool {
    identity.billing_id.as_str() == terms.billing_id
        && identity.billing_attempt == terms.billing_attempt
        && identity.session_id.as_str() == terms.session_id
        && identity.request_hash.as_str() == terms.request_hash
}
impl ProxyJobState {
    pub(crate) fn identity(&self) -> &RequestIdentity {
        &self.identity
    }
    pub(crate) fn terms(&self) -> Option<&ProxySpendTerms> {
        self.terms.as_ref()
    }
    pub(crate) fn authorization(&self) -> Option<&ProxySpendAuthorization> {
        self.authorization.as_ref()
    }
    pub(crate) fn closure(&self) -> Option<&ProxyClosure> {
        self.closure.as_ref()
    }
    pub(crate) fn non_admission(&self) -> Option<&ProxyNonAdmission> {
        self.non_admission.as_ref()
    }
    pub(crate) fn phase(&self) -> ProxyPhase {
        if self.not_authorized {
            ProxyPhase::NotAuthorized
        } else if self.non_admission.is_some() {
            if self.budget_marker.is_some() {
                ProxyPhase::NotAdmitted
            } else {
                ProxyPhase::NonAdmissionPendingBudget
            }
        } else if self.budget_marker.is_some() {
            ProxyPhase::Closed
        } else if self.closure.is_some() {
            ProxyPhase::ClosurePendingBudget
        } else if self.output_receipt.is_some() {
            ProxyPhase::OutputRetained
        } else if self.buyer_journal_seen {
            ProxyPhase::PurchaseRetained
        } else if self.terms.is_some() {
            ProxyPhase::AwaitingBuyerJournal
        } else {
            ProxyPhase::AwaitingAuthorization
        }
    }
    pub(super) fn validate(&self, job: &StoredGatewayJob) -> Result<(), String> {
        Digest::new(&job.request_fingerprint).map_err(err)?;
        require(
            self.identity.billing_attempt > 0
                && self.identity.billing_attempt <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER,
            "invalid proxy owner attempt",
        )?;
        if self.not_authorized {
            return require(
                self.terms.is_none()
                    && self.policy.is_none()
                    && !self.buyer_journal_seen
                    && self.authorization.is_none()
                    && self.output_receipt.is_none()
                    && self.closure.is_none()
                    && self.budget_marker.is_none()
                    && self.non_admission.is_none()
                    && job.status == GatewayJobStatus::Failed
                    && job.result.is_none()
                    && job.receipt.is_none()
                    && job.artifacts.is_empty()
                    && job
                        .error_info
                        .as_ref()
                        .is_some_and(|e| e.code == "proxy_not_authorized"),
                "unsigned proxy failure contains authorized obligations",
            );
        }
        require(
            job.receipt.is_none() && job.artifacts.is_empty(),
            "proxy evidence cannot use native receipt/artifact fields",
        )?;
        require(
            self.terms.is_some() == self.policy.is_some(),
            "proxy policy/terms presence differs",
        )?;
        require(
            !self.buyer_journal_seen || self.terms.is_some(),
            "proxy journal marker lacks authorization",
        )?;
        require(
            self.authorization.is_none() || self.buyer_journal_seen,
            "proxy acceptance lacks original journal marker",
        )?;
        require(
            self.output_receipt.is_some() == job.result.is_some(),
            "proxy output/evidence presence differs",
        )?;
        if let Some(terms) = &self.terms {
            terms.validate()?;
            require(
                identity_matches(&self.identity, terms),
                "proxy original request identity differs",
            )?;
            require(
                self.policy.as_ref().unwrap().digest()? == terms.settlement_policy_hash,
                "proxy settlement policy differs",
            )?;
            let endpoint = serde_json::to_value(terms.offer.endpoint).map_err(err)?;
            require(
                endpoint.as_str() == Some(&job.endpoint_family),
                "proxy endpoint differs",
            )?;
            let model = format!(
                "proxy/offer/{}/{}/{}",
                terms.offer.market_id,
                terms.offer.provider_pubkey,
                terms.offer.slot_id()?
            );
            require(job.model == model, "proxy selected offer differs")?;
        }
        if let Some(fence) = &self.non_admission {
            require(
                fence.identity == self.identity
                    && !self.buyer_journal_seen
                    && self.authorization.is_none()
                    && self.output_receipt.is_none()
                    && self.closure.is_none()
                    && job.result.is_none(),
                "proxy non-admission fence conflicts with purchase evidence",
            )?;
            if let Some(terms) = &self.terms {
                require(
                    terms.digest()? == fence.terms.as_str() && terms.max_spend_au == fence.maximum,
                    "proxy non-admission terms differ",
                )?;
            }
            return match &self.budget_marker {
                Some(marker) => require(
                    marker == &fence.marker
                        && job.status == GatewayJobStatus::Failed
                        && job
                            .error_info
                            .as_ref()
                            .is_some_and(|e| e.code == "proxy_not_admitted"),
                    "proxy non-admission budget closure differs",
                ),
                None => require(
                    job.status == GatewayJobStatus::ReconciliationPending,
                    "proxy non-admission budget recovery must remain pinned",
                ),
            };
        }
        if let Some(auth) = &self.authorization {
            require(
                self.terms.as_ref() == Some(&auth.terms),
                "proxy signed acceptance changed",
            )?;
            auth.verify(mayhem_proxy::receipts::verify_signature)?;
        }
        if let Some(receipt) = &self.output_receipt {
            let auth = self
                .authorization
                .as_ref()
                .ok_or("proxy output lacks acceptance")?;
            let terms = &auth.terms;
            require(
                receipt.draft.body.accepted_terms == terms.digest()?
                    && receipt.draft.invocation
                        == mayhem_proxy::exchange::invocation(auth).map_err(err)?
                    && receipt.draft.attempt > 0
                    && receipt.draft.body.final_receipt,
                "proxy output receipt identity differs",
            )?;
            receipt.draft.body.validate_for(
                terms,
                self.policy.as_ref().unwrap(),
                receipt.draft.previous.as_ref(),
            )?;
            require(
                mayhem_proxy::receipts::verify_signature(
                    &receipt.provider_sig,
                    &receipt.draft.body.provider_signing_bytes()?,
                    &terms.offer.provider_pubkey,
                ),
                "proxy output provider signature differs",
            )?;
            let expected_id = format!("proxy_{}", receipt.draft.invocation.as_str());
            require(
                job.result
                    .as_ref()
                    .and_then(|r| r.get("id"))
                    .and_then(Value::as_str)
                    == Some(expected_id.as_str()),
                "proxy output identity differs",
            )?;
        }
        if let Some(closure) = &self.closure {
            closure.proof.validate().map_err(err)?;
            let auth = self
                .authorization
                .as_ref()
                .ok_or("proxy closure lacks acceptance")?;
            let policy = self.policy.as_ref().unwrap();
            match &closure.outcome {
                FinancialOutcome::Paid { receipt } => {
                    let output = self
                        .output_receipt
                        .as_ref()
                        .ok_or("paid proxy closure lacks durable verified output")?;
                    require(
                        receipt.body == output.draft.body
                            && receipt.provider_sig == output.provider_sig,
                        "proxy paid closure differs from retained output",
                    )?;
                    receipt.verify(
                        &auth.terms,
                        policy,
                        output.draft.previous.as_ref(),
                        mayhem_proxy::receipts::verify_signature,
                    )?;
                }
                FinancialOutcome::Waived { closure } => {
                    closure.verify(&auth.terms, mayhem_proxy::receipts::verify_signature)?
                }
                FinancialOutcome::ExpiredUnknown { expiry } => expiry.verify(
                    &auth.terms,
                    policy,
                    expiry.body.observed_epoch,
                    mayhem_proxy::receipts::verify_signature,
                )?,
            }
        }
        match &self.budget_marker {
            Some(marker) => {
                let closure = self
                    .closure
                    .as_ref()
                    .ok_or("proxy terminal job lacks closure")?;
                require(
                    marker == &closure.commitment()?,
                    "proxy budget marker differs from closure",
                )?;
                let status = if matches!(closure.outcome, FinancialOutcome::Paid { .. }) {
                    GatewayJobStatus::Completed
                } else {
                    GatewayJobStatus::Failed
                };
                require(
                    job.status == status,
                    "proxy terminal status differs from closure",
                )?;
            }
            None => require(
                job.status == GatewayJobStatus::ReconciliationPending,
                "unsettled proxy job must remain pinned",
            )?,
        }
        Ok(())
    }
}

impl GatewayJobStore {
    pub(crate) fn proxy_enabled(&self) -> bool {
        self.directory.is_some() && !self.proxy_failed
    }

    fn proxy_ready(&self) -> Result<(), String> {
        require(
            self.directory.is_some(),
            "proxy owner jobs require a durable encrypted vault",
        )?;
        require(
            !self.proxy_failed,
            "proxy owner persistence is uncertain; reopen the vault",
        )
    }
    fn proxy_job(&self, id: &str) -> Result<StoredGatewayJob, String> {
        self.proxy_ready()?;
        let job = self.records.get(id).ok_or("proxy owner job is missing")?;
        require(job.proxy.is_some(), "job is not a proxy owner job")?;
        Ok(job.clone())
    }
    fn persist_proxy(&mut self, job: StoredGatewayJob) -> Result<StoredGatewayJob, String> {
        self.proxy_ready()?;
        job.proxy
            .as_ref()
            .ok_or("proxy state missing")?
            .validate(&job)?;
        let bytes = seal_job(&self.key, &job)?;
        require(
            bytes.len() <= self.max_bytes,
            "proxy owner record exceeds encrypted vault byte bound",
        )?;
        self.make_room_for_bytes(bytes.len(), Some(&job.id))?;
        if let Err(error) = self.persist(&job.id, &bytes, self.sealed_sizes.contains_key(&job.id)) {
            self.proxy_failed = true;
            return Err(error);
        }
        #[cfg(test)]
        if self.proxy_lose_write_ack {
            self.proxy_failed = true;
            return Err("injected lost proxy write acknowledgment".to_owned());
        }
        let previous = self
            .sealed_sizes
            .insert(job.id.clone(), bytes.len())
            .unwrap_or(0);
        self.total_bytes = self
            .total_bytes
            .saturating_sub(previous)
            .saturating_add(bytes.len());
        if job.status == GatewayJobStatus::ReconciliationPending {
            self.proxy_pending.insert(job.id.clone());
        } else {
            self.proxy_pending.remove(&job.id);
        }
        self.records.insert(job.id.clone(), job.clone());
        Ok(job)
    }
    /// Durable idempotency ownership, even before a quote or owner budget grant.
    pub(crate) fn begin_proxy(
        &mut self,
        id: String,
        endpoint_family: String,
        model: String,
        owner_token_id: Option<String>,
        request_fingerprint: String,
        identity: RequestIdentity,
        now: u64,
    ) -> Result<BeginGatewayJob, String> {
        self.proxy_ready()?;
        validate_job_id(&id)?;
        self.purge_expired(now)?;
        if let Some(existing) = self.records.get(&id) {
            validate_job_identity(
                existing,
                &endpoint_family,
                &model,
                owner_token_id.as_deref(),
                &request_fingerprint,
            )?;
            let proxy = existing
                .proxy
                .as_ref()
                .ok_or("job belongs to the native lane")?;
            require(
                proxy.identity == identity,
                "proxy original request identity changed",
            )?;
            return Ok(BeginGatewayJob::Existing(existing.clone()));
        }
        require(
            !self.active.contains_key(&id),
            "job is already owned by the native lane",
        )?;
        self.make_room_for_job()?;
        let job = StoredGatewayJob {
            schema_version: JOB_SCHEMA_VERSION,
            id,
            endpoint_family,
            model,
            owner_token_id,
            request_fingerprint,
            status: GatewayJobStatus::ReconciliationPending,
            created_at: now,
            finished_at: now,
            expires_at: now.saturating_add(self.ttl_seconds),
            result: None,
            artifacts: Vec::new(),
            receipt: None,
            error: None,
            error_info: None,
            proxy: Some(ProxyJobState {
                identity,
                terms: None,
                policy: None,
                buyer_journal_seen: false,
                authorization: None,
                output_receipt: None,
                closure: None,
                budget_marker: None,
                not_authorized: false,
                non_admission: None,
            }),
        };
        self.persist_proxy(job)?;
        Ok(BeginGatewayJob::Started)
    }
    /// Only the exclusive owner may retire an intent before authorization. The
    /// owner callback must retain terms before reserving budget or granting any
    /// signing. Once terms exist, this path cannot release or retire the job.
    pub(crate) fn fail_proxy_before_authorization(
        &mut self,
        id: &str,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        require(
            proxy.terms.is_none(),
            "authorized proxy purchase requires recovery",
        )?;
        if proxy.not_authorized {
            return Ok(job);
        }
        proxy.not_authorized = true;
        job.status = GatewayJobStatus::Failed;
        job.finished_at = now;
        job.expires_at = now.saturating_add(self.ttl_seconds);
        job.error = Some(
            "Proxy request ended before spending was authorized; no inference was dispatched"
                .into(),
        );
        job.error_info = Some(GatewayJobErrorInfo {
            code: "proxy_not_authorized".into(),
            category: "proxy_admission".into(),
            retryable: true,
            phase: Some("admission".into()),
        });
        self.persist_proxy(job)
    }
    /// Exact prepared terms are durable before the caller grants signing. This
    /// is an owner obligation, never proof that the buyer journal is absent.
    pub(crate) fn authorize_proxy(
        &mut self,
        id: &str,
        purchase: &PreparedPurchase,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        self.authorize_proxy_terms(id, purchase.terms().clone(), purchase.policy().clone(), now)
    }
    pub(crate) fn authorize_proxy_terms(
        &mut self,
        id: &str,
        terms: ProxySpendTerms,
        policy: ProxySettlementPolicy,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        require(
            !proxy.not_authorized && proxy.non_admission.is_none(),
            "proxy signing has been fenced",
        )?;
        if let Some(existing) = &proxy.terms {
            require(
                existing == &terms && proxy.policy.as_ref() == Some(&policy),
                "proxy owner already authorized different terms",
            )?;
            return Ok(job);
        }
        proxy.terms = Some(terms);
        proxy.policy = Some(policy);
        job.finished_at = now;
        self.persist_proxy(job)
    }
    pub(crate) fn retain_proxy_non_admission(
        &mut self,
        id: &str,
        fence: ProxyNonAdmission,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        if let Some(existing) = &proxy.non_admission {
            require(existing == &fence, "proxy non-admission fence changed")?;
            return Ok(job);
        }
        require(
            !proxy.not_authorized && proxy.budget_marker.is_none(),
            "proxy owner is already terminal",
        )?;
        proxy.non_admission = Some(fence);
        job.finished_at = now;
        self.persist_proxy(job)
    }
    pub(crate) fn finish_proxy_non_admission(
        &mut self,
        id: &str,
        marker: &Digest,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        require(
            proxy
                .non_admission
                .as_ref()
                .is_some_and(|f| &f.marker == marker),
            "proxy non-admission budget marker differs",
        )?;
        if let Some(existing) = &proxy.budget_marker {
            require(existing == marker, "proxy budget marker changed")?;
            return Ok(job);
        }
        proxy.budget_marker = Some(marker.clone());
        job.status = GatewayJobStatus::Failed;
        job.finished_at = now;
        job.expires_at = now.saturating_add(self.ttl_seconds);
        job.error = Some(
            "Proxy request was not admitted; no inference was dispatched and nothing was charged"
                .into(),
        );
        job.error_info = Some(GatewayJobErrorInfo {
            code: "proxy_not_admitted".into(),
            category: "proxy_admission".into(),
            retryable: true,
            phase: Some("admission".into()),
        });
        self.persist_proxy(job)
    }
    /// Observe the original durable buyer record; absence has no inverse API.
    pub(crate) fn note_proxy_purchase(
        &mut self,
        id: &str,
        purchase: &SavedPurchase,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        require(
            proxy.terms.as_ref() == Some(&purchase.offer().terms),
            "proxy buyer journal terms differ",
        )?;
        let auth = purchase.authorization();
        if let (Some(old), Some(new)) = (&proxy.authorization, &auth) {
            require(old == new, "proxy buyer acceptance changed")?;
        }
        if proxy.buyer_journal_seen && (auth.is_none() || proxy.authorization == auth) {
            return Ok(job);
        }
        proxy.buyer_journal_seen = true;
        if auth.is_some() {
            proxy.authorization = auth;
        }
        job.finished_at = now;
        self.persist_proxy(job)
    }
    pub(crate) fn retain_proxy_output(
        &mut self,
        id: &str,
        output: ProxyVerifiedOutput,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        require(
            proxy.identity == output.identity
                && proxy.terms.as_ref() == Some(&output.authorization.terms),
            "proxy verified output belongs to another request",
        )?;
        if let Some(existing) = &proxy.output_receipt {
            require(
                existing == &output.receipt
                    && proxy.authorization.as_ref() == Some(&output.authorization)
                    && job.result.as_ref() == Some(&output.response),
                "proxy verified output is immutable",
            )?;
            return Ok(job);
        }
        require(
            proxy.closure.is_none(),
            "proxy financial closure already retained",
        )?;
        if let Some(auth) = &proxy.authorization {
            require(auth == &output.authorization, "proxy acceptance changed")?;
        }
        proxy.authorization = Some(output.authorization);
        proxy.buyer_journal_seen = true;
        proxy.output_receipt = Some(output.receipt);
        job.result = Some(output.response);
        job.finished_at = now;
        self.persist_proxy(job)
    }
    /// Only fresh, authenticated canonical observation can create this record.
    /// Financial closure remains pinned until the common budget owner settles it.
    pub(crate) fn close_proxy(
        &mut self,
        id: &str,
        observation: &Observation,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let outcome = observation
            .financial_outcome()
            .map_err(err)?
            .ok_or("proxy canonical financial closure is not confirmed")?;
        let authorization = &observation.accepted().authorization;
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        require(
            proxy.terms.as_ref() == Some(&authorization.terms),
            "proxy canonical terms differ",
        )?;
        if let Some(auth) = &proxy.authorization {
            require(auth == authorization, "proxy canonical acceptance differs")?;
        }
        if let Some(closure) = &proxy.closure {
            require(
                closure.outcome == outcome,
                "proxy canonical outcome conflicts with retained closure",
            )?;
            return Ok(job);
        }
        proxy.authorization = Some(authorization.clone());
        proxy.buyer_journal_seen = true;
        proxy.closure = Some(ProxyClosure {
            outcome,
            proof: observation.proof().clone(),
        });
        job.finished_at = now;
        self.persist_proxy(job)
    }
    /// The caller invokes this only AFTER common budget accounting durably
    /// commits this exact closure commitment. No local Drop/refund transition.
    pub(crate) fn finish_proxy(
        &mut self,
        id: &str,
        budget_marker: Digest,
        now: u64,
    ) -> Result<StoredGatewayJob, String> {
        let mut job = self.proxy_job(id)?;
        let proxy = job.proxy.as_mut().unwrap();
        let closure = proxy
            .closure
            .as_ref()
            .ok_or("proxy canonical closure is missing")?;
        require(
            budget_marker == closure.commitment()?,
            "proxy budget settlement marker differs",
        )?;
        if proxy.budget_marker.as_ref() == Some(&budget_marker) {
            return Ok(job);
        }
        let paid = matches!(closure.outcome, FinancialOutcome::Paid { .. });
        let unknown = matches!(closure.outcome, FinancialOutcome::ExpiredUnknown { .. });
        proxy.budget_marker = Some(budget_marker);
        job.status = if paid {
            GatewayJobStatus::Completed
        } else {
            GatewayJobStatus::Failed
        };
        job.finished_at = now;
        job.expires_at = now.saturating_add(self.ttl_seconds);
        if !paid {
            job.error = Some(
                if unknown {
                    "Proxy funds closed with execution outcome unknown; do not redispatch"
                } else {
                    "Proxy purchase closed without a paid result"
                }
                .to_owned(),
            );
            job.error_info = Some(GatewayJobErrorInfo {
                code: if unknown {
                    "proxy_execution_unknown"
                } else {
                    "proxy_purchase_waived"
                }
                .to_owned(),
                category: if unknown {
                    "execution_unknown"
                } else {
                    "proxy"
                }
                .to_owned(),
                retryable: false,
                phase: None,
            });
        }
        self.persist_proxy(job)
    }
    /// Indexed bounded iteration; callers fetch only the individual jobs they own.
    pub(crate) fn pending_proxy(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        self.proxy_ready()?;
        require(
            (1..=MAX_PENDING_PAGE).contains(&limit),
            "invalid proxy pending page limit",
        )?;
        Ok(match after {
            Some(after) => self
                .proxy_pending
                .range::<str, _>((Excluded(after), Unbounded))
                .take(limit)
                .cloned()
                .collect(),
            None => self.proxy_pending.iter().take(limit).cloned().collect(),
        })
    }
}

#[cfg(test)]
mod tests;
