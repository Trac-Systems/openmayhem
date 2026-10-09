//! Authenticated owner-facing projection, separate from provider-controlled JSON.
//! This is a trusted gateway observation, not a new ledger receipt or signature.
//! Never deserialize this projection into internal authorization/closure authority.
use super::*;

#[derive(Serialize)]
pub(crate) struct Evidence<'a> {
    schema_version: u32,
    object: &'static str,
    id: &'a str,
    model: &'a str,
    endpoint_family: &'a str,
    request_fingerprint: &'a str,
    identity: &'a RequestIdentity,
    phase: ProxyPhase,
    terms: Option<&'a ProxySpendTerms>,
    terms_hash: Option<String>,
    settlement_policy: Option<&'a ProxySettlementPolicy>,
    acceptance: Option<Acceptance<'a>>,
    result_verified: bool,
    financial: Financial<'a>,
}

#[derive(Serialize)]
struct Acceptance<'a> {
    buyer_sig: &'a str,
    provider_sig: &'a str,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Financial<'a> {
    /// Includes unknown execution and unsigned intents awaiting recovery. No
    /// receipt, timeout or missing journal row authorizes release or repurchase.
    Pending,
    /// The exclusive owner retired the intent before granting any authorization.
    NotAuthorized,
    /// An opaque, durable signing fence, NOT a canonical payment receipt.
    NonAdmission {
        terms_hash: &'a Digest,
        #[serde(with = "mayhem_proto::decimal_u128")]
        maximum_au: mayhem_proto::MoneyAu,
        fence_commitment: &'a Digest,
        budget_settled: bool,
    },
    /// Canonical payment closure, observed at this exact signed view. This is
    /// retained evidence, not a claim that the observation is current/live.
    Canonical {
        outcome: &'a FinancialOutcome,
        observation: &'a Proof,
        commitment: Digest,
        budget_settled: bool,
    },
}

pub(crate) fn project(job: &StoredGatewayJob) -> Result<Evidence<'_>, String> {
    let state = job.proxy.as_ref().ok_or("not a proxy job")?;
    // Verify one bounded owner record, never scan receipts or query the ledger.
    // Authorization/receipt signatures and all original identity bindings are
    // checked before any fields can be used by a retail settlement consumer.
    state.validate(job)?;
    require(
        job.owner_token_id.is_some(),
        "proxy evidence lacks an owner",
    )?;
    let financial = if state.not_authorized {
        Financial::NotAuthorized
    } else if let Some(fence) = &state.non_admission {
        Financial::NonAdmission {
            terms_hash: &fence.terms,
            maximum_au: fence.maximum,
            fence_commitment: &fence.marker,
            budget_settled: state.budget_marker.is_some(),
        }
    } else if let Some(closure) = &state.closure {
        Financial::Canonical {
            outcome: &closure.outcome,
            observation: &closure.proof,
            commitment: closure.commitment()?,
            budget_settled: state.budget_marker.is_some(),
        }
    } else {
        Financial::Pending
    };
    Ok(Evidence {
        schema_version: 1,
        object: "mayhem.proxy.job_evidence",
        id: &job.id,
        model: &job.model,
        endpoint_family: &job.endpoint_family,
        request_fingerprint: &job.request_fingerprint,
        identity: &state.identity,
        phase: state.phase(),
        terms: state.terms.as_ref(),
        terms_hash: state
            .terms
            .as_ref()
            .map(ProxySpendTerms::digest)
            .transpose()?,
        settlement_policy: state.policy.as_ref(),
        acceptance: state.authorization.as_ref().map(|auth| Acceptance {
            buyer_sig: &auth.buyer_sig,
            provider_sig: &auth.provider_sig,
        }),
        result_verified: state.output_receipt.is_some(),
        financial,
    })
}
