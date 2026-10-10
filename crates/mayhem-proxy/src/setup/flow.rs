//! Shared explicit operator flow for CLI and the existing local dashboard.
//! Configuration is private host authority; browser commands never choose a
//! file, destination, credential reference, worker or arbitrary signing body.
use super::*;
use ed25519_dalek::SigningKey;
use mayhem_proto::proxy::ProxyEndpoint;
use serde_json::{json, Value};
use std::sync::Arc;
pub(super) mod declarations;
pub use declarations::DeclarationRegistry;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowConfig {
    pub schema_version: u32,
    pub directory: PathBuf,
    pub profile: ProfileInput,
    pub probe_plan: Option<PathBuf>,
    pub peer_rpc: Option<String>,
    pub admission_origin: Option<String>,
    pub declaration_registry: Option<DeclarationRegistry>,
    pub timeout_ms: u64,
    pub run: Option<RunSettings>,
}
impl FlowConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = private_file(path, MAX_BYTES).map_err(|_| Error::Protection)?;
        let mut v: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        let base = std::fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|_| Error::Protection)?;
        for p in [&mut v.directory, &mut v.profile.connection_file] {
            if p.is_relative() {
                *p = base.join(&p);
            }
        }
        if let Some(p) = &mut v.probe_plan {
            if p.is_relative() {
                *p = base.join(&p);
            }
        }
        if let Some(run) = &mut v.run {
            if run.template.is_relative() {
                run.template = base.join(&run.template);
            }
            if let Some(path) = &mut run.wallet_password_file {
                if path.is_relative() {
                    *path = base.join(&path);
                }
            }
        }
        v.validate()?;
        Ok(v)
    }
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.directory.is_absolute()
                && (1..=10_000).contains(&self.timeout_ms),
        )?;
        self.profile.clone().prepare()?;
        if let Some(registry) = &self.declaration_registry {
            registry.reader()?;
        }
        if let Some(path) = &self.probe_plan {
            require(path.is_absolute())?;
            ProbePlan::load(path)?;
        }
        if let Some(origin) = &self.peer_rpc {
            admission::peer(origin, self.timeout_ms)?;
        }
        if let Some(origin) = &self.admission_origin {
            let (_, url) = admission::peer(origin, self.timeout_ms)?;
            require(
                url.path() == "/"
                    && origin.trim_end_matches('/') == url.origin().ascii_serialization(),
            )?;
        }
        if let Some(run) = &self.run {
            require(
                run.template.is_absolute()
                    && run
                        .wallet_password_file
                        .as_ref()
                        .is_none_or(|p| p.is_absolute())
                    && self.probe_plan.is_some()
                    && self.peer_rpc.is_some(),
            )?;
        }
        Ok(())
    }
}
/// Local operator choices. Network, wallet, private paths, endpoint contract,
/// recipe, model server and credential scope come from protected configuration.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowChoice {
    pub upstream_model: String,
    pub market: ProfileMarket,
    pub membership: MembershipInput,
    pub offers: Vec<OfferInput>,
}
impl FlowChoice {
    fn from_profile(p: &ProfileInput) -> Self {
        Self {
            upstream_model: p.upstream_model.clone(),
            market: p.market.clone(),
            membership: p.membership.clone(),
            offers: p.offers.clone(),
        }
    }
    fn apply(self, mut p: ProfileInput) -> ProfileInput {
        p.upstream_model = self.upstream_model;
        p.market = self.market;
        p.membership = self.membership;
        p.offers = self.offers;
        p
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum FlowAction {
    DeclarationFields {
        release_id: Option<String>,
        cursor: Option<String>,
    },
    DeclarationPlan {
        expected_revision: u64,
        expected_declaration_revision: u64,
        release_id: String,
        release_hash: Digest,
        choices: Vec<DeclarationChoice>,
        expires_at_ms: u64,
    },
    ConfirmDeclaration {
        expected_revision: u64,
        plan_digest: Digest,
    },
    WithdrawDeclarationPlan {
        expected_revision: u64,
        expected_declaration_revision: u64,
        expires_at_ms: u64,
    },
    Connect {},
    Discover {
        expected_inventory_revision: u64,
    },
    Select {
        expected_revision: Option<u64>,
        choice: FlowChoice,
    },
    Check {
        expected_revision: u64,
    },
    Probe {
        expected_revision: u64,
        probe_plan_digest: Digest,
    },
    RecoverProbe {
        expected_revision: u64,
    },
    AdmissionCheck {
        expected_revision: u64,
    },
    AdmissionReturns {
        expected_revision: u64,
        after: Option<String>,
    },
    Enrollment {
        expected_revision: u64,
        operation: EnrollmentAction,
        rail: Option<mayhem_proto::proxy::ProxyRail>,
        quote: Option<EnrollmentQuote>,
    },
    RatePlan { expected_revision: u64, choices: Vec<RateChoice> },
    PublishRates { expected_revision: u64, plan_digest: Digest },
    PublicationPlan {
        expected_revision: u64,
        offers_only: bool,
    },
    Publish {
        expected_revision: u64,
        offers_only: bool,
        plan_digest: Digest,
    },
    RecoverPublication {
        expected_revision: u64,
    },
    RunPlan {
        expected_revision: u64,
    },
    StartRun {
        expected_revision: u64,
        plan_digest: Digest,
    },
    RecoverRun {},
}
#[derive(Serialize)]
pub struct FlowView {
    pub schema_version: u32,
    pub kind: &'static str,
    pub audience: &'static str,
    pub connection: ConnectionView,
    pub endpoint: ProxyEndpoint,
    pub selection: FlowChoice,
    pub inventory: Option<InventoryReview>,
    pub review: Option<Review>,
    pub probe_plan: Option<Value>,
    pub enrollment: Option<Value>,
    pub run: Option<RunReport>,
    pub declaration: Option<DeclarationReport>,
    pub pending_declaration: Option<DeclarationReport>,
    pub rates: Option<RateReport>,
    pub rate_choices: Vec<RateChoice>,
    pub steps: Vec<Value>,
    pub capabilities: Value,
}
#[derive(Serialize)]
pub struct ConnectionView {
    pub id: String,
    pub revision: u64,
    pub configured_endpoints: Vec<ProxyEndpoint>,
    pub credentials: &'static str,
    pub authority: &'static str,
}
#[derive(Serialize)]
pub struct FlowResult {
    pub schema_version: u32,
    pub action_result: Value,
    pub view: FlowView,
}

pub struct Flow {
    config: FlowConfig,
    gate: Arc<tokio::sync::Semaphore>,
    lifecycle: Option<Arc<dyn RunLifecycle>>,
    registry: Option<crate::registry::publication::Reader>,
}
impl Flow {
    pub fn open(config: FlowConfig) -> Result<Self> {
        config.validate()?;
        Store::open(&config.directory)?;
        let registry = config
            .declaration_registry
            .as_ref()
            .map(DeclarationRegistry::reader)
            .transpose()?;
        Ok(Self {
            config,
            gate: Arc::new(tokio::sync::Semaphore::new(1)),
            lifecycle: None,
            registry,
        })
    }
    pub fn run_settings(&self) -> Option<&RunSettings> {
        self.config.run.as_ref()
    }
    pub fn with_run_lifecycle(mut self, lifecycle: Arc<dyn RunLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }
    fn lifecycle(&self) -> Result<&dyn RunLifecycle> {
        self.lifecycle.as_deref().ok_or(Error::RunUnavailable)
    }
    fn run_template(&self) -> Result<RunTemplate> {
        RunTemplate::load(
            &self
                .config
                .run
                .as_ref()
                .ok_or(Error::RunPrerequisite)?
                .template,
        )
    }
    pub fn provider(&self) -> &Digest {
        &self.config.profile.provider_pubkey
    }
    fn store(&self) -> Result<Store> {
        Store::open(&self.config.directory)
    }
    fn peer(&self) -> Result<&str> {
        self.config.peer_rpc.as_deref().ok_or(Error::Invalid)
    }
    fn choice(&self, expected: Option<(&Digest, u64)>) -> Result<FlowChoice> {
        let guard = store::Guard::open(&self.config.directory)?;
        let Some(record) = guard.read()? else {
            if expected.is_some() {
                return Err(Error::Conflict);
            }
            return Ok(FlowChoice::from_profile(&self.config.profile));
        };
        if !expected.is_some_and(|(id, revision)| id == &record.id && revision == record.revision) {
            return Err(Error::Conflict);
        }
        require(
            record.input.network == self.config.profile.network
                && record.input.provider_pubkey == self.config.profile.provider_pubkey,
        )?;
        let input = &record.input;
        Ok(FlowChoice {
            upstream_model: input.adapter.upstream_model.clone(),
            market: match input.selection {
                Selection::CreateMarket => ProfileMarket::CreateMarket {
                    slug: input.market.slug.clone(),
                    model: input.market.model.clone(),
                },
                Selection::JoinMarket => ProfileMarket::JoinMarket {
                    market: input.market.clone(),
                },
            },
            membership: MembershipInput {
                revision: input.membership.revision,
                served_context: input.membership.served_context,
                max_concurrency: input.membership.max_concurrency,
                capacity_group: input.membership.capacity_group.clone(),
                accepted_rails: input.membership.accepted_rails.clone(),
            },
            offers: input
                .offers
                .iter()
                .map(|o| OfferInput {
                    revision: o.revision,
                    ctx_bracket: o.ctx_bracket.clone(),
                    outcome_class: o.outcome_class.clone(),
                    rates: o.rates.clone(),
                    per_request_au: o.per_request_au,
                    min_session_au: o.min_session_au,
                    accepted_rails: o.accepted_rails.clone(),
                })
                .collect(),
        })
    }
    fn probe(&self) -> Result<(ProbePlan, Digest)> {
        let plan = ProbePlan::load(self.config.probe_plan.as_deref().ok_or(Error::Invalid)?)?;
        let bytes = mayhem_proto::stable_json_bytes(
            &serde_json::to_value(&plan).map_err(|_| Error::Invalid)?,
        )
        .map_err(|_| Error::Invalid)?;
        Ok((
            plan,
            Digest::hash("mayhem/proxy/setup-probe-plan/v1", &[&bytes]),
        ))
    }
    pub fn view(&self) -> Result<FlowView> {
        let store = self.store()?;
        let review = match store.inspect() {
            Ok(r) => Some(r),
            Err(Error::Missing) => None,
            Err(e) => return Err(e),
        };
        let inventory = match store.inspect_connection(true) {
            Ok(r) => Some(r),
            Err(Error::DiscoveryMissing) => None,
            Err(e) => return Err(e),
        };
        let config = ConnectionConfig::load(&self.config.profile.connection_file)
            .map_err(|_| Error::Protection)?;
        let endpoint = Adapter::restore(self.config.profile.clone().prepare()?.adapter)
            .map_err(|_| Error::Invalid)?
            .endpoint();
        let mut endpoints = Vec::new();
        for (op, e) in [
            (
                crate::connector::config::Operation::ChatCompletions,
                ProxyEndpoint::Chat,
            ),
            (
                crate::connector::config::Operation::Completions,
                ProxyEndpoint::Completions,
            ),
            (
                crate::connector::config::Operation::Responses,
                ProxyEndpoint::Responses,
            ),
            (
                crate::connector::config::Operation::Decisions,
                ProxyEndpoint::Decisions,
            ),
        ] {
            if config.paths.contains_key(&op) {
                endpoints.push(e)
            }
        }
        let probe_plan=self.config.probe_plan.as_ref().map(|_| -> Result<Value> {let(p,d)=self.probe()?;Ok(json!({"digest":d,"streaming":p.streaming,"max_output_tokens":p.max_output_tokens,"timeout_ms":p.timeout_ms,"budget":p.budget,"scope":"one explicit request using the existing shared capacity authority","may_consume_upstream_allowance":true}))}).transpose()?;
        let guard = store::Guard::open(&self.config.directory)?;
        let mut enrollment: Option<Value> = guard.read_json("wizard-enrollment.json")?;
        drop(guard);
        if let Some(e) = &mut enrollment {
            e["for_current_revision"] =
                json!(review.as_ref().is_some_and(
                    |r| e["draft_id"] == json!(r.draft_id) && e["revision"] == r.revision
                ));
        }
        let run = store.inspect_run()?;
        let now = declarations::now()?;
        let mut declaration = store.inspect_data_handling(now)?;
        if let Some(declaration) = &mut declaration {
            if declaration.state == "signed_not_installed"
                && run.as_ref().is_some_and(|r| {
                    r.state == "installed"
                        && r.for_current_configuration
                        && declaration.signed.as_ref().is_some_and(|s| {
                            declaration.observed_by_controller || r.plan
                                .data_handling
                                .iter()
                                .any(|d| d.signature == s.signature)
                        })
                })
            {
                declaration.state = "configured_in_controller";
                declaration.installed_in_runtime = true;
            }
        }
        let pending_declaration = store.inspect_pending_data_handling(now)?;
        let r = review.as_ref();
        let steps = vec![
            json!({"step":"connect","state":"configured_private_reference"}),
            json!({"step":"discover","state":inventory.as_ref().map(|i|json!(i.state)).unwrap_or(json!("not_run")),"manual_model_supported":true}),
            json!({"step":"select","state":if r.is_some(){"retained"}else{"needs_selection"}}),
            json!({"step":"check","structural":r.map(|r|json!(r.state)).unwrap_or(json!("not_run")),"probe":r.map(|r|r.probe_status).unwrap_or("not_run")}),
            json!({"step":"market","state":if r.is_some(){"operator_selected_not_canonical_confirmation"}else{"needs_selection"}}),
            json!({"step":"data_handling","state":declaration.as_ref().map(|d|d.state).unwrap_or("not_declared"),"assurance":"declared_not_verified"}),
            json!({"step":"admission","state":r.map(|r|r.admission_status).unwrap_or("not_checked"),"payment_does_not_enable_serving":true}),
            json!({"step":"review_publish","state":r.map(|r|r.publication_status).unwrap_or("not_submitted"),"explicit_confirmation_required":true}),
            json!({"step":"run","state":run.as_ref().map(|r|r.state.as_str()).unwrap_or(if self.config.run.is_some()&&self.lifecycle.is_some(){"not_started"}else{"managed_configuration_handoff_required"}),"automatic_start":false}),
        ];
        Ok(FlowView {
            schema_version: 1,
            kind: "provider_setup_flow",
            audience: "local_operator",
            connection: ConnectionView {
                id: config.id,
                revision: config.revision,
                configured_endpoints: endpoints,
                credentials: "private_reference_never_exported",
                authority: "operator_configured_destination_only",
            },
            endpoint,
            selection: self.choice(review.as_ref().map(|r| (&r.draft_id, r.revision)))?,
            inventory,
            rate_choices: review.as_ref().map(|r| r.offers.iter().map(RateChoice::from_offer).collect::<Result<Vec<_>>>()).transpose()?.unwrap_or_default(),
            rates: store.rates()?,
            review,
            probe_plan,
            enrollment,
            run,
            declaration,
            pending_declaration,
            steps,
            capabilities: json!({"declarations":self.registry.is_some(),"probe":self.config.probe_plan.is_some(),"canonical_admission":self.config.peer_rpc.is_some(),"enrollment":self.config.admission_origin.is_some(),"publication":self.config.peer_rpc.is_some(),"rates":self.config.peer_rpc.is_some(),"run":self.config.run.is_some()&&self.lifecycle.is_some()}),
        })
    }
    pub async fn execute(
        &self,
        action: FlowAction,
        key: Option<&SigningKey>,
    ) -> Result<FlowResult> {
        let _permit = self
            .gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let store = self.store()?;
        let result = match action {
            FlowAction::DeclarationFields { release_id, cursor } => {
                self.declaration_fields(release_id, cursor).await?
            }
            FlowAction::DeclarationPlan {
                expected_revision,
                expected_declaration_revision,
                release_id,
                release_hash,
                choices,
                expires_at_ms,
            } => {
                self.declaration_plan(
                    expected_revision,
                    expected_declaration_revision,
                    &release_id,
                    &release_hash,
                    choices,
                    expires_at_ms,
                )
                .await?
            }
            FlowAction::ConfirmDeclaration {
                expected_revision,
                plan_digest,
            } => self.confirm_declaration(
                expected_revision,
                &plan_digest,
                key.ok_or(Error::Invalid)?,
            )?,
            FlowAction::WithdrawDeclarationPlan { expected_revision, expected_declaration_revision, expires_at_ms } => {
                json!(store.plan_declaration_withdrawal(expected_revision, expected_declaration_revision, declarations::now()?, expires_at_ms)?)
            },
            FlowAction::Connect {} => {
                self.config.profile.clone().prepare()?;
                json!({"configured":true,"network_request":false,"scope":"configuration_only"})
            }
            FlowAction::Discover {
                expected_inventory_revision,
            } => json!(
                store
                    .discover(
                        &self.config.profile.connection_file,
                        expected_inventory_revision,
                        self.config.timeout_ms
                    )
                    .await?
            ),
            FlowAction::Select {
                expected_revision,
                choice,
            } => {
                json!(store.prepare(choice.apply(self.config.profile.clone()), expected_revision)?)
            }
            FlowAction::Check { expected_revision } => json!(store.check(expected_revision)?),
            FlowAction::Probe {
                expected_revision,
                probe_plan_digest,
            } => {
                let (p, d) = self.probe()?;
                require(d == probe_plan_digest)?;
                json!(store.probe(expected_revision, p).await?)
            }
            FlowAction::RecoverProbe { expected_revision } => {
                json!(store.recover_probe(expected_revision)?)
            }
            FlowAction::AdmissionCheck { expected_revision } => json!(
                store
                    .admission_check(expected_revision, self.peer()?, self.config.timeout_ms)
                    .await?
            ),
            FlowAction::Enrollment {
                expected_revision,
                operation,
                rail,
                quote,
            } => {
                let client = store.enrollment_client(
                    expected_revision,
                    self.config
                        .admission_origin
                        .as_deref()
                        .ok_or(Error::Invalid)?,
                    self.config.timeout_ms,
                )?;
                let guard = store::Guard::open(&self.config.directory)?;
                let record = guard.read()?.ok_or(Error::Missing)?;
                require(record.revision == expected_revision)?;
                let result = client
                    .execute_with_quote(key.ok_or(Error::Invalid)?, operation, rail, quote)
                    .await?;
                // Checkout is an ephemeral link, not a fresh invoice/permit observation.
                // Retain the last reconciled invoice rather than replacing it with null.
                if !matches!(operation, EnrollmentAction::Checkout | EnrollmentAction::Returns) {
                    let mut retained = serde_json::to_value(&result).map_err(|_| Error::Invalid)?;
                    retained["checkout_url"] = Value::Null;
                    retained["source"] = json!("last_authenticated_response_not_live_status");
                    retained["draft_id"] = json!(record.id);
                    retained["revision"] = json!(record.revision);
                    guard.write_json(
                        "wizard-enrollment.json",
                        "wizard-enrollment.next",
                        &retained,
                    )?;
                }
                json!(result)
            }
            FlowAction::AdmissionReturns {expected_revision,after} => {
                let client=store.enrollment_client(expected_revision,self.config.admission_origin.as_deref().ok_or(Error::Invalid)?,self.config.timeout_ms)?;
                json!(client.returns(key.ok_or(Error::Invalid)?,after).await?)
            }
            FlowAction::RatePlan { expected_revision, choices } => json!(store.plan_rates(expected_revision, choices, self.peer()?, self.config.timeout_ms).await?),
            FlowAction::PublishRates { expected_revision, plan_digest } => json!(store.publish_rates(expected_revision, &plan_digest, self.peer()?, self.config.timeout_ms, key.ok_or(Error::Invalid)?).await?),
            FlowAction::PublicationPlan {
                expected_revision,
                offers_only,
            } => json!(store.publication_plan(expected_revision, offers_only)?),
            FlowAction::Publish {
                expected_revision,
                offers_only,
                plan_digest,
            } => {
                let plan = store.publication_plan(expected_revision, offers_only)?;
                require(plan.plan_digest == plan_digest)?;
                let permit = if offers_only { None } else {
                    let guard = store::Guard::open(&self.config.directory)?;
                    let retained: Option<Value> = guard.read_json("wizard-enrollment.json")?;
                    retained.and_then(|v|v.get("invoice").and_then(|v|v.get("permit")).cloned()).filter(|v|!v.is_null()).map(|v|serde_json::from_value::<AdmissionPermit>(json!({"permit":v["body"],"issuer_signature":v["issuer_signature"]})).map_err(|_|Error::Invalid)).transpose()?
                };
                let auth = plan.authorize(key.ok_or(Error::Invalid)?, permit)?;
                json!(
                    store
                        .publish(
                            expected_revision,
                            self.peer()?,
                            self.config.timeout_ms,
                            auth
                        )
                        .await?
                )
            }
            FlowAction::RunPlan { expected_revision } => json!(store.run_plan(
                expected_revision,
                self.run_template()?,
                self.probe()?.0,
                self.peer()?,
                self.lifecycle()?
            )?),
            FlowAction::StartRun {
                expected_revision,
                plan_digest,
            } => json!(
                store
                    .start_run(
                        expected_revision,
                        self.run_template()?,
                        self.probe()?.0,
                        self.peer()?,
                        &plan_digest,
                        self.lifecycle()?
                    )
                    .await?
            ),
            FlowAction::RecoverRun {} => json!(store.recover_run(self.lifecycle()?).await?),
            FlowAction::RecoverPublication { expected_revision } => json!(
                store
                    .recover_publication(expected_revision, self.peer()?, self.config.timeout_ms)
                    .await?
            ),
        };
        Ok(FlowResult {
            schema_version: 1,
            action_result: result,
            view: self.view()?,
        })
    }
}
