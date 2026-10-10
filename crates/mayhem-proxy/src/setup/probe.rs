//! One explicit operator probe through the real HTTP, decoder and capacity path.
//! No financial authority is created. The same shared capacity file must be used
//! by every alias and by subsequent serving; a new file is never a retry strategy.
use super::*;
use crate::{
    capacity::{self, probes::Budget},
    connector::http::HttpConnection,
    execution::probes,
    health,
    supervisor::RefreshPolicy,
    worker::host::{Pool, PoolLimits},
};
use std::{sync::Arc, time::Duration};

const REQUEST_BYTES: usize = 16 * 1024;
const RESPONSE_BYTES: usize = 64 * 1024;

/// Private, stable capacity ownership. The operator must include every shared
/// physical/credential constraint used by the serving routes. First use pins
/// this complete scope; draft updates and allowance renewals cannot replace it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeScope {
    pub capacity_file: PathBuf,
    pub route: Digest,
    pub connection_group: Digest,
    pub connection_ceiling: u32,
    pub route_ceiling: u32,
    pub constraints: Vec<ProbeGroup>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeGroup {
    pub id: Digest,
    pub ceiling: u32,
}
impl ProbeScope {
    pub(super) fn validate(&self) -> Result<()> {
        require(
            self.capacity_file.is_absolute()
                && self.capacity_file.file_name().is_some()
                && self.connection_ceiling > 0
                && self.route_ceiling > 0
                && self.route_ceiling <= self.connection_ceiling
                && self.constraints.len() <= 8
                && self
                    .constraints
                    .iter()
                    .all(|g| g.ceiling > 0 && g.id != self.connection_group)
                && self.constraints.windows(2).all(|v| v[0].id < v[1].id),
        )
    }
    fn canonicalize(&mut self) -> Result<()> {
        self.validate()?;
        let parent = self.capacity_file.parent().ok_or(Error::Protection)?;
        protected_directory(parent)?;
        let parent = std::fs::canonicalize(parent).map_err(|_| Error::Protection)?;
        self.capacity_file = parent.join(self.capacity_file.file_name().ok_or(Error::Protection)?);
        Ok(())
    }
    fn open(&self, record: &Record, existing: bool) -> Result<Arc<capacity::Authority>> {
        protected_directory(self.capacity_file.parent().ok_or(Error::Protection)?)?;
        // Once pinned, deleting the authority must not replenish consumed budget
        // or erase uncertain work. Authority::open also protects/locks the file.
        if existing && std::fs::symlink_metadata(&self.capacity_file).is_err() {
            return Err(Error::ProbeRecovery);
        }
        let identity = crate::attempts::Identity {
            network_id: record.input.network.network_id.clone(),
            msb_bootstrap: Digest::new(record.input.network.msb_bootstrap.clone())
                .map_err(|_| Error::Invalid)?,
            subnet_bootstrap: Digest::new(record.input.network.subnet_bootstrap.clone())
                .map_err(|_| Error::Invalid)?,
            controller_pubkey: record.input.provider_pubkey.clone(),
        };
        capacity::Authority::open(
            &self.capacity_file,
            identity,
            capacity::Limits {
                max_groups: 1024,
                max_routes: 4096,
                max_leases: 65536,
                max_evidence_age: Duration::from_secs(60),
            },
        )
        .map(Arc::new)
        .map_err(|_| Error::ProbeCapacity)
    }
    fn retained(&self, authority: &capacity::Authority) -> Result<Option<capacity::probes::Probe>> {
        // Include overlapping physical pools, not only this connection alias.
        for group in
            std::iter::once(&self.connection_group).chain(self.constraints.iter().map(|g| &g.id))
        {
            if let Some(probe) = authority
                .probe_for_group(group)
                .map_err(|_| Error::ProbeCapacity)?
            {
                return Ok(Some(probe));
            }
        }
        Ok(None)
    }
}

