//! Authenticated, request-scoped observations. Metadata and signatures describe
//! who measured a result; neither establishes remote weights, privacy or T4.
//! Only verified execution/probe completions can write the local durable store.
pub(crate) mod capture;
mod store;
#[cfg(test)]
mod tests;
use crate::{attempts::Digest, discovery, health, registry, require, Result};
use mayhem_proto::proxy::{finance::ProxySpendTerms, ProxyEndpoint, ProxyOffer};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
pub use store::{Lookup, Store};

pub const SUITE: &str = "proxy-decoder-observation-v1";
pub const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assertion {
    ValidEndpointOutput,
    ValidatedStream,
    ValidatedToolCall,
    ValidatedJsonSchema,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub field_id: String,
    pub schema_revision: u32,
    pub definition_digest: Digest,
    pub assertion: Assertion,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokenizer {
    pub file: PathBuf,
    pub digest: Digest,
    pub limits: health::native::Limits,
}
/// Protected operator configuration, never supplied by a public request.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub tester: Digest,
    pub ttl_ms: u64,
    pub maximum_records: u64,
    pub maximum_bytes: u64,
    pub minimum_interval_tokens: u64,
    pub minimum_interval_us: u64,
    pub mappings: Vec<Mapping>,
    #[serde(deserialize_with = "nullable")]
    pub tokenizer: Option<Tokenizer>,
}
fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && (1..=3_600_000).contains(&self.ttl_ms)
                && (1..=1_000_000).contains(&self.maximum_records)
                && (MAX_RECORD_BYTES as u64..=16 * 1024 * 1024 * 1024)
                    .contains(&self.maximum_bytes)
                && (2..=1_000_000).contains(&self.minimum_interval_tokens)
                && (1..=3_600_000_000).contains(&self.minimum_interval_us)
                && self.mappings.len() <= registry::MAX_FIELDS_PER_REQUEST,
            "invalid conformance resource policy",
        )?;
        let mut keys = std::collections::BTreeSet::new();
        for m in &self.mappings {
            require(
                registry::identifier(&m.field_id)
                    && m.schema_revision > 0
                    && m.schema_revision <= i32::MAX as u32
                    && keys.insert((&m.field_id, m.schema_revision)),
                "invalid conformance mapping",
            )?;
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<Digest> {
        self.validate()?;
        digest("mayhem/proxy/conformance-config/v1", self)
    }
}
fn digest<T: Serialize>(domain: &'static str, value: &T) -> Result<Digest> {
    let bytes = mayhem_proto::stable_json_bytes(&serde_json::to_value(value)?)?;
    Ok(Digest::hash(domain, &[&bytes]))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub offer_digest: Digest,
    pub provider: Digest,
    pub membership_revision: u64,
    pub endpoint: ProxyEndpoint,
    pub endpoint_contract: Digest,
    pub recipe_hash: Digest,
    pub connection_revision: u64,
}
impl Subject {
    pub fn new(
        offer: &ProxyOffer,
        contract: Digest,
        recipe: Digest,
        connection_revision: u64,
    ) -> Result<Self> {
        offer.validate().map_err(crate::invalid)?;
        require(
            connection_revision > 0 && discovery::safe_integer(connection_revision),
            "invalid evidence connection revision",
        )?;
        Ok(Self {
            offer_digest: hash_value(offer.digest().map_err(crate::invalid)?)?,
            provider: hash_value(&offer.provider_pubkey)?,
            membership_revision: offer.membership_revision,
            endpoint: offer.endpoint,
            endpoint_contract: contract,
            recipe_hash: recipe,
            connection_revision,
        })
    }
    pub(crate) fn terms(t: &ProxySpendTerms) -> Result<Self> {
        Self::new(
            &t.offer,
            hash_value(&t.endpoint_contract)?,
            hash_value(&t.recipe_hash)?,
            t.connection_revision,
        )
    }
}

