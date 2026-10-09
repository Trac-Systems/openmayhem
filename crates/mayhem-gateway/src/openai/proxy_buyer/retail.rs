//! Trusted retail credit admission, after durable Core authorization and before
//! signing. Public requests cannot choose the callback or opt out of this gate.
use super::*;
use mayhem_proxy::{
    buyer_controller::{AuthorizationGate, GateError, NonAdmission, VerifiedOutput},
    financial::quote::PreparedPurchase,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{future::Future, pin::Pin};

const MAX_REPLY: usize = 8 * 1024;
const MAX_DEPTH: usize = 64;
static CALLBACKS: Semaphore = Semaphore::const_new(8);

/// Loaded only from the existing protected operator configuration. Intentionally
/// not Debug/Serialize: neither URLs nor machine credentials belong in logs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub url: String,
    pub credential: String,
    pub owner_token_ids: Vec<String>,
    pub timeout_ms: u64,
}
pub(super) struct Authority {
    client: reqwest::Client,
    url: reqwest::Url,
    credential: axum::http::HeaderValue,
    owners: BTreeSet<String>,
    timeout: Duration,
    commitment: Digest,
}
#[derive(Clone)]
pub(super) struct Correlation {
    request_id: String,
    content: String,
}
#[derive(Serialize)]
struct Request<'a> {
    schema_version: u32,
    request_id: &'a str,
    job_id: &'a str,
    terms_hash: &'a str,
    request_content_digest: &'a str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    schema_version: u32,
    job_id: String,
    terms_hash: String,
    authorized: bool,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let config = self;
        let fail = || "invalid retail authorization configuration".to_owned();
        let url = reqwest::Url::parse(&config.url).map_err(|_| fail())?;
        let loopback = url.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if config.url.len() > 4096
            || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(1000..=5000).contains(&config.timeout_ms)
            || config.credential.is_empty()
            || config.credential.len() > 4096
            || !config.credential.bytes().all(|b| b.is_ascii_graphic())
            || config.owner_token_ids.is_empty()
            || config.owner_token_ids.len() > 256
            || config.owner_token_ids.iter().any(|id| {
                id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_graphic())
            })
        {
            return Err(fail());
        }
        Ok(())
    }
}
impl Authority {
    pub(super) fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        let fail = || "invalid retail authorization configuration".to_owned();
        let url = reqwest::Url::parse(&config.url).map_err(|_| fail())?;
        let owners: BTreeSet<_> = config.owner_token_ids.into_iter().collect();
        let mut credential =
            axum::http::HeaderValue::from_str(&format!("Bearer {}", config.credential))
                .map_err(|_| fail())?;
        credential.set_sensitive(true);
        let timeout = Duration::from_millis(config.timeout_ms);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .map_err(|_| fail())?;
        // Credential rotation does not change an existing purchase. Changing
        // the fixed authority or callback participation changes its fingerprint.
        let commitment = digest(
            "mayhem/proxy/retail-authority/v1",
            &[url.as_str().as_bytes()],
        );
        Ok(Self {
            client,
            url,
            credential,
            owners,
            timeout,
            commitment,
        })
    }
    pub(super) fn correlation(
        &self,
        owner: &str,
        key: Option<&str>,
        request: &Value,
    ) -> Result<Option<Correlation>, ApiError> {
        if !self.owners.contains(owner) {
            return Ok(None);
        }
        let request_id = key.filter(|id| !id.is_empty()).ok_or_else(|| {
            ApiError::bad_request(
                "retail proxy requests require the original Idempotency-Key",
                Some("Idempotency-Key"),
            )
        })?;
        Ok(Some(Correlation {
            request_id: request_id.into(),
            content: request_content_digest(request).map_err(|_| invalid())?,
        }))
    }
    pub(super) fn fingerprint(&self, original: &Digest, correlation: &Correlation) -> Digest {
        digest(
            "mayhem/proxy/retail-gateway-request/v1",
            &[
                original.as_str().as_bytes(),
                self.commitment.as_str().as_bytes(),
                correlation.request_id.as_bytes(),
                correlation.content.as_bytes(),
            ],
        )
    }
    async fn authorize(
        &self,
        correlation: &Correlation,
        job: &str,
        terms: &str,
    ) -> Result<(), GateError> {
        let _permit = CALLBACKS
            .try_acquire()
            .map_err(|_| GateError::Unavailable)?;
        let request = Request {
            schema_version: 1,
            request_id: &correlation.request_id,
            job_id: job,
            terms_hash: terms,
            request_content_digest: &correlation.content,
        };
        // No automatic retry, redirect, detached work or lock held across I/O.
        // A lost reply may mean a retail hold exists: the controller permanently
        // fences this original unsigned intent and the retailer reconciles it.
        tokio::time::timeout(self.timeout, async {
            let mut response = self
                .client
                .post(self.url.clone())
                .header(axum::http::header::AUTHORIZATION, self.credential.clone())
                .json(&request)
                .send()
                .await
                .map_err(|_| GateError::Unavailable)?;
            if !response.status().is_success() {
                return Err(GateError::Rejected);
            }
            if response
                .content_length()
                .is_some_and(|size| size > MAX_REPLY as u64)
            {
                return Err(GateError::Unavailable);
            }
            let mut bytes = Vec::with_capacity(MAX_REPLY);
            while let Some(chunk) = response.chunk().await.map_err(|_| GateError::Unavailable)? {
                if chunk.len() > MAX_REPLY - bytes.len() {
                    return Err(GateError::Unavailable);
                }
                bytes.extend_from_slice(&chunk);
            }
            let reply: Reply =
                serde_json::from_slice(&bytes).map_err(|_| GateError::Unavailable)?;
            if reply.schema_version != 1
                || !reply.authorized
                || reply.job_id != job
                || reply.terms_hash != terms
            {
                return Err(GateError::Rejected);
            }
            Ok(())
        })
        .await
        .map_err(|_| GateError::Unavailable)?
    }
}

