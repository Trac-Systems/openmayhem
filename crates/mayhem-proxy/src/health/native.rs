//! Locally tokenized visible-output timing. Pinned data, never remote tokenizer
//! code or reported usage. One encode per output field after validated completion;
//! no prefix retokenization, network calls or per-token database operations.
use super::*;
use crate::worker::host::Pool;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
pub mod engine;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
#[cfg(all(test, unix))]
#[path = "native_tests.rs"]
mod tests;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub artifact_bytes: usize,
    pub output_bytes: usize,
    pub channels: usize,
    pub workers: usize,
    pub minimum_tokens: usize,
}

/// Construct only from operator-approved, digest-pinned local data. No path or
/// URL is interpreted by this component; no remote code or Hub feature is enabled.
pub struct Source {
    bytes: Arc<[u8]>,
    pool: OnceLock<crate::worker::host::Tokenizer>,
    digest: Digest,
    connection: Digest,
    recipe: Digest,
    limits: Limits,
    workers: Arc<Semaphore>,
}
impl Source {
    /// Identity of approved local data; not attestation of remote model weights.
    pub fn digest(&self) -> &Digest {
        &self.digest
    }
    pub fn from_bytes(
        bytes: &[u8],
        digest: Digest,
        connection: Digest,
        recipe: Digest,
        limits: Limits,
    ) -> Result<Self> {
        // This checks the pin and resource policy only. Validation/activation
        // uses an explicit isolated launcher; construction never parses models.
        check(bytes, &digest, limits)?;
        Ok(Self {
            bytes: Arc::from(bytes),
            pool: OnceLock::new(),
            digest,
            connection,
            recipe,
            limits,
            workers: Arc::new(Semaphore::new(limits.workers)),
        })
    }
    /// Explicit trusted launcher supplied by the existing host. No PATH search,
    /// artifact-provided executable, or in-process parser fallback.
    pub fn bind_pool(&self, pool: &Pool) -> Result<()> {
        if let Some(prior) = self.pool.get() {
            return if prior.same_launcher(pool) {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        let bounded = pool
            .tokenizer(self.bytes.clone(), self.digest.clone(), self.limits)
            .map_err(|_| Error::Invalid)?;
        if self.pool.set(bounded).is_err() {
            return self.bind_pool(pool);
        }
        Ok(())
    }
    /// Startup/provisioning validation. The owner must call this in its bounded
    /// blocking scope before activating the source or atomically saving a bundle.
    pub fn validate(&self, pool: &Pool) -> Result<()> {
        self.bind_pool(pool)?;
        self.pool
            .get()
            .ok_or(Error::Unavailable)?
            .validate()
            .map_err(|_| Error::Unavailable)
    }
    /// One-shot installation validation must release retained worker handles
    /// before the caller atomically publishes the configuration directory.
    pub(crate) fn validate_for_installation(self, pool: &Pool) -> Result<()> {
        let result = self.validate(pool);
        if let Some(tokenizer) = self.pool.into_inner() {
            tokenizer
                .finish_installation()
                .map_err(|_| Error::Unavailable)?;
        }
        result
    }
    pub(crate) fn matches(&self, connection: &Digest, recipe: &Digest) -> bool {
        &self.connection == connection && &self.recipe == recipe
    }
    pub(crate) fn capture(&self) -> Option<Capture> {
        // Reserve before retaining any output. No waiting task queue. The permit
        // survives caller cancellation until any spawned child is reaped.
        let permit = self.workers.clone().try_acquire_owned().ok()?;
        Some(Capture {
            pool: self.pool.get()?.clone(),
            digest: self.digest.clone(),
            limits: self.limits,
            permit,
            fields: BTreeMap::new(),
            bytes: 0,
            first: None,
            last: None,
            failed: false,
            backpressured: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Channel(u64, u64, u8);
pub(crate) struct Field {
    pub(crate) text: String,
    pub(crate) first_bytes: usize,
}
pub(crate) struct Capture {
    pool: crate::worker::host::Tokenizer,
    digest: Digest,
    limits: Limits,
    permit: OwnedSemaphorePermit,
    fields: BTreeMap<Channel, Field>,
    bytes: usize,
    first: Option<Instant>,
    last: Option<Instant>,
    failed: bool,
    backpressured: Arc<AtomicBool>,
}
pub(crate) struct Evidence {
    pub digest: Digest,
    pub tokens: u64,
    pub first: Instant,
    pub last: Instant,
}
impl Capture {
    pub(crate) fn backpressure(&self) -> Arc<AtomicBool> {
        self.backpressured.clone()
    }
    fn add(&mut self, channel: Channel, text: &str, at: Instant) {
        if self.failed || text.is_empty() {
            return;
        }
        if self.bytes.saturating_add(text.len()) > self.limits.output_bytes
            || (!self.fields.contains_key(&channel) && self.fields.len() >= self.limits.channels)
            || self.last.is_some_and(|last| at < last)
        {
            self.failed = true;
            self.fields.clear();
            return;
        }
        let first = *self.first.get_or_insert(at);
        self.last = Some(at);
        self.bytes += text.len();
        let field = self.fields.entry(channel).or_insert_with(|| Field {
            text: String::new(),
            first_bytes: 0,
        });
        field.text.push_str(text);
        if first == at {
            field.first_bytes = field.text.len()
        }
    }
    pub(crate) fn delta(&mut self, value: &Value, at: Instant) {
        if self.failed {
            return;
        }
        if let Some(kind) = value["type"].as_str() {
            let ty = match kind {
                "response.output_text.delta" => 1,
                "response.reasoning_text.delta" => 2,
                "response.reasoning_summary_text.delta" => 3,
                "response.refusal.delta" => 4,
                "response.function_call_arguments.delta" => 5,
                // Completed text in an added item/part has no generation timing.
                "response.output_item.added"
                | "response.content_part.added"
                | "response.reasoning_summary_part.added" => {
                    if contains_text(&value["item"]) || contains_text(&value["part"]) {
                        self.failed = true;
                        self.fields.clear()
                    }
                    return;
                }
                _ => return,
            };
            let Some(i) = value["output_index"].as_u64() else {
                self.failed = true;
                return;
            };
            let j = value["content_index"]
                .as_u64()
                .or_else(|| value["summary_index"].as_u64())
                .unwrap_or(0);
            if let Some(text) = value["delta"].as_str() {
                self.add(Channel(i, j, ty), text, at)
            }
            return;
        }
        let Some(choices) = value["choices"].as_array() else {
            return;
        };
        for choice in choices {
            let Some(i) = choice["index"].as_u64() else {
                self.failed = true;
                return;
            };
            if let Some(text) = choice["text"].as_str() {
                self.add(Channel(i, 0, 0), text, at)
            }
            let d = &choice["delta"];
            for (key, ty) in [("content", 1), ("refusal", 4)] {
                if let Some(text) = d[key].as_str() {
                    self.add(Channel(i, 0, ty), text, at)
                }
            }
            // Equal reasoning aliases are one output, never twice the rate.
            let a = d["reasoning"].as_str();
            let b = d["reasoning_content"].as_str();
            if a.zip(b).is_some_and(|(a, b)| a != b) {
                self.failed = true;
                return;
            }
            if let Some(text) = a.or(b) {
                self.add(Channel(i, 0, 2), text, at)
            }
            if let Some(tools) = d["tool_calls"].as_array() {
                for tool in tools {
                    let Some(j) = tool["index"].as_u64() else {
                        self.failed = true;
                        return;
                    };
                    // Function names/IDs are routing metadata and may be repeated
                    // by providers; only append-only generated arguments count.
                    if let Some(text) = tool["function"]["arguments"].as_str() {
                        self.add(Channel(i, j, 5), text, at)
                    }
                }
            }
        }
    }
    pub(crate) async fn finish(self) -> Option<Evidence> {
        if self.failed || self.backpressured.load(Ordering::Acquire) {
            return None;
        }
        let (first, last) = (self.first?, self.last?);
        if last <= first {
            return None;
        }
        let fields = self.fields.into_values().collect();
        let count = self.pool.count(fields, self.permit).await.ok()?;
        if count < self.limits.minimum_tokens as u64 {
            return None;
        }
        Some(Evidence {
            digest: self.digest,
            tokens: count,
            first,
            last,
        })
    }
}
fn contains_text(v: &Value) -> bool {
    match v {
        Value::Object(fields) => fields.iter().any(|(k, v)| {
            (matches!(k.as_str(), "text" | "refusal" | "arguments")
                && v.as_str().is_some_and(|s| !s.is_empty()))
                || ((k == "content" || k == "summary") && contains_text(v))
        }),
        Value::Array(items) => items.iter().any(contains_text),
        _ => false,
    }
}

/// Consumer backpressure must not be attributed to the model. This does not
/// time out, cancel or alter delivery; it merely disqualifies its speed sample.
pub(crate) async fn delivery<F: std::future::Future>(
    future: F,
    blocked: Option<Arc<AtomicBool>>,
) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        let poll = future.as_mut().poll(cx);
        if poll.is_pending() {
            if let Some(flag) = &blocked {
                flag.store(true, Ordering::Release)
            }
        }
        poll
    })
    .await
}

// These checks deliberately do not instantiate a tokenizer or execute patterns.
pub(crate) fn check_limits(limits: Limits) -> Result<()> {
    if !(1..=engine::MAX_ARTIFACT).contains(&limits.artifact_bytes)
        || !(1..=engine::MAX_OUTPUT).contains(&limits.output_bytes)
        || !(1..=1024).contains(&limits.channels)
        || !(1..=16).contains(&limits.workers)
        || !(2..=1_000_000).contains(&limits.minimum_tokens)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(crate) fn check(bytes: &[u8], digest: &Digest, limits: Limits) -> Result<()> {
    check_limits(limits)?;
    if bytes.is_empty()
        || bytes.len() > limits.artifact_bytes
        || blake3::hash(bytes).to_hex().as_str() != digest.as_str()
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
