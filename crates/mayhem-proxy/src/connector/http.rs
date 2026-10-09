//! Core-owned networking for an approved connection. A recipe chooses a declared
//! operation, never a URL/header/socket. Requests cannot follow redirects, inherit
//! environment proxies, downgrade TLS verification or retry behind the journal.

use super::{
    config::{private_ip, ConnectionConfig, ErrorProfile, Limits, NetworkPolicy, Operation},
    failure::{self, Code, Execution, Failure, Scope, Stage},
    SetupError, SetupResult,
};
use bytes::Bytes;
use reqwest::{
    dns::{Addrs, Name, Resolve, Resolving},
    header::{HeaderMap, CONTENT_TYPE},
    Client, Response,
};
use std::{
    collections::BTreeMap,
    fmt, io,
    net::{SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;

struct SystemDns {
    permits: Arc<Semaphore>,
}

async fn blocking_dns(
    permits: Arc<Semaphore>,
    lookup: impl FnOnce() -> io::Result<Vec<SocketAddr>> + Send + 'static,
) -> io::Result<Vec<SocketAddr>> {
    let permit = permits
        .try_acquire_owned()
        .map_err(|_| io::Error::other("upstream DNS capacity unavailable"))?;
    // Ownership remains inside actual work, including after async cancellation.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        lookup()
    })
    .await
    .map_err(|_| io::Error::other("upstream DNS worker failed"))?
}
impl Resolve for SystemDns {
    fn resolve(&self, name: Name) -> Resolving {
        let permits = self.permits.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = blocking_dns(permits, move || {
                (host.as_str(), 0)
                    .to_socket_addrs()
                    .map(|a| a.take(65).collect::<Vec<_>>())
                    .map_err(|_| io::Error::other("upstream DNS lookup failed"))
            })
            .await?;
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn slicing_one_buffered_body_does_not_create_new_timing_evidence() {
        let at = tokio::time::Instant::now();
        let mut response = UpstreamResponse {
            response: None,
            permit: None,
            pending: Bytes::from(vec![1; 130_000]),
            received_at: at,
            terminal_failure: None,
            received_bytes: 130_000,
            max_bytes: 130_000,
            status: 200,
            format: WireFormat::Sse,
            error_profile: ErrorProfile::HttpStatus,
        };
        let mut total = 0;
        while let Some((bytes, observed)) = response.next_timed_chunk().await.unwrap() {
            assert_eq!(observed, at);
            assert!(bytes.len() <= 64 * 1024);
            total += bytes.len();
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        assert_eq!(total, 130_000);
    }
    #[tokio::test]
    async fn cancelled_dns_keeps_its_permit_until_actual_lookup_finishes() {
        let permits = Arc::new(Semaphore::new(1));
        let (started, receive_started) = tokio::sync::oneshot::channel();
        let (release, receive_release) = std::sync::mpsc::channel();
        let lookup = tokio::spawn(blocking_dns(permits.clone(), move || {
            let _ = started.send(());
            // A failed assertion cannot leave this test process hung forever.
            let _ = receive_release.recv_timeout(Duration::from_secs(3));
            Ok(vec![])
        }));
        receive_started.await.unwrap();
        lookup.abort();
        let _ = lookup.await;
        assert!(permits.clone().try_acquire_owned().is_err());
        assert!(
            blocking_dns(permits.clone(), || panic!("queued duplicate DNS lookup"))
                .await
                .is_err()
        );
        release.send(()).unwrap();
        let _guard = tokio::time::timeout(Duration::from_secs(1), permits.acquire_owned())
            .await
            .unwrap()
            .unwrap();
    }
}

struct CheckedDns {
    inner: Arc<dyn Resolve>,
    host: String,
    policy: NetworkPolicy,
    plain_http: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("upstream DNS destination rejected")]
struct DestinationDenied;
impl Resolve for CheckedDns {
    fn resolve(&self, name: Name) -> Resolving {
        let inner = self.inner.clone();
        let host = self.host.clone();
        let policy = self.policy.clone();
        let plain_http = self.plain_http;
        Box::pin(async move {
            let denied =
                || -> Box<dyn std::error::Error + Send + Sync> { Box::new(DestinationDenied) };
            if name.as_str() != host {
                return Err(denied());
            }
            let mut addresses: Vec<SocketAddr> = inner.resolve(name).await?.take(65).collect();
            if addresses.is_empty()
                || addresses.len() > 64
                || addresses
                    .iter()
                    .any(|a| !policy.permits(a.ip()) || (plain_http && !private_ip(a.ip())))
            {
                return Err(denied());
            }
            // A resolver cannot substitute a port; use only the approved URL.
            for address in &mut addresses {
                address.set_port(0);
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

pub struct HttpConnection {
    client: Client,
    paths: BTreeMap<Operation, Url>,
    headers: HeaderMap,
    limits: Limits,
    error_profile: ErrorProfile,
    permits: Arc<Semaphore>,
    revision: u64,
    fingerprint: crate::attempts::Digest,
}

impl fmt::Debug for HttpConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpConnection")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl HttpConnection {
    pub fn new(config: ConnectionConfig) -> SetupResult<Self> {
        config.validate()?;
        let dns = SystemDns {
            permits: Arc::new(Semaphore::new(config.limits.max_in_flight)),
        };
        Self::with_resolver(config, Arc::new(dns))
    }

    /// Resolver injection is trusted Core setup/testing, not a recipe capability.
    /// All returned addresses still pass the identical destination check.
    pub fn with_resolver(
        config: ConnectionConfig,
        resolver: Arc<dyn Resolve>,
    ) -> SetupResult<Self> {
        let base = config.validate()?;
        let fingerprint = config.fingerprint()?;
        let mut paths = BTreeMap::new();
        for (operation, path) in &config.paths {
            let url = base
                .join(path)
                .map_err(|_| SetupError::Invalid("invalid operation path"))?;
            if url.origin() != base.origin() || !url.path().starts_with(base.path()) {
                return Err(SetupError::Invalid("operation escaped its approved base"));
            }
            paths.insert(*operation, url);
        }
        let headers = config.request_headers()?;
        let checked = CheckedDns {
            inner: resolver,
            host: base.host_str().unwrap_or_default().into(),
            policy: config.network.clone(),
            plain_http: base.scheme() == "http",
        };
        let mut client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .dns_resolver(Arc::new(checked))
            .connect_timeout(Duration::from_millis(config.limits.connect_timeout_ms))
            .pool_max_idle_per_host(config.limits.max_in_flight);
        if let Some(timeout) = config.limits.read_idle_timeout() {
            client = client.read_timeout(timeout);
        }
        let client = client.build().map_err(|_| SetupError::Client)?;
        Ok(Self {
            client,
            paths,
            headers,
            permits: Arc::new(Semaphore::new(config.limits.max_in_flight)),
            limits: config.limits,
            error_profile: config.error_profile,
            revision: config.revision,
            fingerprint,
        })
    }

    pub fn fingerprint(&self) -> &crate::attempts::Digest {
        &self.fingerprint
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    pub fn error_profile(&self) -> ErrorProfile {
        self.error_profile
    }

    /// The caller must already have validated the public request and saved its
    /// dispatch intent. This is transport, not inference/billing authorization.
    /// Local I/O permits are not shared upstream capacity leases.
    pub async fn send(
        &self,
        operation: Operation,
        body: Option<Vec<u8>>,
    ) -> Result<UpstreamResponse, Failure> {
        let before = |code| {
            Failure::new(
                code,
                Scope::Request,
                Stage::BeforeDispatch,
                Execution::NotDispatched,
            )
        };
        let url = self
            .paths
            .get(&operation)
            .ok_or_else(|| before(Code::UnsupportedControl))?;
        if (operation == Operation::Models) != body.is_none() {
            return Err(before(Code::InvalidRequest));
        }
        if body
            .as_ref()
            .is_some_and(|b| b.is_empty() || b.len() > self.limits.max_request_bytes)
        {
            return Err(before(Code::RequestTooLarge));
        }
        let permit = self.permits.clone().try_acquire_owned().map_err(|_| {
            Failure::new(
                Code::LocalCapacity,
                Scope::Connection,
                Stage::BeforeDispatch,
                Execution::NotDispatched,
            )
        })?;
        let refusal_eligible =
            super::refusal::eligible(self.error_profile, operation, body.as_deref());
        let mut request = self
            .client
            .request(operation.method(), url.clone())
            .headers(self.headers.clone());
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }
        let response = request.send().await.map_err(|error| {
            let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
            while let Some(current) = cause {
                if current.is::<DestinationDenied>() {
                    return Failure::new(
                        Code::DestinationRejected,
                        Scope::Connection,
                        Stage::BeforeDispatch,
                        Execution::NotDispatched,
                    );
                }
                cause = current.source();
            }
            let code = if error.is_timeout() {
                Code::UpstreamTimeout
            } else {
                Code::UpstreamUnavailable
            };
            // Even connect errors remain conservative until the durable parent
            // can establish non-dispatch. No raw reqwest URL/error is retained.
            Failure::new(
                code,
                if error.is_connect() {
                    Scope::Connection
                } else {
                    Scope::Model
                },
                if error.is_connect() {
                    Stage::Connecting
                } else {
                    Stage::Dispatch
                },
                Execution::Unknown,
            )
        })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(decode_http_failure(response, self.error_profile, refusal_eligible).await);
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.limits.max_response_bytes as u64)
        {
            let mut error = Failure::new(
                Code::ResponseTooLarge,
                Scope::Model,
                Stage::ResponseHeaders,
                Execution::Unknown,
            );
            error.upstream_status = Some(status);
            return Err(error);
        }
        let format = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim);
        let format = match format {
            Some("application/json") => WireFormat::Json,
            Some("text/event-stream") => WireFormat::Sse,
            Some("application/x-ndjson" | "application/ndjson") => WireFormat::Ndjson,
            _ => WireFormat::Unknown,
        };
        Ok(UpstreamResponse {
            response: Some(response),
            permit: Some(permit),
            pending: Bytes::new(),
            received_at: tokio::time::Instant::now(),
            terminal_failure: None,
            received_bytes: 0,
            max_bytes: self.limits.max_response_bytes,
            status,
            format,
            error_profile: self.error_profile,
        })
    }
}

async fn decode_http_failure(
    mut response: Response,
    profile: ErrorProfile,
    refusal_eligible: bool,
) -> Failure {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 128)
        .map(str::to_owned);
    if matches!(profile, ErrorProfile::HttpStatus) {
        return failure::openai_error(status, &[], retry_after.as_deref(), SystemTime::now());
    }
    let json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    let mut bytes = Vec::new();
    // Bounded diagnostic error-body read only. Never applies to a generation.
    // An adversarial error stream cannot retain a dispatch slot indefinitely.
    let complete = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if chunk.len() <= (16 * 1024usize).saturating_sub(bytes.len()) => {
                    bytes.extend_from_slice(&chunk)
                }
                Ok(None) => return true,
                _ => return false,
            }
        }
    })
    .await
    .unwrap_or(false);
    // Incomplete diagnostics may not carry a trusted error code or refusal.
    if !complete {
        bytes.clear();
    }
    let mut failure = failure::openai_error(
        status,
        if profile.openai_framing() {
            &bytes
        } else {
            &[]
        },
        retry_after.as_deref(),
        SystemTime::now(),
    );
    if complete && json && refusal_eligible {
        super::refusal::recognize(&mut failure, &bytes);
    }
    failure
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireFormat {
    Json,
    Sse,
    Ndjson,
    Unknown,
}

