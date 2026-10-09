//! First-create-only private configuration factory, shared by local frontends.
//! No network, wallet unlock, probe, capacity database, invoice or publication.
mod runtime;
use super::*;
use crate::{capacity::probes::Budget, connector::config::*, managed};
use mayhem_proto::proxy::ProxyEndpoint;
use serde_json::json;
use std::{collections::BTreeMap, fs, io::Write};
use zeroize::Zeroizing;

/// Supplied by the trusted local host, never by an upstream or public request.
#[derive(Clone)]
pub struct Host {
    pub network: Identity,
    pub provider_pubkey: Digest,
    pub peer_rpc: String,
    pub bridge_url: String,
    pub bridge_token_file: PathBuf,
    pub worker_program: PathBuf,
    pub wallet_password_file: Option<PathBuf>,
    pub admission_origin: Option<String>,
}
/// Deliberately neither Serialize nor Debug: frontends may accept a secret once,
/// but only a protected reference is written into configuration/review.
pub enum Credential {
    None,
    BearerFile(PathBuf),
    BearerValue(Zeroizing<Vec<u8>>),
}
pub struct Choices {
    pub base_url: String,
    pub network_policy: NetworkPolicy,
    pub credential: Credential,
    pub endpoint: ProxyEndpoint,
    pub upstream_model: String,
    pub market: ProfileMarket,
    pub served_context: u32,
    pub concurrency: u32,
    pub offers: Vec<OfferInput>,
    pub accepted_rails: Vec<mayhem_proto::proxy::ProxyRail>,
    /// Exact intended canonical sequence. Publication independently rechecks it.
    pub sequence: u64,
    pub settlement_policy: ProxySettlementPolicy,
    pub probe_budget: Budget,
    pub probe_output_limit: u64,
    pub probe_timeout_ms: u64,
    pub allow_recovery_probes: bool,
    /// Approved local data; required for LLM profiles, absent for decisions.
    pub tokenizer: Option<managed::Tokenizer>,
    /// Local completed-journal retention, not a commercial retention promise.
    pub closed_retention_ms: u64,
}
impl Choices {
    /// Local review only. No destination, credential, private reference or raw
    /// probe body is exposed; all amounts and charging choices remain exact.
    pub fn review(&self) -> serde_json::Value {
        json!({"schema_version":1,"kind":"proxy_setup_bootstrap_review","profile":"bounded_single_connection_v1",
            "endpoint":self.endpoint,"market":self.market,"served_context":self.served_context,
            "shared_concurrency":self.concurrency,"accepted_rails":self.accepted_rails,"offers":self.offers,
            "sequence":self.sequence,"settlement_policy":self.settlement_policy,
            "metering":Policy::for_endpoint(self.endpoint).definition(),"probe_budget":self.probe_budget,
            "probe_output_limit":self.probe_output_limit,"probe_timeout_ms":self.probe_timeout_ms,
            "allow_recovery_probes":self.allow_recovery_probes,"tokenizer_digest":self.tokenizer.as_ref().map(|t|&t.digest),
            "closed_retention_ms":self.closed_retention_ms,"assurance":"declared_not_probed",
            "authorizes_probe":false,"authorizes_publication":false,"authorizes_run":false})
    }
}
#[derive(Serialize)]
pub struct Bundle {
    pub schema_version: u32,
    /// Host-local path, never part of public discovery or an exported recipe.
    pub config_file: PathBuf,
    pub profile: &'static str,
    pub network_requests: u32,
    pub capacity_created: bool,
    pub authorizes_probe: bool,
    pub authorizes_publication: bool,
    pub authorizes_run: bool,
}

