use std::collections::{BTreeMap, BTreeSet};

use mayhem_proto::{
    AdminAttestationPolicy, AdminEnclaveAttestationBinding, AttestationMeasurementLayer,
    AttestationOriginPin, AttestationQuoteKindPolicy, AttestationTrustDataKind,
    AttestationTrustDataRef, HardwareQuoteKind, ATTESTATION_POLICY_SCHEMA_VERSION,
    TIER3_CONFIDENTIAL_COMPUTE_TIER,
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::MAX_COLLATERAL_BYTES;

pub const MAX_POLICY_BYTES: usize = 256 * 1024;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PolicyError {
    #[error("attestation policy is empty")]
    Empty,
    #[error("attestation policy is {actual} bytes; maximum is {maximum}")]
    TooLarge { actual: usize, maximum: usize },
    #[error("attestation policy JSON is invalid: {0}")]
    InvalidJson(String),
    #[error("attestation policy schema {actual} is unsupported; expected {expected}")]
    UnsupportedSchema { expected: u32, actual: u32 },
    #[error("attestation policy sequence must be positive")]
    InvalidSequence,
    #[error("attestation policy min_verifier_version must be positive")]
    InvalidVerifierVersion,
    #[error("attestation policy effective_epoch precedes issued_epoch")]
    EffectiveBeforeIssue,
    #[error("attestation policy expires_epoch must be later than effective_epoch")]
    InvalidExpiry,
    #[error("{field} must be a lowercase 32-byte SHA-256 digest")]
    InvalidDigest { field: String },
    #[error("{field} must be a non-empty safe identifier")]
    InvalidIdentifier { field: String },
    #[error("duplicate attestation origin pin id {0}")]
    DuplicateOrigin(String),
    #[error("attestation origin {id} is invalid: {reason}")]
    InvalidOrigin { id: String, reason: String },
    #[error("duplicate attestation trust-data id {0}")]
    DuplicateTrustData(String),
    #[error("attestation trust data {id} is invalid: {reason}")]
    InvalidTrustData { id: String, reason: String },
    #[error("managed verifier trust data {id} is invalid: {reason}")]
    InvalidManagedVerifierTrustData { id: String, reason: String },
    #[error(
        "attestation quote kind {kind} has an incomplete managed verifier pair for target {target}"
    )]
    IncompleteManagedVerifierTarget { kind: String, target: String },
    #[error("attestation trust data {trust_data} references unknown origin pin {origin}")]
    UnknownOrigin { trust_data: String, origin: String },
    #[error("duplicate attestation quote-kind policy for {0}")]
    DuplicateQuoteKind(String),
    #[error("attestation policy is missing an explicit entry for {0}")]
    MissingQuoteKind(String),
    #[error("attestation quote kind {kind} cannot use verifier profile {profile}")]
    ProfileKindMismatch { kind: String, profile: String },
    #[error("enabled attestation quote kind {0} has an invalid evidence schema version")]
    InvalidEvidenceSchemaVersion(String),
    #[error("enabled attestation quote kind {0} has no immutable trust data")]
    MissingKindTrustData(String),
    #[error("enabled attestation quote kind {0} has no trust anchor or verification key")]
    MissingKindTrustAnchor(String),
    #[error("attestation quote kind {kind} references unknown trust data {trust_data}")]
    UnknownTrustData { kind: String, trust_data: String },
    #[error("attestation quote kind {kind} measurement {trust_data} is not required trust data")]
    MeasurementNotRequired { kind: String, trust_data: String },
    #[error("attestation quote kind {kind} measurement {trust_data} is not measurement data")]
    InvalidMeasurementKind { kind: String, trust_data: String },
    #[error("attestation quote kind {kind} does not classify measurement data {trust_data}")]
    UnclassifiedMeasurement { kind: String, trust_data: String },
    #[error(
        "enabled attestation quote kind {0} requires measurements but has no measurement data"
    )]
    MissingMeasurementTrustData(String),
    #[error("enabled TPM quote policy has no immutable PCR policy")]
    MissingTpmPcrPolicy,
    #[error("enabled Tier-3 quote kind {0} has no admin-approved platform")]
    MissingTier3Platform(String),
    #[error(
        "enabled Tier-3 quote kind {kind} requires exact measurement layers {expected:?}, got {actual:?}"
    )]
    InvalidTier3MeasurementLayers {
        kind: String,
        expected: BTreeSet<AttestationMeasurementLayer>,
        actual: BTreeSet<AttestationMeasurementLayer>,
    },
    #[error("attestation policy contains unreferenced trust data {0}")]
    UnreferencedTrustData(String),
    #[error("attestation policy contains unreferenced origin pin {0}")]
    UnreferencedOrigin(String),
    #[error("first attestation policy must have sequence 1 and no previous digest")]
    InvalidGenesis,
    #[error("attestation policy sequence {actual} does not follow {previous}")]
    NonConsecutiveSequence { previous: u64, actual: u64 },
    #[error("attestation policy previous digest does not match the chain head")]
    PreviousDigestMismatch,
    #[error("attestation policy issued_epoch regresses from {previous} to {actual}")]
    IssuedEpochRegression { previous: u64, actual: u64 },
    #[error("attestation policy effective_epoch regresses from {previous} to {actual}")]
    EffectiveEpochRegression { previous: u64, actual: u64 },
    #[error("attestation policy chain must not be present but empty")]
    EmptyChain,
    #[error("enclave attestation bindings require an attestation policy chain")]
    BindingWithoutPolicy,
    #[error("enclave attestation bindings are not in canonical order")]
    NonCanonicalBindingOrder,
    #[error(
        "duplicate enclave attestation binding for {enclave_id}, {kind}, platform {platform:?}"
    )]
    DuplicateEnclaveBinding {
        enclave_id: String,
        kind: String,
        platform: Option<String>,
    },
    #[error("enclave attestation binding enclave_id must be a lowercase 32-byte digest")]
    InvalidEnclaveId,
    #[error(
        "enclave attestation binding for {enclave_id} uses quote kind {kind} that is never enabled"
    )]
    BindingKindNeverEnabled { enclave_id: String, kind: String },
    #[error(
        "enclave attestation binding for {enclave_id} has platform {platform:?}, which is not allowed for {kind} in policy {sequence}"
    )]
    BindingPlatformMismatch {
        enclave_id: String,
        kind: String,
        platform: Option<String>,
        sequence: u64,
    },
    #[error(
        "enclave attestation binding for {enclave_id}/{kind} requires exact measurement layers {expected:?}, got {actual:?}"
    )]
    BindingMeasurementSetMismatch {
        enclave_id: String,
        kind: String,
        expected: BTreeSet<AttestationMeasurementLayer>,
        actual: BTreeSet<AttestationMeasurementLayer>,
    },
    #[error(
        "enclave attestation binding for {enclave_id}/{kind} uses measurement reference {trust_data} more than once"
    )]
    DuplicateBindingMeasurementReference {
        enclave_id: String,
        kind: String,
        trust_data: String,
    },
    #[error(
        "enclave attestation binding for {enclave_id}/{kind} references unknown trust data {trust_data} in policy {sequence}"
    )]
    UnknownBindingTrustData {
        enclave_id: String,
        kind: String,
        trust_data: String,
        sequence: u64,
    },
    #[error(
        "enclave attestation binding for {enclave_id}/{kind} reference {trust_data} is not an approved measurement in policy {sequence}"
    )]
    BindingTrustDataMismatch {
        enclave_id: String,
        kind: String,
        trust_data: String,
        sequence: u64,
    },
    #[error("enabled quote kind {0} has no exact enclave attestation binding")]
    MissingEnclaveBinding(String),
    #[error("enclave attestation bindings have already been configured")]
    BindingsAlreadyConfigured,
}

