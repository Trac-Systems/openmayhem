//! Bounded negative execution evidence from the trusted parent broker. A signed
//! provider assertion permits an explicitly agreed zero-charge closure; it is not
//! independent buyer proof of remote execution and never authorizes another POST.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NonExecutionEvidence {
    pub failure: FailureSnapshot,
}
impl NonExecutionEvidence {
    pub fn commitment(
        &self,
        invocation: &Digest,
        attempt: u64,
        binding: &Binding,
    ) -> Result<Digest> {
        binding.validate()?;
        require(attempt > 0 && self.failure.to_failure()?.known_non_execution())?;
        crate::endpoint::digest("mayhem/proxy/verified-nonexecution/v1", &serde_json::json!({
            "invocation":invocation, "attempt":attempt, "binding":binding, "failure":self.failure
        })).map_err(|_| Error::Invalid)
    }
}
impl Record {
    pub(crate) fn failure_non_execution(&self) -> Option<NonExecutionEvidence> {
        if self.remote_id.is_some()
            || self.output_may_have_been_delivered
            || !matches!(self.phase, Phase::Resolved | Phase::Closed)
        {
            return None;
        }
        let evidence = NonExecutionEvidence {
            failure: self.last_failure.clone()?,
        };
        let digest = evidence
            .commitment(&self.invocation, self.attempt, &self.binding)
            .ok()?;
        matches!(&self.resolution, Some(Resolution::NotExecuted { evidence }) if *evidence == digest)
            .then_some(evidence)
    }
    pub(crate) fn non_execution_evidence(&self) -> Option<Digest> {
        self.unsent_cancellation_evidence().or_else(|| {
            self.failure_non_execution()?
                .commitment(&self.invocation, self.attempt, &self.binding)
                .ok()
        })
    }
}

pub(super) fn resolve_failure(
    tx: &redb::WriteTransaction,
    r: &mut Record,
    failure: FailureSnapshot,
) -> Result<()> {
    let verified = failure.to_failure()?.known_non_execution();
    if verified {
        require(r.remote_id.is_none() && !r.output_may_have_been_delivered)?;
        let evidence = NonExecutionEvidence {
            failure: failure.clone(),
        }
        .commitment(&r.invocation, r.attempt, &r.binding)?;
        let resolution = Resolution::NotExecuted { evidence };
        payloads::verify_resolution(tx, &r.key(), &resolution)?;
        r.phase = Phase::Resolved;
        r.resolution = Some(resolution);
    }
    r.last_failure = Some(failure);
    Ok(())
}

impl Journal {
    /// Upgrade one retained pre-integration failure under the normal journal
    /// transaction. No history scan, resend, signature or financial mutation.
    pub(crate) fn resolve_saved_non_execution(
        &self,
        invocation: &Digest,
        attempt: u64,
        now: u64,
    ) -> Result<bool> {
        // No writer lock for normal completions, unknown jobs or already resolved
        // evidence. The transaction below rechecks a qualifying legacy record.
        let saved = self.recovery_header(invocation, attempt)?;
        if saved.record.phase != Phase::Dispatched
            || !saved
                .record
                .last_failure
                .as_ref()
                .is_some_and(|f| f.to_failure().is_ok_and(|f| f.known_non_execution()))
        {
            return Ok(false);
        }
        let tx = self.transaction()?;
        let mut r = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        require(r.attempt == attempt)?;
        if r.phase != Phase::Dispatched {
            return Ok(false);
        }
        let Some(failure) = r.last_failure.clone() else {
            return Ok(false);
        };
        if !failure.to_failure()?.known_non_execution() {
            return Ok(false);
        }
        resolve_failure(&tx, &mut r, failure)?;
        bump(&tx, &mut r, now)?;
        save(&tx, &r)?;
        self.commit(tx)?;
        Ok(true)
    }
}
