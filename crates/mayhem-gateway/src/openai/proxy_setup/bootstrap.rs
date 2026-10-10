//! Host-owned first-create authority. Browser input contains choices, never paths,
//! wallet material, network identity, signing authority or lifecycle callbacks.
use super::*;
mod guide;
use mayhem_proto::proxy::{finance::ProxySettlementPolicy, ProxyEndpoint, ProxyRail};
use mayhem_proxy::{
    connector::config::NetworkPolicy,
    managed::Tokenizer,
    setup::{
        bootstrap::{self, Choices, Credential, Host},
        FlowConfig, OfferInput, ProfileMarket, RunLifecycle,
    },
};
use std::{collections::BTreeMap, path::PathBuf};
use zeroize::Zeroizing;

/// Installed only by the trusted CLI host. These references are never returned
/// to a browser. Opaque IDs select an explicitly approved local asset.
pub struct BootstrapConfig {
    pub destination: PathBuf,
    pub host: Host,
    pub tokenizers: BTreeMap<String, Tokenizer>,
    pub credentials: BTreeMap<String, PathBuf>,
    pub lifecycle: Arc<dyn RunLifecycle>,
}
impl BootstrapConfig {
    pub(super) fn validate(&self) -> Result<(), String> {
        if !self.destination.is_absolute()
            || self.tokenizers.len() > 16
            || self.credentials.len() > 16
            || self
                .tokenizers
                .keys()
                .chain(self.credentials.keys())
                .any(|s| !valid_id(s))
        {
            return Err("invalid protected setup bootstrap configuration".into());
        }
        self.host
            .network
            .validate()
            .map_err(|_| "invalid setup network")?;
        Ok(())
    }
    pub(super) fn restore(&self) -> Result<Option<Arc<Flow>>, String> {
        match std::fs::symlink_metadata(&self.destination) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            _ => return Err("setup original unavailable; inspect without resetting".into()),
        }
        let config = FlowConfig::load(&self.destination.join("wizard.json"))
            .map_err(|_| "setup original unavailable; inspect without resetting")?;
        if config.profile.provider_pubkey != self.host.provider_pubkey
            || config.profile.network != self.host.network
            || config.peer_rpc.as_deref() != Some(self.host.peer_rpc.as_str())
            || config.admission_origin != self.host.admission_origin
            || config.declaration_registry != self.host.declaration_registry
        {
            return Err("setup original host binding mismatch".into());
        }
        let flow = Flow::open(config)
            .map_err(|_| "setup original unavailable; inspect without resetting")?
            .with_run_lifecycle(self.lifecycle.clone());
        Ok(Some(Arc::new(flow)))
    }
    pub(super) fn public(&self) -> Value {
        json!({"schema_version":1,"profile":"bounded_single_connection_v1",
            "network":self.host.network,"provider_pubkey":self.host.provider_pubkey,
            "tokenizers":self.tokenizers.iter().map(|(id,t)| json!({"id":id,"digest":t.digest})).collect::<Vec<_>>(),
            "credential_references":self.credentials.keys().collect::<Vec<_>>(),
            "admission_configured":self.host.admission_origin.is_some(),
            "authorizes_probe":false,"authorizes_publication":false,"authorizes_run":false})
    }
    fn credential(&self, input: CredentialInput) -> Result<Credential, &'static str> {
        match input {
            CredentialInput::None => Ok(Credential::None),
            CredentialInput::BearerValue { value } => Ok(Credential::BearerValue(Zeroizing::new(
                value.as_bytes().to_vec(),
            ))),
            CredentialInput::Reference { id } => self
                .credentials
                .get(&id)
                .cloned()
                .map(Credential::BearerFile)
                .ok_or("setup_unapproved_credential_reference"),
        }
    }
    fn choices(&self, input: Input) -> Result<Choices, &'static str> {
        if input.schema_version != 1 || input.offers.len() != 1 || input.offers[0].revision != 1 {
            return Err("setup_invalid_choices");
        }
        let tokenizer = if input.tokenizer_id == "none" {
            None
        } else {
            let t = self
                .tokenizers
                .get(&input.tokenizer_id)
                .ok_or("setup_unapproved_tokenizer")?;
            Some(Tokenizer {
                file: t.file.clone(),
                digest: t.digest.clone(),
                limits: t.limits.clone(),
            })
        };
        let credential = self.credential(input.credential)?;
        Ok(Choices {
            base_url: input.base_url,
            network_policy: input.network_policy,
            credential,
            endpoint: input.endpoint,
            upstream_model: input.upstream_model,
            market: input.market,
            served_context: input.served_context,
            concurrency: input.concurrency,
            offers: input.offers,
            accepted_rails: input.accepted_rails,
            sequence: input.sequence,
            settlement_policy: input.settlement_policy,
            probe_budget: input.probe_budget,
            probe_output_limit: input.probe_output_limit,
            probe_timeout_ms: input.probe_timeout_ms,
            allow_recovery_probes: input.allow_recovery_probes,
            tokenizer,
            closed_retention_ms: input.closed_retention_ms,
        })
    }
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s != "none"
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn secret<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(d).map(Zeroizing::new)
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CredentialInput {
    None,
    BearerValue {
        #[serde(deserialize_with = "secret")]
        value: Zeroizing<String>,
    },
    Reference {
        id: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    schema_version: u32,
    base_url: String,
    network_policy: NetworkPolicy,
    credential: CredentialInput,
    endpoint: ProxyEndpoint,
    upstream_model: String,
    market: ProfileMarket,
    served_context: u32,
    concurrency: u32,
    offers: Vec<OfferInput>,
    accepted_rails: Vec<ProxyRail>,
    sequence: u64,
    settlement_policy: ProxySettlementPolicy,
    probe_budget: mayhem_proxy::capacity::probes::Budget,
    probe_output_limit: u64,
    probe_timeout_ms: u64,
    allow_recovery_probes: bool,
    tokenizer_id: String,
    closed_retention_ms: u64,
}
/// A canceled HTTP future detaches blocking work; the permit must remain with
/// that work until it actually finishes, including while queued for a thread.
pub(super) fn spawn_creation<T: Send + 'static>(
    permit: tokio::sync::OwnedSemaphorePermit,
    work: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
}
pub(crate) async fn create(State(state): State<SharedState>, request: Request) -> Response {
    let control = match mutation_access(&state, &request) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(config) = control.bootstrap.clone() else {
        return failure(StatusCode::CONFLICT, "setup_original_exists");
    };
    let Ok(permit) = control.create_gate.clone().try_acquire_owned() else {
        return failure(StatusCode::CONFLICT, "setup_creation_in_progress");
    };
    if control.flow.get().is_some() {
        return failure(StatusCode::CONFLICT, "setup_original_exists");
    }
    let bytes = match body(request).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let input = match serde_json::from_slice::<Input>(&bytes) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "setup_invalid_choices"),
    };
    let choices = match config.choices(input) {
        Ok(v) => v,
        Err(e) => return failure(StatusCode::BAD_REQUEST, e),
    };
    // Restore the original first; its recovery never depends on fresh discovery.
    let restore_config = config.clone();
    let restored = tokio::task::spawn_blocking(move || {
        let result = restore_config.restore();
        (permit, result)
    })
    .await;
    let (permit, original) = match restored {
        Ok((permit, Ok(original))) => (permit, original),
        _ => return failure(StatusCode::CONFLICT, "setup_create_failed_inspect_original"),
    };
    let result = if let Some(flow) = original {
        Ok(Ok((flow, false)))
    } else {
        let canonical = match mayhem_proxy::setup::guided::Canonical::new(&config.host) {
            Ok(v) => v,
            Err(_) => {
                return failure(
                    StatusCode::CONFLICT,
                    "setup_selection_changed_or_unavailable",
                )
            }
        };
        if let Err(error) = canonical.revalidate(&choices).await {
            return failure(
                StatusCode::CONFLICT,
                match error {
                    mayhem_proxy::setup::Error::Bootstrap("market already exists; choose join") => {
                        "setup_market_exists_choose_join"
                    }
                    _ => "setup_selection_changed_or_unavailable",
                },
            );
        }
        spawn_creation(permit, move || {
            // Another process may have committed while these reads were pending.
            // Retain the original; never overwrite it with newly selected terms.
            if let Some(flow) = config.restore()? {
                return Ok((flow, false));
            }
            bootstrap::create(&config.destination, config.host.clone(), choices)
                .map_err(|_| "setup_create_failed_inspect_original".to_owned())?;
            config
                .restore()?
                .map(|v| (v, true))
                .ok_or_else(|| "setup_original_unavailable".to_owned())
        })
        .await
    };
    match result {
        Ok(Ok((flow, created))) => {
            let _ = control.flow.set(flow);
            dashboard_json_response(
                StatusCode::OK,
                json!({"schema_version":1,"created":created,
                "resume":"/mayhem/dashboard/provider/setup","authorizes_probe":false,
                "authorizes_publication":false,"authorizes_run":false}),
                None,
            )
        }
        _ => failure(StatusCode::CONFLICT, "setup_create_failed_inspect_original"),
    }
}
#[cfg(test)]
mod tests;

pub(crate) use guide::read as guided_read;
