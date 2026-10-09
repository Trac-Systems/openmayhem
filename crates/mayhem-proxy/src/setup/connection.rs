//! Explicit operator discovery, separate from public declarations and probes.
//! Model names are untrusted observations, never identity/capability/readiness proof.
use super::*;
use crate::connector::{config::Operation, failure::Code, http::HttpConnection};
use mayhem_proto::proxy::ProxyEndpoint;
use serde_json::Value;
use std::time::Duration;

const FILE: &str = "discovery.json";
const TEMPORARY: &str = "discovery.next";
const MAX_MODELS: usize = 128;
const MAX_RESPONSE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryState {
    Pending,
    Listed,
    Unsupported,
    Unavailable,
    InvalidResponse,
    LimitExceeded,
    TimedOut,
    ConfigurationChanged,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema_version: u32,
    id: Digest,
    revision: u64,
    connection_file: PathBuf,
    connection: Digest,
    connection_revision: u64,
    configured_endpoints: Vec<ProxyEndpoint>,
    state: DiscoveryState,
    models: Vec<String>,
    truncated: bool,
    observed_at_ms: Option<u64>,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
impl Inventory {
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.connection_file.is_absolute()
                && (1..=mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER).contains(&self.revision)
                && (1..=mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
                    .contains(&self.connection_revision)
                && self
                    .observed_at_ms
                    .is_none_or(|v| v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
                && self.configured_endpoints.len() <= 4
                && self.configured_endpoints.windows(2).all(|v| v[0] < v[1])
                && self.models.len() <= MAX_MODELS
                && self.models.iter().all(|s| identifier(s))
                && self.models.windows(2).all(|v| v[0] < v[1])
                && (self.state == DiscoveryState::Listed
                    || (self.models.is_empty() && !self.truncated))
                && (self.state == DiscoveryState::Pending) == self.observed_at_ms.is_none(),
        )
    }
    fn current(&self) -> bool {
        ConnectionConfig::load(&self.connection_file)
            .ok()
            .and_then(|c| c.fingerprint().ok())
            .is_some_and(|v| v == self.connection)
    }
    fn review(&self, include_model_ids: bool) -> Result<InventoryReview> {
        self.validate()?;
        Ok(InventoryReview {
            schema_version: 1,
            kind: "provider_connection_inventory",
            audience: "local_operator",
            discovery_id: self.id.clone(),
            revision: self.revision,
            state: self.state,
            for_current_configuration: self.current(),
            connection_revision: self.connection_revision,
            configured_endpoints: self.configured_endpoints.clone(),
            model_count: self.models.len(),
            truncated: self.truncated,
            model_ids: include_model_ids.then(|| self.models.clone()),
            observed_at_ms: self.observed_at_ms,
            scope: "single_model_list_response",
            model_identity: "not_verified",
            capabilities: "not_verified",
            readiness: "not_verified",
            concurrency: "not_verified",
            admission_status: "not_checked",
        })
    }
}
/// Model IDs are omitted unless the operator explicitly requests the private
/// inventory. Do not use this local-only projection as a public catalog record.
#[derive(Serialize)]
pub struct InventoryReview {
    pub schema_version: u32,
    pub kind: &'static str,
    pub audience: &'static str,
    pub discovery_id: Digest,
    pub revision: u64,
    pub state: DiscoveryState,
    pub for_current_configuration: bool,
    pub connection_revision: u64,
    pub configured_endpoints: Vec<ProxyEndpoint>,
    pub model_count: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_ids: Option<Vec<String>>,
    pub observed_at_ms: Option<u64>,
    pub scope: &'static str,
    pub model_identity: &'static str,
    pub capabilities: &'static str,
    pub readiness: &'static str,
    pub concurrency: &'static str,
    pub admission_status: &'static str,
}
fn decode(value: Value) -> Option<(Vec<String>, bool)> {
    let object = value.as_object()?;
    let data = object.get("data")?.as_array()?;
    if object
        .get("object")
        .is_some_and(|v| v.as_str() != Some("list"))
    {
        return None;
    }
    let has_more = match object.get("has_more") {
        None => false,
        Some(v) => v.as_bool()?,
    };
    let mut ids = BTreeSet::new();
    // Every element is checked, but only IDs survive. The transport byte bound
    // also bounds traversal of discarded arbitrary upstream metadata.
    for row in data {
        let row = row.as_object()?;
        if row
            .get("object")
            .is_some_and(|v| v.as_str() != Some("model"))
        {
            return None;
        }
        let id = row.get("id")?.as_str()?;
        if !identifier(id) || !ids.insert(id.to_owned()) {
            return None;
        }
    }
    let truncated = has_more || ids.len() > MAX_MODELS;
    Some((ids.into_iter().take(MAX_MODELS).collect(), truncated))
}
fn failure(code: Code) -> DiscoveryState {
    match code {
        Code::UpstreamEndpointUnavailable | Code::UnsupportedControl => DiscoveryState::Unsupported,
        Code::ResponseTooLarge => DiscoveryState::LimitExceeded,
        Code::UpstreamTimeout => DiscoveryState::TimedOut,
        Code::UpstreamProtocol => DiscoveryState::InvalidResponse,
        _ => DiscoveryState::Unavailable,
    }
}
/// One explicit pre-save read. It is not retained probe or conformance evidence.
#[derive(Serialize)]
pub struct ModelsPreview {
    pub state: DiscoveryState,
    pub model_ids: Vec<String>,
    pub truncated: bool,
    pub observed_at_ms: u64,
    pub model_identity: &'static str,
    pub capabilities: &'static str,
    pub readiness: &'static str,
}
pub async fn preview_models(connection: HttpConnection) -> ModelsPreview {
    let (state, models) = observe(connection, 10_000).await;
    let (model_ids, truncated) = models.unwrap_or_default();
    ModelsPreview {
        state,
        model_ids,
        truncated,
        observed_at_ms: crate::supervisor::unix_ms(),
        model_identity: "not_verified",
        capabilities: "not_verified",
        readiness: "not_verified",
    }
}
async fn observe(
    connection: HttpConnection,
    timeout_ms: u64,
) -> (DiscoveryState, Option<(Vec<String>, bool)>) {
    match tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        connection
            .send(Operation::Models, None)
            .await?
            .collect_json()
            .await
    })
    .await
    {
        Err(_) => (DiscoveryState::TimedOut, None),
        Ok(Err(error)) => (failure(error.code), None),
        Ok(Ok(value)) => match decode(value) {
            None => (DiscoveryState::InvalidResponse, None),
            Some(found) => (DiscoveryState::Listed, Some(found)),
        },
    }
}
impl Store {
    /// Read the original retained observation. This never creates a client,
    /// resolves a credential, retries Pending, or updates a declaration.
    pub fn inspect_connection(&self, include_model_ids: bool) -> Result<InventoryReview> {
        let guard = store::Guard::open(&self.directory)?;
        let inventory: Inventory = guard.read_json(FILE)?.ok_or(Error::DiscoveryMissing)?;
        inventory.review(include_model_ids)
    }
    /// One explicit bounded GET. Revision 0 creates the inventory; subsequent
    /// calls require its exact revision and explicitly authorize another read.
    /// The lock serializes discovery with setup/probes, including across processes.
    pub async fn discover(
        &self,
        connection_file: &Path,
        expected_revision: u64,
        timeout_ms: u64,
    ) -> Result<InventoryReview> {
        require(connection_file.is_absolute() && (1..=10_000).contains(&timeout_ms))?;
        let mut config = ConnectionConfig::load(connection_file).map_err(|_| Error::Protection)?;
        let fingerprint = config.fingerprint().map_err(|_| Error::Invalid)?;
        let guard = store::Guard::open(&self.directory)?;
        let old: Option<Inventory> = guard.read_json(FILE)?;
        let (id, revision) = if let Some(old) = old {
            old.validate()?;
            if old.revision != expected_revision {
                return Err(Error::Conflict);
            }
            (old.id, old.revision)
        } else {
            if expected_revision != 0 {
                return Err(Error::Conflict);
            }
            let mut nonce = [0u8; 32];
            getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
            (
                Digest::hash("mayhem/proxy/setup-discovery/v1", &[&nonce]),
                0,
            )
        };
        // Reserve both revisions before any request: no successful I/O can be
        // followed by an arithmetic failure that leaves a misleading old result.
        let final_revision = revision
            .checked_add(2)
            .filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
            .ok_or(Error::Invalid)?;
        let mut configured_endpoints = Vec::new();
        for (operation, endpoint) in [
            (Operation::ChatCompletions, ProxyEndpoint::Chat),
            (Operation::Completions, ProxyEndpoint::Completions),
            (Operation::Responses, ProxyEndpoint::Responses),
            (Operation::Decisions, ProxyEndpoint::Decisions),
        ] {
            if config.paths.contains_key(&operation) {
                configured_endpoints.push(endpoint);
            }
        }
        configured_endpoints.sort();
        let mut inventory = Inventory {
            schema_version: 1,
            id,
            revision: revision + 1,
            connection_file: connection_file.to_owned(),
            connection: fingerprint,
            connection_revision: config.revision,
            configured_endpoints,
            state: DiscoveryState::Pending,
            models: vec![],
            truncated: false,
            observed_at_ms: None,
        };
        inventory.validate()?;
        guard.write_json(FILE, TEMPORARY, &inventory)?;
        let mut models = None;
        inventory.state = if !config.paths.contains_key(&Operation::Models) {
            DiscoveryState::Unsupported
        } else {
            // Clamp only this read's transport resources. The retained binding
            // remains to the complete original configuration, not these limits.
            config.limits.max_in_flight = 1;
            config.limits.max_response_bytes = config.limits.max_response_bytes.min(MAX_RESPONSE);
            config.limits.connect_timeout_ms = config.limits.connect_timeout_ms.min(timeout_ms);
            config.limits.read_idle_timeout_ms = Some(
                config
                    .limits
                    .read_idle_timeout_ms
                    .unwrap_or(timeout_ms)
                    .min(timeout_ms),
            );
            match HttpConnection::new(config) {
                Err(_) => DiscoveryState::Unavailable,
                Ok(connection) => {
                    let (state, found) = observe(connection, timeout_ms).await;
                    models = found;
                    state
                }
            }
        };
        if !inventory.current() {
            inventory.state = DiscoveryState::ConfigurationChanged;
        } else if let Some((found, truncated)) = models {
            inventory.models = found;
            inventory.truncated = truncated;
        }
        inventory.revision = final_revision;
        inventory.observed_at_ms = Some(crate::supervisor::unix_ms());
        inventory.validate()?;
        guard.write_json(FILE, TEMPORARY, &inventory)?;
        inventory.review(false)
    }
}