#[derive(Clone, Debug)]
pub struct ValidatedAttestationPolicy {
    policy: AdminAttestationPolicy,
    digest: String,
    canonical_json: Vec<u8>,
    origins: BTreeMap<String, AttestationOriginPin>,
    trust_data: BTreeMap<String, AttestationTrustDataRef>,
    quote_kinds: BTreeMap<HardwareQuoteKind, AttestationQuoteKindPolicy>,
}

impl ValidatedAttestationPolicy {
    pub const MANAGED_VERIFIER_EXECUTABLE_MEDIA_TYPE: &'static str =
        "application/vnd.mayhem.attestation-verifier";
    pub const MANAGED_VERIFIER_MANIFEST_MEDIA_TYPE: &'static str =
        "application/vnd.mayhem.attestation-verifier-manifest+json";
    pub const MANAGED_VERIFIER_TARGET_AARCH64_APPLE_DARWIN: &'static str = "aarch64-apple-darwin";
    pub const MANAGED_VERIFIER_TARGET_AARCH64_PC_WINDOWS_MSVC: &'static str =
        "aarch64-pc-windows-msvc";
    pub const MANAGED_VERIFIER_TARGET_AARCH64_UNKNOWN_LINUX_GNU: &'static str =
        "aarch64-unknown-linux-gnu";
    pub const MANAGED_VERIFIER_TARGET_X86_64_APPLE_DARWIN: &'static str = "x86_64-apple-darwin";
    pub const MANAGED_VERIFIER_TARGET_X86_64_PC_WINDOWS_MSVC: &'static str =
        "x86_64-pc-windows-msvc";
    pub const MANAGED_VERIFIER_TARGET_X86_64_UNKNOWN_LINUX_GNU: &'static str =
        "x86_64-unknown-linux-gnu";
    pub const MANAGED_VERIFIER_TARGETS: [&'static str; 6] = [
        Self::MANAGED_VERIFIER_TARGET_AARCH64_APPLE_DARWIN,
        Self::MANAGED_VERIFIER_TARGET_AARCH64_PC_WINDOWS_MSVC,
        Self::MANAGED_VERIFIER_TARGET_AARCH64_UNKNOWN_LINUX_GNU,
        Self::MANAGED_VERIFIER_TARGET_X86_64_APPLE_DARWIN,
        Self::MANAGED_VERIFIER_TARGET_X86_64_PC_WINDOWS_MSVC,
        Self::MANAGED_VERIFIER_TARGET_X86_64_UNKNOWN_LINUX_GNU,
    ];

    pub fn is_managed_verifier_kind(kind: HardwareQuoteKind) -> bool {
        matches!(
            kind,
            HardwareQuoteKind::AmdSevSnpVcek
                | HardwareQuoteKind::IntelTdxDcap
                | HardwareQuoteKind::NvidiaNrasJwt
                | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
        )
    }

