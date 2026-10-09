//! Bounded passive upstream evidence. This observes serving, never authorizes a
//! POST, frees capacity, retries work, signs availability or settles money.
mod admission;
mod measurement;
#[cfg(test)]
mod tests;
use crate::{
    attempts::Digest,
    connector::failure::{Code, Failure, Scope},
    supervisor::{RefreshPolicy, Schedule},
};
pub use measurement::{Measurement, Sample};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid proxy health configuration or observation")]
    Invalid,
    #[error("proxy health evidence is unavailable")]
    Unavailable,
    #[error("proxy recovery observation is already running or not due")]
    RecoveryBusy,
}
pub type Result<T> = std::result::Result<T, Error>;

/// Explicit local acceptance policy; no universal inference-duration limit.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub max_routes: usize,
    pub max_classes_per_route: usize,
    pub evidence_ttl_ms: u64,
    pub successes_to_increase: u32,
    pub bad_samples_to_reduce: u32,
    pub latency_baseline_samples: u32,
    pub latency_multiplier: u32,
    pub latency_increase_ms: u64,
    pub min_native_tok_s: u32,
    pub recovery: RefreshPolicy,
}
impl Policy {
    fn validate(&self) -> Result<()> {
        if self.max_routes == 0
            || self.max_routes > 100_000
            || self.max_classes_per_route == 0
            || self.max_classes_per_route > 256
            || self.evidence_ttl_ms == 0
            || self.evidence_ttl_ms > 3_600_000
            || self.successes_to_increase == 0
            || self.bad_samples_to_reduce == 0
            || self.latency_baseline_samples == 0
            || self.latency_multiplier < 2
            || self.min_native_tok_s < 5
            || self.recovery.validate().is_err()
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Thinking {
    Disabled,
    Enabled,
    Unknown,
}
/// Size classes describe serialized request size, never alleged native tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Class {
    pub input_bytes_log2: u8,
    pub thinking: Thinking,
    pub streaming: bool,
    pub controls_digest: [u8; 32],
}
impl Class {
    pub fn new(bytes: usize, thinking: Thinking, streaming: bool) -> Self {
        Self {
            input_bytes_log2: (usize::BITS - bytes.max(1).leading_zeros()) as u8,
            thinking,
            streaming,
            controls_digest: [0; 32],
        }
    }
    pub(crate) fn request(value: &serde_json::Value, bytes: usize, streaming: bool) -> Self {
        let thinking = match value
            .get("thinking_mode")
            .and_then(serde_json::Value::as_str)
        {
            Some("disabled") => Thinking::Disabled,
            Some("enabled") => Thinking::Enabled,
            _ => Thinking::Unknown,
        };
        let mut class = Self::new(bytes, thinking, streaming);
        // Borrow only controls, no prompt/tool content. Stream serialization into
        // a fixed-size digest so large values cannot create another payload copy.
        let controls: [(&str, Option<&serde_json::Value>); 9] = [
            ("reasoning", value.get("reasoning")),
            ("reasoning_effort", value.get("reasoning_effort")),
            ("chat_template_kwargs", value.get("chat_template_kwargs")),
            ("thinking_mode", value.get("thinking_mode")),
            ("max_tokens", value.get("max_tokens")),
            ("max_completion_tokens", value.get("max_completion_tokens")),
            ("max_output_tokens", value.get("max_output_tokens")),
            ("n", value.get("n")),
            ("response_format", value.get("response_format")),
        ];
        struct Hash(blake3::Hasher);
        impl std::io::Write for Hash {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut hash = Hash(blake3::Hasher::new_derive_key(
            "mayhem/proxy/health-controls/v1",
        ));
        // Value serialization into an infallible writer cannot fail.
        if serde_json::to_writer(&mut hash, &controls).is_ok() {
            class.controls_digest = *hash.0.finalize().as_bytes();
        }
        class
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    NoEvidence,
    Fresh,
    Stale,
    Busy,
    RateLimited,
    Authentication,
    PaymentRequired,
    Unreachable,
    ModelUnavailable,
    InvalidResponse,
    SlowGeneration,
    SlowResponse,
    RecoveryRequired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ready,
    Degraded,
    Busy,
    Unavailable,
    Checking,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub state: State,
    pub reason: Reason,
    /// TOTAL allowance, not a free-slot claim. Durable capacity subtracts work.
    pub allowance: u32,
    pub evidence_age_ms: Option<u64>,
    pub recovery_after_ms: u64,
    pub recovery_in_progress: bool,
    pub last_measurement: Option<Measurement>,
    pub native_speed: Option<NativeSpeed>,
}
#[derive(Clone, Debug, Serialize)]
pub struct NativeSpeed {
    pub tok_s: f64,
    pub tokenizer: Digest,
    pub age_ms: u64,
    pub valid_for_ms: u64,
}
impl Snapshot {
    pub fn meets_native_floor(&self, min_tok_s: u32) -> bool {
        min_tok_s > 0
            && self.allowance > 0
            && self.native_speed.as_ref().is_some_and(|speed| {
                speed.age_ms < speed.valid_for_ms && speed.tok_s >= f64::from(min_tok_s)
            })
    }
}
struct Gate {
    generation: u64,
    allowance: u32,
    state: State,
    reason: Reason,
    observed: Option<Instant>,
    retry_start: Instant,
    retry_ms: u64,
    failures: u32,
    successes: u32,
    slow: u32,
    performance_class: Option<Class>,
}
impl Gate {
    fn new(now: Instant) -> Self {
        Self {
            generation: 0,
            allowance: 0,
            state: State::Checking,
            reason: Reason::NoEvidence,
            observed: None,
            retry_start: now,
            retry_ms: 0,
            failures: 0,
            successes: 0,
            slow: 0,
            performance_class: None,
        }
    }
    fn fresh(&self, now: Instant, ttl: u64) -> bool {
        self.observed
            .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(ttl))
    }
    fn block(&mut self, reason: Reason, now: Instant, delay: u64) {
        self.generation = self.generation.saturating_add(1);
        self.allowance = 0;
        self.reason = reason;
        self.observed = Some(now);
        self.successes = 0;
        self.failures = self.failures.saturating_add(1);
        self.retry_start = now;
        self.retry_ms = delay;
        self.performance_class = None;
        self.state = match reason {
            Reason::Busy | Reason::RateLimited => State::Busy,
            Reason::SlowGeneration | Reason::SlowResponse => State::Degraded,
            _ => State::Unavailable,
        };
    }
    fn degrade(&mut self, now: Instant, class: Class) {
        self.generation = self.generation.saturating_add(1);
        self.allowance = (self.allowance / 2).max(1);
        self.state = State::Degraded;
        self.reason = Reason::SlowResponse;
        self.observed = Some(now);
        self.successes = 0;
        self.performance_class = Some(class);
    }
    fn good(&mut self, now: Instant, ceiling: u32, policy: &Policy) {
        if !self.fresh(now, policy.evidence_ttl_ms) {
            self.allowance = 0;
            self.successes = 0;
        }
        if self.allowance == 0 {
            self.allowance = 1;
            self.successes = 0
        } else {
            self.successes = self.successes.saturating_add(1);
            if self.successes >= policy.successes_to_increase {
                self.allowance = self.allowance.saturating_add(1).min(ceiling);
                self.successes = 0;
                self.failures = self.failures.saturating_sub(1);
            }
        }
        self.state = State::Ready;
        self.reason = Reason::Fresh;
        self.observed = Some(now);
        self.retry_start = now;
        self.retry_ms = 0;
        self.slow = 0;
        self.performance_class = None;
    }
}
struct Baseline {
    mean_ms: u64,
    samples: u32,
    last: Instant,
    bad: u32,
}
struct Route {
    ceiling: u32,
    llm: bool,
    gate: Gate,
    classes: BTreeMap<Class, Baseline>,
    last: Option<Measurement>,
    native: Option<(Instant, f64, Digest)>,
}
struct Data {
    routes: BTreeMap<Digest, Route>,
    connection: Gate,
    schedule: Schedule,
    recovery: Option<u64>,
    sequence: u64,
    revision: u64,
}
struct Inner {
    policy: Policy,
    connection_ceiling: u32,
    data: Mutex<Data>,
}
/// One monitor per declared connection/credential scope, shared across aliases.
/// Physical shared-backend allocation remains in capacity::Authority.
#[derive(Clone)]
pub struct Monitor {
    inner: Arc<Inner>,
}
impl Monitor {
    pub fn new(policy: Policy, connection_ceiling: u32, seed: u64) -> Result<Self> {
        policy.validate()?;
        if connection_ceiling == 0 {
            return Err(Error::Invalid);
        }
        let schedule = Schedule::new(policy.recovery.clone(), seed);
        Ok(Self {
            inner: Arc::new(Inner {
                policy,
                connection_ceiling,
                data: Mutex::new(Data {
                    routes: BTreeMap::new(),
                    connection: Gate::new(Instant::now()),
                    schedule,
                    recovery: None,
                    sequence: 0,
                    revision: 0,
                }),
            }),
        })
    }
    pub fn register(&self, id: Digest, ceiling: u32, llm: bool) -> Result<()> {
        if ceiling == 0 || ceiling > self.inner.connection_ceiling {
            return Err(Error::Invalid);
        }
        let mut data = self.inner.data.lock().map_err(|_| Error::Unavailable)?;
        if let Some(route) = data.routes.get(&id) {
            return if route.ceiling == ceiling && route.llm == llm {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        if data.routes.len() >= self.inner.policy.max_routes {
            return Err(Error::Invalid);
        }
        data.routes.insert(
            id,
            Route {
                ceiling,
                llm,
                gate: Gate::new(Instant::now()),
                classes: BTreeMap::new(),
                last: None,
                native: None,
            },
        );
        data.revision = data.revision.saturating_add(1);
        Ok(())
    }
    pub fn snapshot(&self, id: &Digest) -> Result<Snapshot> {
        let data = self.inner.data.lock().map_err(|_| Error::Unavailable)?;
        snapshot(&data, id, &self.inner.policy, Instant::now())
    }
    /// Passive observation of an independently admitted request. This cannot
    /// bypass the durable capacity/financial admission checks of its caller.
    pub fn observe_request(&self, id: &Digest, class: Class) -> Result<Sample> {
        self.sample(id, class, false)
    }
    /// At most one observation per shared scope. A permit is NOT authority to
    /// launch paid traffic: the caller needs independent durable probe admission,
    /// operator funding and safe same-job recovery before making any request.
    pub fn observe_recovery(&self, id: &Digest, class: Class) -> Result<Sample> {
        self.sample(id, class, true)
    }
    fn sample(&self, id: &Digest, class: Class, recovery: bool) -> Result<Sample> {
        if class.input_bytes_log2 > usize::BITS as u8 {
            return Err(Error::Invalid);
        }
        let mut data = self.inner.data.lock().map_err(|_| Error::Unavailable)?;
        let now = Instant::now();
        let view = snapshot(&data, id, &self.inner.policy, now)?;
        if recovery && (data.recovery.is_some() || view.recovery_after_ms > 0 || view.allowance > 0)
        {
            return Err(Error::RecoveryBusy);
        }
        let route = data.routes.get(id).ok_or(Error::Invalid)?;
        let generation = (data.connection.generation, route.gate.generation);
        data.sequence = data.sequence.checked_add(1).ok_or(Error::Unavailable)?;
        let sequence = data.sequence;
        if recovery {
            data.recovery = Some(sequence)
        }
        Ok(Sample::new(
            self.clone(),
            id.clone(),
            class,
            generation,
            sequence,
            recovery,
            now,
        ))
    }
}
fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}
fn snapshot(data: &Data, id: &Digest, policy: &Policy, now: Instant) -> Result<Snapshot> {
    let route = data.routes.get(id).ok_or(Error::Invalid)?;
    let group = &data.connection;
    let own = &route.gate;
    let fresh_fault = |gate: &Gate| {
        gate.fresh(now, policy.evidence_ttl_ms)
            && matches!(
                gate.state,
                State::Busy | State::Unavailable | State::Degraded
            )
    };
    // A first failed model request is useful evidence even before any shared
    // connection success. Unknown group health must not hide that actual fault.
    let gate = if fresh_fault(group) {
        group
    } else if fresh_fault(own) {
        own
    } else if group.state != State::Ready || !group.fresh(now, policy.evidence_ttl_ms) {
        group
    } else {
        own
    };
    let fresh = gate.fresh(now, policy.evidence_ttl_ms);
    let all_fresh =
        group.fresh(now, policy.evidence_ttl_ms) && own.fresh(now, policy.evidence_ttl_ms);
    let ready = group.state == State::Ready
        && matches!(own.state, State::Ready | State::Degraded)
        && own.allowance > 0
        && all_fresh;
    let (state, reason) = if ready {
        (own.state, own.reason)
    } else if gate.state == State::Ready || !fresh {
        (
            State::Checking,
            if gate.observed.is_none() {
                Reason::NoEvidence
            } else {
                Reason::Stale
            },
        )
    } else {
        (gate.state, gate.reason)
    };
    let retry = group
        .retry_ms
        .saturating_sub(millis(now.saturating_duration_since(group.retry_start)))
        .max(
            own.retry_ms
                .saturating_sub(millis(now.saturating_duration_since(own.retry_start))),
        );
    Ok(Snapshot {
        state,
        reason,
        allowance: if ready {
            group.allowance.min(own.allowance)
        } else {
            0
        },
        evidence_age_ms: match (group.observed, own.observed) {
            (Some(a), Some(b)) => Some(millis(now.saturating_duration_since(a.min(b)))),
            _ => None,
        },
        recovery_after_ms: retry,
        recovery_in_progress: data.recovery.is_some(),
        last_measurement: route.last.clone(),
        native_speed: route
            .native
            .as_ref()
            .map(|(at, rate, tokenizer)| NativeSpeed {
                tok_s: *rate,
                tokenizer: tokenizer.clone(),
                age_ms: millis(now.saturating_duration_since(*at)),
                valid_for_ms: policy.evidence_ttl_ms,
            }),
    })
}

enum Outcome {
    Success,
    Failure(Failure),
    Abandoned,
}
impl Monitor {
    fn finish(&self, sample: &Sample, measurement: Measurement, outcome: Outcome) {
        let Ok(mut data) = self.inner.data.lock() else {
            return;
        };
        if sample.recovery && data.recovery == Some(sample.sequence) {
            data.recovery = None
        }
        let now = Instant::now();
        let policy = &self.inner.policy;
        let Some(route) = data.routes.get(&sample.route) else {
            return;
        };
        if matches!(&outcome, Outcome::Success)
            && sample.generation != (data.connection.generation, route.gate.generation)
        {
            return;
        }
        match outcome {
            Outcome::Abandoned => (), // Caller cancellation/storage/display failures are not upstream faults.
            Outcome::Failure(failure) => {
                let Some(reason) = fault(&failure) else {
                    return;
                };
                let connection = failure.scope == Scope::Connection;
                let failures = if connection {
                    data.connection.failures
                } else {
                    route.gate.failures
                };
                data.revision = data.revision.saturating_add(1);
                let delay = data
                    .schedule
                    .retry(failures.saturating_add(1))
                    .max(failure.retry_after_ms.unwrap_or(0));
                if connection {
                    data.connection.block(reason, now, delay)
                } else if let Some(route) = data.routes.get_mut(&sample.route) {
                    route.gate.block(reason, now, delay)
                }
            }
            Outcome::Success => {
                data.revision = data.revision.saturating_add(1);
                let mut slow_reason = None;
                let route = data.routes.get_mut(&sample.route).expect("route checked");
                if route.llm {
                    if let Some(rate) = measurement.native_tok_s {
                        if rate < f64::from(policy.min_native_tok_s) {
                            route.gate.slow = route.gate.slow.saturating_add(1);
                            if route.gate.slow >= policy.bad_samples_to_reduce {
                                slow_reason = Some(Reason::SlowGeneration)
                            }
                        } else {
                            route.gate.slow = 0
                        }
                    }
                }
                let latency = measurement.first_output_ms.unwrap_or(measurement.total_ms);
                let mut latency_bad = false;
                if let Some(baseline) = route.classes.get_mut(&sample.class) {
                    if millis(now.saturating_duration_since(baseline.last))
                        >= policy.evidence_ttl_ms
                    {
                        *baseline = Baseline {
                            mean_ms: latency,
                            samples: 1,
                            last: now,
                            bad: 0,
                        };
                    } else {
                        latency_bad = baseline.samples >= policy.latency_baseline_samples
                            && latency
                                > baseline
                                    .mean_ms
                                    .saturating_mul(u64::from(policy.latency_multiplier))
                            && latency.saturating_sub(baseline.mean_ms)
                                >= policy.latency_increase_ms;
                        if latency_bad {
                            baseline.bad = baseline.bad.saturating_add(1)
                        } else {
                            baseline.bad = 0;
                            baseline.mean_ms =
                                baseline.mean_ms.saturating_mul(7).saturating_add(latency) / 8;
                            baseline.samples = baseline.samples.saturating_add(1);
                            baseline.last = now;
                        }
                        if baseline.bad >= policy.bad_samples_to_reduce && slow_reason.is_none() {
                            slow_reason = Some(Reason::SlowResponse)
                        }
                    }
                } else {
                    if route.classes.len() >= policy.max_classes_per_route {
                        let oldest = route
                            .classes
                            .iter()
                            .min_by_key(|(_, v)| v.last)
                            .map(|(k, _)| *k);
                        if let Some(key) = oldest {
                            route.classes.remove(&key);
                        }
                    }
                    route.classes.insert(
                        sample.class,
                        Baseline {
                            mean_ms: latency,
                            samples: 1,
                            last: now,
                            bad: 0,
                        },
                    );
                }
                let was_slow = matches!(route.gate.reason, Reason::SlowGeneration);
                let recovery_class_matches = route
                    .gate
                    .performance_class
                    .is_none_or(|class| class == sample.class);
                let speed_recovery = measurement
                    .native_tok_s
                    .is_some_and(|rate| rate >= f64::from(policy.min_native_tok_s));
                route.last = Some(measurement.clone());
                if let (Some(rate), Some(tokenizer)) =
                    (measurement.native_tok_s, &measurement.tokenizer)
                {
                    route.native = Some((now, rate, tokenizer.clone()));
                }
                if let Some(reason) = slow_reason {
                    if reason == Reason::SlowResponse {
                        route.gate.degrade(now, sample.class);
                        data.connection
                            .good(now, self.inner.connection_ceiling, policy);
                        return;
                    }
                    let failures = route.gate.failures;
                    let delay = data.schedule.retry(failures.saturating_add(1));
                    let route = data.routes.get_mut(&sample.route).expect("route checked");
                    route.gate.block(reason, now, delay);
                    route.gate.performance_class = Some(sample.class);
                } else if recovery_class_matches && (!was_slow || speed_recovery) && !latency_bad {
                    let route = data.routes.get_mut(&sample.route).expect("route checked");
                    // A first weak/slow sample cannot grow allowance. Keep the
                    // existing allowance while gathering a second observation.
                    let slow = route.gate.slow;
                    if slow == 0 {
                        route.gate.good(now, route.ceiling, policy)
                    } else {
                        route.gate.observed = Some(now)
                    }
                    data.connection
                        .good(now, self.inner.connection_ceiling, policy);
                }
            }
        }
    }
}
fn fault(f: &Failure) -> Option<Reason> {
    if f.scope == Scope::Request {
        return None;
    }
    Some(match f.code {
        Code::UpstreamBusy => Reason::Busy,
        Code::UpstreamRateLimited => Reason::RateLimited,
        Code::UpstreamAuthentication => Reason::Authentication,
        Code::UpstreamPaymentRequired => Reason::PaymentRequired,
        Code::UpstreamModelUnavailable | Code::UpstreamEndpointUnavailable => {
            Reason::ModelUnavailable
        }
        Code::UpstreamUnavailable | Code::UpstreamTimeout => Reason::Unreachable,
        Code::UpstreamProtocol => Reason::InvalidResponse,
        _ => return None,
    })
}
