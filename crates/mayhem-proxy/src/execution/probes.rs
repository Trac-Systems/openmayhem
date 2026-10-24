//! Demand-triggered operator recovery, using the same HTTP/decoder/endpoint path
//! as ordinary inference. No buyer identity, accepted offer, receipt, ledger write
//! or market-demand event is created. The operator explicitly configures the body,
//! cost/attempt allowance and probe-only duration and output limits.

use super::*;
use capacity::probes::{Probe, ReservationIntent, Specification, VerifiedCompletion};
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
    /// Health-observation deadline for this operator-authorized probe, never
    /// customer generation. Expiry marks health failed while the original
    /// exchange drains to verified completion; it does not authorize a retry.
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
    request_class: crate::conformance::Class,
    specification: Specification,
    limits: Limits,
    streaming: bool,
    storage: Arc<Semaphore>,
    tokenizer: Option<Arc<health::native::Source>>,
    reservation_intent: Option<ReservationIntent>,
}

/// Evidence about this probe, never a certification of all advertised contexts,
/// global concurrency, native-token speed or an exact upstream monetary charge.
pub struct Outcome {
    pub probe: Digest,
    pub evidence: Digest,
    pub conformance: Conformance,
}

/// Sealed actual decoder result. It is not deserializable and contains no prompt,
/// credential, upstream URL or generic caller-supplied capability observations.
pub struct Conformance {
    pub(crate) network: crate::discovery::Identity,
    pub(crate) provider: Digest,
    pub(crate) endpoint: mayhem_proto::proxy::ProxyEndpoint,
    pub(crate) contract: Digest,
    pub(crate) recipe: Digest,
    pub(crate) connection: Digest,
    pub(crate) connection_revision: u64,
    pub(crate) request_hash: Digest,
    pub(crate) probe: Digest,
    pub(crate) result: Digest,
    pub(crate) class: crate::conformance::Class,
    pub(crate) assertions: Vec<crate::conformance::Assertion>,
    pub(crate) completed: std::time::Instant,
}

struct ProbeWait<T> {
    result: Result<T>,
    expired: bool,
    started: bool,
}

