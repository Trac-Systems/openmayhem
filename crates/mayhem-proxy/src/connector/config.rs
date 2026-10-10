use super::{require, SetupError, SetupResult};
use ipnet::IpNet;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use url::{Host, Url};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Models,
    ChatCompletions,
    Completions,
    Responses,
    Decisions,
    JobPoll,
    JobResult,
    JobCancel,
    JobLookup,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorProfile {
    #[default]
    HttpStatus,
    OpenAi,
    /// OpenAI framing plus the audited single-request vLLM admission refusal
    /// contract. Explicit private configuration, never inferred from a model name.
    VllmAdmissionV1,
}

impl ErrorProfile {
    pub(crate) fn openai_framing(self) -> bool {
        matches!(self, Self::OpenAi | Self::VllmAdmissionV1)
    }
}

impl Operation {
    pub fn method(self) -> reqwest::Method {
        if self == Self::Models {
            reqwest::Method::GET
        } else {
            reqwest::Method::POST
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkPolicy {
    PublicHttps,
    /// Explicit operator authority for local/private servers. Recipes and public
    /// requests cannot add networks, change the origin or select a destination.
    Pinned {
        networks: Vec<IpNet>,
        #[serde(default)]
        allow_http: bool,
    },
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        _ => ip,
    }
}

fn never_destination(ip: IpAddr) -> bool {
    match canonical_ip(ip) {
        IpAddr::V4(v) => {
            v.is_unspecified()
                || v.is_link_local()
                || v.is_multicast()
                || v.octets()[0] == 0
                || v.octets()[0] >= 240
                || v.octets() == [100, 100, 100, 200]
        }
        IpAddr::V6(v) => v.is_unspecified() || v.is_unicast_link_local() || v.is_multicast(),
    }
}

fn public_ip(ip: IpAddr) -> bool {
    let ip = canonical_ip(ip);
    if never_destination(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !v.is_private()
                && !v.is_loopback()
                && !v.is_documentation()
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && ((b == 0 && c == 0) || (b == 88 && c == 99)))
                && !(a == 198 && (b == 18 || b == 19))
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            // Current global-unicast space, excluding documentation and tunneled
            // special-use ranges which can encode a forbidden IPv4 destination.
            s[0] & 0xe000 == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

pub(crate) fn private_ip(ip: IpAddr) -> bool {
    match canonical_ip(ip) {
        IpAddr::V4(v) => {
            v.is_private()
                || v.is_loopback()
                || (v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]))
        }
        IpAddr::V6(v) => v.is_loopback() || v.is_unique_local(),
    }
}

impl NetworkPolicy {
    pub fn permits(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        !never_destination(ip)
            && match self {
                Self::PublicHttps => public_ip(ip),
                Self::Pinned { networks, .. } => networks.iter().any(|n| n.contains(&ip)),
            }
    }