/// Explicit private authorization for one attempt. Budget is cumulative in the
/// shared authority, not a per-command allowance or an exact upstream price.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbePlan {
    pub schema_version: u32,
    pub scope: ProbeScope,
    pub budget: Budget,
    pub worker_program: PathBuf,
    pub worker_directory: PathBuf,
    pub request: serde_json::Value,
    pub streaming: bool,
    pub max_output_tokens: u64,
    pub timeout_ms: u64,
}
impl ProbePlan {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = private_file(path, MAX_BYTES).map_err(|_| Error::Protection)?;
        let mut plan: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        let parent = std::fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|_| Error::Protection)?;
        for reference in [
            &mut plan.scope.capacity_file,
            &mut plan.worker_program,
            &mut plan.worker_directory,
        ] {
            if reference.is_relative() {
                *reference = parent.join(&reference);
            }
        }
        plan.validate()?;
        Ok(plan)
    }
    fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        require(
            self.schema_version == 1
                && self.worker_program.is_absolute()
                && self.worker_directory.is_absolute()
                && self.request.is_object()
                && serde_json::to_vec(&self.request)
                    .map_err(|_| Error::Invalid)?
                    .len()
                    <= REQUEST_BYTES
                && (1..=1024).contains(&self.max_output_tokens)
                && (1..=30_000).contains(&self.timeout_ms)
                && self.budget.max_attempts > 0
                && self.budget.max_attempts <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
                && self.budget.max_cost_microusd <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
                && self.budget.per_attempt_cost_microusd <= self.budget.max_cost_microusd,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Pending,
    Validated,
    NotValidated,
    Uncertain,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationBinding {
    schema_version: u32,
    digest: Digest,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    binding: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    configuration_binding: Option<ConfigurationBinding>,
    specification: capacity::probes::Specification,
    state: ProbeState,
    probe_id: Option<Digest>,
    evidence_hash: Option<Digest>,
    #[serde(default)]
    reservation_intent: Option<capacity::probes::ReservationIntent>,
}
impl Attempt {
    pub(super) fn validate(&self) -> Result<()> {
        require(
            self.configuration_binding
                .as_ref()
                .is_none_or(|v| v.schema_version == 1),
        )?;
        if let Some(intent) = &self.reservation_intent {
            require(intent.expected_used_attempts < u64::MAX && self.probe_id.is_some())?;
        }
        require(
            self.state != ProbeState::Validated
                || (self.probe_id.is_some() && self.evidence_hash.is_some()),
        )
    }
    pub(super) fn report(&self, record: &Record) -> Result<ProbeReport> {
        Ok(ProbeReport {
            state: self.state,
            for_current_configuration: match &self.configuration_binding {
                Some(binding) => binding.digest == record.probe_configuration_binding()?,
                None => self.binding == record.binding()?,
            } && record
                .input
                .connection()
                .is_ok_and(|v| v == record.connection),
            probe_id: self.probe_id.clone(),
            evidence_hash: self.evidence_hash.clone(),
            native_throughput: "not_verified",
            recovery_reason: match (self.state, self.probe_id.is_some()) {
                (ProbeState::Pending | ProbeState::Uncertain, false) => {
                    Some("legacy_probe_identity_unavailable")
                }
                (ProbeState::Pending | ProbeState::Uncertain, true) => {
                    Some("retained_capacity_requires_reconciliation")
                }
                _ => None,
            },
        })
    }
}

impl Record {
    /// Bind the entire observed protocol/configuration, independently of prices
    /// and publication counters. Deliberately conservative: any capability
    /// change invalidates the whole observation, not only a feature subset.
    fn probe_configuration_binding(&self) -> Result<Digest> {
        // Exhaustive destructuring makes additions to these input types require
        // an explicit decision here, rather than silently escaping the binding.
        let Input {
            schema_version,
            network,
            provider_pubkey,
            connection_file: _,
            adapter,
            market,
            membership,
            offers,
            selection: _,
            sequence: _,
            settlement_policy: _,
        } = &self.input;
        let mayhem_proto::proxy::ProxyMembership {
            schema_version: membership_version,
            lane,
            market_id,
            provider_pubkey: member_provider,
            revision: _,
            endpoints,
            served_context,
            max_concurrency,
            recipe_hash,
            connection_revision,
            capacity_group,
            accepted_rails: _,
        } = membership;
        let mut slots = Vec::with_capacity(offers.len());
        for offer in offers {
            let mayhem_proto::proxy::ProxyOffer {
                schema_version,
                lane,
                market_id,
                provider_pubkey,
                membership_revision: _,
                revision: _,
                endpoint,
                ctx_bracket,
                outcome_class,
                metering_policy_hash,
                rates: _,
                per_request_au: _,
                min_session_au: _,
                accepted_rails: _,
            } = offer;
            slots.push((
                *endpoint,
                ctx_bracket,
                outcome_class,
                serde_json::json!({
                    "schema_version":schema_version,"lane":lane,"market_id":market_id,
                    "provider_pubkey":provider_pubkey,"endpoint":endpoint,"ctx_bracket":ctx_bracket,
                    "outcome_class":outcome_class,"metering_policy_hash":metering_policy_hash,
                }),
            ));
        }
        slots.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
        let slots: Vec<_> = slots.into_iter().map(|(_, _, _, slot)| slot).collect();
        let scope = self.probe_scope.as_ref().ok_or(Error::Invalid)?;
        let bytes = mayhem_proto::stable_json_bytes(&serde_json::json!({
            "schema_version":schema_version,"network":network,"provider_pubkey":provider_pubkey,
            "market":market,"adapter":adapter,"connection":self.connection,
            "membership":{"schema_version":membership_version,"lane":lane,"market_id":market_id,
                "provider_pubkey":member_provider,"endpoints":endpoints,"served_context":served_context,
                "max_concurrency":max_concurrency,"recipe_hash":recipe_hash,
                "connection_revision":connection_revision,"capacity_group":capacity_group},
            "offer_slots":slots,"probe_scope":scope,
        })).map_err(|_| Error::Invalid)?;
        Ok(Digest::hash(
            "mayhem/proxy/setup-probe-configuration/v1",
            &[&bytes],
        ))
    }

    /// Upgrade only a still-valid legacy success before changing its original
    /// declaration. Lost/stale/uncertain evidence cannot acquire a new binding.
    pub(super) fn retain_probe_configuration(&mut self) -> Result<()> {
        let Some(attempt) = &self.probe else {
            return Ok(());
        };
        if attempt.configuration_binding.is_some()
            || attempt.state != ProbeState::Validated
            || attempt.binding != self.binding()?
            || !self.input.connection().is_ok_and(|v| v == self.connection)
        {
            return Ok(());
        }
        let digest = self.probe_configuration_binding()?;
        self.probe
            .as_mut()
            .ok_or(Error::Invalid)?
            .configuration_binding = Some(ConfigurationBinding {
            schema_version: 1,
            digest,
        });
        Ok(())
    }
}
/// This is a local controller observation, not a public conformance certificate.
/// No request, upstream reply, path, fingerprint, credential or local limit leaks.
#[derive(Serialize)]
pub struct ProbeReport {
    pub state: ProbeState,
    pub for_current_configuration: bool,
    pub probe_id: Option<Digest>,
    pub evidence_hash: Option<Digest>,
    pub native_throughput: &'static str,
    pub recovery_reason: Option<&'static str>,
}
impl ProbeReport {
    pub(super) fn status_name(&self) -> &'static str {
        if matches!(self.state, ProbeState::Pending | ProbeState::Uncertain) {
            "recovery_required"
        } else if !self.for_current_configuration {
            "recheck_required"
        } else if self.state == ProbeState::Validated {
            "protocol_validated"
        } else {
            "not_validated"
        }
    }
}

