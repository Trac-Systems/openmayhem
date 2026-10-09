//! Explicit canonical observation, never invoice/payment or publication authority.
use super::*;
use crate::discovery::{Context, Proof};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_RESPONSE: usize = 8192;
const MAX_AGE_MS: u64 = 15_000;
const MAX_SAFE: u64 = mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionState {
    Pending,
    Observed,
    Unavailable,
    InvalidResponse,
    TimedOut,
    ConfigurationChanged,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalProvider {
    pub sequence: u64,
    pub operation_digest: Digest,
    pub entitlement_id: Digest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionEvidence {
    pub context: Context,
    pub proof: Proof,
    pub registry_enabled: bool,
    pub fee_policy_hash: Digest,
    pub provider: Option<CanonicalProvider>,
    pub provider_revoked: bool,
    pub admission_revoked: bool,
}
impl AdmissionEvidence {
    pub(super) fn validate(&self) -> Result<()> {
        self.context
            .identity()
            .validate()
            .map_err(|_| Error::Invalid)?;
        self.proof.validate().map_err(|_| Error::Invalid)?;
        require(
            self.context.epoch <= MAX_SAFE
                && self
                    .provider
                    .as_ref()
                    .is_none_or(|p| (1..=MAX_SAFE).contains(&p.sequence))
                && (self.provider.is_some() || !self.admission_revoked),
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    ok: bool,
    schema_version: u32,
    lane: ProxyLane,
    requester: Digest,
    provider_pubkey: Digest,
    initial_operation_digest: Digest,
    request_nonce: Digest,
    context: Context,
    proof: Proof,
    registry_enabled: bool,
    fee_policy_hash: Digest,
    provider: Option<CanonicalProvider>,
    provider_revoked: bool,
    admission_revoked: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    schema_version: u32,
    draft_id: Digest,
    binding: Digest,
    initial_operation_digest: Digest,
    state: AdmissionState,
    started_at_ms: u64,
    expires_at_ms: u64,
    // One high-water proof survives unavailable reads; no history or scans.
    high_water: Option<(Proof, u64)>,
    evidence: Option<AdmissionEvidence>,
}
impl Attempt {
    pub(super) fn high_water(&self) -> Option<(Proof, u64)> {
        self.high_water.clone()
    }
    pub(super) fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.expires_at_ms <= MAX_SAFE
                && self.started_at_ms.checked_add(MAX_AGE_MS) == Some(self.expires_at_ms)
                && (self.state == AdmissionState::Observed) == self.evidence.is_some(),
        )?;
        if let Some((proof, epoch)) = &self.high_water {
            proof.validate().map_err(|_| Error::Invalid)?;
            require(*epoch <= MAX_SAFE)?;
        }
        if let Some(evidence) = &self.evidence {
            evidence.validate()?;
            require(
                self.high_water
                    .as_ref()
                    .is_some_and(|(p, e)| p == &evidence.proof && *e == evidence.context.epoch),
            )?;
        }
        Ok(())
    }
    pub(super) fn report(&self, record: &Record) -> Result<AdmissionReport> {
        self.validate()?;
        let current = self.draft_id == record.id
            && record.binding()? == self.binding
            && record.checked.as_ref() == Some(&self.binding)
            && record
                .input
                .connection()
                .is_ok_and(|value| value == record.connection);
        let expired = now_ms().map_or(true, |now| {
            now < self.started_at_ms || now > self.expires_at_ms
        });
        let evidence = self.evidence.clone();
        let next = evidence.as_ref().and_then(|e| match &e.provider {
            Some(p) => p.sequence.checked_add(1).filter(|v| *v <= MAX_SAFE),
            None => Some(1),
        });
        let status = if !current || expired {
            "refresh_required"
        } else {
            match self.state {
                AdmissionState::Observed => {
                    let e = evidence.as_ref().ok_or(Error::Invalid)?;
                    if e.provider_revoked || e.admission_revoked {
                        "observed_revoked"
                    } else if e.provider.is_some() {
                        "observed_admitted"
                    } else {
                        "observed_not_registered"
                    }
                }
                AdmissionState::Pending => "observation_pending",
                _ => "observation_unavailable",
            }
        };
        Ok(AdmissionReport {
            state: self.state,
            status,
            for_current_configuration: current,
            expired,
            observed_at_ms: self.started_at_ms,
            expires_at_ms: self.expires_at_ms,
            initial_operation_digest: self.initial_operation_digest.clone(),
            sequence_matches: next.map(|v| v == record.input.sequence),
            next_sequence: next,
            operation_already_applied: evidence.as_ref().map(|e| {
                e.provider.as_ref().is_some_and(|p| {
                    p.sequence == record.input.sequence
                        && p.operation_digest == self.initial_operation_digest
                })
            }),
            evidence,
            payment_status: "not_checked",
            authorizes_publication: false,
        })
    }
}

/// These are retained observations. Every later invoice/publication path must
/// obtain fresh canonical facts; no field in this report is an execution permit.
#[derive(Serialize)]
pub struct AdmissionReport {
    pub state: AdmissionState,
    pub status: &'static str,
    pub for_current_configuration: bool,
    pub expired: bool,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    pub initial_operation_digest: Digest,
    pub next_sequence: Option<u64>,
    pub sequence_matches: Option<bool>,
    pub operation_already_applied: Option<bool>,
    pub evidence: Option<AdmissionEvidence>,
    pub payment_status: &'static str,
    pub authorizes_publication: bool,
}

fn now_ms() -> Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid)?
            .as_millis(),
    )
    .ok()
    .filter(|v| *v <= MAX_SAFE)
    .ok_or(Error::Invalid)
}