    pub fn managed_verifier_trust_data_ids(
        kind: HardwareQuoteKind,
        target: &str,
    ) -> Result<(String, String), String> {
        if !Self::is_managed_verifier_kind(kind) {
            return Err(format!(
                "{} is not a managed verifier quote kind",
                kind.as_str()
            ));
        }
        if !Self::MANAGED_VERIFIER_TARGETS.contains(&target) {
            return Err(format!("managed verifier target {target} is unsupported"));
        }
        Ok((
            managed_verifier_trust_data_id(kind, target, "executable"),
            managed_verifier_trust_data_id(kind, target, "manifest"),
        ))
    }

    pub fn parse_managed_verifier_trust_data(
        reference: &AttestationTrustDataRef,
    ) -> Result<Option<(HardwareQuoteKind, &'static str, &'static str)>, PolicyError> {
        let candidate = reference.id.starts_with("managed-verifier")
            || reference
                .media_type
                .starts_with("application/vnd.mayhem.attestation-verifier");
        if !candidate {
            return Ok(None);
        }

        let invalid = |reason: &str| PolicyError::InvalidManagedVerifierTrustData {
            id: reference.id.clone(),
            reason: reason.to_owned(),
        };
        let mut parts = reference.id.split('.');
        if parts.next() != Some("managed-verifier") {
            return Err(invalid(
                "managed verifier media requires a canonical managed-verifier id",
            ));
        }
        let kind_name = parts
            .next()
            .ok_or_else(|| invalid("id is missing its quote kind"))?;
        let kind = HardwareQuoteKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == kind_name)
            .filter(|kind| Self::is_managed_verifier_kind(*kind))
            .ok_or_else(|| invalid("id has an unsupported managed quote kind"))?;
        let target_name = parts
            .next()
            .ok_or_else(|| invalid("id is missing its release target"))?;
        let target = Self::MANAGED_VERIFIER_TARGETS
            .into_iter()
            .find(|target| *target == target_name)
            .ok_or_else(|| invalid("id must canonically bind a supported release target"))?;
        let role = parts
            .next()
            .ok_or_else(|| invalid("id is missing its collateral role"))?;
        if parts.next().is_some() {
            return Err(invalid("id has unexpected trailing components"));
        }
        let (role, expected_media_type) = match role {
            "executable" => ("executable", Self::MANAGED_VERIFIER_EXECUTABLE_MEDIA_TYPE),
            "manifest" => ("manifest", Self::MANAGED_VERIFIER_MANIFEST_MEDIA_TYPE),
            _ => return Err(invalid("id has an unsupported collateral role")),
        };
        if reference.kind != AttestationTrustDataKind::VerificationKey {
            return Err(invalid("must be verification-key trust data"));
        }
        if reference.media_type != expected_media_type {
            return Err(invalid(&format!(
                "must use media type {expected_media_type}"
            )));
        }
        Ok(Some((kind, target, role)))
    }

    pub fn compiled_managed_verifier_target() -> Result<&'static str, String> {
        #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
        {
            return Ok(Self::MANAGED_VERIFIER_TARGET_AARCH64_APPLE_DARWIN);
        }
        #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
        {
            return Ok(Self::MANAGED_VERIFIER_TARGET_X86_64_APPLE_DARWIN);
        }
        #[cfg(all(target_arch = "aarch64", target_os = "windows", target_env = "msvc"))]
        {
            return Ok(Self::MANAGED_VERIFIER_TARGET_AARCH64_PC_WINDOWS_MSVC);
        }
        #[cfg(all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"))]
        {
            return Ok(Self::MANAGED_VERIFIER_TARGET_X86_64_PC_WINDOWS_MSVC);
        }
        #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
        {
            return Ok(Self::MANAGED_VERIFIER_TARGET_AARCH64_UNKNOWN_LINUX_GNU);
        }
        #[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
        {
            return Ok(Self::MANAGED_VERIFIER_TARGET_X86_64_UNKNOWN_LINUX_GNU);
        }
        #[allow(unreachable_code)]
        Err(format!(
            "managed verifier target {}-{} is unsupported",
            std::env::consts::ARCH,
            std::env::consts::OS
        ))
    }

    /// Parse and structurally validate one policy record.
    ///
    /// Record authorship remains the caller's ledger trust-boundary check.
    pub fn parse_json(bytes: &[u8]) -> Result<Self, PolicyError> {
        if bytes.is_empty() {
            return Err(PolicyError::Empty);
        }
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(PolicyError::TooLarge {
                actual: bytes.len(),
                maximum: MAX_POLICY_BYTES,
            });
        }
        let policy = serde_json::from_slice::<AdminAttestationPolicy>(bytes)
            .map_err(|err| PolicyError::InvalidJson(err.to_string()))?;
        Self::validate(policy)
    }

    pub fn validate(mut policy: AdminAttestationPolicy) -> Result<Self, PolicyError> {
        policy
            .origin_pins
            .sort_by(|left, right| left.id.cmp(&right.id));
        policy
            .trust_data
            .sort_by(|left, right| left.id.cmp(&right.id));
        policy.quote_kinds.sort_by_key(|entry| entry.kind);
        validate_policy_header(&policy)?;
        let origins = validate_origins(&policy.origin_pins)?;
        let trust_data = validate_trust_data(&policy.trust_data, &origins)?;
        let quote_kinds = validate_quote_kinds(&policy.quote_kinds, &trust_data)?;
        let managed_verifier_trust_data =
            validate_managed_verifier_trust_data(&quote_kinds, &trust_data)?;
        validate_reference_closure(
            &origins,
            &trust_data,
            &quote_kinds,
            &managed_verifier_trust_data,
        )?;

        let canonical_json =
            serde_json::to_vec(&policy).map_err(|err| PolicyError::InvalidJson(err.to_string()))?;
        let digest = hex::encode(Sha256::digest(&canonical_json));
        Ok(Self {
            policy,
            digest,
            canonical_json,
            origins,
            trust_data,
            quote_kinds,
        })
    }

    pub fn policy(&self) -> &AdminAttestationPolicy {
        &self.policy
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn canonical_json(&self) -> &[u8] {
        &self.canonical_json
    }

    pub fn origin_pin(&self, id: &str) -> Option<&AttestationOriginPin> {
        self.origins.get(id)
    }

    pub fn trust_data(&self, id: &str) -> Option<&AttestationTrustDataRef> {
        self.trust_data.get(id)
    }

    pub fn quote_kind(&self, kind: HardwareQuoteKind) -> Option<&AttestationQuoteKindPolicy> {
        self.quote_kinds.get(&kind)
    }

    pub fn policy_source_url(&self, trust_data_id: &str) -> Option<String> {
        let reference = self.trust_data(trust_data_id)?;
        let source = reference.source.as_ref()?;
        let origin = self.origin_pin(&source.origin_pin)?;
        Some(format!("{}{}", origin.https_origin, source.path))
    }
}

