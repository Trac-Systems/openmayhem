//! Signed, data-only synchronous JSON connectors. Recipes never own networking,
//! credentials, executable code, usage policy, retries or payment authority.
mod transform;
use crate::{
    attempts::Digest,
    connector::failure::{Code, Execution, Failure, Scope, Stage},
};
use mayhem_proto::proxy::ProxyEndpoint;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, path::Path};
pub use transform::{Field, Projection, RequestField, Transform};

pub const ABI: u32 = 1;
pub const MAX_RECIPE_BYTES: usize = 64 * 1024;
pub const SIGNING_DOMAIN: &str = "mayhem/proxy/declarative-recipe/v1";
#[derive(Debug, thiserror::Error)]
#[error("declarative recipe or input exceeds its supported schema or bounds")]
pub struct Error;
pub type Result<T> = std::result::Result<T, Error>;
pub(super) fn check(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub revision: u32,
    pub abi_min: u32,
    pub abi_max: u32,
    pub publisher: Digest,
    pub endpoint: ProxyEndpoint,
    pub contract_hash: Digest,
    pub max_json_bytes: usize,
    pub request: Transform,
    pub outcome: Outcome,
    pub response: Projection,
    pub fixtures: Vec<Fixture>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub recipe: Recipe,
    pub signature: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub request: Value,
    pub upstream_request: Value,
    pub upstream_response: Value,
    pub normalized_response: Value,
}
/// A mandatory exact outcome discriminator. Unknown/missing outcomes are protocol
/// errors. Error messages are never projected and never prove non-execution.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub path: Vec<String>,
    pub success: String,
    pub error_path: Vec<String>,
    pub errors: BTreeMap<String, ErrorKind>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Busy,
    RateLimited,
    Unavailable,
    InvalidRequest,
}
impl ErrorKind {
    fn failure(self) -> Failure {
        let code = match self {
            Self::Busy => Code::UpstreamBusy,
            Self::RateLimited => Code::UpstreamRateLimited,
            Self::Unavailable => Code::UpstreamUnavailable,
            Self::InvalidRequest => Code::UpstreamProtocol,
        };
        Failure::new(code, Scope::Model, Stage::ResponseBody, Execution::Unknown)
    }
}
impl Recipe {
    fn validate(&self) -> Result<()> {
        check(
            self.schema_version == 1
                && self.revision > 0
                && self.abi_min == ABI
                && self.abi_max == ABI
                && (1..=16 * 1024 * 1024).contains(&self.max_json_bytes)
                && (1..=8).contains(&self.fixtures.len()),
        )?;
        check(serde_json::to_vec(self).map_err(|_| Error)?.len() <= MAX_RECIPE_BYTES)?;
        self.request.validate()?;
        check(matches!(self.request, Transform::Object { .. }))?;
        self.response.validate()?;
        // Recipes cannot synthesize or reinterpret billing/usage claims. The
        // endpoint's existing independent meter sees only normalized model output.
        match &self.response {
            Projection::Object { fields } => check(
                !fields.contains_key("usage")
                    && !fields.contains_key("mayhem")
                    && !fields.contains_key("receipt")
                    && !fields.contains_key("cost"),
            )?,
            _ => return Err(Error),
        }
        transform::path(&self.outcome.path)?;
        transform::path(&self.outcome.error_path)?;
        check(
            !self.outcome.path.is_empty()
                && !self.outcome.error_path.is_empty()
                && self.outcome.path != self.outcome.error_path
                && transform::key(&self.outcome.success)
                && self.outcome.errors.len() <= 16
                && !self.outcome.errors.contains_key(&self.outcome.success)
                && self.outcome.errors.keys().all(|v| transform::key(v)),
        )?;
        for fixture in &self.fixtures {
            check(self.map_request(&fixture.request)? == fixture.upstream_request)?;
            check(
                self.map_response(&fixture.upstream_response, self.max_json_bytes)
                    .map_err(|_| Error)?
                    == fixture.normalized_response,
            )?;
        }
        Ok(())
    }
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let body = mayhem_proto::stable_json_bytes(&serde_json::to_value(self).map_err(|_| Error)?)
            .map_err(|_| Error)?;
        let mut bytes = SIGNING_DOMAIN.as_bytes().to_vec();
        bytes.push(0);
        bytes.extend(body);
        Ok(bytes)
    }
    pub fn digest(&self) -> Result<Digest> {
        Ok(Digest::hash(SIGNING_DOMAIN, &[&self.signing_bytes()?]))
    }
    pub fn map_request(&self, input: &Value) -> Result<Value> {
        self.request.apply(input, self.max_json_bytes)
    }
    pub(crate) fn map_response(
        &self,
        input: &Value,
        max_bytes: usize,
    ) -> std::result::Result<Value, Failure> {
        let bad = || {
            Failure::new(
                Code::UpstreamProtocol,
                Scope::Model,
                Stage::ResponseBody,
                Execution::Unknown,
            )
        };
        let max_bytes = self.max_json_bytes.min(max_bytes);
        transform::bounded(input, max_bytes).map_err(|_| bad())?;
        let status = transform::read(input, &self.outcome.path)
            .and_then(Value::as_str)
            .ok_or_else(bad)?;
        if status != self.outcome.success {
            return Err(self
                .outcome
                .errors
                .get(status)
                .map_or_else(bad, |v| v.failure()));
        }
        if transform::read(input, &self.outcome.error_path) != Some(&Value::Null) {
            return Err(bad());
        }
        self.response.apply(input, max_bytes).map_err(|_| bad())
    }
}
impl Signed {
    pub fn validate(&self) -> Result<()> {
        check(serde_json::to_vec(self).map_err(|_| Error)?.len() <= MAX_RECIPE_BYTES)?;
        let bytes = self.recipe.signing_bytes()?;
        check(crate::receipts::verify_signature(
            &self.signature,
            &bytes,
            self.recipe.publisher.as_str(),
        ))
    }
    pub fn import(bytes: &[u8]) -> Result<Self> {
        check(bytes.len() <= MAX_RECIPE_BYTES)?;
        let result: Self = serde_json::from_slice(bytes).map_err(|_| Error)?;
        result.validate()?;
        Ok(result)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|_| Error)?;
        let mut bytes = Vec::new();
        file.take(MAX_RECIPE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error)?;
        Self::import(&bytes)
    }
    /// Export contains public recipe and explicit conformance examples only.
    /// Never automatically capture real prompts, addresses or credentials.
    pub fn export(&self) -> Result<Vec<u8>> {
        self.validate()?;
        mayhem_proto::stable_json_bytes(&serde_json::to_value(self).map_err(|_| Error)?)
            .map_err(|_| Error)
    }
}

