//! Fixed-operation asynchronous control and exact-original result recovery.
use super::*;
use crate::{
    attempts::jobs::Lease,
    connector::config::Operation,
    recipe::job::{Job, Status},
};
use serde_json::Value;
use std::time::Duration;
fn invalid() -> Error {
    Error::Upstream(Failure::new(
        Code::UpstreamProtocol,
        Scope::Model,
        Stage::ResponseBody,
        Execution::Unknown,
    ))
}
fn uncertain(mut failure: Failure) -> Error {
    failure.execution = Execution::Unknown;
    Error::Upstream(failure)
}
async fn control(
    connection: &HttpConnection,
    operation: Operation,
    body: Vec<u8>,
    status: u16,
    timeout: u64,
) -> Result<Value> {
    tokio::time::timeout(Duration::from_millis(timeout), async {
        let mut response = connection
            .send(operation, Some(body))
            .await
            .map_err(uncertain)?;
        if response.status != status || response.format != WireFormat::Json {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        while let Some((chunk, _)) = response.next_timed_chunk().await.map_err(uncertain)? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|n| n > 16 * 1024)
            {
                return Err(invalid());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid())
    })
    .await
    .map_err(|_| invalid())?
}
impl Executor {
    pub(super) async fn reserve_job(&self, record: &Record) -> Result<Option<Lease>> {
        let Some(job) = self.adapter.job() else {
            return Ok(None);
        };
        let k = record.invocation.clone();
        let attempt = record.attempt;
        let duration = job.control_timeout_ms + 5000;
        self.storage
            .run(move |j| {
                j.reserve_job(&k, attempt)?;
                j.claim_job(&k, attempt, now_ms(), duration)?
                    .ok_or(attempts::Error::Stale)
            })
            .await
            .map(Some)
    }
    pub(super) async fn submit_job(
        &self,
        record: &Record,
        request: &Request,
        lease: Lease,
    ) -> Result<ProtocolReply> {
        let job = self.adapter.job().ok_or(Error::Configuration)?;
        let body = job
            .submit_body(request.body(), record, self.adapter.recipe_request_limit())
            .map_err(|_| invalid())?;
        let response = control(
            &self.connection,
            self.adapter.operation(),
            body,
            job.submit_status,
            job.control_timeout_ms,
        )
        .await?;
        let id = job.id(&response).map_err(|_| invalid())?;
        let saved = lease.clone();
        self.storage
            .run(move |j| {
                j.accept_job(&saved, id, now_ms())?;
                j.finish_job_step(&saved, now_ms(), 0, false)
            })
            .await?;
        loop {
            if self.resume_job(&record.invocation, record.attempt).await? {
                return self
                    .storage
                    .recover(&record.invocation, record.attempt)
                    .await?
                    .result
                    .map(|r| r.reply)
                    .ok_or(Error::Binding);
            }
            let current = self.storage.current(&record.invocation).await?;
            if current.cancellation_requested {
                return Err(Error::Cancelled);
            }
            // Long inference has no invented execution timeout. Each control operation,
            // lease, response and poll cadence remains independently bounded.
            tokio::time::sleep(Duration::from_millis(job.poll_interval_ms)).await;
        }
    }
    /// One bounded original-job recovery step. Never submits inference or authorizes
    /// money. A caller must separately authenticate any delivery and reconcile the
    /// retained original financial terms. Unknown outcomes remain occupied.
    pub async fn resume_job(&self, invocation: &Digest, attempt: u64) -> Result<bool> {
        let key = invocation.clone();
        if !self.storage.run(move |j| j.has_job(&key, attempt)).await? {
            return Ok(false);
        }
        let key = invocation.clone();
        let header = self
            .storage
            .run(move |j| j.recovery_header(&key, attempt))
            .await?;
        if header.has_result {
            return Ok(true);
        }
        if header.record.phase != Phase::Dispatched {
            return Ok(false);
        }
        let saved = self.storage.recover(invocation, attempt).await?;
        let Some(accepted) = saved.acceptance else {
            return Ok(false);
        };
        accepted.validate_for(&saved.record)?;
        let adapter = Adapter::restore(accepted.snapshot.adapter)?;
        let Some(job) = adapter.job() else {
            return Ok(false);
        };
        if &saved.record.binding.connection_digest != self.connection.fingerprint()
            || saved.record.binding.connection_revision != self.connection.revision()
            || job
                .operations()
                .iter()
                .any(|op| !self.connection.supports(*op))
        {
            return Err(Error::Binding);
        }
        let request = adapter.prepare_json(&saved.request.ok_or(Error::Binding)?.body)?;
        if !request.matches_binding(&saved.record.binding)
            || request.metering_policy_hash() != saved.record.binding.metering_policy
        {
            return Err(Error::Binding);
        }
        let k = invocation.clone();
        let duration = job.control_timeout_ms + 5000;
        let Some(lease) = self
            .storage
            .run(move |j| j.claim_job(&k, attempt, now_ms(), duration))
            .await?
        else {
            return Ok(false);
        };
        let result = tokio::time::timeout(
            Duration::from_millis(job.control_timeout_ms),
            self.job_step(&adapter, &request, job, &lease),
        )
        .await
        .map_err(|_| invalid())?;
        match result {
            Ok(JobStep::Ready(reply)) => {
                let l = lease.clone();
                self.storage
                    .run(move |j| j.retain_job_result(&l, &reply, now_ms()))
                    .await?;
                Ok(true)
            }
            Ok(JobStep::Pending) => {
                let l = lease;
                let next = now_ms().saturating_add(job.poll_interval_ms);
                self.storage
                    .run(move |j| j.finish_job_step(&l, now_ms(), next, false))
                    .await?;
                Ok(false)
            }
            Ok(JobStep::Unresolved) => {
                let l = lease;
                let next = now_ms().saturating_add(job.poll_interval_ms);
                self.storage
                    .run(move |j| j.finish_job_step(&l, now_ms(), next, false))
                    .await?;
                Err(Error::RecoveryRequired)
            }
            Ok(JobStep::Unrecoverable) => {
                let l = lease;
                self.storage
                    .run(move |j| j.finish_job_step(&l, now_ms(), 0, true))
                    .await?;
                Err(Error::RecoveryRequired)
            }
            Err(error) => Err(error), // Durable lease expires; it never frees capacity.
        }
    }
    async fn job_step(
        &self,
        adapter: &Adapter,
        request: &Request,
        job: &Job,
        lease: &Lease,
    ) -> Result<JobStep> {
        let mut record = self.storage.current(&lease.record().invocation).await?;
        let id = if let Some(id) = record.remote_id.clone() {
            id
        } else {
            if job.lookup.is_none() {
                return Ok(JobStep::Unrecoverable);
            }
            let value = control(
                &self.connection,
                Operation::JobLookup,
                job.lookup_body(&record).map_err(|_| invalid())?,
                200,
                job.control_timeout_ms,
            )
            .await?;
            job.check_lookup(&value, &record).map_err(|_| invalid())?;
            // Missing lookup is not permission to repeat submit, even with an idempotency key.
            if job.status(&value).map_err(|_| invalid())? == Status::Missing {
                return Ok(JobStep::Pending);
            }
            let id = job.id(&value).map_err(|_| invalid())?;
            let l = lease.clone();
            let saved = id.clone();
            record = self
                .storage
                .run(move |j| j.accept_job(&l, saved, now_ms()))
                .await?;
            id
        };
        if record.cancellation_requested {
            if let Some(cancel) = &job.cancel {
                if cancel.idempotent || !lease.cancel_sent {
                    let l = lease.clone();
                    self.storage
                        .run(move |j| j.mark_job_cancel(&l, now_ms()))
                        .await?;
                    let value = control(
                        &self.connection,
                        Operation::JobCancel,
                        cancel.control.body(&id).map_err(|_| invalid())?,
                        200,
                        job.control_timeout_ms,
                    )
                    .await?;
                    cancel
                        .control
                        .check_response(&value, &id)
                        .map_err(|_| invalid())?;
                }
            }
        }
        let value = control(
            &self.connection,
            Operation::JobPoll,
            job.poll.body(&id).map_err(|_| invalid())?,
            200,
            job.control_timeout_ms,
        )
        .await?;
        job.poll
            .check_response(&value, &id)
            .map_err(|_| invalid())?;
        match job.status(&value).map_err(|_| invalid())? {
            Status::Pending | Status::Missing => return Ok(JobStep::Pending),
            Status::Cancelled | Status::Failed => return Ok(JobStep::Unresolved),
            Status::Ready => (),
        }
        let limits = adapter.limits();
        let init = Init::new(
            &record,
            WireFormat::Json,
            self.connection.error_profile(),
            DecodeLimits {
                max_total_bytes: limits.response_bytes,
                max_event_bytes: limits.response_bytes,
            },
        )?
        .with_semantics(request.semantic_policy())?;
        let decoder = self
            .pool
            .start(init)
            .await?
            .configure_semantics(request.semantic_policy())
            .await?
            .attach_job(lease)?;
        let response = self
            .connection
            .send(
                Operation::JobResult,
                Some(job.result.body(&id).map_err(|_| invalid())?),
            )
            .await
            .map_err(uncertain)?;
        if response.status != 200 || response.format != WireFormat::Json {
            return Err(invalid());
        }
        let reply = transport::Transport {
            connection: &self.connection,
            adapter,
        }
        .receive_job_result(
            request,
            decoder,
            response,
            job,
            &id,
            &format!("proxy_{}", record.invocation.as_str()),
            record.created_at_ms / 1000,
        )
        .await?;
        Ok(JobStep::Ready(reply))
    }
}
enum JobStep {
    Pending,
    Unresolved,
    Unrecoverable,
    Ready(ProtocolReply),
}