#[derive(Clone, Debug, Default)]
pub struct AttestationPolicyChain {
    policies: Vec<ValidatedAttestationPolicy>,
    enclave_bindings:
        BTreeMap<(String, HardwareQuoteKind, Option<String>), AdminEnclaveAttestationBinding>,
    bindings_configured: bool,
}

impl AttestationPolicyChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_genesis_json(bytes: &[u8]) -> Result<Self, PolicyError> {
        let mut chain = Self::new();
        chain.append_json(bytes)?;
        Ok(chain)
    }

    /// Build authority from fields already authenticated by the signed catalog.
    ///
    /// `None` is an explicit Tier-1-only authority. It never enables hardware
    /// quote compatibility behavior.
    pub fn from_catalog_records(
        policies: Option<Vec<AdminAttestationPolicy>>,
        enclave_bindings: Vec<AdminEnclaveAttestationBinding>,
    ) -> Result<Self, PolicyError> {
        let Some(policies) = policies else {
            if !enclave_bindings.is_empty() {
                return Err(PolicyError::BindingWithoutPolicy);
            }
            return Ok(Self {
                bindings_configured: true,
                ..Self::default()
            });
        };
        if policies.is_empty() {
            return Err(PolicyError::EmptyChain);
        }

        let mut chain = Self::new();
        for policy in policies {
            chain.append(ValidatedAttestationPolicy::validate(policy)?)?;
        }
        chain.configure_enclave_bindings(enclave_bindings)?;
        Ok(chain)
    }

    pub fn append_json(
        &mut self,
        bytes: &[u8],
    ) -> Result<&ValidatedAttestationPolicy, PolicyError> {
        let policy = ValidatedAttestationPolicy::parse_json(bytes)?;
        self.append(policy)
    }

    pub fn append(
        &mut self,
        policy: ValidatedAttestationPolicy,
    ) -> Result<&ValidatedAttestationPolicy, PolicyError> {
        match self.policies.last() {
            None => {
                if policy.policy.sequence != 1 || policy.policy.previous_policy_digest.is_some() {
                    return Err(PolicyError::InvalidGenesis);
                }
            }
            Some(previous) => {
                let expected_sequence = previous.policy.sequence.checked_add(1);
                if Some(policy.policy.sequence) != expected_sequence {
                    return Err(PolicyError::NonConsecutiveSequence {
                        previous: previous.policy.sequence,
                        actual: policy.policy.sequence,
                    });
                }
                if policy.policy.previous_policy_digest.as_deref() != Some(previous.digest.as_str())
                {
                    return Err(PolicyError::PreviousDigestMismatch);
                }
                if policy.policy.issued_epoch < previous.policy.issued_epoch {
                    return Err(PolicyError::IssuedEpochRegression {
                        previous: previous.policy.issued_epoch,
                        actual: policy.policy.issued_epoch,
                    });
                }
                if policy.policy.effective_epoch < previous.policy.effective_epoch {
                    return Err(PolicyError::EffectiveEpochRegression {
                        previous: previous.policy.effective_epoch,
                        actual: policy.policy.effective_epoch,
                    });
                }
            }
        }
        if self.bindings_configured {
            let mut policies = self.policies.clone();
            policies.push(policy.clone());
            validate_enclave_bindings(
                &policies,
                self.enclave_bindings.values().cloned().collect(),
            )?;
        }
        self.policies.push(policy);
        Ok(self.policies.last().expect("just pushed policy"))
    }

    pub fn configure_enclave_bindings(
        &mut self,
        bindings: Vec<AdminEnclaveAttestationBinding>,
    ) -> Result<(), PolicyError> {
        if self.bindings_configured {
            return Err(PolicyError::BindingsAlreadyConfigured);
        }
        if self.policies.is_empty() && !bindings.is_empty() {
            return Err(PolicyError::BindingWithoutPolicy);
        }
        self.enclave_bindings = validate_enclave_bindings(&self.policies, bindings)?;
        self.bindings_configured = true;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.policies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    pub fn head(&self) -> Option<&ValidatedAttestationPolicy> {
        self.policies.last()
    }

    /// Select policy only from the caller-supplied canonical ledger epoch.
    pub fn active_at(&self, ledger_epoch: u64) -> Option<&ValidatedAttestationPolicy> {
        self.policies
            .iter()
            .rev()
            .find(|policy| policy.policy.effective_epoch <= ledger_epoch)
    }

    pub fn next_effective_after(&self, ledger_epoch: u64) -> Option<u64> {
        self.policies
            .iter()
            .map(|policy| policy.policy.effective_epoch)
            .find(|effective| *effective > ledger_epoch)
    }

    pub fn emergency_disabled_at(&self, ledger_epoch: u64) -> BTreeSet<HardwareQuoteKind> {
        let active_index = self
            .policies
            .iter()
            .rposition(|policy| policy.policy.effective_epoch <= ledger_epoch);
        let mut disabled = active_index
            .map(|index| {
                self.policies[index]
                    .policy
                    .emergency_disabled_quote_kinds
                    .clone()
            })
            .unwrap_or_default();
        let pending_start = active_index.map_or(0, |index| index + 1);
        for policy in self.policies.iter().skip(pending_start) {
            if policy.policy.issued_epoch <= ledger_epoch {
                disabled.extend(policy.policy.emergency_disabled_quote_kinds.iter().copied());
            }
        }
        disabled
    }

    pub fn enclave_binding(
        &self,
        enclave_id: &str,
        kind: HardwareQuoteKind,
        platform: Option<&str>,
    ) -> Option<&AdminEnclaveAttestationBinding> {
        self.enclave_bindings
            .get(&(enclave_id.to_owned(), kind, platform.map(str::to_owned)))
    }

    pub fn enclave_bindings(&self) -> impl Iterator<Item = &AdminEnclaveAttestationBinding> {
        self.enclave_bindings.values()
    }
}

