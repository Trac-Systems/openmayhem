use std::{collections::BTreeMap, time::Duration};

use mayhem_proto::proxy::{
    ProxyMarketDescriptor, ProxyMembership, ProxyOffer, PROXY_MAX_SAFE_INTEGER,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::{invalid, require, Error, Result};

pub const CATALOG_PREFIX: &str = "proxy/v1/catalog/";
pub const MAX_PAGE_ENTRIES: usize = 100;
pub const MAX_PAGE_ENTRY_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_BYTES: usize = MAX_PAGE_ENTRY_BYTES + 16 * 1024;
pub const MAX_RECORD_BYTES: usize = 20_480; // Wire record plus its public wrapper.

pub(crate) fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub(crate) fn safe_integer(value: u64) -> bool {
    value <= PROXY_MAX_SAFE_INTEGER
}

pub(crate) fn token(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    value.len() <= 4096
        && parts.len() == 3
        && parts[0] == "pdc1"
        && !parts[1].is_empty()
        && parts[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && parts[2].len() == 128
        && parts[2]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub network_id: String,
    pub msb_bootstrap: String,
    pub subnet_bootstrap: String,
    pub contract_version: u32,
}

impl Identity {
    pub fn validate(&self) -> Result<()> {
        require(
            !self.network_id.is_empty()
                && self.network_id.len() <= 128
                && self.network_id.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                })
                && self.contract_version > 0
                && hex(&self.msb_bootstrap)
                && hex(&self.subnet_bootstrap),
            "invalid network identity",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub network_id: String,
    pub msb_bootstrap: String,
    pub subnet_bootstrap: String,
    pub contract_version: u32,
    pub epoch: u64,
}

impl Context {
    pub fn identity(&self) -> Identity {
        Identity {
            network_id: self.network_id.clone(),
            msb_bootstrap: self.msb_bootstrap.clone(),
            subnet_bootstrap: self.subnet_bootstrap.clone(),
            contract_version: self.contract_version,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub view_key: String,
    pub fork: u64,
    pub signed_length: u64,
    pub tree_hash: String,
}

impl Proof {
    pub fn validate(&self) -> Result<()> {
        require(
            hex(&self.view_key)
                && hex(&self.tree_hash)
                && self.signed_length > 0
                && safe_integer(self.signed_length)
                && safe_integer(self.fork),
            "invalid signed snapshot identity",
        )
    }

    pub fn follows(&self, older: &Self) -> bool {
        self.view_key == older.view_key
            && self.fork == older.fork
            && self.signed_length >= older.signed_length
            && (self.signed_length != older.signed_length || self.tree_hash == older.tree_hash)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryBinding {
    pub kind: String,
    pub filter: BTreeMap<String, String>,
    pub lookup: Option<String>,
    pub limit: usize,
}

impl QueryBinding {
    pub fn catalog() -> Self {
        Self {
            kind: "catalog".into(),
            filter: BTreeMap::new(),
            lookup: None,
            limit: MAX_PAGE_ENTRIES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    #[serde(flatten)]
    pub binding: QueryBinding,
    pub cursor: Option<String>,
    pub since: Option<String>,
}

impl Query {
    pub fn catalog() -> Self {
        Self {
            binding: QueryBinding::catalog(),
            cursor: None,
            since: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        require(
            self.binding == QueryBinding::catalog(),
            "hydration requires the complete catalog query",
        )?;
        require(
            !(self.cursor.is_some() && self.since.is_some())
                && self.cursor.as_deref().is_none_or(token)
                && self.since.as_deref().is_none_or(token),
            "invalid hydration cursor",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Snapshot,
    Changes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub key: String,
    pub value: Value,
}

fn integer(value: &Value, minimum: u64) -> bool {
    value
        .as_u64()
        .is_some_and(|n| n >= minimum && safe_integer(n))
}

fn shape(value: &Value, fields: &[&str]) -> Result<()> {
    require(
        value.as_object().is_some_and(|map| {
            map.len() == fields.len() && map.keys().all(|key| fields.contains(&key.as_str()))
        }),
        "unexpected public catalog fields",
    )
}

fn sorted_strings(value: &Value, maximum: usize, valid: impl Fn(&str) -> bool) -> bool {
    value.as_array().is_some_and(|items| {
        !items.is_empty()
            && items.len() <= maximum
            && items.iter().all(|item| item.as_str().is_some_and(&valid))
            && items
                .windows(2)
                .all(|pair| pair[0].as_str() < pair[1].as_str())
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MembershipRecord {
    active: bool,
    revision: u64,
    member: ProxyMembership,
    offer_slots: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OfferRecord {
    active: bool,
    revision: u64,
    digest: String,
    offer: ProxyOffer,
}

impl Entry {
    /// Local shape/digest checks, not proof of capacity or paid admission. Exact
    /// cross-record eligibility must be checked against current state at dispatch.
    pub fn validate(&self, mode: Mode) -> Result<()> {
        let suffix = self
            .key
            .strip_prefix(CATALOG_PREFIX)
            .ok_or_else(|| invalid("catalog key escaped public namespace"))?;
        require(self.key.len() <= 256, "catalog key is too long")?;
        let parts: Vec<_> = suffix.split('/').collect();
        let valid_key = match parts.first().copied() {
            Some(
                "markets" | "providers" | "endpoints" | "metering" | "provider_status"
                | "admission_status",
            ) => parts.len() == 2 && hex(parts[1]),
            Some("memberships") => parts.len() == 3 && parts[1..].iter().all(|p| hex(p)),
            Some("offers") => parts.len() == 4 && parts[1..].iter().all(|p| hex(p)),
            Some("families") => parts.len() == 2 && identifier(parts[1]),
            Some("network") => parts == ["network", "current"],
            _ => false,
        };
        require(valid_key, "unknown or malformed catalog identity")?;
        require(
            serde_json::to_vec(self)?.len() <= MAX_RECORD_BYTES,
            "catalog record is too large",
        )?;
        if self.value.is_null() {
            return require(mode == Mode::Changes, "snapshot cannot contain deletions");
        }
        require(self.value.is_object(), "catalog value must be an object")?;
        match parts[0] {
            "markets" => {
                let market: ProxyMarketDescriptor = serde_json::from_value(self.value.clone())?;
                require(
                    market.id().map_err(invalid)? == parts[1],
                    "market digest differs from catalog key",
                )?;
            }
            "memberships" => {
                let record: MembershipRecord = serde_json::from_value(self.value.clone())?;
                record.member.validate().map_err(invalid)?;
                require(
                    record.member.market_id == parts[1]
                        && record.member.provider_pubkey == parts[2]
                        && record.revision >= record.member.revision
                        && safe_integer(record.revision)
                        && safe_integer(record.offer_slots)
                        && (!record.active || record.revision == record.member.revision),
                    "membership identity/revision mismatch",
                )?;
            }
            "offers" => {
                let record: OfferRecord = serde_json::from_value(self.value.clone())?;
                record.offer.validate().map_err(invalid)?;
                require(
                    record.offer.market_id == parts[1]
                        && record.offer.provider_pubkey == parts[2]
                        && record.offer.slot_id().map_err(invalid)? == parts[3]
                        && record.revision >= record.offer.revision
                        && safe_integer(record.revision)
                        && (!record.active || record.revision == record.offer.revision)
                        && record.offer.digest().map_err(invalid)? == record.digest,
                    "offer identity/digest/revision mismatch",
                )?;
            }
            "providers" => {
                shape(
                    &self.value,
                    &[
                        "provider_pubkey",
                        "admission_id",
                        "sequence",
                        "active_memberships",
                    ],
                )?;
                require(
                    self.value["provider_pubkey"].as_str() == Some(parts[1])
                        && self.value["admission_id"].as_str().is_some_and(hex)
                        && integer(&self.value["sequence"], 1)
                        && integer(&self.value["active_memberships"], 0),
                    "invalid provider summary",
                )?;
            }
            "provider_status" | "admission_status" => {
                shape(&self.value, &["revision", "reason_hash"])?;
                require(
                    integer(&self.value["revision"], 1)
                        && self.value["reason_hash"].as_str().is_some_and(hex),
                    "invalid revocation summary",
                )?;
            }
            "families" => {
                shape(&self.value, &["enabled", "label"])?;
                require(
                    self.value["enabled"].is_boolean()
                        && self.value["label"].as_str().is_some_and(|s| {
                            !s.is_empty()
                                && s.len() <= 128
                                && s.bytes().all(|b| (32..=126).contains(&b))
                        }),
                    "invalid family summary",
                )?;
            }
            "network" => {
                let mut identity = self.value.clone();
                let enabled = identity.as_object_mut().unwrap().remove("enabled");
                require(
                    enabled.is_some_and(|v| v.is_boolean()),
                    "invalid public network policy status",
                )?;
                serde_json::from_value::<Identity>(identity)?.validate()?;
            }
            "endpoints" => {
                shape(
                    &self.value,
                    &[
                        "enabled",
                        "endpoint",
                        "family",
                        "max_context",
                        "ctx_brackets",
                        "outcome_classes",
                    ],
                )?;
                let decision = self.value["endpoint"] == "mayhem_decisions";
                let known = decision
                    || self.value["endpoint"].as_str().is_some_and(|s| {
                        matches!(
                            s,
                            "openai_chat_completions" | "openai_completions" | "openai_responses"
                        )
                    });
                require(
                    self.value["enabled"].is_boolean()
                        && known
                        && self.value["family"] == if decision { "decisions" } else { "llm" }
                        && self.value["max_context"]
                            .as_u64()
                            .is_some_and(|n| n > 0 && n <= u32::MAX as u64)
                        && sorted_strings(&self.value["ctx_brackets"], 32, identifier)
                        && sorted_strings(&self.value["outcome_classes"], 32, |s| {
                            s.is_empty() || (decision && hex(s))
                        }),
                    "invalid public endpoint policy",
                )?;
            }
            "metering" => {
                shape(&self.value, &["enabled", "units"])?;
                require(
                    self.value["enabled"].is_boolean()
                        && sorted_strings(&self.value["units"], 16, identifier),
                    "invalid public metering policy",
                )?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub ok: bool,
    pub lane: String,
    pub schema_version: u32,
    pub request_nonce: String,
    pub query: QueryBinding,
    pub context: Context,
    pub proof: Proof,
    pub base_proof: Option<Proof>,
    pub mode: Mode,
    pub entries: Vec<Entry>,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub checkpoint: Option<String>,
}

impl Page {
    pub fn validate(&self, identity: &Identity) -> Result<()> {
        identity.validate()?;
        if self.context.identity() != *identity {
            return Err(Error::Identity);
        }
        require(
            self.ok
                && self.lane == "proxy"
                && self.schema_version == 1
                && hex(&self.request_nonce)
                && safe_integer(self.context.epoch)
                && self.query == QueryBinding::catalog(),
            "invalid discovery response binding",
        )?;
        self.proof.validate()?;
        if let Some(base) = &self.base_proof {
            base.validate()?;
            require(
                self.proof.follows(base),
                "delta snapshot regressed or changed fork",
            )?;
        }
        require(
            (self.mode == Mode::Changes) == self.base_proof.is_some(),
            "invalid discovery mode/base proof",
        )?;
        require(
            self.entries.len() <= MAX_PAGE_ENTRIES
                && serde_json::to_vec(self)?.len() <= MAX_RESPONSE_BYTES,
            "discovery page exceeds bounds",
        )?;
        let mut total = 0;
        let mut previous = None;
        for entry in &self.entries {
            entry.validate(self.mode)?;
            if entry.key == format!("{CATALOG_PREFIX}network/current") && !entry.value.is_null() {
                let mut network = entry.value.clone();
                network.as_object_mut().unwrap().remove("enabled");
                if serde_json::from_value::<Identity>(network)? != *identity {
                    return Err(Error::Identity);
                }
            }
            total += serde_json::to_vec(entry)?.len();
            require(
                previous.is_none_or(|key: &str| key < entry.key.as_str()),
                "discovery keys are not strictly increasing",
            )?;
            previous = Some(entry.key.as_str());
        }
        require(
            total <= MAX_PAGE_ENTRY_BYTES,
            "discovery entry bytes exceed bound",
        )?;
        require(
            if self.truncated {
                !self.entries.is_empty()
                    && self.next_cursor.as_deref().is_some_and(token)
                    && self.checkpoint.is_none()
            } else {
                self.next_cursor.is_none() && self.checkpoint.as_deref().is_some_and(token)
            },
            "invalid discovery pagination state",
        )
    }
}

/// This connects to the operator-configured trusted peer RPC, whose signed service
/// transport authenticates the canonical admin. A public arbitrary HTTP server is
/// NOT an equivalent trust source. Redirects are forbidden and bodies are bounded.
pub struct DiscoveryClient {
    http: reqwest::Client,
    endpoint: Url,
    identity: Identity,
}

impl DiscoveryClient {
    pub fn new(rpc_base: &str, identity: Identity) -> Result<Self> {
        Self::with_timeout(rpc_base, identity, Duration::from_secs(20))
    }

    pub fn with_timeout(rpc_base: &str, identity: Identity, timeout: Duration) -> Result<Self> {
        identity.validate()?;
        require(!timeout.is_zero(), "discovery timeout must be bounded")?;
        let base = format!("{}/", rpc_base.trim_end_matches('/'));
        let url = Url::parse(&base).map_err(|_| invalid("invalid peer RPC URL"))?;
        require(
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid peer RPC URL",
        )?;
        let endpoint = url
            .join("proxy/discovery")
            .map_err(|_| invalid("invalid peer RPC path"))?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(Error::Transport)?;
        Ok(Self {
            http,
            endpoint,
            identity,
        })
    }

    pub async fn page(&self, query: &Query) -> Result<Page> {
        query.validate()?;
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .json(&serde_json::json!({ "query": query }))
            .send()
            .await
            .map_err(Error::Transport)?;
        let status = response.status();
        require(
            response
                .content_length()
                .is_none_or(|n| n <= MAX_RESPONSE_BYTES as u64),
            "discovery response body exceeds bound",
        )?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            require(
                body.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES,
                "discovery response body exceeds bound",
            )?;
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let value = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let code = value["code"]
                .as_str()
                .filter(|code| {
                    matches!(
                        *code,
                        "proxy_cursor_expired"
                            | "proxy_cursor_invalidated"
                            | "proxy_discovery_invalid"
                            | "proxy_discovery_busy"
                            | "proxy_discovery_timeout"
                            | "proxy_discovery_unavailable"
                    )
                })
                .unwrap_or("proxy_discovery_unavailable")
                .to_owned();
            return Err(Error::Http {
                status: status.as_u16(),
                code,
            });
        }
        let page: Page = serde_json::from_slice(&body)?;
        page.validate(&self.identity)?;
        Ok(page)
    }
}