struct Prepared {
    record: Record,
    authority: Arc<capacity::Authority>,
    controller: probes::Controller,
    deadline: Duration,
    // Drop the authority/controller before unlocking the draft, so a resumed
    // command cannot race their ordinary database teardown after cancellation.
    guard: store::Guard,
}
impl Store {
    /// Exactly one explicit attempt, no background polling or automatic retry.
    /// The draft lock remains held until its final report is durable. Dropping
    /// this future leaves the original pending intent available to recovery.
    pub async fn probe(&self, expected_revision: u64, plan: ProbePlan) -> Result<Review> {
        let directory = self.directory.clone();
        let prepared =
            tokio::task::spawn_blocking(move || prepare(directory, expected_revision, plan))
                .await
                .map_err(|_| Error::Storage)??;
        let result = tokio::time::timeout(prepared.deadline, prepared.controller.run()).await;
        tokio::task::spawn_blocking(move || prepared.finish(result))
            .await
            .map_err(|_| Error::Storage)?
    }

    /// Bounded read/reconciliation of the original intent. It never dispatches,
    /// reloads credentials or frees unknown work. Only Prepared has durable proof
    /// that no dispatch permit existed and can be cancelled through Authority.
    pub fn recover_probe(&self, expected_revision: u64) -> Result<Review> {
        let guard = store::Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        record.next(expected_revision)?;
        let scope = record.probe_scope.as_ref().ok_or(Error::Invalid)?;
        let authority = scope.open(&record, true)?;
        let attempt = record.probe.as_mut().ok_or(Error::Invalid)?;
        if let Some(intent) = &attempt.reservation_intent {
            let id = authority
                .probe_intent_id(&attempt.specification, intent)
                .map_err(|_| Error::Invalid)?;
            require(attempt.probe_id.as_ref() == Some(&id))?;
        }
        if matches!(attempt.state, ProbeState::Pending | ProbeState::Uncertain) {
            if let Some(probe) = scope.retained(&authority)? {
                // A foreign alias's work is never ours to cancel or adopt.
                if probe.specification == attempt.specification
                    && attempt.probe_id.as_ref() == Some(&probe.id)
                {
                    if probe.phase == capacity::probes::ProbePhase::Prepared {
                        authority
                            .cancel_prepared_probe(&probe.id)
                            .map_err(|_| Error::ProbeCapacity)?;
                        attempt.state = ProbeState::NotValidated;
                    } else {
                        attempt.state = ProbeState::Uncertain;
                    }
                } else {
                    // Matching content/configuration does not identify an
                    // invocation. A lost ID cannot adopt or cancel a later
                    // identical probe, even if it is still Prepared.
                    attempt.state = ProbeState::Uncertain;
                }
            } else {
                // Even a retained completion digest cannot prove that a lost
                // Controller result was success rather than verified rejection.
                attempt.state = ProbeState::NotValidated;
            }
            attempt.evidence_hash = None;
        }
        guard.write(&record)?;
        record.review()
    }
}