fn validate_policy_header(policy: &AdminAttestationPolicy) -> Result<(), PolicyError> {
    if policy.schema_version != ATTESTATION_POLICY_SCHEMA_VERSION {
        return Err(PolicyError::UnsupportedSchema {
            expected: ATTESTATION_POLICY_SCHEMA_VERSION,
            actual: policy.schema_version,
        });
    }
    if policy.sequence == 0 {
        return Err(PolicyError::InvalidSequence);
    }
    if policy.min_verifier_version == 0 {
        return Err(PolicyError::InvalidVerifierVersion);
    }
    if policy.effective_epoch < policy.issued_epoch {
        return Err(PolicyError::EffectiveBeforeIssue);
    }
    if policy
        .expires_epoch
        .is_some_and(|expires| expires <= policy.effective_epoch)
    {
        return Err(PolicyError::InvalidExpiry);
    }
    if let Some(previous) = &policy.previous_policy_digest {
        validate_sha256("previous_policy_digest", previous)?;
    }
    Ok(())
}

fn validate_origins(
    entries: &[AttestationOriginPin],
) -> Result<BTreeMap<String, AttestationOriginPin>, PolicyError> {
    let mut origins = BTreeMap::new();
    for entry in entries {
        validate_identifier("origin_pins.id", &entry.id)?;
        let parsed = Url::parse(&entry.https_origin).map_err(|err| PolicyError::InvalidOrigin {
            id: entry.id.clone(),
            reason: err.to_string(),
        })?;
        let canonical_origin = parsed.origin().ascii_serialization();
        let valid = parsed.scheme() == "https"
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.path() == "/"
            && entry.https_origin == canonical_origin;
        if !valid {
            return Err(PolicyError::InvalidOrigin {
                id: entry.id.clone(),
                reason:
                    "must be a canonical HTTPS origin without credentials, path, query, or fragment"
                        .to_owned(),
            });
        }
        if origins.insert(entry.id.clone(), entry.clone()).is_some() {
            return Err(PolicyError::DuplicateOrigin(entry.id.clone()));
        }
    }
    Ok(origins)
}

