//! Revisioned capability/filter/control semantics. This module performs no
//! discovery, network, inference, finance, or ledger work. Registry data cannot
//! confer operator trust or implement a new endpoint/billing protocol.
mod controls;
mod values;
pub use controls::{apply_controls, Control};
pub use values::TypedValue;

use crate::{invalid, require, Result};
use mayhem_proto::{proxy::ProxyEndpoint, stable_json_bytes};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_DEFINITION_BYTES: usize = 16 * 1024;
pub const MAX_FIELDS_PER_REQUEST: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Eq,
    Gte,
    Lte,
    ContainsAll,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueSchema {
    Boolean,
    Enum { values: Vec<String> },
    Set { values: Vec<String> },
    Integer { minimum: i64, maximum: i64 },
    Decimal { minimum: String, maximum: String },
    Text { min_length: u32, max_length: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Usage {
    FilterOnly,
    /// Only an existing accepted public endpoint field, never an upstream URL,
    /// header, private native parameter, price, model or routing identity.
    RequestControl {
        request_path: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub field_id: String,
    pub schema_revision: u32,
    pub operator: Operator,
    pub value: TypedValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// All when conditions must match. Missing values are unknown, not defaults.
    pub when: Vec<Condition>,
    pub require: Vec<Condition>,
    /// Any matching forbidden condition rejects the combination.
    pub forbid: Vec<Condition>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub schema_version: u32,
    pub field_id: String,
    pub schema_revision: u32,
    pub labels: BTreeMap<String, String>,
    pub help: BTreeMap<String, String>,
    #[serde(deserialize_with = "required_nullable")]
    pub units: Option<String>,
    pub group: String,
    pub order: i32,
    pub endpoints: Vec<ProxyEndpoint>,
    pub value_schema: ValueSchema,
    /// UI advice only; execution never injects defaults or changes user intent.
    #[serde(deserialize_with = "required_nullable")]
    pub default: Option<TypedValue>,
    pub operators: Vec<Operator>,
    /// Registry requirements are independent hard bounds. A profile may narrow
    /// them but cannot turn fresh/probed evidence into an unlimited claim.
    pub minimum_assurance: Assurance,
    #[serde(deserialize_with = "required_nullable")]
    pub max_evidence_age_ms: Option<u32>,
    pub usage: Usage,
    pub rules: Vec<Rule>,
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.:-".contains(&b))
}
fn ordered<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|p| p[0] < p[1])
}
fn text(value: &str, max_chars: usize, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control)
}
fn localized(value: &BTreeMap<String, String>, required: bool) -> bool {
    (!required || !value.is_empty())
        && value.len() <= 32
        && value.iter().all(|(locale, v)| {
            !locale.is_empty()
                && locale.len() <= 32
                && locale
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && text(v, 1024, 4096)
        })
}