impl transport::Transport<'_> {
    pub(super) async fn perform_probe_job(
        &self,
        request: &Request,
        decoder: worker::host::Active,
        probe: &capacity::probes::Probe,
        public_id: &str,
        sample: &mut Option<health::Sample>,
    ) -> Result<ProtocolReply> {
        let job = self.adapter.job().ok_or(Error::Configuration)?;
        let key = Digest::hash(
            "mayhem/proxy/upstream-probe-job-key/v1",
            &[
                probe.id.as_str().as_bytes(),
                probe.specification.request_hash.as_str().as_bytes(),
                probe.specification.recipe_digest.as_str().as_bytes(),
            ],
        );
        let body = job
            .submit_body_with_key(request.body(), &key, self.adapter.recipe_request_limit())
            .map_err(|_| invalid())?;
        let value = control(
            self.connection,
            self.adapter.operation(),
            body,
            job.submit_status,
            job.control_timeout_ms,
        )
        .await?;
        let id = job.id(&value).map_err(|_| invalid())?;
        loop {
            let value = control(
                self.connection,
                Operation::JobPoll,
                job.poll.body(&id).map_err(|_| invalid())?,
                200,
                job.control_timeout_ms,
            )
            .await?;
            job.poll
                .check_response(&value, &id)
                .map_err(|_| invalid())?;
            match job.status(&value).map_err(|_| invalid())? {
                Status::Pending | Status::Missing => {
                    tokio::time::sleep(Duration::from_millis(job.poll_interval_ms)).await
                }
                Status::Ready => break,
                Status::Cancelled | Status::Failed => return Err(Error::RecoveryRequired),
            }
        }
        // A probe retains the existing explicit timeout/cost/capacity authority. Any
        // interrupted or ambiguous operation leaves its original probe occupied;
        // this path never recreates a reservation, retries submit, or invents closure.
        let response = self
            .connection
            .send(
                Operation::JobResult,
                Some(job.result.body(&id).map_err(|_| invalid())?),
            )
            .await
            .map_err(uncertain)?;
        let reply = self
            .receive_job_result(
                request,
                decoder,
                response,
                job,
                &id,
                public_id,
                now_ms() / 1000,
            )
            .await?;
        if let Some(sample) = sample {
            sample.network_complete(tokio::time::Instant::now());
        }
        Ok(reply)
    }
}