fn prepare(directory: PathBuf, expected: u64, mut plan: ProbePlan) -> Result<Prepared> {
    plan.validate()?;
    plan.scope.canonicalize()?;
    let guard = store::Guard::open(&directory)?;
    let mut record = guard.read()?.ok_or(Error::Missing)?;
    record.next(expected)?;
    if record
        .probe
        .as_ref()
        .is_some_and(|p| matches!(p.state, ProbeState::Pending | ProbeState::Uncertain))
    {
        return Err(Error::ProbeRecovery);
    }
    if record.checked.as_ref() != Some(&record.binding()?) {
        return Err(Error::Invalid);
    }
    if record.input.connection()? != record.connection {
        return Err(Error::ConnectionChanged);
    }
    require(plan.scope.connection_group.as_str() == record.input.membership.capacity_group)?;
    if let Some(scope) = &record.probe_scope {
        require(scope == &plan.scope)?;
    }
    let adapter =
        Arc::new(Adapter::restore(record.input.adapter.clone()).map_err(|_| Error::Invalid)?);
    let config =
        ConnectionConfig::load(&record.input.connection_file).map_err(|_| Error::Protection)?;
    if config.fingerprint().map_err(|_| Error::Invalid)? != record.connection {
        return Err(Error::ConnectionChanged);
    }
    let request_limit = REQUEST_BYTES
        .min(config.limits.max_request_bytes)
        .min(adapter.limits().request_bytes);
    let response_limit = RESPONSE_BYTES
        .min(config.limits.max_response_bytes)
        .min(adapter.limits().response_bytes);
    let body = serde_json::to_vec(&plan.request).map_err(|_| Error::Invalid)?;
    require(body.len() <= request_limit)?;
    let request = if plan.streaming {
        adapter.prepare_stream(&body)
    } else {
        adapter.prepare_json(&body)
    }
    .map_err(|_| Error::Invalid)?;
    let specification = capacity::probes::Specification {
        route: plan.scope.route.clone(),
        budget_group: plan.scope.connection_group.clone(),
        request_hash: request.request_hash().clone(),
        connection_digest: record.connection.clone(),
        connection_revision: config.revision,
        recipe_digest: adapter.recipe_hash().clone(),
    };
    let pool = Arc::new(
        Pool::new(
            &plan.worker_program,
            &plan.worker_directory,
            PoolLimits {
                max_children: 1,
                max_buffer_bytes: 4 * 1024 * 1024,
                startup_timeout: Duration::from_secs(5),
                processing_timeout: Duration::from_secs(3),
            },
        )
        .map_err(|_| Error::Protection)?,
    );
    let authority = plan.scope.open(&record, record.probe_scope.is_some())?;
    if plan.scope.retained(&authority)?.is_some() {
        return Err(Error::ProbeRecovery);
    }
    authority
        .configure_group(
            plan.scope.connection_group.clone(),
            plan.scope.connection_ceiling,
        )
        .map_err(|_| Error::ProbeCapacity)?;
    for group in &plan.scope.constraints {
        authority
            .configure_allocation_group(group.id.clone(), group.ceiling)
            .map_err(|_| Error::ProbeCapacity)?;
    }
    authority
        .configure_route_with_constraints(
            capacity::Route {
                id: plan.scope.route.clone(),
                group: plan.scope.connection_group.clone(),
                lane: capacity::Lane::Proxy,
                max_concurrency: plan.scope.route_ceiling,
            },
            plan.scope
                .constraints
                .iter()
                .map(|g| g.id.clone())
                .collect(),
        )
        .map_err(|_| Error::ProbeCapacity)?;
    authority
        .configure_probe_budget(&plan.scope.connection_group, plan.budget)
        .map_err(|_| Error::ProbeCapacity)?;
    let monitor = monitor(&plan.scope, adapter.endpoint())?;
    authority
        .bind_live(
            capacity::Scope::Group(plan.scope.connection_group.clone()),
            monitor.connection_source(),
        )
        .map_err(|_| Error::ProbeCapacity)?;
    authority
        .bind_live(
            capacity::Scope::Route(plan.scope.route.clone()),
            monitor
                .route_source(&plan.scope.route)
                .map_err(|_| Error::Invalid)?,
        )
        .map_err(|_| Error::ProbeCapacity)?;
    let connection = Arc::new(HttpConnection::new(config).map_err(|_| Error::Protection)?);
    let budget = authority
        .probe_budget(&plan.scope.connection_group)
        .map_err(|_| Error::ProbeCapacity)?
        .ok_or(Error::ProbeCapacity)?;
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
    let reservation_intent = capacity::probes::ReservationIntent {
        nonce: Digest::hash(
            "mayhem/proxy/setup-probe-nonce/v1",
            &[record.id.as_str().as_bytes(), &nonce],
        ),
        expected_used_attempts: budget.used_attempts,
    };
    let probe_id = authority
        .probe_intent_id(&specification, &reservation_intent)
        .map_err(|_| Error::ProbeCapacity)?;
    let controller = probes::Controller::new(
        connection,
        adapter,
        pool,
        authority.clone(),
        monitor,
        plan.scope.route.clone(),
        plan.scope.connection_group.clone(),
        &body,
        plan.streaming,
        probes::Limits {
            max_request_bytes: request_limit,
            max_response_bytes: response_limit,
            max_output_tokens: plan.max_output_tokens,
            timeout: Duration::from_millis(plan.timeout_ms),
            storage_workers: 1,
        },
    )
    .and_then(|controller| controller.with_reservation_intent(reservation_intent.clone()))
    .map_err(|_| Error::Invalid)?;
    record.probe_scope = Some(plan.scope);
    record.probe = Some(Attempt {
        binding: record.binding()?,
        configuration_binding: Some(ConfigurationBinding {
            schema_version: 1,
            digest: record.probe_configuration_binding()?,
        }),
        specification,
        state: ProbeState::Pending,
        probe_id: Some(probe_id),
        evidence_hash: None,
        reservation_intent: Some(reservation_intent),
    });
    guard.write(&record)?;
    Ok(Prepared {
        guard,
        record,
        authority,
        controller,
        deadline: Duration::from_millis(plan.timeout_ms + 10_000),
    })
}
impl Prepared {
    fn finish(
        mut self,
        result: std::result::Result<
            std::result::Result<probes::Outcome, probes::ProbeError>,
            tokio::time::error::Elapsed,
        >,
    ) -> Result<Review> {
        let attempt = self.record.probe.as_mut().ok_or(Error::Invalid)?;
        match result {
            Ok(Ok(outcome)) => {
                // The controller may only validate the identity saved before
                // reserve; never adopt a different result by matching content.
                require(attempt.probe_id.as_ref() == Some(&outcome.probe))?;
                attempt.state = ProbeState::Validated;
                attempt.evidence_hash = Some(outcome.evidence);
            }
            Ok(Err(_)) => {
                if self
                    .record
                    .probe_scope
                    .as_ref()
                    .ok_or(Error::Invalid)?
                    .retained(&self.authority)?
                    .is_some()
                {
                    attempt.state = ProbeState::Uncertain;
                } else {
                    attempt.state = ProbeState::NotValidated;
                }
            }
            Err(_) => attempt.state = ProbeState::Uncertain,
        }
        self.record.next(self.record.revision)?;
        self.guard.write(&self.record)?;
        self.record.review()
    }
}
fn monitor(
    scope: &ProbeScope,
    endpoint: mayhem_proto::proxy::ProxyEndpoint,
) -> Result<health::Monitor> {
    let monitor = health::Monitor::new(
        health::Policy {
            max_routes: 1,
            max_classes_per_route: 8,
            evidence_ttl_ms: 60_000,
            successes_to_increase: 2,
            bad_samples_to_reduce: 2,
            latency_baseline_samples: 3,
            latency_multiplier: 4,
            latency_increase_ms: 1000,
            min_native_tok_s: 5,
            recovery: RefreshPolicy {
                interval_ms: 10_000,
                page_pause_ms: 1,
                retry_initial_ms: 10,
                retry_max_ms: 100,
                jitter_percent: 0,
            },
        },
        scope.connection_ceiling,
        1,
    )
    .map_err(|_| Error::Invalid)?;
    monitor
        .register(
            scope.route.clone(),
            scope.route_ceiling,
            endpoint != mayhem_proto::proxy::ProxyEndpoint::Decisions,
        )
        .map_err(|_| Error::Invalid)?;
    Ok(monitor)
}
#[cfg(unix)]
fn protected_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::Protection)?;
    if metadata.is_dir()
        && metadata.mode() & 0o077 == 0
        && metadata.uid() == rustix::process::geteuid().as_raw()
    {
        Ok(())
    } else {
        Err(Error::Protection)
    }
}
#[cfg(windows)]
fn protected_directory(path: &Path) -> Result<()> {
    mayhem_windows_sandbox::validate_private_directory(path).map_err(|_| Error::Protection)
}
#[cfg(not(any(unix, windows)))]
fn protected_directory(_: &Path) -> Result<()> {
    Err(Error::Protection)
}