/// No prompt/tool content. Exact controls are hashed separately from size class.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Class {
    pub input_bytes_log2: u8,
    pub thinking: String,
    pub streaming: bool,
    pub controls_digest: Digest,
    pub shape_digest: Digest,
}
impl Class {
    pub fn request(value: &serde_json::Value) -> Result<Self> {
        let obj = value
            .as_object()
            .ok_or_else(|| crate::invalid("invalid evidence request"))?;
        let normalized = obj
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "model" | "proxy"))
            .collect::<std::collections::BTreeMap<_, _>>();
        struct Count(usize);
        impl std::io::Write for Count {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(b.len())
                    .ok_or(std::io::ErrorKind::OutOfMemory)?;
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut count = Count(0);
        serde_json::to_writer(&mut count, &normalized)?;
        let h = health::Class::request(value, count.0, value["stream"] == true);
        // Every explicit non-input control participates, including future custom
        // contract fields. Input structure distinguishes modalities and histories
        // without retaining their content or claiming a native token count.
        let input_keys = ["messages", "input", "prompt", "state", "questions"];
        let controls = normalized
            .iter()
            .filter(|(k, _)| !input_keys.contains(&k.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut nodes = 0;
        let mut inputs = std::collections::BTreeMap::new();
        for key in input_keys {
            if let Some(v) = value.get(key) {
                inputs.insert(key, input_shape(v, None, 0, &mut nodes)?);
            }
        }
        Ok(Self {
            input_bytes_log2: h.input_bytes_log2,
            thinking: format!("{:?}", h.thinking).to_ascii_lowercase(),
            streaming: h.streaming,
            controls_digest: hash_value(
                h.controls_digest
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
            )?,
            shape_digest: digest("mayhem/proxy/conformance-class/v1", &(controls, inputs))?,
        })
    }
}
fn input_shape(
    value: &serde_json::Value,
    key: Option<&str>,
    depth: usize,
    nodes: &mut usize,
) -> Result<serde_json::Value> {
    use serde_json::{json, Value};
    *nodes += 1;
    require(
        depth <= 32 && *nodes <= 4096,
        "conformance request class exceeds structural bound",
    )?;
    Ok(match value {
        Value::Null => json!("null"),
        Value::Bool(_) => json!("boolean"),
        Value::Number(_) => json!("number"),
        Value::String(v) if matches!(key, Some("role" | "type")) && v.len() <= 128 => {
            json!(["tag", v])
        }
        Value::String(_) => json!("string"),
        Value::Array(v) => Value::Array(
            v.iter()
                .map(|v| input_shape(v, None, depth + 1, nodes))
                .collect::<Result<Vec<_>>>()?,
        ),
        Value::Object(v) => {
            let fields = v
                .iter()
                .map(|(k, v)| Ok((k, input_shape(v, Some(k), depth + 1, nodes)?)))
                .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
            serde_json::to_value(fields)?
        }
    })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    GatewayObservation,
    ProviderSelfTest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Speed {
    pub tokenizer: Digest,
    pub interval_tokens: u64,
    pub interval_us: u64,
    pub samples: u32,
    pub first_output_ms: u64,
    pub total_ms: u64,
    pub local_concurrency: u32,
    // Upstream conditions cannot be inferred from local counters or reported usage.
    pub remote_cache: String,
    pub remote_concurrency: String,
}
impl Speed {
    pub fn comparable(&self, other: &Self) -> bool {
        self.tokenizer == other.tokenizer
            && self.local_concurrency == other.local_concurrency
            && self.remote_cache == other.remote_cache
            && self.remote_concurrency == other.remote_concurrency
    }
    pub fn faster_than(&self, other: &Self) -> bool {
        u128::from(self.interval_tokens) * u128::from(other.interval_us)
            > u128::from(other.interval_tokens) * u128::from(self.interval_us)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub schema_version: u32,
    pub network: discovery::Identity,
    pub tester: Digest,
    pub provenance: Provenance,
    pub boot: Digest,
    pub configuration: Digest,
    pub suite: String,
    pub subject: Subject,
    pub class: Class,
    pub session: Digest,
    pub request_hash: Digest,
    pub connection_digest: Digest,
    pub result_digest: Digest,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    pub assertions: Vec<Assertion>,
    #[serde(deserialize_with = "nullable")]
    pub speed: Option<Speed>,
}
impl Body {
    pub(crate) fn signing_bytes(&self) -> Result<Vec<u8>> {
        self.network.validate()?;
        require(
            self.schema_version == 1
                && self.suite == SUITE
                && self.observed_at_ms > 0
                && self.expires_at_ms > self.observed_at_ms
                && self.expires_at_ms - self.observed_at_ms <= 3_600_000
                && discovery::safe_integer(self.expires_at_ms)
                && self.subject.membership_revision > 0
                && self.subject.connection_revision > 0
                && discovery::safe_integer(self.subject.membership_revision)
                && discovery::safe_integer(self.subject.connection_revision)
                && self.class.input_bytes_log2 > 0
                && self.class.input_bytes_log2 <= 64
                && ["enabled", "disabled", "unknown"].contains(&self.class.thinking.as_str())
                && !self.assertions.is_empty()
                && self.assertions.len() <= 4
                && self.assertions.windows(2).all(|v| v[0] < v[1]),
            "invalid conformance observation",
        )?;
        if let Some(s) = &self.speed {
            require(
                self.class.streaming
                    && self.subject.endpoint != ProxyEndpoint::Decisions
                    && s.interval_tokens >= 2
                    && s.interval_us > 0
                    && s.samples == 1
                    && s.first_output_ms <= s.total_ms
                    && s.local_concurrency > 0
                    && s.remote_cache == "unknown"
                    && s.remote_concurrency == "unknown"
                    && [s.interval_tokens, s.interval_us, s.total_ms]
                        .into_iter()
                        .all(discovery::safe_integer),
                "invalid conformance speed",
            )?;
        }
        let mut bytes = b"mayhem.proxy.conformance.v1\0".to_vec();
        bytes.extend(mayhem_proto::stable_json_bytes(&serde_json::to_value(
            self,
        )?)?);
        require(
            bytes.len() <= MAX_RECORD_BYTES - 256,
            "conformance record exceeds byte bound",
        )?;
        Ok(bytes)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub body: Body,
    pub signature: String,
}
impl Signed {
    pub fn verify(&self) -> Result<()> {
        require(
            crate::receipts::verify_signature(
                &self.signature,
                &self.body.signing_bytes()?,
                self.body.tester.as_str(),
            ),
            "conformance signature rejected",
        )
    }
    pub fn digest(&self) -> Result<Digest> {
        digest("mayhem/proxy/conformance-record/v1", self)
    }
}

/// A local recorder owns the signing capability; no public JSON-to-sign API.
pub struct Recorder {
    store: Arc<Store>,
    signer: Arc<crate::signing::Authority>,
    active: std::sync::atomic::AtomicU64,
    generation: std::sync::atomic::AtomicU64,
    writes: Arc<tokio::sync::Semaphore>,
}
impl Recorder {
    pub fn new(store: Arc<Store>, signer: Arc<crate::signing::Authority>) -> Result<Self> {
        let own = signer.identity();
        let n = store.network();
        require(
            own.controller_pubkey == store.config().tester
                && own.network_id == n.network_id
                && own.msb_bootstrap.as_str() == n.msb_bootstrap
                && own.subnet_bootstrap.as_str() == n.subnet_bootstrap,
            "conformance recorder authority differs",
        )?;
        Ok(Self {
            store,
            signer,
            active: 0.into(),
            generation: 0.into(),
            writes: Arc::new(tokio::sync::Semaphore::new(4)),
        })
    }
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
}

fn hash_value(value: impl Into<String>) -> Result<Digest> {
    Digest::new(value).map_err(|_| crate::invalid("invalid conformance digest"))
}