impl Definition {
    pub fn validate(&self) -> Result<()> {
        require(
            serde_json::to_vec(self)?.len() <= MAX_DEFINITION_BYTES
                && self.schema_version == 1
                && self.schema_revision > 0
                && self.schema_revision <= i32::MAX as u32
                && identifier(&self.field_id)
                && identifier(&self.group)
                && self.units.as_ref().is_none_or(|v| identifier(v))
                && localized(&self.labels, true)
                && localized(&self.help, false),
            "invalid registry definition metadata",
        )?;
        require(
            !self.endpoints.is_empty() && self.endpoints.len() <= 4 && ordered(&self.endpoints),
            "invalid registry endpoints",
        )?;
        require(
            self.max_evidence_age_ms != Some(0),
            "invalid registry freshness requirement",
        )?;
        self.value_schema.validate()?;
        require(
            !self.operators.is_empty()
                && self.operators.len() <= 4
                && ordered(&self.operators)
                && self
                    .operators
                    .iter()
                    .all(|op| self.value_schema.supports(*op)),
            "invalid registry filter operators",
        )?;
        if let Some(value) = &self.default {
            self.value_schema.accepts(value)?;
        }
        if let Usage::RequestControl { request_path } = &self.usage {
            require(
                controls::safe_path(request_path),
                "registry control path is protected or unsupported",
            )?;
        }
        require(self.rules.len() <= 8, "too many registry conditions")?;
        for rule in &self.rules {
            require(
                !rule.when.is_empty()
                    && rule.when.len() <= 8
                    && rule.require.len() <= 8
                    && rule.forbid.len() <= 8
                    && (!rule.require.is_empty() || !rule.forbid.is_empty()),
                "invalid registry condition complexity",
            )?;
            for condition in rule.when.iter().chain(&rule.require).chain(&rule.forbid) {
                condition.validate()?;
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        let mut bytes = b"mayhem/proxy/registry-definition/v1\0".to_vec();
        bytes.extend(stable_json_bytes(&serde_json::to_value(self)?)?);
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }

    /// A revision is immutable in semantic meaning, units and allowed values.
    /// UI label/help/order/group edits may reuse a semantic revision. Retained
    /// old revisions remain distinct; never reinterpret a saved profile.
    pub fn check_successor(&self, next: &Self) -> Result<()> {
        self.validate()?;
        next.validate()?;
        require(
            self.field_id == next.field_id
                && next.schema_revision >= self.schema_revision
                && next.schema_revision <= self.schema_revision + 1,
            "registry identity or revision changed backwards",
        )?;
        if self.schema_revision == next.schema_revision {
            require(
                self.units == next.units
                    && self.endpoints == next.endpoints
                    && self.value_schema == next.value_schema
                    && self.default == next.default
                    && self.operators == next.operators
                    && self.minimum_assurance == next.minimum_assurance
                    && self.max_evidence_age_ms == next.max_evidence_age_ms
                    && self.usage == next.usage
                    && self.rules == next.rules,
                "registry semantics require a new revision",
            )?;
        }
        Ok(())
    }
}

impl Condition {
    fn validate(&self) -> Result<()> {
        require(
            identifier(&self.field_id)
                && self.schema_revision > 0
                && self.schema_revision <= i32::MAX as u32,
            "invalid registry field reference",
        )?;
        self.value.validate()?;
        require(
            self.value.supports(self.operator),
            "invalid registry predicate operator",
        )
    }
    fn check_definition(&self, definition: &Definition, endpoint: ProxyEndpoint) -> Result<()> {
        definition.validate()?;
        self.check_reference(definition, endpoint)
    }
    fn check_reference(&self, definition: &Definition, endpoint: ProxyEndpoint) -> Result<()> {
        self.validate()?;
        require(
            self.field_id == definition.field_id
                && self.schema_revision == definition.schema_revision
                && definition.endpoints.contains(&endpoint)
                && definition.operators.contains(&self.operator),
            "registry field revision or endpoint is unsupported",
        )?;
        definition.value_schema.accepts(&self.value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assurance {
    Declared,
    Probed,
    Verified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Unknown,
    Unsupported,
    Supported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    pub field_id: String,
    pub schema_revision: u32,
    pub operator: Operator,
    pub value: TypedValue,
    pub evidence: Assurance,
    #[serde(deserialize_with = "required_nullable")]
    pub max_age_ms: Option<u32>,
}

fn required_nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

impl Predicate {
    /// Validate persisted intent independently from resolving a known definition.
    /// Passing this does not establish support, trust, freshness or authority.
    pub fn validate(&self) -> Result<()> {
        Condition {
            field_id: self.field_id.clone(),
            schema_revision: self.schema_revision,
            operator: self.operator,
            value: self.value.clone(),
        }
        .validate()?;
        require(self.max_age_ms != Some(0), "invalid registry evidence time")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub field_id: String,
    pub schema_revision: u32,
    pub endpoint: ProxyEndpoint,
    pub status: Support,
    pub value: Option<TypedValue>,
    pub observed_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub source_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    Satisfied,
    Unknown,
    Unsupported,
    Stale,
    InsufficientEvidence,
    DifferentValue,
}

/// `assessed_assurance` comes from the caller's authenticated evidence store,
/// NOT from provider JSON or taxonomy metadata. Unknown never satisfies a hard
/// requirement. This function cannot grant T4 or prove remote model identity.
pub fn evaluate(
    definition: &Definition,
    predicate: &Predicate,
    observation: Option<&Observation>,
    assessed_assurance: Assurance,
    endpoint: ProxyEndpoint,
    now_ms: u64,
) -> Result<Match> {
    predicate.validate()?;
    let condition = Condition {
        field_id: predicate.field_id.clone(),
        schema_revision: predicate.schema_revision,
        operator: predicate.operator,
        value: predicate.value.clone(),
    };
    condition.check_definition(definition, endpoint)?;
    require(
        predicate.max_age_ms != Some(0) && now_ms <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER,
        "invalid registry evidence time",
    )?;
    let Some(observation) = observation else {
        return Ok(Match::Unknown);
    };
    require(
        observation.field_id == predicate.field_id
            && observation.schema_revision == predicate.schema_revision
            && observation.endpoint == endpoint
            && identifier(&observation.source_id)
            && observation.observed_at_ms <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
            && observation.expires_at_ms.is_none_or(|t| {
                t > observation.observed_at_ms && t <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
            }),
        "registry observation binding differs",
    )?;
    require(
        (observation.status == Support::Supported) == observation.value.is_some(),
        "invalid registry support value",
    )?;
    let max_age = match (definition.max_evidence_age_ms, predicate.max_age_ms) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    if observation.observed_at_ms > now_ms
        || observation.expires_at_ms.is_some_and(|t| t <= now_ms)
        || max_age.is_some_and(|age| now_ms - observation.observed_at_ms > u64::from(age))
    {
        return Ok(Match::Stale);
    }
    if assessed_assurance < predicate.evidence.max(definition.minimum_assurance) {
        return Ok(Match::InsufficientEvidence);
    }
    match observation.status {
        Support::Unknown => Ok(Match::Unknown),
        Support::Unsupported => Ok(Match::Unsupported),
        Support::Supported => {
            let value = observation
                .value
                .as_ref()
                .ok_or_else(|| invalid("missing registry evidence value"))?;
            definition.value_schema.accepts(value)?;
            Ok(if value.matches(predicate.operator, &predicate.value)? {
                Match::Satisfied
            } else {
                Match::DifferentValue
            })
        }
    }
}