impl transport::Transport<'_> {
    async fn receive_job_result(
        &self,
        request: &Request,
        mut decoder: worker::host::Active,
        mut response: crate::connector::http::UpstreamResponse,
        job: &Job,
        id: &attempts::RemoteId,
        public_id: &str,
        created: u64,
    ) -> Result<ProtocolReply> {
        if response.status != 200 || response.format != WireFormat::Json {
            return Err(invalid());
        }
        // Optional result ID echo is checked before worker mapping, within the same
        // configured response bound; no upstream result is accepted without the worker.
        let mut bytes = Vec::new();
        while let Some((chunk, _)) = response.next_timed_chunk().await.map_err(uncertain)? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|n| n > self.adapter.limits().response_bytes)
            {
                return Err(invalid());
            }
            bytes.extend_from_slice(&chunk);
        }
        if job.result.response_id_path.is_some() {
            let value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            job.result
                .check_response(&value, &id)
                .map_err(|_| invalid())?;
        }
        for chunk in bytes.chunks(16 * 1024) {
            decoder
                .push(chunk, |_| async { Err(worker::Error::Protocol) })
                .await?;
        }
        let mut value = None;
        decoder
            .finish(|decoded| {
                let valid = match decoded {
                    Decoded::Json { value: v } if value.is_none() => {
                        value = Some(v);
                        true
                    }
                    _ => false,
                };
                async move {
                    if valid {
                        Ok(())
                    } else {
                        Err(worker::Error::Protocol)
                    }
                }
            })
            .await?;
        let mut reply = request.decode_json(value.ok_or_else(invalid)?, public_id, created)?;
        reply.upstream_id = Some(id.clone());
        Ok(reply)
    }
}
