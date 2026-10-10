//! Opt-in reader for administrator-published registry semantics. The configured
//! HTTPS origin is the trust anchor; these hashes are integrity identities, not
//! signatures or evidence of provider capability. Nothing is activated here.
pub mod taxonomy;
mod fields;
pub use fields::FieldsPage;
mod wire;
pub use wire::{Document, DocumentReference, Manifest, Reference, Release};

use super::{Definition, Usage, MAX_DEFINITION_BYTES, MAX_FIELDS_PER_REQUEST};
use serde::de::DeserializeOwned;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::Semaphore,
    time::{timeout, Instant},
};
use url::Url;

pub const MAX_SNAPSHOT_BYTES: usize =
    MAX_FIELDS_PER_REQUEST * (MAX_DEFINITION_BYTES + wire::DOCUMENT_OVERHEAD);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid published registry: {0}")]
    Invalid(&'static str),
    #[error("registry metadata reader is busy")]
    Busy,
    #[error("registry metadata deadline elapsed")]
    Deadline,
    #[error("registry metadata transport failed")]
    Transport,
    #[error("registry metadata HTTP status {0}")]
    Http(u16),
    #[error("registry release or exact reference is not published")]
    NotPublished,
    #[error("registry head revision moved backwards")]
    Rollback,
    #[error("registry metadata changed an immutable identity")]
    Equivocation,
}
pub type Result<T> = std::result::Result<T, Error>;
fn check(ok: bool, why: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(why))
    }
}

/// Construct only from operator configuration. No inference request or provider
/// response may choose this origin. Paths, queries and credentials are forbidden.
#[derive(Clone, Debug)]
pub struct TrustedOrigin(Url);
impl TrustedOrigin {
    pub fn https(origin: &str) -> Result<Self> {
        Self::parse(origin, false)
    }
    /// Explicit opt-in for isolated local acceptance. Hostnames such as localhost
    /// are excluded; HTTP requires a literal loopback address.
    pub fn local_loopback_http(origin: &str) -> Result<Self> {
        let result = Self::parse(origin, true)?;
        check(
            result.0.scheme() == "http",
            "local registry origin must use loopback HTTP",
        )?;
        Ok(result)
    }
    fn parse(origin: &str, local: bool) -> Result<Self> {
        check(
            origin.len() <= 2048 && origin.trim() == origin,
            "invalid registry origin",
        )?;
        let url = Url::parse(origin).map_err(|_| Error::Invalid("invalid registry origin"))?;
        check(
            url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && !origin.split('/').nth(2).unwrap_or("").contains('@')
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "registry configuration requires an origin only",
        )?;
        let loopback = matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
        check(
            url.scheme() == "https" || (local && url.scheme() == "http" && loopback),
            "registry origin requires HTTPS",
        )?;
        Ok(Self(url))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug)]
pub struct Limits {
    /// Entire operation, including every exact batch needed by a closure.
    pub operation_timeout: Duration,
    pub head_ttl: Duration,
    pub concurrent_operations: usize,
    /// Shared FIFO bound across release metadata and definition documents.
    pub cache_entries: usize,
    pub cache_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            operation_timeout: Duration::from_secs(5),
            head_ttl: Duration::from_secs(60),
            concurrent_operations: 4,
            cache_entries: 512,
            cache_bytes: 8 * 1024 * 1024,
        }
    }
}
impl Limits {
    fn validate(&self) -> Result<()> {
        check(
            (Duration::from_millis(50)..=Duration::from_secs(30)).contains(&self.operation_timeout)
                && (Duration::from_secs(1)..=Duration::from_secs(300)).contains(&self.head_ttl)
                && (1..=16).contains(&self.concurrent_operations)
                && (1..=4096).contains(&self.cache_entries)
                && (32 * 1024..=64 * 1024 * 1024).contains(&self.cache_bytes),
            "invalid registry reader limits",
        )
    }
}

#[derive(Clone, Debug)]
pub struct PinnedRelease {
    origin: String,
    release: Arc<Release>,
}
impl PinnedRelease {
    pub fn metadata(&self) -> &Release {
        &self.release
    }
}