fn validate_trust_data(
    entries: &[AttestationTrustDataRef],
    origins: &BTreeMap<String, AttestationOriginPin>,
) -> Result<BTreeMap<String, AttestationTrustDataRef>, PolicyError> {
    let mut trust_data = BTreeMap::new();
    for entry in entries {
        validate_identifier("trust_data.id", &entry.id)?;
        validate_sha256(&format!("trust_data.{}.sha256", entry.id), &entry.sha256)?;
        if entry.media_type.is_empty()
            || entry.media_type.len() > 128
            || !entry.media_type.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(PolicyError::InvalidTrustData {
                id: entry.id.clone(),
                reason: "media_type must be 1-128 visible ASCII bytes".to_owned(),
            });
        }
        if entry.max_bytes == 0 || entry.max_bytes > MAX_COLLATERAL_BYTES as u64 {
            return Err(PolicyError::InvalidTrustData {
                id: entry.id.clone(),
                reason: format!("max_bytes must be between 1 and {MAX_COLLATERAL_BYTES}"),
            });
        }
        if matches!(
            (entry.valid_from_epoch, entry.valid_until_epoch),
            (Some(from), Some(until)) if until <= from
        ) {
            return Err(PolicyError::InvalidTrustData {
                id: entry.id.clone(),
                reason: "valid_until_epoch must be later than valid_from_epoch".to_owned(),
            });
        }
        if let Some(source) = &entry.source {
            validate_identifier("trust_data.source.origin_pin", &source.origin_pin)?;
            if !origins.contains_key(&source.origin_pin) {
                return Err(PolicyError::UnknownOrigin {
                    trust_data: entry.id.clone(),
                    origin: source.origin_pin.clone(),
                });
            }
            if !valid_policy_path(&source.path) {
                return Err(PolicyError::InvalidTrustData {
                    id: entry.id.clone(),
                    reason: "source path must be an absolute canonical path without query, fragment, backslash, empty segment, or traversal"
                        .to_owned(),
                });
            }
        }
        if trust_data.insert(entry.id.clone(), entry.clone()).is_some() {
            return Err(PolicyError::DuplicateTrustData(entry.id.clone()));
        }
    }
    Ok(trust_data)
}

fn validate_quote_kinds(
    entries: &[AttestationQuoteKindPolicy],
    trust_data: &BTreeMap<String, AttestationTrustDataRef>,
) -> Result<BTreeMap<HardwareQuoteKind, AttestationQuoteKindPolicy>, PolicyError> {
    let mut quote_kinds = BTreeMap::new();
    for entry in entries {
        if entry.verifier_profile.quote_kind() != entry.kind {
            return Err(PolicyError::ProfileKindMismatch {
                kind: entry.kind.as_str().to_owned(),
                profile: format!("{:?}", entry.verifier_profile),
            });
        }
        if entry.enabled && entry.evidence_schema_version == 0 {
            return Err(PolicyError::InvalidEvidenceSchemaVersion(
                entry.kind.as_str().to_owned(),
            ));
        }
        if entry.enabled && entry.required_trust_data.is_empty() {
            return Err(PolicyError::MissingKindTrustData(
                entry.kind.as_str().to_owned(),
            ));
        }
        for trust_data_id in &entry.required_trust_data {
            if !trust_data.contains_key(trust_data_id) {
                return Err(PolicyError::UnknownTrustData {
                    kind: entry.kind.as_str().to_owned(),
                    trust_data: trust_data_id.clone(),
                });
            }
        }
        if entry.enabled
            && !entry.required_trust_data.iter().any(|id| {
                trust_data.get(id).is_some_and(|reference| {
                    matches!(
                        reference.kind,
                        AttestationTrustDataKind::TrustAnchor
                            | AttestationTrustDataKind::VerificationKey
                    )
                })
            })
        {
            return Err(PolicyError::MissingKindTrustAnchor(
                entry.kind.as_str().to_owned(),
            ));
        }
        for measurement_id in &entry.measurement_trust_data {
            if !entry.required_trust_data.contains(measurement_id) {
                return Err(PolicyError::MeasurementNotRequired {
                    kind: entry.kind.as_str().to_owned(),
                    trust_data: measurement_id.clone(),
                });
            }
            let reference = trust_data
                .get(measurement_id)
                .expect("required trust data was validated above");
            if reference.kind != AttestationTrustDataKind::Measurement {
                return Err(PolicyError::InvalidMeasurementKind {
                    kind: entry.kind.as_str().to_owned(),
                    trust_data: measurement_id.clone(),
                });
            }
        }
        for required_id in &entry.required_trust_data {
            if trust_data
                .get(required_id)
                .is_some_and(|reference| reference.kind == AttestationTrustDataKind::Measurement)
                && !entry.measurement_trust_data.contains(required_id)
            {
                return Err(PolicyError::UnclassifiedMeasurement {
                    kind: entry.kind.as_str().to_owned(),
                    trust_data: required_id.clone(),
                });
            }
        }
        if entry.enabled
            && !entry.required_measurement_layers.is_empty()
            && entry.measurement_trust_data.is_empty()
        {
            return Err(PolicyError::MissingMeasurementTrustData(
                entry.kind.as_str().to_owned(),
            ));
        }
        if entry.enabled
            && entry.kind == HardwareQuoteKind::Tpm2QuoteEk
            && entry.measurement_trust_data.is_empty()
        {
            return Err(PolicyError::MissingTpmPcrPolicy);
        }
        for platform in &entry.platforms {
            validate_identifier(
                &format!("quote_kinds.{}.platforms", entry.kind.as_str()),
                platform,
            )?;
        }
        if entry.enabled && entry.kind.attestation_tier() >= TIER3_CONFIDENTIAL_COMPUTE_TIER {
            if entry.platforms.is_empty() {
                return Err(PolicyError::MissingTier3Platform(
                    entry.kind.as_str().to_owned(),
                ));
            }
            let expected = exact_tier3_measurement_layers(entry.kind)
                .expect("Tier-3 quote kinds have an exact measurement matrix");
            if entry.required_measurement_layers != expected {
                return Err(PolicyError::InvalidTier3MeasurementLayers {
                    kind: entry.kind.as_str().to_owned(),
                    expected,
                    actual: entry.required_measurement_layers.clone(),
                });
            }
        }
        if quote_kinds.insert(entry.kind, entry.clone()).is_some() {
            return Err(PolicyError::DuplicateQuoteKind(
                entry.kind.as_str().to_owned(),
            ));
        }
    }
    for kind in HardwareQuoteKind::ALL {
        if !quote_kinds.contains_key(&kind) {
            return Err(PolicyError::MissingQuoteKind(kind.as_str().to_owned()));
        }
    }
    Ok(quote_kinds)
}