fn serialize<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Invalid)?;
    require(bytes.len() <= MAX_BYTES)?;
    Ok(bytes)
}
fn secret_valid(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Invalid)?;
    require(
        !text.trim().is_empty()
            && bytes.len() <= 8192
            && text.trim().bytes().all(|b| b.is_ascii_graphic()),
    )
}
/// Install a complete validated bundle into a new child of an existing private
/// directory. An existing bundle is always a conflict, never an overwrite/reset.
#[cfg(unix)]
pub fn create(destination: &Path, host: Host, choices: Choices) -> Result<Bundle> {
    use rustix::fs::{flock, open, openat, FlockOperation, Mode, OFlags};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    require(destination.is_absolute())?;
    let name = destination
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or(Error::Protection)?;
    require(
        !name.starts_with('.')
            && name.len() <= 64
            && name
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"_-".contains(&v)),
    )?;
    let parent = destination.parent().ok_or(Error::Protection)?;
    let dir = fs::File::from(
        open(
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| Error::Protection)?,
    );
    let meta = dir.metadata().map_err(|_| Error::Protection)?;
    require(meta.mode() & 0o077 == 0 && meta.uid() == rustix::process::geteuid().as_raw())?;
    let lock = fs::File::from(
        openat(
            &dir,
            ".proxy-bootstrap.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| Error::Protection)?,
    );
    let meta = lock.metadata().map_err(|_| Error::Protection)?;
    require(
        meta.is_file()
            && meta.nlink() == 1
            && meta.mode() & 0o077 == 0
            && meta.uid() == rustix::process::geteuid().as_raw(),
    )?;
    flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(|_| Error::Busy)?;
    let parent = fs::canonicalize(parent).map_err(|_| Error::Protection)?;
    let destination = parent.join(name);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(Error::Conflict);
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
    let stage = parent.join(format!(
        ".proxy-bootstrap-{}",
        blake3::hash(&nonce).to_hex()
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|_| Error::Storage)?;
    let result = (|| {
        for name in ["state", "runtime", "worker"] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(stage.join(name))
                .map_err(|_| Error::Storage)?;
        }
        build(&stage, host, choices)?;
        for name in ["state", "runtime", "worker"] {
            fs::File::open(stage.join(name))
                .and_then(|v| v.sync_all())
                .map_err(|_| Error::Storage)?;
        }
        fs::File::open(&stage)
            .and_then(|v| v.sync_all())
            .map_err(|_| Error::Storage)?;
        // The stable parent lock serializes all cooperative creators. Never
        // remove an existing destination, including an empty directory/symlink.
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(Error::Conflict);
        }
        fs::rename(&stage, &destination).map_err(|_| Error::Storage)?;
        dir.sync_all().map_err(|_| Error::CommitUnknown)?;
        Ok(Bundle {
            schema_version: 1,
            config_file: destination.join("wizard.json"),
            profile: "bounded_single_connection_v1",
            network_requests: 0,
            capacity_created: false,
            authorizes_probe: false,
            authorizes_publication: false,
            authorizes_run: false,
        })
    })();
    // Only our unpredictable private staging directory is removed. If rename
    // succeeded but fsync failed, leave the committed original for inspection.
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}
#[cfg(not(unix))]
pub fn create(_: &Path, _: Host, _: Choices) -> Result<Bundle> {
    Err(Error::Protection)
}

#[cfg(unix)]
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| Error::Storage)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| Error::Storage)
}
#[cfg(not(unix))]
fn write(_: &Path, _: &[u8]) -> Result<()> {
    Err(Error::Protection)
}