/// Bounded owned snapshot suitable for `evaluate` and `apply_controls`. It holds
/// no provider observations or assessed assurance. Retaining it preserves the
/// pinned definitions even after the reader cache evicts them or head advances.
#[derive(Clone, Debug)]
pub struct Definitions {
    pin: PinnedRelease,
    documents: BTreeMap<Reference, Arc<Document>>,
    bytes: usize,
}
impl Definitions {
    pub fn release(&self) -> &PinnedRelease {
        &self.pin
    }
    pub fn get(&self, field_id: &str, schema_revision: u32) -> Option<&Definition> {
        self.documents
            .get(&Reference {
                field_id: field_id.into(),
                schema_revision,
            })
            .map(|d| &d.definition)
    }
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values().map(Arc::as_ref)
    }
    pub fn len(&self) -> usize {
        self.documents.len()
    }
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
    pub fn encoded_bytes(&self) -> usize {
        self.bytes
    }
    fn add(&mut self, document: Arc<Document>) -> Result<()> {
        let key = document.reference();
        if let Some(old) = self.documents.get(&key) {
            return if old == &document {
                Ok(())
            } else {
                Err(Error::Equivocation)
            };
        }
        let bytes = document.bytes()?;
        check(
            self.documents.len() < MAX_FIELDS_PER_REQUEST
                && self.bytes + bytes <= MAX_SNAPSHOT_BYTES,
            "registry snapshot exceeds definition or byte bound",
        )?;
        self.bytes += bytes;
        self.documents.insert(key, document);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Key {
    Release(String),
    TaxonomyRelease(String),
    Document(String, String, Reference),
}
enum Cached {
    Release(Arc<Release>),
    TaxonomyRelease(Arc<taxonomy::Release>),
    Document(Arc<Document>),
}
struct Cache {
    entries: BTreeMap<Key, (Cached, usize)>,
    order: VecDeque<Key>,
    bytes: usize,
    // One retained high-water identity is independent of FIFO eviction.
    known: Option<Arc<Release>>,
    head: Option<(PinnedRelease, Instant)>,
}
impl Cache {
    fn insert(&mut self, key: Key, value: Cached, bytes: usize, limits: &Limits) -> Result<()> {
        if let Some((old, _)) = self.entries.get(&key) {
            let equal = match (old, &value) {
                (Cached::Release(a), Cached::Release(b)) => a == b,
                (Cached::TaxonomyRelease(a), Cached::TaxonomyRelease(b)) => a == b,
                (Cached::Document(a), Cached::Document(b)) => a == b,
                _ => false,
            };
            return if equal {
                Ok(())
            } else {
                Err(Error::Equivocation)
            };
        }
        if bytes > limits.cache_bytes {
            return Ok(());
        }
        while self.entries.len() >= limits.cache_entries || self.bytes + bytes > limits.cache_bytes
        {
            let key = self
                .order
                .pop_front()
                .ok_or(Error::Invalid("registry cache accounting differs"))?;
            if let Some((_, removed)) = self.entries.remove(&key) {
                self.bytes -= removed;
            }
        }
        self.bytes += bytes;
        self.order.push_back(key.clone());
        self.entries.insert(key, (value, bytes));
        Ok(())
    }
}

pub struct Reader {
    origin: TrustedOrigin,
    http: reqwest::Client,
    limits: Limits,
    slots: Semaphore,
    cache: Mutex<Cache>,
}
impl Reader {
    pub fn new(origin: TrustedOrigin, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(limits.operation_timeout)
            .connect_timeout(limits.operation_timeout)
            .pool_max_idle_per_host(limits.concurrent_operations)
            .build()
            .map_err(|_| Error::Transport)?;
        Ok(Self {
            origin,
            http,
            slots: Semaphore::new(limits.concurrent_operations),
            limits,
            cache: Mutex::new(Cache {
                entries: BTreeMap::new(),
                order: VecDeque::new(),
                bytes: 0,
                known: None,
                head: None,
            }),
        })
    }
    /// Reuse a current head only within its configured bounded TTL. Refresh
    /// failures never return an expired head as though it were current.
    pub async fn current(&self) -> Result<PinnedRelease> {
        {
            let cache = self.lock()?;
            if let Some((head, observed)) = &cache.head {
                if observed.elapsed() < self.limits.head_ttl
                    && cache.known.as_ref() == Some(&head.release)
                {
                    return Ok(head.clone());
                }
            }
        }
        self.refresh_head().await
    }
    pub async fn refresh_head(&self) -> Result<PinnedRelease> {
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout, async {
            let (release, etag) = self
                .read::<Release>("current", None, wire::MAX_RELEASE_BYTES)
                .await?;
            release.validate()?;
            check(
                etag == wire::etag(&release, None)?,
                "registry release ETag differs",
            )?;
            let pin = self.pin(release);
            let mut cache = self.lock()?;
            self.record_known(&mut cache, &pin, true)?;
            self.cache_release(&mut cache, &pin)?;
            cache.head = Some((pin.clone(), Instant::now()));
            Ok(pin)
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
    /// Exact reads do not assert head freshness. A newly seen higher release
    /// still raises the monotonic watermark; old pinned reads remain available.
    pub async fn pin_release(&self, id: &str) -> Result<PinnedRelease> {
        check(wire::release_id(id), "invalid registry release ID")?;
        {
            let cache = self.lock()?;
            if let Some((Cached::Release(release), _)) = cache.entries.get(&Key::Release(id.into()))
            {
                return Ok(PinnedRelease {
                    origin: self.origin.as_str().into(),
                    release: release.clone(),
                });
            }
        }
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout, async {
            let (release, etag) = self
                .read::<Release>(id, None, wire::MAX_RELEASE_BYTES)
                .await?;
            release.validate()?;
            check(
                release.release_id == id && etag == wire::etag(&release, None)?,
                "exact registry release differs",
            )?;
            let pin = self.pin(release);
            let mut cache = self.lock()?;
            self.record_known(&mut cache, &pin, false)?;
            self.cache_release(&mut cache, &pin)?;
            Ok(pin)
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
    pub async fn lookup_exact(
        &self,
        pin: &PinnedRelease,
        references: &[Reference],
    ) -> Result<Definitions> {
        self.check_pin(pin)?;
        wire::validate_references(references)?;
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout, self.lookup(pin, references))
            .await
            .map_err(|_| Error::Deadline)?
    }
    /// Resolve exact rule references in bounded batches at one pinned release.
    /// No full registry walk, mutable draft lookup, default injection or rule
    /// execution. Per-root cycles are visited once; mixed meanings fail closed.
    pub async fn resolve_closure(
        &self,
        pin: &PinnedRelease,
        roots: &[Reference],
    ) -> Result<Definitions> {
        self.check_pin(pin)?;
        wire::validate_references(roots)?;
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout, async {
            let mut snapshot = self.lookup(pin, roots).await?;
            loop {
                let mut missing = BTreeSet::new();
                for document in snapshot.documents.values() {
                    check(
                        !matches!(document.definition.usage, Usage::FilterOnly)
                            || document.definition.rules.is_empty(),
                        "filter-only registry rules are unsupported",
                    )?;
                    for condition in conditions(&document.definition) {
                        let reference = Reference {
                            field_id: condition.field_id.clone(),
                            schema_revision: condition.schema_revision,
                        };
                        if !snapshot.documents.contains_key(&reference) {
                            missing.insert(reference);
                        }
                    }
                }
                if missing.is_empty() {
                    break;
                }
                check(
                    snapshot.len() + missing.len() <= MAX_FIELDS_PER_REQUEST,
                    "registry closure exceeds 96 definitions",
                )?;
                for document in self
                    .lookup(pin, &missing.into_iter().collect::<Vec<_>>())
                    .await?
                    .documents
                    .into_values()
                {
                    snapshot.add(document)?;
                }
            }
            validate_closure(&snapshot, roots)?;
            Ok(snapshot)
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
    async fn lookup(&self, pin: &PinnedRelease, refs: &[Reference]) -> Result<Definitions> {
        let mut snapshot = Definitions {
            pin: pin.clone(),
            documents: BTreeMap::new(),
            bytes: 0,
        };
        let mut missing = Vec::new();
        {
            let cache = self.lock()?;
            for reference in refs {
                match cache.entries.get(&Key::Document(
                    pin.release.release_id.clone(),
                    pin.release.release_hash.clone(),
                    reference.clone(),
                )) {
                    Some((Cached::Document(document), _)) => snapshot.add(document.clone())?,
                    _ => missing.push(reference.clone()),
                }
            }
        }
        if !missing.is_empty() {
            let body = serde_json::json!({ "references": missing });
            let max_bytes =
                16 * 1024 + missing.len() * (MAX_DEFINITION_BYTES + wire::DOCUMENT_OVERHEAD);
            let (lookup, etag) = self
                .read::<wire::Lookup>(
                    &format!("{}/lookup", pin.release.release_id),
                    Some(&body),
                    max_bytes,
                )
                .await?;
            lookup.validate(&pin.release, &missing)?;
            check(
                etag == wire::etag(&pin.release, Some(&missing))?,
                "registry lookup ETag differs",
            )?;
            // Validate the complete batch before exposing or caching any item.
            for document in lookup.definitions {
                snapshot.add(Arc::new(document))?;
            }
            let mut cache = self.lock()?;
            // Check the whole validated batch before mutating the shared cache.
            // Concurrent requests may have learned a conflicting immutable
            // document while this request was in flight.
            for document in snapshot.documents.values() {
                let key = Key::Document(
                    pin.release.release_id.clone(),
                    pin.release.release_hash.clone(),
                    document.reference(),
                );
                if let Some((Cached::Document(known), _)) = cache.entries.get(&key) {
                    if known != document {
                        return Err(Error::Equivocation);
                    }
                }
            }
            for document in snapshot.documents.values() {
                let key = Key::Document(
                    pin.release.release_id.clone(),
                    pin.release.release_hash.clone(),
                    document.reference(),
                );
                cache.insert(
                    key,
                    Cached::Document(document.clone()),
                    document.bytes()?,
                    &self.limits,
                )?;
            }
        }
        Ok(snapshot)
    }
    async fn read<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<&serde_json::Value>,
        max_bytes: usize,
    ) -> Result<(T, String)> {
        let url = self
            .origin
            .0
            .join(&format!("v1/proxy/registry/releases/{path}"))
            .map_err(|_| Error::Invalid("invalid fixed registry route"))?;
        let request = if let Some(body) = body {
            self.http.post(url).json(body)
        } else {
            self.http.get(url)
        };
        let response = request
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            let bytes = response_bytes(response, 4096).await?;
            let expected = if path == "current" {
                "proxy_registry_unpublished"
            } else if body.is_some() {
                "proxy_registry_reference_unpublished"
            } else {
                "proxy_registry_release_not_found"
            };
            let known = serde_json::from_slice::<wire::Failure>(&bytes)
                .ok()
                .is_some_and(|failure| {
                    failure.error.status == 404
                        && failure.error.code == expected
                        && failure.error.message.len() <= 2048
                });
            return Err(if known {
                Error::NotPublished
            } else {
                Error::Http(404)
            });
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Http(response.status().as_u16()));
        }
        check(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';')
                        .next()
                        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
                }),
            "registry response is not JSON",
        )?;
        check(
            response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .is_none_or(|v| v == "identity"),
            "registry content encoding is unsupported",
        )?;
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() == 66)
            .ok_or(Error::Invalid("registry response ETag is missing"))?
            .to_owned();
        let bytes = response_bytes(response, max_bytes).await?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Invalid("invalid registry response JSON"))?;
        Ok((value, etag))
    }
    fn check_pin(&self, pin: &PinnedRelease) -> Result<()> {
        check(
            pin.origin == self.origin.as_str(),
            "registry release belongs to another trusted origin",
        )
    }
    fn pin(&self, release: Release) -> PinnedRelease {
        PinnedRelease {
            origin: self.origin.as_str().into(),
            release: Arc::new(release),
        }
    }
    fn cache_release(&self, cache: &mut Cache, pin: &PinnedRelease) -> Result<()> {
        let bytes = serde_json::to_vec(pin.metadata())
            .map_err(|_| Error::Invalid("invalid release JSON"))?
            .len();
        cache.insert(
            Key::Release(pin.release.release_id.clone()),
            Cached::Release(pin.release.clone()),
            bytes,
            &self.limits,
        )
    }
    fn record_known(&self, cache: &mut Cache, pin: &PinnedRelease, is_head: bool) -> Result<()> {
        if let Some((Cached::Release(known), _)) = cache
            .entries
            .get(&Key::Release(pin.release.release_id.clone()))
        {
            if known != &pin.release {
                return Err(Error::Equivocation);
            }
        }
        let sequence = pin.release.sequence()?;
        if let Some(known) = &cache.known {
            let previous = known.sequence()?;
            if is_head && sequence < previous {
                return Err(Error::Rollback);
            }
            if (sequence == previous || pin.release.release_id == known.release_id)
                && pin.release != *known
            {
                return Err(Error::Equivocation);
            }
            if sequence == previous + 1
                && (pin.release.manifest.parent_release_id.as_deref() != Some(&known.release_id)
                    || pin.release.manifest.parent_release_hash.as_deref()
                        != Some(&known.release_hash))
            {
                return Err(Error::Equivocation);
            }
            if sequence <= previous {
                return Ok(());
            }
        }
        cache.known = Some(pin.release.clone());
        Ok(())
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Cache>> {
        self.cache
            .lock()
            .map_err(|_| Error::Invalid("registry cache unavailable"))
    }
}
fn transport(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Deadline
    } else {
        Error::Transport
    }
}
async fn response_bytes(mut response: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>> {
    check(
        response
            .content_length()
            .is_none_or(|n| n <= max_bytes as u64),
        "registry response exceeds byte bound",
    )?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        check(
            chunk.len() <= max_bytes - bytes.len(),
            "registry response exceeds byte bound",
        )?;
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn conditions(definition: &Definition) -> impl Iterator<Item = &super::Condition> {
    definition
        .rules
        .iter()
        .flat_map(|r| r.when.iter().chain(&r.require).chain(&r.forbid))
}
fn validate_closure(snapshot: &Definitions, roots: &[Reference]) -> Result<()> {
    for document in snapshot.documents.values() {
        for condition in conditions(&document.definition) {
            let target = snapshot
                .get(&condition.field_id, condition.schema_revision)
                .ok_or(Error::Invalid("registry closure reference is missing"))?;
            check(
                matches!(target.usage, Usage::RequestControl { .. })
                    && document
                        .definition
                        .endpoints
                        .iter()
                        .all(|endpoint| target.endpoints.contains(endpoint))
                    && target.operators.contains(&condition.operator),
                "registry conditional reference is incompatible",
            )?;
            target
                .value_schema
                .accepts(&condition.value)
                .map_err(|_| Error::Invalid("registry conditional value is incompatible"))?;
        }
    }
    for root in roots {
        let mut queue = VecDeque::from([root.clone()]);
        let mut visited = BTreeSet::new();
        let mut revisions = BTreeMap::new();
        while let Some(reference) = queue.pop_front() {
            if !visited.insert(reference.clone()) {
                continue;
            }
            check(
                revisions
                    .insert(reference.field_id.clone(), reference.schema_revision)
                    .is_none_or(|r| r == reference.schema_revision),
                "registry conditional graph mixes semantic revisions",
            )?;
            for condition in conditions(
                snapshot
                    .get(&reference.field_id, reference.schema_revision)
                    .ok_or(Error::Invalid("registry closure reference is missing"))?,
            ) {
                let next = Reference {
                    field_id: condition.field_id.clone(),
                    schema_revision: condition.schema_revision,
                };
                if !visited.contains(&next) {
                    queue.push_back(next);
                }
            }
        }
    }
    Ok(())
}