fn validate_enclave_bindings(
    policies: &[ValidatedAttestationPolicy],
    bindings: Vec<AdminEnclaveAttestationBinding>,
) -> Result<
    BTreeMap<(String, HardwareQuoteKind, Option<String>), AdminEnclaveAttestationBinding>,
    PolicyError,
> {
    let mut validated = BTreeMap::new();
    let mut previous_key: Option<(String, HardwareQuoteKind, Option<String>)> = None;

    for binding in bindings {
        if !valid_sha256(&binding.enclave_id) {
            return Err(PolicyError::InvalidEnclaveId);
        }
        if let Some(platform) = &binding.platform {
            validate_identifier("enclave_attestation_bindings.platform", platform)?;
        }
        let key = (
            binding.enclave_id.clone(),
            binding.kind,
            binding.platform.clone(),
        );
        if let Some(previous) = &previous_key {
            if key == *previous {
                return Err(PolicyError::DuplicateEnclaveBinding {
                    enclave_id: binding.enclave_id.clone(),
                    kind: binding.kind.as_str().to_owned(),
                    platform: binding.platform.clone(),
                });
            }
            if key < *previous {
                return Err(PolicyError::NonCanonicalBindingOrder);
            }
        }
        previous_key = Some(key.clone());

        let actual_layers = binding
            .measurement_trust_data
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut unique_references = BTreeSet::new();
        for trust_data in binding.measurement_trust_data.values() {
            if !unique_references.insert(trust_data.as_str()) {
                return Err(PolicyError::DuplicateBindingMeasurementReference {
                    enclave_id: binding.enclave_id.clone(),
                    kind: binding.kind.as_str().to_owned(),
                    trust_data: trust_data.clone(),
                });
            }
        }

        let mut saw_enabled_policy = false;
        for policy in policies {
            let kind_policy = policy
                .quote_kind(binding.kind)
                .expect("validated policy contains every quote kind");
            if !kind_policy.enabled {
                continue;
            }
            saw_enabled_policy = true;
            let platform_allowed = match binding.platform.as_deref() {
                None => kind_policy.platforms.is_empty(),
                Some(platform) => kind_policy.platforms.contains(platform),
            };
            if !platform_allowed {
                return Err(PolicyError::BindingPlatformMismatch {
                    enclave_id: binding.enclave_id.clone(),
                    kind: binding.kind.as_str().to_owned(),
                    platform: binding.platform.clone(),
                    sequence: policy.policy.sequence,
                });
            }
            if actual_layers != kind_policy.required_measurement_layers {
                return Err(PolicyError::BindingMeasurementSetMismatch {
                    enclave_id: binding.enclave_id.clone(),
                    kind: binding.kind.as_str().to_owned(),
                    expected: kind_policy.required_measurement_layers.clone(),
                    actual: actual_layers.clone(),
                });
            }
            for trust_data_id in binding.measurement_trust_data.values() {
                let Some(reference) = policy.trust_data(trust_data_id) else {
                    return Err(PolicyError::UnknownBindingTrustData {
                        enclave_id: binding.enclave_id.clone(),
                        kind: binding.kind.as_str().to_owned(),
                        trust_data: trust_data_id.clone(),
                        sequence: policy.policy.sequence,
                    });
                };
                if reference.kind != AttestationTrustDataKind::Measurement
                    || !kind_policy.required_trust_data.contains(trust_data_id)
                    || !kind_policy.measurement_trust_data.contains(trust_data_id)
                {
                    return Err(PolicyError::BindingTrustDataMismatch {
                        enclave_id: binding.enclave_id.clone(),
                        kind: binding.kind.as_str().to_owned(),
                        trust_data: trust_data_id.clone(),
                        sequence: policy.policy.sequence,
                    });
                }
            }
        }
        if !saw_enabled_policy {
            return Err(PolicyError::BindingKindNeverEnabled {
                enclave_id: binding.enclave_id.clone(),
                kind: binding.kind.as_str().to_owned(),
            });
        }
        validated.insert(key, binding);
    }

    for kind in HardwareQuoteKind::ALL {
        let enabled = policies.iter().any(|policy| {
            policy
                .quote_kind(kind)
                .is_some_and(|kind_policy| kind_policy.enabled)
        });
        if enabled && !validated.keys().any(|(_, candidate, _)| *candidate == kind) {
            return Err(PolicyError::MissingEnclaveBinding(kind.as_str().to_owned()));
        }
    }
    Ok(validated)
}