pub struct UpstreamResponse {
    response: Option<Response>,
    permit: Option<OwnedSemaphorePermit>,
    pending: Bytes,
    received_at: tokio::time::Instant,
    terminal_failure: Option<Failure>,
    received_bytes: usize,
    max_bytes: usize,
    pub status: u16,
    pub format: WireFormat,
    error_profile: ErrorProfile,
}

impl fmt::Debug for UpstreamResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpstreamResponse")
            .field("status", &self.status)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl UpstreamResponse {
    fn fail(&mut self, code: Code) -> Failure {
        self.response = None;
        self.pending = Bytes::new();
        self.permit = None;
        let mut failure = Failure::new(code, Scope::Model, Stage::ResponseBody, Execution::Unknown);
        failure.upstream_status = Some(self.status);
        self.terminal_failure = Some(failure.clone());
        failure
    }

    /// Bounded chunks for the isolated decoder; slow consumers apply backpressure.
    /// EOF is transport EOF only. A protocol decoder must verify its finish marker.
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>, Failure> {
        self.next_timed_chunk()
            .await
            .map(|v| v.map(|(bytes, _)| bytes))
    }

    /// Splitting one buffered HTTP chunk must retain one observation timestamp.
    pub(crate) async fn next_timed_chunk(
        &mut self,
    ) -> Result<Option<(Bytes, tokio::time::Instant)>, Failure> {
        if let Some(failure) = &self.terminal_failure {
            return Err(failure.clone());
        }
        loop {
            if !self.pending.is_empty() {
                return Ok(Some((
                    self.pending.split_to(self.pending.len().min(64 * 1024)),
                    self.received_at,
                )));
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            let mut waited = false;
            let chunk = {
                let mut read = std::pin::pin!(response.chunk());
                std::future::poll_fn(|cx| {
                    let poll = std::future::Future::poll(read.as_mut(), cx);
                    waited |= poll.is_pending();
                    poll
                })
                .await
            };
            match chunk {
                Ok(Some(chunk)) => {
                    // Multiple HTTP frames already buffered when polled belong
                    // to one observation batch. Splitting them cannot certify a
                    // generation interval. A bounded reader's stalls invalidate
                    // measurement independently.
                    if waited || self.received_bytes == 0 {
                        self.received_at = tokio::time::Instant::now();
                    }
                    if chunk.len() > self.max_bytes.saturating_sub(self.received_bytes) {
                        return Err(self.fail(Code::ResponseTooLarge));
                    }
                    self.received_bytes += chunk.len();
                    self.pending = chunk;
                }
                Ok(None) => {
                    self.response = None;
                    self.permit = None;
                    return Ok(None);
                }
                Err(error) => {
                    return Err(self.fail(if error.is_timeout() {
                        Code::UpstreamTimeout
                    } else {
                        Code::UpstreamUnavailable
                    }))
                }
            }
        }
    }

    pub async fn collect_json(mut self) -> Result<serde_json::Value, Failure> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.next_chunk().await? {
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| self.fail(Code::UpstreamProtocol))?;
        if self.error_profile.openai_framing()
            && value.get("error").is_some_and(serde_json::Value::is_object)
        {
            let mut error = failure::openai_error(self.status, &bytes, None, SystemTime::now());
            error.stage = Stage::ResponseBody;
            return Err(error);
        }
        Ok(value)
    }
}