impl Store {
    /// A trusted local peer authenticates this read with its existing identity.
    /// HTTPS remote peers must be explicitly trusted by the operator. No wallet
    /// or inference connection credentials are read by this command.
    pub async fn admission_check(
        &self,
        expected_revision: u64,
        peer_rpc: &str,
        timeout_ms: u64,
    ) -> Result<Review> {
        require((1..=MAX_AGE_MS).contains(&timeout_ms))?;
        let (client, base) = peer(peer_rpc, timeout_ms)?;
        let guard = store::Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        let binding = record.binding()?;
        require(record.checked.as_ref() == Some(&binding))?;
        if record.input.connection()? != record.connection {
            return Err(Error::ConnectionChanged);
        }
        require(
            record
                .revision
                .checked_add(2)
                .is_some_and(|v| v <= MAX_SAFE),
        )?;
        record.next(expected_revision)?;
        let started_at_ms = now_ms()?;
        let expires_at_ms = started_at_ms
            .checked_add(MAX_AGE_MS)
            .filter(|v| *v <= MAX_SAFE)
            .ok_or(Error::Invalid)?;
        let initial_operation_digest = Digest::new(
            record
                .input
                .operation()
                .digest()
                .map_err(|_| Error::Invalid)?,
        )
        .map_err(|_| Error::Invalid)?;
        let high_water = record.admission.as_ref().and_then(|v| v.high_water.clone());
        record.admission = Some(Attempt {
            schema_version: 1,
            draft_id: record.id.clone(),
            binding,
            initial_operation_digest: initial_operation_digest.clone(),
            state: AdmissionState::Pending,
            started_at_ms,
            expires_at_ms,
            high_water,
            evidence: None,
        });
        guard.write(&record)?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| Error::Storage)?;
        let nonce = Digest::hash("mayhem/proxy/setup-admission-read/v1", &[&random]);
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
            let evidence = observe(
                &client,
                &base,
                &record.input.network,
                &record.input.provider_pubkey,
                &initial_operation_digest,
                &nonce,
            )
            .await?;
            if record
                .admission
                .as_ref()
                .and_then(|a| a.high_water.as_ref())
                .is_some_and(|(proof, epoch)| {
                    !evidence.proof.follows(proof) || evidence.context.epoch < *epoch
                })
            {
                return Err(AdmissionState::InvalidResponse);
            }
            Ok(evidence)
        })
        .await;
        let attempt = record.admission.as_mut().ok_or(Error::Invalid)?;
        attempt.state = match result {
            Err(_) => AdmissionState::TimedOut,
            Ok(Err(state)) => state,
            Ok(Ok(evidence)) => {
                if started.elapsed() > Duration::from_millis(MAX_AGE_MS)
                    || !now_ms().is_ok_and(|now| now >= started_at_ms && now <= expires_at_ms)
                {
                    AdmissionState::TimedOut
                } else if !record
                    .input
                    .connection()
                    .is_ok_and(|value| value == record.connection)
                {
                    AdmissionState::ConfigurationChanged
                } else {
                    attempt.high_water = Some((evidence.proof.clone(), evidence.context.epoch));
                    attempt.evidence = Some(evidence);
                    AdmissionState::Observed
                }
            }
        };
        record.next(record.revision)?;
        guard.write(&record)?;
        record.review()
    }
}