#[derive(Serialize)]
pub struct Review {
    pub schema_version: u32,
    pub kind: &'static str,
    pub recipe_hash: Digest,
    pub publisher: Digest,
    pub revision: u32,
    pub abi_min: u32,
    pub abi_max: u32,
    pub endpoint: ProxyEndpoint,
    pub contract_hash: Digest,
    pub fixture_count: usize,
    pub assurance: &'static str,
}
impl Signed {
    pub fn review(&self) -> Result<Review> {
        self.validate()?;
        Ok(Review {
            schema_version: 1,
            kind: "declarative_json_recipe",
            recipe_hash: self.recipe.digest()?,
            publisher: self.recipe.publisher.clone(),
            revision: self.recipe.revision,
            abi_min: self.recipe.abi_min,
            abi_max: self.recipe.abi_max,
            endpoint: self.recipe.endpoint,
            contract_hash: self.recipe.contract_hash.clone(),
            fixture_count: self.recipe.fixtures.len(),
            assurance: "signature_and_offline_mapping_fixtures_only",
        })
    }
}

pub const MAX_PREVIEW_BYTES: usize = 512 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewInput {
    adapter: crate::endpoint::AdapterSnapshot,
    request: Value,
    response: Value,
}
impl Signed {
    /// Explicit local fixture file only; never accepts connection credentials or
    /// fetches schema references. Reject already-bound adapters to avoid confusion.
    pub fn preview_file(&self, path: &Path) -> Result<Value> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| Error)?
            .take(MAX_PREVIEW_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error)?;
        check(bytes.len() <= MAX_PREVIEW_BYTES)?;
        let input: PreviewInput = serde_json::from_slice(&bytes).map_err(|_| Error)?;
        check(input.adapter.recipe.is_none() && input.adapter.version == 1)?;
        let adapter = crate::endpoint::Adapter::restore(input.adapter)
            .map_err(|_| Error)?
            .with_recipe(self.clone())
            .map_err(|_| Error)?;
        adapter
            .preview(&input.request, &input.response)
            .map_err(|_| Error)
    }
}