pub(super) struct Gate {
    pub(super) owner: Arc<Owner>,
    pub(super) authority: Arc<Authority>,
    pub(super) correlation: Correlation,
    pub(super) job: String,
}
impl AuthorizationGate for Gate {
    fn authorize<'a>(
        &'a self,
        purchase: &'a PreparedPurchase,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        Box::pin(async move {
            self.owner.authorize(purchase).await?;
            let terms = purchase.terms().digest().map_err(|_| GateError::Rejected)?;
            self.authority
                .authorize(&self.correlation, &self.job, &terms)
                .await
        })
    }
    fn retain_non_admission<'a>(
        &'a self,
        proof: &'a NonAdmission,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        self.owner.retain_non_admission(proof)
    }
    fn retain_verified_output<'a>(
        &'a self,
        output: VerifiedOutput<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        self.owner.retain_verified_output(output)
    }
}

/// Cross-language request correlation, deliberately separate from existing
/// protocol/token-metering hashes. See PROXY_BUYER.md for the exact wire encoding.
pub fn request_content_digest(value: &Value) -> Result<String, String> {
    fn visit(hash: &mut Sha256, value: &Value, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("retail request nesting exceeds bound".into());
        }
        let length = |hash: &mut Sha256, n: usize| hash.update((n as u64).to_be_bytes());
        match value {
            Value::Null => hash.update(b"n"),
            Value::Bool(false) => hash.update(b"f"),
            Value::Bool(true) => hash.update(b"t"),
            Value::Number(number) => {
                let mut value = number.as_f64().ok_or("invalid retail number")?;
                // Casting back to u64/i64 would saturate at their boundaries.
                // Wider integers detect that rounding rather than accepting it.
                if !value.is_finite()
                    || number.as_u64().is_some_and(|n| value as u128 != n as u128)
                    || number.as_i64().is_some_and(|n| value as i128 != n as i128)
                {
                    return Err("retail integer cannot be represented exactly".into());
                }
                if value == 0.0 {
                    value = 0.0;
                }
                hash.update(b"d");
                hash.update(value.to_bits().to_be_bytes());
            }
            Value::String(value) => {
                hash.update(b"s");
                length(hash, value.len());
                hash.update(value.as_bytes());
            }
            Value::Array(values) => {
                hash.update(b"a");
                length(hash, values.len());
                for value in values {
                    visit(hash, value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                hash.update(b"o");
                length(hash, values.len());
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_unstable_by(|(left, _), (right, _)| {
                    left.as_bytes().cmp(right.as_bytes())
                });
                for (key, value) in entries {
                    hash.update(b"s");
                    length(hash, key.len());
                    hash.update(key.as_bytes());
                    visit(hash, value, depth + 1)?;
                }
            }
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    hash.update(b"mayhem/proxy/retail-request-content/v1\0");
    visit(&mut hash, value, 0)?;
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests;