fn build(stage: &Path, host: Host, choices: Choices) -> Result<()> {
    require(
        (1..=1024).contains(&choices.concurrency)
            && choices.served_context > 0
            && choices.closed_retention_ms > 0
            && host.worker_program.is_absolute()
            && host.bridge_token_file.is_absolute()
            && host
                .wallet_password_file
                .as_ref()
                .is_none_or(|p| p.is_absolute()),
    )?;
    host.network.validate().map_err(|_| Error::Invalid)?;
    if let Some(path) = &host.wallet_password_file {
        let bytes = private_file(path, 8192).map_err(|_| Error::Protection)?;
        require(std::str::from_utf8(&bytes).is_ok_and(|v| !v.trim_end().is_empty()))?;
    }
    let worker = fs::symlink_metadata(&host.worker_program).map_err(|_| Error::Protection)?;
    require(worker.is_file() && !worker.file_type().is_symlink())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        require(worker.permissions().mode() & 0o111 != 0)?;
    }
    let authentication = match choices.credential {
        Credential::None => Authentication::None,
        Credential::BearerFile(path) => {
            require(path.is_absolute())?;
            secret_valid(&private_file(&path, 8192).map_err(|_| Error::Protection)?)?;
            Authentication::Bearer {
                secret: SecretSource::File { path },
            }
        }
        Credential::BearerValue(value) => {
            secret_valid(&value)?;
            write(&stage.join("upstream-key"), &value)?;
            Authentication::Bearer {
                secret: SecretSource::File {
                    path: "upstream-key".into(),
                },
            }
        }
    };
    let operation = match choices.endpoint {
        ProxyEndpoint::Chat => (Operation::ChatCompletions, "chat/completions"),
        ProxyEndpoint::Completions => (Operation::Completions, "completions"),
        ProxyEndpoint::Responses => (Operation::Responses, "responses"),
        ProxyEndpoint::Decisions => (Operation::Decisions, "decisions"),
    };
    let connection = ConnectionConfig {
        schema_version: 1,
        id: "setup_connection".into(),
        revision: 1,
        base_url: choices.base_url,
        network: choices.network_policy,
        paths: BTreeMap::from([
            (Operation::Models, "models".into()),
            (operation.0, operation.1.into()),
        ]),
        authentication,
        headers: BTreeMap::new(),
        limits: crate::connector::config::Limits {
            max_in_flight: choices.concurrency as usize,
            max_request_bytes: 1024 * 1024,
            max_response_bytes: 4 * 1024 * 1024,
            ..Default::default()
        },
        error_profile: ErrorProfile::OpenAi,
    };
    connection
        .fingerprint()
        .map_err(|_| Error::Bootstrap("connection policy"))?;
    write(&stage.join("connection.json"), &serialize(&connection)?)?;
    let scope_bytes = serialize(&(host.network.clone(), host.provider_pubkey.clone()))?;
    let connection_group = Digest::hash("mayhem/proxy/bootstrap-connection/v1", &[&scope_bytes]);
    let profile = ProfileInput {
        schema_version: 1,
        network: host.network.clone(),
        provider_pubkey: host.provider_pubkey.clone(),
        connection_file: "connection.json".into(),
        profile: EndpointProfile::Standard {
            endpoint: choices.endpoint,
        },
        upstream_model: choices.upstream_model,
        limits: runtime::endpoint_limits(),
        market: choices.market,
        membership: MembershipInput {
            revision: 1,
            served_context: choices.served_context,
            max_concurrency: choices.concurrency,
            capacity_group: connection_group.as_str().into(),
            accepted_rails: choices.accepted_rails,
        },
        offers: choices.offers,
        sequence: choices.sequence,
        settlement_policy: choices.settlement_policy,
    };
    let streaming = choices.endpoint != ProxyEndpoint::Decisions;
    let request = match choices.endpoint {
        ProxyEndpoint::Chat => {
            json!({"model":"setup-probe","messages":[{"role":"user","content":"Write a short friendly greeting."}],"stream":true,"max_tokens":choices.probe_output_limit})
        }
        ProxyEndpoint::Completions => {
            json!({"model":"setup-probe","prompt":"Write a short friendly greeting.","stream":true,"max_tokens":choices.probe_output_limit})
        }
        ProxyEndpoint::Responses => {
            json!({"model":"setup-probe","input":"Write a short friendly greeting.","stream":true,"max_output_tokens":choices.probe_output_limit})
        }
        ProxyEndpoint::Decisions => {
            json!({"model":"setup-probe","state":"The local operator is checking endpoint compatibility.","questions":{"ready":{"type":"noul","instructions":"Return a value between zero and one."}}})
        }
    };
    let probe = ProbePlan {
        schema_version: 1,
        scope: ProbeScope {
            capacity_file: "runtime/capacity.redb".into(),
            route: Digest::hash("mayhem/proxy/bootstrap-route/v1", &[&scope_bytes]),
            connection_group,
            connection_ceiling: choices.concurrency,
            route_ceiling: choices.concurrency,
            constraints: vec![],
        },
        budget: choices.probe_budget,
        worker_program: host.worker_program.clone(),
        worker_directory: "worker".into(),
        request,
        streaming,
        max_output_tokens: choices.probe_output_limit,
        timeout_ms: choices.probe_timeout_ms,
    };
    write(&stage.join("probe.json"), &serialize(&probe)?)?;
    let mut template = runtime::template(
        &host,
        choices.tokenizer,
        choices.concurrency,
        choices.closed_retention_ms,
        choices.allow_recovery_probes,
    );
    match (&mut template.tokenizer, choices.endpoint) {
        (None, ProxyEndpoint::Decisions) => (),
        (Some(pin), endpoint) if endpoint != ProxyEndpoint::Decisions => {
            require(pin.file.is_absolute())?;
            let bytes = private_file(&pin.file, 64 * 1024 * 1024).map_err(|_| Error::Protection)?;
            require(blake3::hash(&bytes).to_hex().as_str() == pin.digest.as_str())?;
            write(&stage.join("tokenizer.json"), &bytes)?;
            pin.file = "tokenizer.json".into();
        }
        _ => return Err(Error::Invalid),
    }
    write(&stage.join("runtime-policy.json"), &serialize(&template)?)?;
    let flow = FlowConfig {
        schema_version: 1,
        directory: "state".into(),
        profile,
        probe_plan: Some("probe.json".into()),
        peer_rpc: Some(host.peer_rpc.clone()),
        admission_origin: host.admission_origin,
        timeout_ms: 5000,
        run: Some(RunSettings {
            template: "runtime-policy.json".into(),
            wallet_password_file: host.wallet_password_file,
        }),
    };
    write(&stage.join("wizard.json"), &serialize(&flow)?)?;
    // Validate the complete generated cross-component configuration before its
    // atomic publication; this parses local secrets/tokenizer, never dispatches.
    let flow = FlowConfig::load(&stage.join("wizard.json"))
        .map_err(|_| Error::Bootstrap("profile, probe or host references"))?;
    let input = flow
        .profile
        .prepare()
        .map_err(|_| Error::Bootstrap("profile"))?;
    let probe =
        ProbePlan::load(&stage.join("probe.json")).map_err(|_| Error::Bootstrap("probe plan"))?;
    let adapter = Adapter::restore(input.adapter.clone()).map_err(|_| Error::Invalid)?;
    let request = serialize(&probe.request)?;
    if streaming {
        adapter.prepare_stream(&request)
    } else {
        adapter.prepare_json(&request)
    }
    .map_err(|_| Error::Bootstrap("probe request"))?;
    let template = RunTemplate::load(&stage.join("runtime-policy.json"))?;
    let config = managed::Config {
        schema_version: 1,
        network: input.network,
        provider_pubkey: input.provider_pubkey,
        peer_rpc_url: host.peer_rpc,
        bridge: template.bridge,
        state_dir: stage.join("runtime"),
        worker_program: probe.worker_program,
        health: template.health,
        limits: template.limits,
        connections: vec![managed::ConnectionSpec {
            group: probe.scope.connection_group.clone(),
            ceiling: choices.concurrency,
            config_file: input.connection_file.clone(),
            probe_budget: probe.budget,
            expected_fingerprint: Some(
                ConnectionConfig::load(&input.connection_file)
                    .map_err(|_| Error::Invalid)?
                    .fingerprint()
                    .map_err(|_| Error::Invalid)?,
            ),
        }],
        allocations: vec![],
        routes: vec![managed::RouteConfig {
            data_handling: Vec::new(),
            id: probe.scope.route,
            connection: probe.scope.connection_group,
            ceiling: choices.concurrency,
            constraints: vec![],
            adapter: input.adapter,
            offers: input.offers,
            settlement_policy: input.settlement_policy,
            tokenizer: template.tokenizer,
            recovery: template.allow_recovery_probes.then_some(managed::Recovery {
                request: probe.request,
                streaming,
                max_output_tokens: probe.max_output_tokens,
                timeout_ms: probe.timeout_ms,
            }),
        }],
    };
    managed::Prepared::check_supervised_config(&config, &stage.join("validation.json"))
        .map_err(|_| Error::Bootstrap("managed runtime or protected references"))
}
