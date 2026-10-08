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
    pub revision: u64,
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
        })
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
            return Err(decode_http_failure(response, self.error_profile).await);
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
            terminal_failure: None,
            received_bytes: 0,
            max_bytes: self.limits.max_response_bytes,
            status,
            format,
            error_profile: self.error_profile,
        })
    }
}

async fn decode_http_failure(mut response: Response, profile: ErrorProfile) -> Failure {
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
    let mut bytes = Vec::new();
    // Bounded diagnostic error-body read only. Never applies to a generation.
    // An adversarial error stream cannot retain a dispatch slot indefinitely.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Ok(Some(chunk)) = response.chunk().await {
            if bytes.len() + chunk.len() > 16 * 1024 {
                bytes.clear();
                break;
            }
            bytes.extend_from_slice(&chunk);
        }
    })
    .await;
    failure::openai_error(
        status,
        if matches!(profile, ErrorProfile::OpenAi) {
            &bytes
        } else {
            &[]
        },
        retry_after.as_deref(),
        SystemTime::now(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        if let Some(failure) = &self.terminal_failure {
            return Err(failure.clone());
        }
        loop {
            if !self.pending.is_empty() {
                return Ok(Some(
                    self.pending.split_to(self.pending.len().min(64 * 1024)),
                ));
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            match response.chunk().await {
                Ok(Some(chunk)) => {
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
        if matches!(self.error_profile, ErrorProfile::OpenAi)
            && value.get("error").is_some_and(serde_json::Value::is_object)
        {
            let mut error = failure::openai_error(self.status, &bytes, None, SystemTime::now());
            error.stage = Stage::ResponseBody;
            return Err(error);
        }
        Ok(value)
    }
}