// Shared trusted-origin read used by setup observation and publication recovery.
pub(super) fn peer(peer_rpc: &str, timeout_ms: u64) -> Result<(reqwest::Client, url::Url)> {
    require((1..=MAX_AGE_MS).contains(&timeout_ms))?;
    let base = url::Url::parse(&format!("{}/", peer_rpc.trim_end_matches('/')))
        .map_err(|_| Error::Invalid)?;
    require(
        base.username().is_empty()
            && base.password().is_none()
            && base.query().is_none()
            && base.fragment().is_none()
            && (base.scheme() == "https"
                || base.scheme() == "http"
                    && match base.host() {
                        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                        _ => false,
                    }),
    )?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|_| Error::Invalid)?;
    Ok((client, base))
}
pub(super) async fn observe(
    client: &reqwest::Client,
    base: &url::Url,
    network: &Identity,
    provider: &Digest,
    operation: &Digest,
    nonce: &Digest,
) -> std::result::Result<AdmissionEvidence, AdmissionState> {
    let endpoint = base
        .join("proxy/provider-state")
        .map_err(|_| AdmissionState::InvalidResponse)?;
    let response = client
        .post(endpoint)
        .json(&serde_json::json!({
            "provider_pubkey":provider,"initial_operation_digest":operation,"request_nonce":nonce,
        }))
        .send()
        .await
        .map_err(|_| AdmissionState::Unavailable)?;
    let bytes = bounded_response(response, MAX_RESPONSE).await?;
    let wire: Wire = serde_json::from_slice(&bytes).map_err(|_| AdmissionState::InvalidResponse)?;
    if !wire.ok
        || wire.schema_version != 1
        || wire.lane != ProxyLane::Proxy
        || &wire.requester != provider
        || &wire.provider_pubkey != provider
        || &wire.request_nonce != nonce
        || &wire.initial_operation_digest != operation
        || &wire.context.identity() != network
    {
        return Err(AdmissionState::InvalidResponse);
    }
    let evidence = AdmissionEvidence {
        context: wire.context,
        proof: wire.proof,
        registry_enabled: wire.registry_enabled,
        fee_policy_hash: wire.fee_policy_hash,
        provider: wire.provider,
        provider_revoked: wire.provider_revoked,
        admission_revoked: wire.admission_revoked,
    };
    evidence
        .validate()
        .map_err(|_| AdmissionState::InvalidResponse)?;
    Ok(evidence)
}
pub(super) async fn bounded_response(
    mut response: reqwest::Response,
    maximum: usize,
) -> std::result::Result<Vec<u8>, AdmissionState> {
    if !response.status().is_success() {
        return Err(AdmissionState::Unavailable);
    }
    if response
        .content_length()
        .is_some_and(|v| v > maximum as u64)
    {
        return Err(AdmissionState::InvalidResponse);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AdmissionState::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(AdmissionState::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