    pub(crate) fn validate(&self, url: &Url) -> SetupResult<()> {
        match self {
            Self::PublicHttps => {
                require(url.scheme() == "https", "public upstreams require HTTPS")?
            }
            Self::Pinned {
                networks,
                allow_http,
            } => {
                require(
                    !networks.is_empty() && networks.len() <= 32,
                    "explicit destination networks are required",
                )?;
                require(
                    url.scheme() == "https" || *allow_http,
                    "plain HTTP requires explicit local approval",
                )?;
                // /0 is not a meaningful restricted connection authority.
                require(
                    networks.iter().all(|n| n.prefix_len() > 0),
                    "unrestricted destination networks are forbidden",
                )?;
            }
        }
        if let Some(host) = url.host() {
            let ip = match host {
                Host::Ipv4(v) => Some(IpAddr::V4(v)),
                Host::Ipv6(v) => Some(IpAddr::V6(v)),
                _ => None,
            };
            if let Some(ip) = ip {
                require(
                    self.permits(ip) && (url.scheme() == "https" || private_ip(ip)),
                    "literal destination is outside approved networks",
                )?;
            }
        }
        // Core control and standard host-management ports are not inference APIs.
        require(
            !matches!(
                url.port_or_known_default(),
                Some(22 | 2375 | 2376 | 11435 | 11437)
            ),
            "host control ports are forbidden",
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretSource {
    File { path: PathBuf },
    Environment { name: String },
}

impl SecretSource {
    fn load(&self) -> SetupResult<Zeroizing<Vec<u8>>> {
        let bytes = match self {
            Self::File { path } => private_file(path, 8192)?,
            Self::Environment { name } => {
                require(
                    !name.is_empty()
                        && name.len() <= 128
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                    "invalid credential environment reference",
                )?;
                Zeroizing::new(
                    std::env::var(name)
                        .map_err(|_| SetupError::Credential)?
                        .into_bytes(),
                )
            }
        };
        if bytes.is_empty() || bytes.len() > 8192 {
            return Err(SetupError::Credential);
        }
        Ok(bytes)
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Authentication {
    #[default]
    None,
    Bearer {
        secret: SecretSource,
    },
    Header {
        name: String,
        secret: SecretSource,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_in_flight: usize,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub connect_timeout_ms: u64,
    /// Optional connection inactivity guard, never a total generation deadline.
    /// Absence leaves long upstream processing to accepted request/cancel policy.
    pub read_idle_timeout_ms: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_in_flight: 2,
            max_request_bytes: 32 * 1024 * 1024,
            max_response_bytes: 32 * 1024 * 1024,
            connect_timeout_ms: 10_000,
            read_idle_timeout_ms: None,
        }
    }
}

impl Limits {
    fn validate(&self) -> SetupResult<()> {
        require(
            (1..=1024).contains(&self.max_in_flight),
            "invalid dispatch concurrency",
        )?;
        require(
            [self.max_request_bytes, self.max_response_bytes]
                .iter()
                .all(|n| (1..=256 * 1024 * 1024).contains(n)),
            "invalid connection byte limit",
        )?;
        require(
            (1..=86_400_000).contains(&self.connect_timeout_ms)
                && self
                    .read_idle_timeout_ms
                    .is_none_or(|n| (1..=86_400_000).contains(&n)),
            "invalid connection timeout",
        )
    }

    pub(crate) fn read_idle_timeout(&self) -> Option<Duration> {
        self.read_idle_timeout_ms.map(Duration::from_millis)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub base_url: String,
    pub network: NetworkPolicy,
    pub paths: BTreeMap<Operation, String>,
    #[serde(default)]
    pub authentication: Authentication,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub error_profile: ErrorProfile,
}

impl fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("revision", &self.revision)
            .field("operations", &self.paths.len())
            .finish_non_exhaustive()
    }
}

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 2048
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
        && path
            .split('/')
            .filter(|s| !s.is_empty())
            .all(|s| s != "." && s != "..")
        && !path.contains("//")
}

fn allowed_header(name: &str) -> SetupResult<HeaderName> {
    let lower = name.to_ascii_lowercase();
    require(
        !lower.starts_with("proxy-")
            && !lower.starts_with("x-forwarded-")
            && !matches!(
                lower.as_str(),
                "host"
                    | "cookie"
                    | "connection"
                    | "content-length"
                    | "content-type"
                    | "transfer-encoding"
                    | "accept-encoding"
                    | "forwarded"
                    | "trailer"
                    | "te"
                    | "upgrade"
                    | "expect"
                    | "range"
                    | "x-http-method-override"
                    | "x-original-url"
                    | "x-rewrite-url"
            ),
        "header is not a permitted upstream API header",
    )?;
    HeaderName::from_bytes(lower.as_bytes())
        .map_err(|_| SetupError::Invalid("invalid upstream header name"))
}

impl ConnectionConfig {
    /// Private configuration commitment; credential sources are bound, never the
    /// loaded credential value. Rotation requires a new connection revision.
    pub fn fingerprint(&self) -> SetupResult<crate::attempts::Digest> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|_| SetupError::Invalid("invalid connection commitment"))?;
        crate::endpoint::digest("mayhem/proxy/connection/v1", &value)
            .map_err(|_| SetupError::Invalid("invalid connection commitment"))
    }
    pub fn load(path: &Path) -> SetupResult<Self> {
        let bytes = private_file(path, 32 * 1024)?;
        let mut config: Self = serde_json::from_slice(&bytes)
            .map_err(|_| SetupError::Invalid("invalid connection JSON"))?;
        let secret = match &mut config.authentication {
            Authentication::Bearer { secret } | Authentication::Header { secret, .. } => {
                Some(secret)
            }
            Authentication::None => None,
        };
        if let Some(SecretSource::File { path: secret_path }) = secret {
            if secret_path.is_relative() {
                *secret_path = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(&*secret_path);
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> SetupResult<Url> {
        require(
            self.schema_version == 1 && self.revision > 0,
            "unsupported connection version",
        )?;
        require(
            !self.id.is_empty()
                && self.id.len() <= 64
                && self
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "invalid connection identifier",
        )?;
        require(
            self.base_url.len() <= 2048
                && !self.base_url.contains(['%', '\\'])
                && !self
                    .base_url
                    .split('/')
                    .any(|part| matches!(part, "." | ".."))
                && self.base_url.bytes().all(|b| b.is_ascii_graphic()),
            "invalid upstream base URL",
        )?;
        let url = Url::parse(&self.base_url)
            .map_err(|_| SetupError::Invalid("invalid upstream base URL"))?;
        require(
            !url.domain().is_some_and(|host| {
                matches!(
                    host.trim_end_matches('.'),
                    "openmayhem.ai" | "api.openmayhem.ai" | "mcp.openmayhem.ai"
                )
            }),
            "OpenMayhem cannot be its own proxy upstream",
        )?;
        require(
            matches!(url.scheme(), "https" | "http")
                && url.has_host()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path().ends_with('/')
                && safe_path(url.path()),
            "base URL requires a directory path and no credentials, query or fragment",
        )?;
        self.network.validate(&url)?;
        self.limits.validate()?;
        require(
            !self.paths.is_empty(),
            "at least one LLM or decision operation is required",
        )?;
        for path in self.paths.values() {
            require(
                safe_path(path) && !path.starts_with('/'),
                "operation paths must stay relative to the approved base",
            )?;
        }
        require(self.headers.len() <= 16, "too many upstream headers")?;
        for (name, value) in &self.headers {
            let header = allowed_header(name)?;
            require(
                header != "authorization"
                    && header != "api-key"
                    && !name.to_ascii_lowercase().contains("key"),
                "credentials require a secret reference",
            )?;
            require(
                value.len() <= 1024 && HeaderValue::from_str(value).is_ok(),
                "invalid upstream header value",
            )?;
        }
        Ok(url)
    }

    pub(crate) fn request_headers(&self) -> SetupResult<HeaderMap> {
        let mut headers = HeaderMap::new();
        for (name, value) in &self.headers {
            let name = allowed_header(name)?;
            let mut value = HeaderValue::from_str(value)
                .map_err(|_| SetupError::Invalid("invalid upstream header"))?;
            value.set_sensitive(true);
            if headers.insert(name, value).is_some() {
                return Err(SetupError::Invalid("duplicate upstream header"));
            }
        }
        let (name, secret, bearer) = match &self.authentication {
            Authentication::None => return Ok(headers),
            Authentication::Bearer { secret } => {
                (HeaderName::from_static("authorization"), secret, true)
            }
            Authentication::Header { name, secret } => (allowed_header(name)?, secret, false),
        };
        let secret = secret.load()?;
        let text = std::str::from_utf8(&secret)
            .map_err(|_| SetupError::Credential)?
            .trim_end_matches(['\r', '\n']);
        if text.is_empty()
            || text
                .bytes()
                .any(|b| !b.is_ascii_graphic() && !(b == b' ' && !bearer))
        {
            return Err(SetupError::Credential);
        }
        let value = Zeroizing::new(if bearer {
            format!("Bearer {text}")
        } else {
            text.to_owned()
        });
        let mut value = HeaderValue::from_str(&value).map_err(|_| SetupError::Credential)?;
        value.set_sensitive(true);
        require(
            !headers.contains_key(&name),
            "authentication header conflicts with a fixed header",
        )?;
        headers.insert(name, value);
        Ok(headers)
    }
}

/// No symlink/FIFO blocking, no non-owner permissions, no unbounded read. Paths
/// and OS error strings are deliberately absent from returned public errors.
pub fn private_file(path: &Path, max_bytes: usize) -> SetupResult<Zeroizing<Vec<u8>>> {
    #[cfg(unix)]
    {
        use rustix::fs::{open, Mode, OFlags};
        use std::os::unix::fs::MetadataExt;
        let fd = open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| SetupError::File)?;
        let file = std::fs::File::from(fd);
        let meta = file.metadata().map_err(|_| SetupError::File)?;
        if !meta.is_file()
            || meta.mode() & 0o077 != 0
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.nlink() != 1
        {
            return Err(SetupError::FilePermissions);
        }
        if meta.len() > max_bytes as u64 {
            return Err(SetupError::Invalid("private file exceeds byte limit"));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| SetupError::File)?;
        require(bytes.len() <= max_bytes, "private file exceeds byte limit")?;
        Ok(bytes)
    }
    #[cfg(windows)]
    {
        mayhem_windows_sandbox::read_private_file(path, max_bytes)
            .map_err(|_| SetupError::FilePermissions)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, max_bytes);
        Err(SetupError::UnsupportedFilePermissions)
    }
}