fn exact_tier3_measurement_layers(
    kind: HardwareQuoteKind,
) -> Option<BTreeSet<AttestationMeasurementLayer>> {
    match kind {
        HardwareQuoteKind::AmdSevSnpVcek | HardwareQuoteKind::IntelTdxDcap => {
            Some(BTreeSet::from([
                AttestationMeasurementLayer::Cpu,
                AttestationMeasurementLayer::Workload,
            ]))
        }
        HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
            Some(BTreeSet::from([
                AttestationMeasurementLayer::Cpu,
                AttestationMeasurementLayer::Gpu,
                AttestationMeasurementLayer::Workload,
            ]))
        }
        HardwareQuoteKind::AppleAppAttestJwt
        | HardwareQuoteKind::NvidiaGb10DeviceJwt
        | HardwareQuoteKind::Tpm2QuoteEk => None,
    }
}

fn managed_verifier_trust_data_id(
    kind: HardwareQuoteKind,
    target: &str,
    collateral: &str,
) -> String {
    format!(
        "managed-verifier.{}.{}.{}",
        kind.as_str(),
        target,
        collateral
    )
}

fn validate_managed_verifier_trust_data(
    quote_kinds: &BTreeMap<HardwareQuoteKind, AttestationQuoteKindPolicy>,
    trust_data: &BTreeMap<String, AttestationTrustDataRef>,
) -> Result<BTreeSet<String>, PolicyError> {
    let mut managed_ids = BTreeSet::new();
    for reference in trust_data.values() {
        if ValidatedAttestationPolicy::parse_managed_verifier_trust_data(reference)?.is_some() {
            managed_ids.insert(reference.id.clone());
        }
    }
    for kind in HardwareQuoteKind::ALL
        .into_iter()
        .filter(|kind| ValidatedAttestationPolicy::is_managed_verifier_kind(*kind))
    {
        let kind_policy = quote_kinds
            .get(&kind)
            .expect("validated policy contains every quote kind");
        for target in ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS {
            let executable_id = managed_verifier_trust_data_id(kind, target, "executable");
            let manifest_id = managed_verifier_trust_data_id(kind, target, "manifest");
            let executable = trust_data.get(&executable_id);
            let manifest = trust_data.get(&manifest_id);
            if executable.is_some() != manifest.is_some() {
                return Err(PolicyError::IncompleteManagedVerifierTarget {
                    kind: kind.as_str().to_owned(),
                    target: target.to_owned(),
                });
            }
            let (Some(executable), Some(manifest)) = (executable, manifest) else {
                continue;
            };
            if !kind_policy.enabled {
                return Err(PolicyError::InvalidManagedVerifierTrustData {
                    id: executable_id,
                    reason: format!("quote kind {} is disabled", kind.as_str()),
                });
            }
            for reference in [executable, manifest] {
                if quote_kinds
                    .values()
                    .any(|entry| entry.required_trust_data.contains(&reference.id))
                {
                    return Err(PolicyError::InvalidManagedVerifierTrustData {
                        id: reference.id.clone(),
                        reason: "target-specific collateral must not be globally required"
                            .to_owned(),
                    });
                }
            }
        }
    }
    Ok(managed_ids)
}

fn validate_reference_closure(
    origins: &BTreeMap<String, AttestationOriginPin>,
    trust_data: &BTreeMap<String, AttestationTrustDataRef>,
    quote_kinds: &BTreeMap<HardwareQuoteKind, AttestationQuoteKindPolicy>,
    managed_verifier_trust_data: &BTreeSet<String>,
) -> Result<(), PolicyError> {
    let mut referenced_trust_data = quote_kinds
        .values()
        .flat_map(|entry| entry.required_trust_data.iter().cloned())
        .collect::<BTreeSet<_>>();
    referenced_trust_data.extend(managed_verifier_trust_data.iter().cloned());
    if let Some(unreferenced) = trust_data
        .keys()
        .find(|id| !referenced_trust_data.contains(*id))
    {
        return Err(PolicyError::UnreferencedTrustData(unreferenced.clone()));
    }
    let referenced_origins = trust_data
        .values()
        .filter_map(|entry| entry.source.as_ref())
        .map(|source| source.origin_pin.clone())
        .collect::<BTreeSet<_>>();
    if let Some(unreferenced) = origins.keys().find(|id| !referenced_origins.contains(*id)) {
        return Err(PolicyError::UnreferencedOrigin(unreferenced.clone()));
    }
    Ok(())
}

fn validate_identifier(field: &str, value: &str) -> Result<(), PolicyError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        });
    if valid {
        Ok(())
    } else {
        Err(PolicyError::InvalidIdentifier {
            field: field.to_owned(),
        })
    }
}

fn validate_sha256(field: &str, value: &str) -> Result<(), PolicyError> {
    if valid_sha256(value) {
        Ok(())
    } else {
        Err(PolicyError::InvalidDigest {
            field: field.to_owned(),
        })
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_policy_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 2048
        && !path.contains('\\')
        && !path.contains('?')
        && !path.contains('#')
        && !path.contains('%')
        && path.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~')
        })
        && path
            .split('/')
            .skip(1)
            .all(|component| !component.is_empty() && component != "." && component != "..")
}