async fn wait_for_probe<T>(
    inference: impl Future<Output = Result<T>>,
    shutdown: impl Future<Output = ()>,
    deadline: tokio::time::Instant,
    on_timeout: impl FnOnce(),
) -> ProbeWait<T> {
    let mut started = false;
    let mut expired = false;
    let mut on_timeout = Some(on_timeout);
    let result = {
        let inference = async {
            started = true;
            inference.await
        };
        tokio::pin!(inference, shutdown);
        let timer = tokio::time::sleep_until(deadline);
        tokio::pin!(timer);
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => break Err(Error::Cancelled),
                _ = &mut timer, if !expired => {
                    expired = true;
                    if let Some(observe) = on_timeout.take() { observe(); }
                }
                result = &mut inference => {
                    // The inference poll itself may cross the deadline even
                    // after the timer was checked. Never publish late success.
                    if !expired && tokio::time::Instant::now() >= deadline {
                        expired = true;
                        if let Some(observe) = on_timeout.take() { observe(); }
                    }
                    break result;
                }
            }
        }
    };
    ProbeWait {
        result,
        expired,
        started,
    }
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
        if adapter
            .job()
            .is_some_and(|job| job.operations().iter().any(|op| !connection.supports(*op)))
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
        let request_class = crate::conformance::Class::request(
            &serde_json::from_slice(body).map_err(|_| ProbeError::Configuration)?,
        )
        .map_err(|_| ProbeError::Configuration)?;
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
            request_class,
            specification,
            limits,
            streaming,
            storage,
            tokenizer: None,
            reservation_intent: None,
        })
    }
    /// The trusted caller must persist this intent and its derived probe ID
    /// before run(). It grants at most one new reservation and never recreates
    /// a Dispatch permit for an existing or completed attempt.
    pub fn with_reservation_intent(mut self, intent: ReservationIntent) -> ProbeResult<Self> {
        self.authority
            .probe_intent_id(&self.specification, &intent)
            .map_err(Error::Capacity)?;
        self.reservation_intent = Some(intent);
        Ok(self)
    }
    pub fn with_tokenizer(mut self, source: Arc<health::native::Source>) -> ProbeResult<Self> {
        if self.adapter.endpoint() == mayhem_proto::proxy::ProxyEndpoint::Decisions
            || !source.matches(self.connection.fingerprint(), self.adapter.recipe_hash())
        {
            return Err(ProbeError::Configuration);
        }
        self.monitor
            .validate_tokenizer(&self.specification.route, source.digest())?;
        source
            .bind_pool(&self.pool)
            .map_err(|_| ProbeError::Configuration)?;
        self.tokenizer = Some(source);
        Ok(self)
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
        self.run_until(std::future::pending()).await
    }
    /// Shutdown may abandon an unresolved exchange, never its durable occupancy.
    /// Storage transitions are outside the cancellable network wait.
    pub(crate) async fn run_until(
        &self,
        shutdown: impl Future<Output = ()>,
    ) -> ProbeResult<Outcome> {
        tokio::pin!(shutdown);
        tokio::select! {
            biased;
            _ = &mut shutdown => return Err(Error::Cancelled.into()),
            _ = std::future::ready(()) => (),
        }
        let expansion = self.monitor.snapshot(&self.specification.route)?.allowance > 0;
        if expansion {
            let route = self.specification.route.clone();
            let status = self.storage(move |a| a.status(&route)).await?;
            if status.state != capacity::Readiness::Busy || status.available != 0 {
                return Err(health::Error::RecoveryBusy.into());
            }
        }
        let mut sample = Some(if expansion {
            self.monitor
                .observe_capacity_recovery(&self.specification.route, self.request.health_class)?
        } else {
            self.monitor
                .observe_recovery(&self.specification.route, self.request.health_class)?
        });
        let specification = self.specification.clone();
        let intent = self.reservation_intent.clone();
        let reserved = self
            .storage(move |a| match (expansion, intent) {
                (true, intent) => a.reserve_capacity_probe(specification, intent),
                (false, Some(intent)) => a.reserve_probe_once(specification, intent),
                (false, None) => a.reserve_probe(specification),
            })
            .await?;
        let id = reserved.probe().id.clone();
        let init = Init::probe(
            reserved.probe(),
            if self.streaming {
                self.adapter.stream_format()
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
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                self.cancel_prepared(id).await?;
                return Err(Error::Cancelled.into());
            }
            _ = std::future::ready(()) => (),
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
        if let (Some(sample), Some(source)) = (&mut sample, &self.tokenizer) {
            sample.use_native(source)
        }
        let transport = transport::Transport {
            completion: None,
            connection: &self.connection,
            adapter: &self.adapter,
        };
        let public_id = format!("probe_{}", probe.id.as_str());
        let inference = async {
            if self.adapter.job().is_some() {
                transport
                    .perform_probe_job(&self.request, active, &probe, &public_id, &mut sample)
                    .await
            } else if self.streaming {
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
        let timeout_failure = Failure::new(
            Code::UpstreamTimeout,
            Scope::Model,
            Stage::ResponseBody,
            Execution::Unknown,
        );
        let ProbeWait {
            result,
            expired,
            started,
        } = wait_for_probe(
            inference,
            shutdown,
            tokio::time::Instant::now() + self.limits.timeout,
            || {
                // The original exchange still owns its measurement. Publish
                // failure without dropping it or granting another dispatch.
                if let Ok(observer) = self
                    .monitor
                    .observe_request(&self.specification.route, self.request.health_class)
                {
                    observer.failure(timeout_failure.clone());
                }
            },
        )
        .await;
        if !started {
            // Shutdown won before this process ever polled the HTTP operation.
            // A dispatch record alone must not turn known-unsent work unknown.
            let evidence = Digest::hash(
                "mayhem/proxy/probe-unsent-shutdown/v1",
                &[probe.id.as_str().as_bytes()],
            );
            self.complete(probe, evidence).await?;
            return Err(Error::Cancelled.into());
        }
        match result {
            Ok(reply) => {
                let bytes =
                    serde_json::to_vec(&reply.body).map_err(|_| ProbeError::Configuration)?;
                let evidence = Digest::hash(
                    "mayhem/proxy/probe-result/v1",
                    &[probe.id.as_str().as_bytes(), &bytes],
                );
                self.complete(probe.clone(), evidence.clone()).await?;
                if expired {
                    // A late verified result proves physical completion, not a
                    // successful deadline or a fresh conformance measurement.
                    drop(sample.take());
                    return Err(Error::Upstream(timeout_failure).into());
                }
                let observation = sample.take().map(|sample| {
                    sample.prepare_success(reply.reported_usage.as_ref().map(|v| v.output_tokens))
                });
                if let Some(observation) = observation {
                    observation.publish();
                }
                Ok(Outcome {
                    conformance: Conformance {
                        network: crate::discovery::Identity {
                            network_id: self.authority.identity().network_id.clone(),
                            msb_bootstrap: self.authority.identity().msb_bootstrap.as_str().into(),
                            subnet_bootstrap: self
                                .authority
                                .identity()
                                .subnet_bootstrap
                                .as_str()
                                .into(),
                            contract_version: mayhem_proto::CONTRACT_VERSION,
                        },
                        provider: self.authority.identity().controller_pubkey.clone(),
                        endpoint: self.adapter.endpoint(),
                        contract: self.adapter.contract_hash().clone(),
                        recipe: self.adapter.recipe_hash().clone(),
                        connection: self.specification.connection_digest.clone(),
                        connection_revision: self.specification.connection_revision,
                        request_hash: self.request.request_hash().clone(),
                        probe: probe.id.clone(),
                        result: evidence.clone(),
                        class: self.request_class.clone(),
                        assertions: crate::conformance::capture::assertions(
                            self.streaming,
                            matches!(
                                self.request.semantic_policy().output,
                                crate::semantics::Output::JsonSchema { .. }
                            ),
                            &reply.body,
                        ),
                        completed: std::time::Instant::now(),
                    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn expired_deadline_wins_when_completion_is_already_ready() {
        let observations = Cell::new(0);
        let result = wait_for_probe(
            std::future::ready(Ok(())),
            std::future::pending(),
            tokio::time::Instant::now() - Duration::from_millis(1),
            || observations.set(observations.get() + 1),
        )
        .await;
        assert!(result.result.is_ok());
        assert!(result.started && result.expired);
        assert_eq!(observations.get(), 1);
    }

    #[tokio::test]
    async fn completion_poll_crossing_deadline_cannot_skip_timeout_observation() {
        let observations = Cell::new(0);
        let result = wait_for_probe(
            async {
                std::thread::sleep(Duration::from_millis(20));
                Ok(())
            },
            std::future::pending(),
            tokio::time::Instant::now() + Duration::from_millis(5),
            || observations.set(observations.get() + 1),
        )
        .await;
        assert!(result.result.is_ok());
        assert!(result.started && result.expired);
        assert_eq!(observations.get(), 1);
    }

    #[tokio::test]
    async fn shutdown_before_first_inference_poll_preserves_nonexecution_proof() {
        let result = wait_for_probe::<()>(
            async { panic!("shutdown must not start upstream work") },
            std::future::ready(()),
            tokio::time::Instant::now() + Duration::from_secs(1),
            || panic!("shutdown is not an upstream health failure"),
        )
        .await;
        assert!(matches!(result.result, Err(Error::Cancelled)));
        assert!(!result.started && !result.expired);
    }

    #[tokio::test]
    async fn shutdown_after_first_inference_poll_cannot_claim_nonexecution() {
        let dispatched = tokio::sync::Notify::new();
        let result = wait_for_probe::<()>(
            async {
                dispatched.notify_one();
                std::future::pending().await
            },
            dispatched.notified(),
            tokio::time::Instant::now() + Duration::from_secs(1),
            || panic!("shutdown is not an upstream health failure"),
        )
        .await;
        assert!(matches!(result.result, Err(Error::Cancelled)));
        assert!(result.started && !result.expired);
    }
}
