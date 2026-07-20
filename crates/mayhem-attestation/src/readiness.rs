use std::collections::{BTreeMap, BTreeSet};

use mayhem_proto::{
    AdminEnclaveAttestationBinding, AttestationVerifierProfile, HardwareQuoteKind,
    HardwareQuoteRouteAdvertisement, HardwareQuoteRoutePolicyBinding,
};
use thiserror::Error;

use crate::{AttestationPolicyChain, CollateralInventory};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifierCapabilities {
    pub version: u32,
    pub profiles: BTreeMap<AttestationVerifierProfile, BTreeSet<u32>>,
}

impl VerifierCapabilities {
    pub fn new(
        version: u32,
        profiles: impl IntoIterator<Item = AttestationVerifierProfile>,
    ) -> Self {
        Self {
            version,
            profiles: profiles
                .into_iter()
                .map(|profile| (profile, BTreeSet::from([1])))
                .collect(),
        }
    }

    pub fn with_evidence_schemas(
        version: u32,
        profiles: impl IntoIterator<Item = (AttestationVerifierProfile, BTreeSet<u32>)>,
    ) -> Self {
        Self {
            version,
            profiles: profiles.into_iter().collect(),
        }
    }

    pub fn supports(&self, profile: AttestationVerifierProfile, schema_version: u32) -> bool {
        self.profiles
            .get(&profile)
            .is_some_and(|versions| versions.contains(&schema_version))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuoteKindReadinessStatus {
    Ready,
    NoActivePolicy {
        next_effective_epoch: Option<u64>,
    },
    PolicyExpired {
        expires_epoch: u64,
    },
    EmergencyDisabled,
    Disabled,
    VerifierTooOld {
        required: u32,
        actual: u32,
    },
    VerifierProfileUnavailable {
        profile: AttestationVerifierProfile,
    },
    EvidenceSchemaUnavailable {
        profile: AttestationVerifierProfile,
        required: u32,
    },
    MissingCollateral {
        trust_data: BTreeSet<String>,
    },
    CollateralNotYetValid {
        trust_data: BTreeSet<String>,
    },
    CollateralExpired {
        trust_data: BTreeSet<String>,
    },
}

impl QuoteKindReadinessStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuoteKindReadiness {
    pub kind: HardwareQuoteKind,
    pub status: QuoteKindReadinessStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttestationReadiness {
    pub evaluated_epoch: u64,
    pub policy_sequence: Option<u64>,
    pub policy_digest: Option<String>,
    pub quote_kinds: BTreeMap<HardwareQuoteKind, QuoteKindReadiness>,
    allowed_platforms: BTreeMap<HardwareQuoteKind, BTreeSet<String>>,
    evidence_schema_versions: BTreeMap<HardwareQuoteKind, u32>,
    enclave_bindings:
        BTreeMap<(String, HardwareQuoteKind, Option<String>), AdminEnclaveAttestationBinding>,
}

impl AttestationReadiness {
    pub fn evaluate(
        chain: &AttestationPolicyChain,
        collateral: &CollateralInventory,
        capabilities: &VerifierCapabilities,
        epoch: u64,
    ) -> Self {
        let Some(policy) = chain.active_at(epoch) else {
            let next_effective_epoch = chain.next_effective_after(epoch);
            return Self {
                evaluated_epoch: epoch,
                policy_sequence: None,
                policy_digest: None,
                quote_kinds: HardwareQuoteKind::ALL
                    .into_iter()
                    .map(|kind| {
                        (
                            kind,
                            QuoteKindReadiness {
                                kind,
                                status: QuoteKindReadinessStatus::NoActivePolicy {
                                    next_effective_epoch,
                                },
                            },
                        )
                    })
                    .collect(),
                allowed_platforms: BTreeMap::new(),
                evidence_schema_versions: BTreeMap::new(),
                enclave_bindings: BTreeMap::new(),
            };
        };

        let emergency_disabled = chain.emergency_disabled_at(epoch);
        let mut quote_kinds = BTreeMap::new();
        let mut allowed_platforms = BTreeMap::new();
        let mut evidence_schema_versions = BTreeMap::new();
        for kind in HardwareQuoteKind::ALL {
            let kind_policy = policy
                .quote_kind(kind)
                .expect("validated policy contains every quote kind");
            allowed_platforms.insert(kind, kind_policy.platforms.clone());
            evidence_schema_versions.insert(kind, kind_policy.evidence_schema_version);
            let status = if let Some(expires_epoch) = policy
                .policy()
                .expires_epoch
                .filter(|expires| epoch >= *expires)
            {
                QuoteKindReadinessStatus::PolicyExpired { expires_epoch }
            } else if emergency_disabled.contains(&kind) {
                QuoteKindReadinessStatus::EmergencyDisabled
            } else if !kind_policy.enabled {
                QuoteKindReadinessStatus::Disabled
            } else if capabilities.version < policy.policy().min_verifier_version {
                QuoteKindReadinessStatus::VerifierTooOld {
                    required: policy.policy().min_verifier_version,
                    actual: capabilities.version,
                }
            } else if !capabilities
                .profiles
                .contains_key(&kind_policy.verifier_profile)
            {
                QuoteKindReadinessStatus::VerifierProfileUnavailable {
                    profile: kind_policy.verifier_profile,
                }
            } else if !capabilities.supports(
                kind_policy.verifier_profile,
                kind_policy.evidence_schema_version,
            ) {
                QuoteKindReadinessStatus::EvidenceSchemaUnavailable {
                    profile: kind_policy.verifier_profile,
                    required: kind_policy.evidence_schema_version,
                }
            } else {
                collateral_status(policy, kind_policy, collateral, epoch)
            };
            quote_kinds.insert(kind, QuoteKindReadiness { kind, status });
        }
        Self {
            evaluated_epoch: epoch,
            policy_sequence: Some(policy.policy().sequence),
            policy_digest: Some(policy.digest().to_owned()),
            quote_kinds,
            allowed_platforms,
            evidence_schema_versions,
            enclave_bindings: chain
                .enclave_bindings()
                .map(|binding| {
                    (
                        (
                            binding.enclave_id.clone(),
                            binding.kind,
                            binding.platform.clone(),
                        ),
                        binding.clone(),
                    )
                })
                .collect(),
        }
    }

    pub fn quote_kind(&self, kind: HardwareQuoteKind) -> &QuoteKindReadiness {
        self.quote_kinds
            .get(&kind)
            .expect("readiness contains every quote kind")
    }

    pub fn bind_route(
        &self,
        enclave_id: &str,
        canonical_device_id: &str,
        route: &HardwareQuoteRouteAdvertisement,
    ) -> Result<HardwareQuoteRoutePolicyBinding, ReadinessError> {
        validate_route_identity("enclave_id", enclave_id)?;
        validate_route_identity("device_id", canonical_device_id)?;
        let readiness = self.quote_kind(route.kind);
        if !readiness.status.is_ready() {
            return Err(ReadinessError::QuoteKindNotReady {
                kind: route.kind,
                status: readiness.status.clone(),
            });
        }
        let policy_sequence = self.policy_sequence.ok_or(ReadinessError::NoActivePolicy)?;
        let policy_digest = self
            .policy_digest
            .clone()
            .ok_or(ReadinessError::NoActivePolicy)?;
        let allowed = self
            .allowed_platforms
            .get(&route.kind)
            .cloned()
            .unwrap_or_default();
        let platform = match (allowed.is_empty(), route.declared_platform.as_deref()) {
            (true, None) => None,
            (true, Some(platform)) => {
                return Err(ReadinessError::UnexpectedPlatform {
                    kind: route.kind,
                    platform: platform.to_owned(),
                })
            }
            (false, None) => return Err(ReadinessError::PlatformRequired { kind: route.kind }),
            (false, Some(platform)) if allowed.contains(platform) => Some(platform.to_owned()),
            (false, Some(platform)) => {
                return Err(ReadinessError::PlatformNotAllowed {
                    kind: route.kind,
                    platform: platform.to_owned(),
                })
            }
        };
        let binding_key = (enclave_id.to_owned(), route.kind, platform.clone());
        if !self.enclave_bindings.contains_key(&binding_key) {
            return Err(ReadinessError::UnknownEnclaveBinding {
                enclave_id: enclave_id.to_owned(),
                kind: route.kind,
                platform,
            });
        }
        Ok(HardwareQuoteRoutePolicyBinding {
            enclave_id: enclave_id.to_owned(),
            device_id: canonical_device_id.to_owned(),
            kind: route.kind,
            evidence_schema_version: self
                .quote_kind_policy_schema(route.kind)
                .ok_or(ReadinessError::NoActivePolicy)?,
            policy_sequence,
            policy_digest,
            platform,
        })
    }

    pub fn verify_binding(
        &self,
        binding: &HardwareQuoteRoutePolicyBinding,
    ) -> Result<(), ReadinessError> {
        validate_route_identity("enclave_id", &binding.enclave_id)?;
        validate_route_identity("device_id", &binding.device_id)?;
        let readiness = self.quote_kind(binding.kind);
        if !readiness.status.is_ready() {
            return Err(ReadinessError::QuoteKindNotReady {
                kind: binding.kind,
                status: readiness.status.clone(),
            });
        }
        let expected_sequence = self.policy_sequence.ok_or(ReadinessError::NoActivePolicy)?;
        let expected_digest = self
            .policy_digest
            .as_deref()
            .ok_or(ReadinessError::NoActivePolicy)?;
        let expected_schema = self
            .quote_kind_policy_schema(binding.kind)
            .ok_or(ReadinessError::NoActivePolicy)?;
        if binding.policy_sequence != expected_sequence
            || binding.policy_digest != expected_digest
            || binding.evidence_schema_version != expected_schema
        {
            return Err(ReadinessError::PolicyBindingMismatch { kind: binding.kind });
        }
        let allowed = self
            .allowed_platforms
            .get(&binding.kind)
            .cloned()
            .unwrap_or_default();
        match binding.platform.as_deref() {
            None if allowed.is_empty() => {}
            Some(platform) if allowed.contains(platform) => {}
            _ => return Err(ReadinessError::PolicyBindingMismatch { kind: binding.kind }),
        }
        if !self.enclave_bindings.contains_key(&(
            binding.enclave_id.clone(),
            binding.kind,
            binding.platform.clone(),
        )) {
            return Err(ReadinessError::UnknownEnclaveBinding {
                enclave_id: binding.enclave_id.clone(),
                kind: binding.kind,
                platform: binding.platform.clone(),
            });
        }
        Ok(())
    }

    pub fn enclave_binding(
        &self,
        binding: &HardwareQuoteRoutePolicyBinding,
    ) -> Result<&AdminEnclaveAttestationBinding, ReadinessError> {
        self.verify_binding(binding)?;
        self.enclave_bindings
            .get(&(
                binding.enclave_id.clone(),
                binding.kind,
                binding.platform.clone(),
            ))
            .ok_or_else(|| ReadinessError::UnknownEnclaveBinding {
                enclave_id: binding.enclave_id.clone(),
                kind: binding.kind,
                platform: binding.platform.clone(),
            })
    }

    fn quote_kind_policy_schema(&self, kind: HardwareQuoteKind) -> Option<u32> {
        self.evidence_schema_versions.get(&kind).copied()
    }
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ReadinessError {
    #[error("no active attestation policy")]
    NoActivePolicy,
    #[error("hardware quote kind {kind:?} is not ready: {status:?}")]
    QuoteKindNotReady {
        kind: HardwareQuoteKind,
        status: QuoteKindReadinessStatus,
    },
    #[error("hardware quote kind {kind:?} requires an admin-approved platform")]
    PlatformRequired { kind: HardwareQuoteKind },
    #[error("hardware quote kind {kind:?} does not accept platform {platform}")]
    PlatformNotAllowed {
        kind: HardwareQuoteKind,
        platform: String,
    },
    #[error("hardware quote kind {kind:?} has no platform policy but route declared {platform}")]
    UnexpectedPlatform {
        kind: HardwareQuoteKind,
        platform: String,
    },
    #[error(
        "no admin enclave attestation binding for {enclave_id}, {kind:?}, platform {platform:?}"
    )]
    UnknownEnclaveBinding {
        enclave_id: String,
        kind: HardwareQuoteKind,
        platform: Option<String>,
    },
    #[error("{field} must be a canonical lowercase 32-byte digest")]
    InvalidRouteIdentity { field: &'static str },
    #[error("hardware quote kind {kind:?} policy binding is stale or mismatched")]
    PolicyBindingMismatch { kind: HardwareQuoteKind },
}

fn validate_route_identity(field: &'static str, value: &str) -> Result<(), ReadinessError> {
    let valid = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if valid {
        Ok(())
    } else {
        Err(ReadinessError::InvalidRouteIdentity { field })
    }
}

fn collateral_status(
    policy: &crate::ValidatedAttestationPolicy,
    kind_policy: &mayhem_proto::AttestationQuoteKindPolicy,
    collateral: &CollateralInventory,
    epoch: u64,
) -> QuoteKindReadinessStatus {
    let mut missing = BTreeSet::new();
    let mut not_yet_valid = BTreeSet::new();
    let mut expired = BTreeSet::new();
    for id in &kind_policy.required_trust_data {
        let reference = policy
            .trust_data(id)
            .expect("validated kind policy references existing trust data");
        if !collateral.contains_reference_at(reference, epoch) {
            missing.insert(id.clone());
        } else if reference
            .valid_from_epoch
            .is_some_and(|valid_from| epoch < valid_from)
        {
            not_yet_valid.insert(id.clone());
        } else if reference
            .valid_until_epoch
            .is_some_and(|valid_until| epoch >= valid_until)
        {
            expired.insert(id.clone());
        }
    }
    if !missing.is_empty() {
        QuoteKindReadinessStatus::MissingCollateral {
            trust_data: missing,
        }
    } else if !not_yet_valid.is_empty() {
        QuoteKindReadinessStatus::CollateralNotYetValid {
            trust_data: not_yet_valid,
        }
    } else if !expired.is_empty() {
        QuoteKindReadinessStatus::CollateralExpired {
            trust_data: expired,
        }
    } else {
        QuoteKindReadinessStatus::Ready
    }
}
