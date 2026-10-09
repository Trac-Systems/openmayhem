//! Demand-triggered operator recovery, using the same HTTP/decoder/endpoint path
//! as ordinary inference. No buyer identity, accepted offer, receipt, ledger write
//! or market-demand event is created. The operator explicitly configures the body,
//! cost/attempt allowance and probe-only duration and output limits.

use super::*;
use capacity::probes::{Probe, Specification, VerifiedCompletion};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("recovery-probe configuration is invalid")]
    Configuration,
    #[error("recovery probe is not due or its health observer is unavailable")]
    Health(#[from] health::Error),
    #[error("recovery probe execution: {0}")]
    Execution(#[from] Error),
}
type ProbeResult<T> = std::result::Result<T, ProbeError>;

pub struct Limits {
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_output_tokens: u64,
    /// Applies only to this operator-authorized probe, never customer generation.
    /// Expiry leaves unknown dispatch retained; it does not authorize a retry.
    pub timeout: Duration,
    pub storage_workers: usize,
}

pub struct Controller {
    connection: Arc<HttpConnection>,
    adapter: Arc<Adapter>,
    pool: Arc<Pool>,
    authority: Arc<capacity::Authority>,
    monitor: health::Monitor,
    request: Request,
    specification: Specification,
    limits: Limits,
    streaming: bool,
    storage: Arc<Semaphore>,
}

/// Evidence about this probe, never a certification of all advertised contexts,
/// global concurrency, native-token speed or an exact upstream monetary charge.
pub struct Outcome {
    pub probe: Digest,
    pub evidence: Digest,
}

impl Controller {
    pub fn new(
        connection: Arc<HttpConnection>,
        adapter: Arc<Adapter>,
        pool: Arc<Pool>,
        authority: Arc<capacity::Authority>,
        monitor: health::Monitor,
        route: Digest,
        budget_group: Digest,
        body: &[u8],
        streaming: bool,
        limits: Limits,
    ) -> ProbeResult<Self> {
        if limits.max_request_bytes == 0
            || body.len() > limits.max_request_bytes
            || limits.max_request_bytes > connection.limits().max_request_bytes
            || limits.max_response_bytes == 0
            || limits.max_response_bytes > adapter.limits().response_bytes
            || limits.max_response_bytes > connection.limits().max_response_bytes
            || limits.max_output_tokens == 0
            || limits.timeout.is_zero()
            || std::time::Instant::now()
                .checked_add(limits.timeout)
                .is_none()
            || !(1..=64).contains(&limits.storage_workers)
        {
            return Err(ProbeError::Configuration);
        }
        let request = if streaming {
            adapter.prepare_stream(body)
        } else {
            adapter.prepare_json(body)
        }
        .map_err(Error::Endpoint)?;
        if adapter.endpoint() != mayhem_proto::proxy::ProxyEndpoint::Decisions {
            let original: serde_json::Value =
                serde_json::from_slice(body).map_err(|_| ProbeError::Configuration)?;
            let mut found = false;
            for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
                if let Some(value) = original.get(key) {
                    if !value
                        .as_u64()
                        .is_some_and(|n| n > 0 && n <= limits.max_output_tokens)
                    {
                        return Err(ProbeError::Configuration);
                    }
                    found = true;
                }
            }
            if !found {
                return Err(ProbeError::Configuration);
            }
        }
        monitor.snapshot(&route)?;
        let specification = Specification {
            route,
            budget_group,
            request_hash: request.request_hash().clone(),
            connection_digest: connection.fingerprint().clone(),
            connection_revision: connection.revision(),
            recipe_digest: adapter.recipe_hash().clone(),
        };
        let storage = Arc::new(Semaphore::new(limits.storage_workers));
        Ok(Self {
            connection,
            adapter,
            pool,
            authority,
            monitor,
            request,
            specification,
            limits,
            streaming,
            storage,
        })
    }
    async fn storage<T: Send + 'static>(
        &self,
        f: impl FnOnce(&capacity::Authority) -> capacity::Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .storage
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::StorageCapacity)?;
        let authority = self.authority.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f(&authority).map_err(Error::Capacity)
        })
        .await
        .map_err(|_| Error::StorageWorker)?
    }
    /// Call only when recovery is needed by demand or approved monitoring policy.
    /// There is no idle polling loop here. Due/backoff and single-scope observation
    /// are enforced before the durable attempt/cost/capacity reservation.
    pub async fn run(&self) -> ProbeResult<Outcome> {
        let mut sample = Some(
            self.monitor
                .observe_recovery(&self.specification.route, self.request.health_class)?,
        );
        let specification = self.specification.clone();
        let reserved = self
            .storage(move |a| a.reserve_probe(specification))
            .await?;
        let id = reserved.probe().id.clone();
        let init = Init::probe(
            reserved.probe(),
            if self.streaming {
                WireFormat::Sse
            } else {
                WireFormat::Json
            },
            self.connection.error_profile(),
            DecodeLimits {
                max_total_bytes: self.limits.max_response_bytes,
                max_event_bytes: self.limits.max_response_bytes,
            },
        )
        .and_then(|init| init.with_semantics(self.request.semantic_policy()))
        .map_err(Error::Decoder);
        let ready = match init {
            Ok(init) => self.pool.start(init).await,
            Err(error) => {
                self.cancel_prepared(id).await?;
                return Err(error.into());
            }
        };
        let ready = match ready {
            Ok(ready) => match ready
                .configure_semantics(self.request.semantic_policy())
                .await
            {
                Ok(ready) => ready,
                Err(error) => {
                    self.cancel_prepared(id).await?;
                    return Err(Error::Decoder(error).into());
                }
            },
            Err(error) => {
                self.cancel_prepared(id).await?;
                return Err(Error::Decoder(error).into());
            }
        };
        if !sample
            .as_ref()
            .ok_or(ProbeError::Configuration)?
            .recovery_is_current()?
        {
            self.cancel_prepared(id).await?;
            return Err(health::Error::RecoveryBusy.into());
        }
        let dispatched = match self.storage(move |a| a.dispatch_probe(reserved)).await {
            Ok(dispatched) => dispatched,
            Err(error) => {
                self.cancel_prepared(id).await?;
                return Err(error.into());
            }
        };
        let probe = dispatched.probe().clone();
        let active = ready.attach_probe(dispatched).map_err(Error::Decoder)?;
        sample
            .as_mut()
            .ok_or(ProbeError::Configuration)?
            .start_execution()?;
        let transport = transport::Transport {
            connection: &self.connection,
            adapter: &self.adapter,
        };
        let public_id = format!("probe_{}", probe.id.as_str());
        let inference = async {
            if self.streaming {
                transport
                    .perform_stream(
                        &self.request,
                        active,
                        &public_id,
                        &transport::Delivery::Probe {
                            created: now_ms() / 1000,
                        },
                        &mut |_| async { Ok(()) },
                        &mut sample,
                    )
                    .await
            } else {
                transport
                    .perform(
                        &self.request,
                        active,
                        &public_id,
                        now_ms() / 1000,
                        &mut sample,
                    )
                    .await
            }
        };
        let result = match tokio::time::timeout(self.limits.timeout, inference).await {
            Ok(result) => result,
            Err(_) => Err(Error::Upstream(Failure::new(
                Code::UpstreamTimeout,
                Scope::Model,
                Stage::ResponseBody,
                Execution::Unknown,
            ))),
        };
        match result {
            Ok(reply) => {
                let observation = sample.take().map(|sample| {
                    sample.prepare_success(reply.reported_usage.as_ref().map(|v| v.output_tokens))
                });
                let bytes =
                    serde_json::to_vec(&reply.body).map_err(|_| ProbeError::Configuration)?;
                let evidence = Digest::hash(
                    "mayhem/proxy/probe-result/v1",
                    &[probe.id.as_str().as_bytes(), &bytes],
                );
                self.complete(probe.clone(), evidence.clone()).await?;
                if let Some(observation) = observation {
                    observation.publish();
                }
                Ok(Outcome {
                    probe: probe.id,
                    evidence,
                })
            }
            Err(error) => {
                let failure = match &error {
                    Error::Upstream(f) | Error::Decoder(worker::Error::Upstream(f)) => {
                        Some(f.clone())
                    }
                    Error::Endpoint(crate::endpoint::Error::Protocol) => Some(Failure::new(
                        Code::UpstreamProtocol,
                        Scope::Model,
                        Stage::ResponseBody,
                        Execution::Unknown,
                    )),
                    _ => None,
                };
                if let Some(failure) = failure {
                    if failure.execution == Execution::NotDispatched || failure.verified_rejection()
                    {
                        let bytes =
                            serde_json::to_vec(&failure).map_err(|_| ProbeError::Configuration)?;
                        let evidence = Digest::hash(
                            "mayhem/proxy/probe-nonexecution/v1",
                            &[probe.id.as_str().as_bytes(), &bytes],
                        );
                        self.complete(probe, evidence).await?;
                    }
                    if let Some(sample) = sample {
                        sample.failure(failure)
                    }
                }
                Err(error.into())
            }
        }
    }
    async fn cancel_prepared(&self, id: Digest) -> Result<()> {
        self.storage(move |a| a.cancel_prepared_probe(&id)).await?;
        Ok(())
    }
    async fn complete(&self, probe: Probe, evidence: Digest) -> Result<()> {
        self.storage(move |a| a.complete_probe(VerifiedCompletion { probe, evidence }))
            .await?;
        Ok(())
    }
}
