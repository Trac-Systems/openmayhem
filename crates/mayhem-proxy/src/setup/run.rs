//! Exact published-draft handoff to an injected host-owned lifecycle controller.
//! Retain before dispatch; one immutable runtime configuration and child identity.
use super::*;
use crate::{attempts, managed};
use serde_json::json;
use std::{future::Future, pin::Pin};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSettings {
    pub template: PathBuf,
    pub wallet_password_file: Option<PathBuf>,
}
/// Private operator policy, with no offer/recipe placeholders or implicit budgets.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTemplate {
    pub schema_version: u32,
    pub bridge: managed::Bridge,
    pub health: crate::health::Policy,
    pub limits: managed::Limits,
    pub tokenizer: Option<managed::Tokenizer>,
    pub allow_recovery_probes: bool,
}
impl RunTemplate {
    pub fn load(path: &Path) -> Result<Self> {
        let data = private_file(path, 64 * 1024).map_err(|_| Error::Protection)?;
        let mut v: Self = serde_json::from_slice(&data).map_err(|_| Error::Invalid)?;
        require(v.schema_version == 1)?;
        let base = std::fs::canonicalize(path.parent().ok_or(Error::Protection)?)
            .map_err(|_| Error::Protection)?;
        if v.bridge.token_file.is_relative() {
            v.bridge.token_file = base.join(&v.bridge.token_file)
        }
        if let Some(pin) = &mut v.tokenizer {
            if pin.file.is_relative() {
                pin.file = base.join(&pin.file)
            }
        }
        Ok(v)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchBinding {
    pub schema_version: u32,
    pub authority_digest: Digest,
    pub child_name: String,
    pub child_config_hash: Digest,
}
impl LaunchBinding {
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && (1..=128).contains(&self.child_name.len())
                && self
                    .child_name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleObservation {
    pub schema_version: u32,
    pub name: String,
    pub state: String,
    pub persistent: bool,
    pub config_matches: Option<bool>,
    pub lifecycle: Option<ProcessObservation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessObservation {
    pub running: bool,
    pub pid: Option<u32>,
    pub restart_pending: bool,
    pub restarts: u64,
    pub crash_loop: bool,
}
impl LifecycleObservation {
    pub fn validate(&self, binding: &LaunchBinding) -> Result<()> {
        binding.validate()?;
        require(self.schema_version == 1 && self.name == binding.child_name)?;
        require(match self.state.as_str() {
            "missing" => {
                !self.persistent && self.config_matches.is_none() && self.lifecycle.is_none()
            }
            "nonpersistent" => {
                !self.persistent && self.config_matches.is_none() && self.lifecycle.is_some()
            }
            "mismatch" => self.persistent && self.config_matches == Some(false),
            "matched" => self.persistent && self.config_matches == Some(true),
            _ => false,
        })
    }
}
pub type RunFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
/// Only the host constructs this authority. Browser inputs cannot supply it.
pub trait RunLifecycle: Send + Sync {
    fn binding(
        &self,
        identity: &attempts::Identity,
        config_path: &Path,
        config_digest: &Digest,
    ) -> Result<LaunchBinding>;
    fn inspect<'a>(&'a self, binding: &'a LaunchBinding) -> RunFuture<'a, LifecycleObservation>;
    fn install<'a>(
        &'a self,
        binding: &'a LaunchBinding,
        config_path: &'a Path,
        config_digest: &'a Digest,
    ) -> RunFuture<'a, ()>;
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPlan {
    pub schema_version: u32,
    pub kind: String,
    pub draft_id: Digest,
    pub draft_revision: u64,
    pub publication_plan_digest: Digest,
    pub config_digest: Digest,
    pub launch: LaunchBinding,
    pub recovery_probes_enabled: bool,
    pub probe_budget: crate::capacity::probes::Budget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data_handling: Vec<crate::declaration::Signed>,
    /// Pinned execution/configuration identity excluding commercial revisions.
    /// Absent on older retained plans, which keep exact-publication recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_binding: Option<Digest>,
    pub plan_digest: Digest,
}
#[derive(Serialize)]
pub struct RunReport {
    pub schema_version: u32,
    pub kind: &'static str,
    pub plan: RunPlan,
    pub state: String,
    pub for_current_configuration: bool,
    pub observation: Option<LifecycleObservation>,
    pub readiness: &'static str,
    pub capacity_advertised_by_setup: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retained {
    schema_version: u32,
    plan: RunPlan,
    state: String,
    observation: Option<LifecycleObservation>,
}
fn digest(domain: &'static str, value: &impl Serialize) -> Result<Digest> {
    let bytes =
        mayhem_proto::stable_json_bytes(&serde_json::to_value(value).map_err(|_| Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
    Ok(Digest::hash(domain, &[&bytes]))
}
fn plan_digest(plan: &RunPlan) -> Result<Digest> {
    let mut v = json!(plan);
    v.as_object_mut()
        .ok_or(Error::Invalid)?
        .remove("plan_digest");
    digest("mayhem/proxy/setup-run-plan/v1", &v)
}
fn filename(digest: &Digest) -> String {
    format!("wizard-managed-{}.json", digest.as_str())
}
fn identity(record: &Record) -> Result<attempts::Identity> {
    Ok(attempts::Identity {
        network_id: record.input.network.network_id.clone(),
        msb_bootstrap: Digest::new(&record.input.network.msb_bootstrap)
            .map_err(|_| Error::Invalid)?,
        subnet_bootstrap: Digest::new(&record.input.network.subnet_bootstrap)
            .map_err(|_| Error::Invalid)?,
        controller_pubkey: record.input.provider_pubkey.clone(),
    })
}
fn runtime_binding(record: &Record) -> Result<Digest> {
    let mut input = record.input.clone();
    // These are the only fields the existing runtime may follow through signed
    // canonical offer updates. Every endpoint, rail, adapter, capacity and probe
    // binding stays in this digest. No mutable history is loaded.
    input.sequence = 1;
    for offer in &mut input.offers {
        offer.revision = 1;
        offer.per_request_au = 0;
        offer.min_session_au = 0;
        for rate in &mut offer.rates { rate.per_unit_au = 0; rate.granularity = 1; }
    }
    digest("mayhem/proxy/setup-run-execution/v1", &json!({
        "input": input, "connection":record.connection, "probe_scope":record.probe_scope
    }))
}
fn current(record: &Record, plan: &RunPlan) -> Result<bool> {
    let review = record.review()?;
    let same_runtime = plan.runtime_binding.as_ref() == Some(&runtime_binding(record)?);
    Ok(record.id == plan.draft_id
        && review.state == State::StructurallyValid
        && review.publication.as_ref().is_some_and(|p| {
            p.state == PublicationState::Complete
                && p.for_current_configuration
                && ((record.revision == plan.draft_revision && p.plan_digest == plan.publication_plan_digest) || same_runtime)
        }))
}
impl Retained {
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.plan.schema_version == 1
                && self.plan.kind == "provider_setup_run_plan"
                && self.plan.plan_digest == plan_digest(&self.plan)?
                && matches!(
                    self.state.as_str(),
                    "prepared" | "pending" | "installed" | "missing" | "conflict" | "unavailable"
                ),
        )?;
        self.plan.launch.validate()?;
        if let Some(v) = &self.observation {
            v.validate(&self.plan.launch)?
        }
        Ok(())
    }
    fn report(&self, record: &Record) -> Result<RunReport> {
        self.validate()?;
        Ok(RunReport {
            schema_version: 1,
            kind: "provider_setup_run",
            plan: self.plan.clone(),
            state: self.state.clone(),
            for_current_configuration: current(record, &self.plan)?,
            observation: self.observation.clone(),
            readiness: "not_attested_by_supervisor_health_and_canonical_gates_remain_required",
            capacity_advertised_by_setup: false,
        })
    }
}
fn build(
    record: &Record,
    data_handling: Vec<crate::declaration::Signed>,
    template: RunTemplate,
    mut probe: ProbePlan,
    peer: &str,
    directory: &Path,
    host: &dyn RunLifecycle,
) -> Result<(managed::Config, RunPlan)> {
    let review = record.review()?;
    let publication = review
        .publication
        .as_ref()
        .filter(|p| p.state == PublicationState::Complete && p.for_current_configuration)
        .ok_or(Error::RunPrerequisite)?;
    if review.state != State::StructurallyValid || review.probe_status != "protocol_validated" {
        return Err(Error::RunPrerequisite);
    }
    let scope = record.probe_scope.as_ref().ok_or(Error::RunPrerequisite)?;
    let budget = probe.budget.clone();
    let file = probe
        .scope
        .capacity_file
        .file_name()
        .ok_or(Error::RunPrerequisite)?
        .to_owned();
    let parent = std::fs::canonicalize(
        probe
            .scope
            .capacity_file
            .parent()
            .ok_or(Error::RunPrerequisite)?,
    )
    .map_err(|_| Error::Protection)?;
    probe.scope.capacity_file = parent.join(file);
    // No new capacity database, group alias, route or refreshed cumulative budget.
    require(
        probe.scope == *scope
            && scope
                .capacity_file
                .file_name()
                .is_some_and(|v| v == "capacity.redb"),
    )?;
    let meta =
        std::fs::symlink_metadata(&scope.capacity_file).map_err(|_| Error::RunPrerequisite)?;
    require(meta.is_file() && !meta.file_type().is_symlink() && meta.len() > 0)?;
    let state_dir = scope
        .capacity_file
        .parent()
        .ok_or(Error::Invalid)?
        .to_path_buf();
    let connection_file =
        std::fs::canonicalize(&record.input.connection_file).map_err(|_| Error::Protection)?;
    let config = managed::Config {
        schema_version: 1,
        network: record.input.network.clone(),
        provider_pubkey: record.input.provider_pubkey.clone(),
        peer_rpc_url: peer.into(),
        bridge: template.bridge,
        state_dir,
        worker_program: probe.worker_program,
        health: template.health,
        limits: template.limits,
        connections: vec![managed::ConnectionSpec {
            group: scope.connection_group.clone(),
            ceiling: scope.connection_ceiling,
            config_file: connection_file,
            probe_budget: probe.budget,
            expected_fingerprint: Some(record.connection.clone()),
        }],
        allocations: scope
            .constraints
            .iter()
            .map(|g| managed::Group {
                id: g.id.clone(),
                ceiling: g.ceiling,
            })
            .collect(),
        routes: vec![managed::RouteConfig {
            data_handling: data_handling.clone(),
            declaration_source: Some(DeclarationSource::from_record(directory, record)?),
            id: scope.route.clone(),
            connection: scope.connection_group.clone(),
            ceiling: scope.route_ceiling,
            constraints: scope.constraints.iter().map(|g| g.id.clone()).collect(),
            adapter: record.input.adapter.clone(),
            offers: record.input.offers.clone(),
            settlement_policy: record.input.settlement_policy.clone(),
            tokenizer: template.tokenizer,
            recovery: template.allow_recovery_probes.then_some(managed::Recovery {
                request: probe.request,
                streaming: probe.streaming,
                max_output_tokens: probe.max_output_tokens,
                timeout_ms: probe.timeout_ms,
            }),
        }],
    };
    let config_digest = digest("mayhem/proxy/setup-managed-config/v1", &config)?;
    let path = directory.join(filename(&config_digest));
    managed::Prepared::check_supervised_config(&config, &path)
        .map_err(|_| Error::RunPrerequisite)?;
    let launch = host.binding(&identity(record)?, &path, &config_digest)?;
    launch.validate()?;
    let mut plan = RunPlan {
        schema_version: 1,
        kind: "provider_setup_run_plan".into(),
        draft_id: record.id.clone(),
        draft_revision: record.revision,
        publication_plan_digest: publication.plan_digest.clone(),
        config_digest,
        launch,
        recovery_probes_enabled: template.allow_recovery_probes,
        probe_budget: budget,
        data_handling,
        runtime_binding: Some(runtime_binding(record)?),
        plan_digest: Digest::hash("placeholder", &[]),
    };
    plan.plan_digest = plan_digest(&plan)?;
    Ok((config, plan))
}

/// Prove a revised review still addresses the original installed controller.
/// Only canonical offer rates can differ; changed templates, paths, recovery
/// allowances or runtime limits must not silently replace the retained Run.
fn retained_plan(
    guard: &store::Guard,
    record: &Record,
    mut candidate: managed::Config,
    saved: &Retained,
    host: &dyn RunLifecycle,
    directory: &Path,
) -> Result<RunPlan> {
    saved.validate()?;
    if !current(record, &saved.plan)? { return Err(Error::RunConflict); }
    let original: managed::Config = guard.read_json(&filename(&saved.plan.config_digest))?
        .ok_or(Error::RunConflict)?;
    if digest("mayhem/proxy/setup-managed-config/v1", &original)? != saved.plan.config_digest
        || candidate.routes.len() != original.routes.len() { return Err(Error::RunConflict); }
    for (next, old) in candidate.routes.iter_mut().zip(&original.routes) {
        if next.offers.len() != old.offers.len() || !old.offers.iter().zip(&next.offers)
            .all(|(a,b)| crate::financial::offer::rate_successor(a,b)) { return Err(Error::RunConflict); }
        next.offers = old.offers.clone();
    }
    if digest("mayhem/proxy/setup-managed-config/v1", &candidate)? != saved.plan.config_digest
        || host.binding(&identity(record)?, &directory.join(filename(&saved.plan.config_digest)), &saved.plan.config_digest)? != saved.plan.launch {
        return Err(Error::RunConflict);
    }
    Ok(saved.plan.clone())
}
// A renewed declaration must not replace the original installed Run identity.
// Its configured source supplies fresh metadata independently of the retained
// financial/runtime binding; other template changes still conflict below.
fn run_declarations(
    guard: &store::Guard,
    record: &Record,
) -> Result<Vec<crate::declaration::Signed>> {
    if let Some(saved) = guard.read_json::<Retained>("wizard-run.json")? {
        saved.validate()?;
        return Ok(saved.plan.data_handling);
    }
    declarations::signed_for_run(guard, record, flow::declarations::now()?)
}
impl Store {
    pub fn run_plan(
        &self,
        expected: u64,
        template: RunTemplate,
        probe: ProbePlan,
        peer: &str,
        host: &dyn RunLifecycle,
    ) -> Result<RunPlan> {
        let guard = store::Guard::open(&self.directory)?;
        let record = guard.read()?.ok_or(Error::Missing)?;
        if record.revision != expected {
            return Err(Error::Conflict);
        }
        let declarations = run_declarations(&guard, &record)?;
        let (config, plan) = build(
            &record,
            declarations,
            template,
            probe,
            peer,
            &self.directory,
            host,
        )?;
        if let Some(saved) = guard.read_json::<Retained>("wizard-run.json")? {
            saved.validate()?;
            if saved.plan.plan_digest != plan.plan_digest {
                return retained_plan(&guard, &record, config, &saved, host, &self.directory);
            }
        }
        Ok(plan)
    }
    pub fn inspect_run(&self) -> Result<Option<RunReport>> {
        let guard = store::Guard::open(&self.directory)?;
        let Some(saved) = guard.read_json::<Retained>("wizard-run.json")? else {
            return Ok(None);
        };
        let record = guard.read()?.ok_or(Error::Missing)?;
        Ok(Some(saved.report(&record)?))
    }
    pub async fn start_run(
        &self,
        expected: u64,
        template: RunTemplate,
        probe: ProbePlan,
        peer: &str,
        expected_plan: &Digest,
        host: &dyn RunLifecycle,
    ) -> Result<RunReport> {
        let guard = store::Guard::open(&self.directory)?;
        let record = guard.read()?.ok_or(Error::Missing)?;
        if record.revision != expected {
            return Err(Error::Conflict);
        }
        let declarations = run_declarations(&guard, &record)?;
        let (config, plan) = build(
            &record,
            declarations,
            template,
            probe,
            peer,
            &self.directory,
            host,
        )?;
        if let Some(mut saved) = guard.read_json::<Retained>("wizard-run.json")? {
            let original = retained_plan(&guard, &record, config, &saved, host, &self.directory)?;
            if &original.plan_digest != expected_plan { return Err(Error::RunConflict); }
            return reconcile(&guard, &record, &self.directory, &mut saved, host, true).await;
        }
        if &plan.plan_digest != expected_plan {
            return Err(Error::RunConflict);
        }
        let name = filename(&plan.config_digest);
        if let Some(existing) = guard.read_json::<managed::Config>(&name)? {
            if digest("mayhem/proxy/setup-managed-config/v1", &existing)? != plan.config_digest {
                return Err(Error::RunConflict);
            }
        } else {
            guard.write_json(&name, "wizard-managed.next", &config)?;
        }
        let mut saved = match guard.read_json::<Retained>("wizard-run.json")? {
            Some(s) => {
                s.validate()?;
                if s.plan.plan_digest != plan.plan_digest {
                    return Err(Error::RunConflict);
                }
                s
            }
            None => Retained {
                schema_version: 1,
                plan,
                state: "prepared".into(),
                observation: None,
            },
        };
        // Both immutable config and intent are fsynced before any child installation.
        guard.write_json("wizard-run.json", "wizard-run.next", &saved)?;
        reconcile(&guard, &record, &self.directory, &mut saved, host, true).await
    }
    pub async fn recover_run(&self, host: &dyn RunLifecycle) -> Result<RunReport> {
        let guard = store::Guard::open(&self.directory)?;
        let record = guard.read()?.ok_or(Error::Missing)?;
        let mut saved = guard
            .read_json::<Retained>("wizard-run.json")?
            .ok_or(Error::Missing)?;
        saved.validate()?;
        reconcile(&guard, &record, &self.directory, &mut saved, host, false).await
    }
}
async fn reconcile(
    guard: &store::Guard,
    record: &Record,
    directory: &Path,
    saved: &mut Retained,
    host: &dyn RunLifecycle,
    install: bool,
) -> Result<RunReport> {
    let path = directory.join(filename(&saved.plan.config_digest));
    let config: managed::Config = guard
        .read_json(&filename(&saved.plan.config_digest))?
        .ok_or(Error::RunConflict)?;
    if digest("mayhem/proxy/setup-managed-config/v1", &config)? != saved.plan.config_digest {
        return Err(Error::RunConflict);
    }
    if host.binding(&identity(record)?, &path, &saved.plan.config_digest)? != saved.plan.launch {
        return Err(Error::RunConflict);
    }
    let observation = host.inspect(&saved.plan.launch).await;
    match observation {
        Ok(v) => {
            v.validate(&saved.plan.launch)?;
            if install && v.state == "missing" {
                if !current(record, &saved.plan)? {
                    return Err(Error::RunConflict);
                }
                saved.state = "pending".into();
                saved.observation = Some(v);
                guard.write_json("wizard-run.json", "wizard-run.next", saved)?;
                // Even a lost ACK remains the original retained identity. Inspect again;
                // never create a substitute child or assume a process is ready from its PID.
                let install_result = host
                    .install(&saved.plan.launch, &path, &saved.plan.config_digest)
                    .await;
                match host.inspect(&saved.plan.launch).await {
                    Ok(v) => {
                        v.validate(&saved.plan.launch)?;
                        update(saved, v);
                        if saved.state == "missing" {
                            if let Err(error) = install_result {
                                guard.write_json("wizard-run.json", "wizard-run.next", saved)?;
                                return Err(error);
                            }
                        }
                    }
                    Err(_) => {
                        saved.state = "unavailable".into();
                        saved.observation = None;
                    }
                }
            } else {
                update(saved, v)
            }
        }
        Err(_) => {
            saved.state = "unavailable".into();
            saved.observation = None;
        }
    }
    guard.write_json("wizard-run.json", "wizard-run.next", saved)?;
    saved.report(record)
}
fn update(saved: &mut Retained, v: LifecycleObservation) {
    saved.state = match v.state.as_str() {
        "matched" => "installed",
        "missing" => "missing",
        _ => "conflict",
    }
    .into();
    saved.observation = Some(v)
}
