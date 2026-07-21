use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mayhem_attestation::{
    collateral_cache_relative_path, AttestationPolicyChain, AttestationReadiness, CollateralError,
    CollateralInventory, EkCertificateError, PolicyError, QuoteKindReadinessStatus, ReadinessError,
    TpmQuoteError, TpmVerificationMaterials, ValidatedAttestationPolicy, VerifierCapabilities,
};
use mayhem_proto::{
    AdminAttestationPolicy, AdminEnclaveAttestationBinding, AttestationMeasurementLayer,
    AttestationOriginPin, AttestationQuoteKindPolicy, AttestationTrustDataKind,
    AttestationTrustDataRef, AttestationTrustDataSource, AttestationVerifierProfile,
    HardwareQuoteKind, HardwareQuoteRouteAdvertisement, ATTESTATION_POLICY_SCHEMA_VERSION,
    TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

const ROOT_B64: &str = include_str!("fixtures/tpm-root.der.b64");
const PCR_BYTES: &[u8] = br#"{"schema_version":2,"hash_algorithm":"sha256","pcrs":[0]}"#;

fn root_bytes() -> &'static [u8] {
    static ROOT: OnceLock<Vec<u8>> = OnceLock::new();
    ROOT.get_or_init(|| BASE64.decode(ROOT_B64.trim()).unwrap())
        .as_slice()
}

fn profile(kind: HardwareQuoteKind) -> AttestationVerifierProfile {
    match kind {
        HardwareQuoteKind::AppleAppAttestJwt => AttestationVerifierProfile::AppleAppAttestNativeV1,
        HardwareQuoteKind::AmdSevSnpVcek => AttestationVerifierProfile::AmdSevSnpVcekV1,
        HardwareQuoteKind::IntelTdxDcap => AttestationVerifierProfile::IntelTdxDcapV1,
        HardwareQuoteKind::NvidiaGb10DeviceJwt => AttestationVerifierProfile::NvidiaGb10DeviceV1,
        HardwareQuoteKind::NvidiaNrasJwt => AttestationVerifierProfile::NvidiaNrasCompositeV1,
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
            AttestationVerifierProfile::NvidiaNvtrustOfflineCompositeV1
        }
        HardwareQuoteKind::Tpm2QuoteEk => AttestationVerifierProfile::Tpm2EkActivateCredentialV1,
    }
}

fn policy(sequence: u64, previous: Option<String>) -> AdminAttestationPolicy {
    AdminAttestationPolicy {
        schema_version: ATTESTATION_POLICY_SCHEMA_VERSION,
        sequence,
        previous_policy_digest: previous,
        issued_epoch: 10 + sequence,
        effective_epoch: 12 + sequence,
        expires_epoch: None,
        min_verifier_version: 3,
        emergency_disabled_quote_kinds: BTreeSet::new(),
        origin_pins: vec![AttestationOriginPin {
            id: "tpm-roots-origin".to_owned(),
            https_origin: "https://trust.example".to_owned(),
        }],
        trust_data: vec![
            AttestationTrustDataRef {
                id: "tpm-roots".to_owned(),
                kind: AttestationTrustDataKind::TrustAnchor,
                sha256: hex::encode(Sha256::digest(root_bytes())),
                media_type: "application/pkix-cert".to_owned(),
                max_bytes: 4096,
                valid_from_epoch: Some(10),
                valid_until_epoch: Some(100),
                source: Some(AttestationTrustDataSource {
                    origin_pin: "tpm-roots-origin".to_owned(),
                    path: "/tpm/roots.der".to_owned(),
                }),
            },
            AttestationTrustDataRef {
                id: "tpm-pcrs".to_owned(),
                kind: AttestationTrustDataKind::Measurement,
                sha256: hex::encode(Sha256::digest(PCR_BYTES)),
                media_type: "application/vnd.mayhem.tpm-pcr-policy+json".to_owned(),
                max_bytes: 4096,
                valid_from_epoch: Some(10),
                valid_until_epoch: Some(100),
                source: Some(AttestationTrustDataSource {
                    origin_pin: "tpm-roots-origin".to_owned(),
                    path: "/tpm/pcrs.json".to_owned(),
                }),
            },
        ],
        quote_kinds: HardwareQuoteKind::ALL
            .into_iter()
            .map(|kind| AttestationQuoteKindPolicy {
                kind,
                enabled: kind == HardwareQuoteKind::Tpm2QuoteEk,
                verifier_profile: profile(kind),
                evidence_schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
                required_trust_data: if kind == HardwareQuoteKind::Tpm2QuoteEk {
                    BTreeSet::from(["tpm-pcrs".to_owned(), "tpm-roots".to_owned()])
                } else {
                    BTreeSet::new()
                },
                measurement_trust_data: if kind == HardwareQuoteKind::Tpm2QuoteEk {
                    BTreeSet::from(["tpm-pcrs".to_owned()])
                } else {
                    BTreeSet::new()
                },
                platforms: if kind == HardwareQuoteKind::Tpm2QuoteEk {
                    BTreeSet::from(["windows-tpm2".to_owned()])
                } else {
                    BTreeSet::new()
                },
                required_measurement_layers: BTreeSet::new(),
            })
            .collect(),
    }
}

const MANAGED_VERIFIER_EXECUTABLE_MEDIA_TYPE: &str = "application/vnd.mayhem.attestation-verifier";
const MANAGED_VERIFIER_MANIFEST_MEDIA_TYPE: &str =
    "application/vnd.mayhem.attestation-verifier-manifest+json";

fn managed_verifier_trust_data_id(target: &str, collateral: &str) -> String {
    format!("managed-verifier.intel_tdx_dcap.{target}.{collateral}")
}

fn policy_with_managed_verifier_targets(targets: &[&str]) -> AdminAttestationPolicy {
    let mut policy = policy(1, None);
    policy.trust_data.push(AttestationTrustDataRef {
        id: "intel-cpu".to_owned(),
        kind: AttestationTrustDataKind::Measurement,
        sha256: "44".repeat(32),
        media_type: "application/vnd.mayhem.cpu-measurement+json".to_owned(),
        max_bytes: 4096,
        valid_from_epoch: Some(10),
        valid_until_epoch: Some(100),
        source: Some(AttestationTrustDataSource {
            origin_pin: "tpm-roots-origin".to_owned(),
            path: "/intel/cpu.json".to_owned(),
        }),
    });
    let intel = policy
        .quote_kinds
        .iter_mut()
        .find(|entry| entry.kind == HardwareQuoteKind::IntelTdxDcap)
        .unwrap();
    intel.enabled = true;
    intel.required_trust_data.extend([
        "intel-cpu".to_owned(),
        "tpm-pcrs".to_owned(),
        "tpm-roots".to_owned(),
    ]);
    intel
        .measurement_trust_data
        .extend(["intel-cpu".to_owned(), "tpm-pcrs".to_owned()]);
    intel.platforms.insert("test-tdx".to_owned());
    intel.required_measurement_layers = BTreeSet::from([
        AttestationMeasurementLayer::Cpu,
        AttestationMeasurementLayer::Workload,
    ]);

    for target in targets {
        for (collateral, media_type) in [
            ("executable", MANAGED_VERIFIER_EXECUTABLE_MEDIA_TYPE),
            ("manifest", MANAGED_VERIFIER_MANIFEST_MEDIA_TYPE),
        ] {
            let bytes = format!("{target}:{collateral}");
            policy.trust_data.push(AttestationTrustDataRef {
                id: managed_verifier_trust_data_id(target, collateral),
                kind: AttestationTrustDataKind::VerificationKey,
                sha256: hex::encode(Sha256::digest(bytes.as_bytes())),
                media_type: media_type.to_owned(),
                max_bytes: bytes.len() as u64,
                valid_from_epoch: Some(10),
                valid_until_epoch: Some(100),
                source: None,
            });
        }
    }
    policy
}

fn capabilities() -> VerifierCapabilities {
    VerifierCapabilities::new(3, [AttestationVerifierProfile::Tpm2EkActivateCredentialV1])
}

fn enclave_id() -> String {
    "11".repeat(32)
}

fn device_id() -> String {
    "22".repeat(32)
}

fn tpm_enclave_binding() -> AdminEnclaveAttestationBinding {
    AdminEnclaveAttestationBinding {
        enclave_id: enclave_id(),
        kind: HardwareQuoteKind::Tpm2QuoteEk,
        platform: Some("windows-tpm2".to_owned()),
        measurement_trust_data: BTreeMap::new(),
    }
}

#[test]
fn policy_chain_is_hash_linked_consecutive_and_non_regressing() {
    let first = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();
    let first_digest = first.digest().to_owned();
    let mut chain = AttestationPolicyChain::new();
    chain.append(first).unwrap();

    let wrong_link =
        ValidatedAttestationPolicy::validate(policy(2, Some("00".repeat(32)))).unwrap();
    assert_eq!(
        chain.append(wrong_link).unwrap_err(),
        PolicyError::PreviousDigestMismatch
    );

    let second = ValidatedAttestationPolicy::validate(policy(2, Some(first_digest))).unwrap();
    chain.append(second).unwrap();
    assert_eq!(chain.len(), 2);

    let mut skipped = policy(4, Some(chain.head().unwrap().digest().to_owned()));
    skipped.issued_epoch = 30;
    skipped.effective_epoch = 31;
    let skipped = ValidatedAttestationPolicy::validate(skipped).unwrap();
    assert!(matches!(
        chain.append(skipped),
        Err(PolicyError::NonConsecutiveSequence {
            previous: 2,
            actual: 4
        })
    ));
}

#[test]
fn policy_digest_is_canonical_across_semantically_irrelevant_ordering() {
    let ordered = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();
    let mut reordered_policy = policy(1, None);
    reordered_policy.quote_kinds.reverse();
    let reordered = ValidatedAttestationPolicy::validate(reordered_policy).unwrap();

    assert_eq!(ordered.digest(), reordered.digest());
    assert_eq!(ordered.canonical_json(), reordered.canonical_json());
}

#[test]
fn managed_verifier_policy_accepts_canonical_target_pairs_without_global_requirement() {
    assert_eq!(
        ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS.len(),
        6
    );
    assert!(
        ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS.contains(&"aarch64-unknown-linux-gnu")
    );
    assert!(
        ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS.contains(&"aarch64-pc-windows-msvc")
    );
    let validated = ValidatedAttestationPolicy::validate(policy_with_managed_verifier_targets(
        &ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS,
    ))
    .unwrap();
    for target in ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS {
        for collateral in ["executable", "manifest"] {
            let id = managed_verifier_trust_data_id(target, collateral);
            assert!(
                validated.trust_data(&id).is_some(),
                "validated policy omitted {id}"
            );
            assert!(
                !validated
                    .quote_kind(HardwareQuoteKind::IntelTdxDcap)
                    .unwrap()
                    .required_trust_data
                    .contains(&id),
                "target collateral became globally required"
            );
        }
    }
}

#[test]
fn managed_verifier_policy_rejects_incomplete_target_pairs() {
    let target = ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS[0];
    let mut incomplete = policy_with_managed_verifier_targets(&[target]);
    let manifest_id = managed_verifier_trust_data_id(target, "manifest");
    incomplete
        .trust_data
        .retain(|reference| reference.id != manifest_id);

    assert_eq!(
        ValidatedAttestationPolicy::validate(incomplete).unwrap_err(),
        PolicyError::IncompleteManagedVerifierTarget {
            kind: HardwareQuoteKind::IntelTdxDcap.as_str().to_owned(),
            target: target.to_owned(),
        }
    );
}

#[test]
fn managed_verifier_policy_rejects_noncanonical_or_globally_required_targets() {
    let target = ValidatedAttestationPolicy::MANAGED_VERIFIER_TARGETS[0];
    let mut globally_required = policy_with_managed_verifier_targets(&[target]);
    let executable_id = managed_verifier_trust_data_id(target, "executable");
    globally_required
        .quote_kinds
        .iter_mut()
        .find(|entry| entry.kind == HardwareQuoteKind::IntelTdxDcap)
        .unwrap()
        .required_trust_data
        .insert(executable_id.clone());
    assert!(matches!(
        ValidatedAttestationPolicy::validate(globally_required),
        Err(PolicyError::InvalidManagedVerifierTrustData { id, reason })
            if id == executable_id && reason.contains("must not be globally required")
    ));

    let unsupported_target = "aarch64-unknown-linux-musl";
    let noncanonical = policy_with_managed_verifier_targets(&[unsupported_target]);
    assert!(matches!(
        ValidatedAttestationPolicy::validate(noncanonical),
        Err(PolicyError::InvalidManagedVerifierTrustData { id, reason })
            if id == managed_verifier_trust_data_id(unsupported_target, "executable")
                && reason.contains("canonically bind")
    ));
}

#[test]
fn pending_policy_emergency_disable_applies_before_normal_activation() {
    let first = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();
    let mut second_policy = policy(2, Some(first.digest().to_owned()));
    second_policy.issued_epoch = 15;
    second_policy.effective_epoch = 30;
    second_policy
        .emergency_disabled_quote_kinds
        .insert(HardwareQuoteKind::Tpm2QuoteEk);
    let second = ValidatedAttestationPolicy::validate(second_policy).unwrap();

    let mut chain = AttestationPolicyChain::new();
    chain.append(first).unwrap();
    chain.append(second).unwrap();
    let mut collateral = CollateralInventory::new();
    collateral
        .insert(chain.active_at(16).unwrap(), "tpm-roots", root_bytes(), 16)
        .unwrap();
    collateral
        .insert(chain.active_at(16).unwrap(), "tpm-pcrs", PCR_BYTES, 16)
        .unwrap();

    let readiness = AttestationReadiness::evaluate(&chain, &collateral, &capabilities(), 16);
    assert_eq!(
        readiness.quote_kind(HardwareQuoteKind::Tpm2QuoteEk).status,
        QuoteKindReadinessStatus::EmergencyDisabled
    );
}

#[test]
fn normal_policy_activation_can_clear_an_older_emergency_disable() {
    let mut first_policy = policy(1, None);
    first_policy
        .emergency_disabled_quote_kinds
        .insert(HardwareQuoteKind::Tpm2QuoteEk);
    let first = ValidatedAttestationPolicy::validate(first_policy).unwrap();
    let mut second_policy = policy(2, Some(first.digest().to_owned()));
    second_policy.issued_epoch = 15;
    second_policy.effective_epoch = 30;
    let second = ValidatedAttestationPolicy::validate(second_policy).unwrap();
    let mut chain = AttestationPolicyChain::new();
    chain.append(first).unwrap();
    chain.append(second).unwrap();
    let mut collateral = CollateralInventory::new();
    collateral
        .insert(chain.active_at(20).unwrap(), "tpm-roots", root_bytes(), 20)
        .unwrap();
    collateral
        .insert(chain.active_at(20).unwrap(), "tpm-pcrs", PCR_BYTES, 20)
        .unwrap();

    assert_eq!(
        AttestationReadiness::evaluate(&chain, &collateral, &capabilities(), 20)
            .quote_kind(HardwareQuoteKind::Tpm2QuoteEk)
            .status,
        QuoteKindReadinessStatus::EmergencyDisabled
    );
    assert_eq!(
        AttestationReadiness::evaluate(&chain, &collateral, &capabilities(), 30)
            .quote_kind(HardwareQuoteKind::Tpm2QuoteEk)
            .status,
        QuoteKindReadinessStatus::Ready
    );
}

#[test]
fn collateral_is_content_addressed_and_required_for_readiness() {
    let validated = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();
    assert_eq!(
        validated.policy_source_url("tpm-roots").as_deref(),
        Some("https://trust.example/tpm/roots.der")
    );
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated).unwrap();
    let mut inventory = CollateralInventory::new();

    let missing = AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 20);
    assert!(matches!(
        missing.quote_kind(HardwareQuoteKind::Tpm2QuoteEk).status,
        QuoteKindReadinessStatus::MissingCollateral { .. }
    ));

    assert!(matches!(
        inventory.insert(
            chain.active_at(20).unwrap(),
            "tpm-roots",
            b"provider-substituted-roots",
            20
        ),
        Err(CollateralError::DigestMismatch { .. })
    ));
    let inserted = inventory
        .insert(chain.active_at(20).unwrap(), "tpm-roots", root_bytes(), 20)
        .unwrap();
    let inserted_digest = inserted.sha256().to_owned();
    inventory
        .insert(chain.active_at(20).unwrap(), "tpm-pcrs", PCR_BYTES, 20)
        .unwrap();
    assert_eq!(
        collateral_cache_relative_path(&inserted_digest).unwrap(),
        std::path::PathBuf::from(format!(
            "sha256/{}/{}",
            &inserted_digest[..2],
            inserted_digest
        ))
    );

    let ready = AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 20);
    assert_eq!(
        ready.quote_kind(HardwareQuoteKind::Tpm2QuoteEk).status,
        QuoteKindReadinessStatus::Ready
    );
    assert_eq!(
        ready
            .quote_kind(HardwareQuoteKind::AppleAppAttestJwt)
            .status,
        QuoteKindReadinessStatus::Disabled
    );
    assert_eq!(
        TpmVerificationMaterials::from_policy(
            chain.active_at(20).unwrap(),
            &inventory,
            1_800_000_000,
        )
        .unwrap()
        .pcr_policy()
        .pcrs
        .iter()
        .copied()
        .collect::<Vec<_>>(),
        vec![0]
    );
    assert!(matches!(
        TpmVerificationMaterials::from_policy(
            chain.active_at(20).unwrap(),
            &inventory,
            2_200_000_000,
        ),
        Err(TpmQuoteError::EkCertificate(
            EkCertificateError::CertificateTime { index: 0, .. }
        ))
    ));

    let expired = AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 100);
    assert!(matches!(
        expired.quote_kind(HardwareQuoteKind::Tpm2QuoteEk).status,
        QuoteKindReadinessStatus::CollateralExpired { .. }
    ));
}

#[test]
fn route_binding_requires_ready_kind_and_admin_platform() {
    let validated = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated).unwrap();
    chain
        .configure_enclave_bindings(vec![tpm_enclave_binding()])
        .unwrap();
    let active = chain.active_at(20).unwrap();
    let mut inventory = CollateralInventory::new();
    inventory
        .insert(active, "tpm-roots", root_bytes(), 20)
        .unwrap();
    inventory.insert(active, "tpm-pcrs", PCR_BYTES, 20).unwrap();
    let readiness = AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 20);

    let bound = readiness
        .bind_route(
            &enclave_id(),
            &device_id(),
            &HardwareQuoteRouteAdvertisement {
                kind: HardwareQuoteKind::Tpm2QuoteEk,
                declared_platform: Some("windows-tpm2".to_owned()),
            },
        )
        .unwrap();
    assert_eq!(bound.enclave_id, enclave_id());
    assert_eq!(bound.device_id, device_id());
    assert_eq!(bound.kind, HardwareQuoteKind::Tpm2QuoteEk);
    assert_eq!(bound.policy_digest, active.digest());
    assert_eq!(
        bound.evidence_schema_version,
        TPM_QUOTE_EVIDENCE_SCHEMA_VERSION
    );
    assert_eq!(bound.platform.as_deref(), Some("windows-tpm2"));
    readiness.verify_binding(&bound).unwrap();

    assert!(matches!(
        readiness.bind_route(
            &enclave_id(),
            &device_id(),
            &HardwareQuoteRouteAdvertisement {
                kind: HardwareQuoteKind::Tpm2QuoteEk,
                declared_platform: Some("provider-invented".to_owned()),
            },
        ),
        Err(ReadinessError::PlatformNotAllowed { .. })
    ));
    assert!(matches!(
        readiness.bind_route(
            &enclave_id(),
            &device_id(),
            &HardwareQuoteRouteAdvertisement {
                kind: HardwareQuoteKind::AppleAppAttestJwt,
                declared_platform: None,
            },
        ),
        Err(ReadinessError::QuoteKindNotReady { .. })
    ));
}

#[test]
fn activation_readiness_covers_every_quote_kind_and_evidence_schema() {
    let mut all_kinds = policy(1, None);
    for entry in &mut all_kinds.quote_kinds {
        entry.enabled = true;
        entry.required_trust_data.insert("tpm-roots".to_owned());
        entry
            .platforms
            .insert(format!("{}-platform", entry.kind.as_str()));
        if entry.kind == HardwareQuoteKind::Tpm2QuoteEk
            || entry.kind.attestation_tier() >= mayhem_proto::TIER3_CONFIDENTIAL_COMPUTE_TIER
        {
            entry.required_trust_data.insert("tpm-pcrs".to_owned());
            entry.measurement_trust_data.insert("tpm-pcrs".to_owned());
        }
        if entry.kind.attestation_tier() >= mayhem_proto::TIER3_CONFIDENTIAL_COMPUTE_TIER {
            entry.required_measurement_layers = match entry.kind {
                HardwareQuoteKind::AmdSevSnpVcek | HardwareQuoteKind::IntelTdxDcap => {
                    BTreeSet::from([
                        AttestationMeasurementLayer::Cpu,
                        AttestationMeasurementLayer::Workload,
                    ])
                }
                HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
                    BTreeSet::from([
                        AttestationMeasurementLayer::Cpu,
                        AttestationMeasurementLayer::Gpu,
                        AttestationMeasurementLayer::Workload,
                    ])
                }
                _ => unreachable!("Tier-3 matrix covers every Tier-3 quote kind"),
            };
        }
    }
    let validated = ValidatedAttestationPolicy::validate(all_kinds).unwrap();
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated).unwrap();
    let active = chain.active_at(20).unwrap();
    let mut inventory = CollateralInventory::new();
    inventory
        .insert(active, "tpm-roots", root_bytes(), 20)
        .unwrap();
    inventory.insert(active, "tpm-pcrs", PCR_BYTES, 20).unwrap();
    let capabilities =
        VerifierCapabilities::new(3, HardwareQuoteKind::ALL.into_iter().map(profile));
    let readiness = AttestationReadiness::evaluate(&chain, &inventory, &capabilities, 20);
    for kind in HardwareQuoteKind::ALL {
        assert_eq!(
            readiness.quote_kind(kind).status,
            QuoteKindReadinessStatus::Ready,
            "{} was not activation-ready",
            kind.as_str()
        );
    }

    let mut newer_schema = policy(1, None);
    newer_schema
        .quote_kinds
        .iter_mut()
        .find(|entry| entry.kind == HardwareQuoteKind::Tpm2QuoteEk)
        .unwrap()
        .evidence_schema_version = 2;
    let mut chain = AttestationPolicyChain::new();
    chain
        .append(ValidatedAttestationPolicy::validate(newer_schema).unwrap())
        .unwrap();
    assert!(matches!(
        AttestationReadiness::evaluate(&chain, &inventory, &capabilities, 20)
            .quote_kind(HardwareQuoteKind::Tpm2QuoteEk)
            .status,
        QuoteKindReadinessStatus::EvidenceSchemaUnavailable { required: 2, .. }
    ));
}

#[test]
fn activation_rollover_rejects_a_stale_route_policy_binding() {
    let first = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();
    let first_digest = first.digest().to_owned();
    let mut chain = AttestationPolicyChain::new();
    chain.append(first).unwrap();
    chain
        .configure_enclave_bindings(vec![tpm_enclave_binding()])
        .unwrap();
    let mut inventory = CollateralInventory::new();
    inventory
        .insert(chain.active_at(20).unwrap(), "tpm-roots", root_bytes(), 20)
        .unwrap();
    inventory
        .insert(chain.active_at(20).unwrap(), "tpm-pcrs", PCR_BYTES, 20)
        .unwrap();
    let before = AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 20);
    let old_binding = before
        .bind_route(
            &enclave_id(),
            &device_id(),
            &HardwareQuoteRouteAdvertisement {
                kind: HardwareQuoteKind::Tpm2QuoteEk,
                declared_platform: Some("windows-tpm2".to_owned()),
            },
        )
        .unwrap();

    let mut second_policy = policy(2, Some(first_digest));
    second_policy.issued_epoch = 21;
    second_policy.effective_epoch = 30;
    chain
        .append(ValidatedAttestationPolicy::validate(second_policy).unwrap())
        .unwrap();
    let after = AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 30);
    assert!(matches!(
        after.verify_binding(&old_binding),
        Err(ReadinessError::PolicyBindingMismatch {
            kind: HardwareQuoteKind::Tpm2QuoteEk
        })
    ));
}

#[test]
fn readiness_fails_closed_for_old_or_missing_verifier_profiles_and_expired_policy() {
    let mut expiring_policy = policy(1, None);
    expiring_policy.expires_epoch = Some(40);
    let validated = ValidatedAttestationPolicy::validate(expiring_policy).unwrap();
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated).unwrap();
    let mut inventory = CollateralInventory::new();
    inventory
        .insert(chain.active_at(20).unwrap(), "tpm-roots", root_bytes(), 20)
        .unwrap();
    inventory
        .insert(chain.active_at(20).unwrap(), "tpm-pcrs", PCR_BYTES, 20)
        .unwrap();

    let old =
        VerifierCapabilities::new(2, [AttestationVerifierProfile::Tpm2EkActivateCredentialV1]);
    assert!(matches!(
        AttestationReadiness::evaluate(&chain, &inventory, &old, 20)
            .quote_kind(HardwareQuoteKind::Tpm2QuoteEk)
            .status,
        QuoteKindReadinessStatus::VerifierTooOld {
            required: 3,
            actual: 2
        }
    ));

    let missing_profile = VerifierCapabilities::new(3, []);
    assert!(matches!(
        AttestationReadiness::evaluate(&chain, &inventory, &missing_profile, 20)
            .quote_kind(HardwareQuoteKind::Tpm2QuoteEk)
            .status,
        QuoteKindReadinessStatus::VerifierProfileUnavailable { .. }
    ));
    assert_eq!(
        AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 40)
            .quote_kind(HardwareQuoteKind::Tpm2QuoteEk)
            .status,
        QuoteKindReadinessStatus::PolicyExpired { expires_epoch: 40 }
    );
}

#[test]
fn policy_rejects_incomplete_kind_matrix_bad_origins_and_weak_tier3_rules() {
    let mut incomplete = policy(1, None);
    incomplete.quote_kinds.pop();
    assert!(matches!(
        ValidatedAttestationPolicy::validate(incomplete),
        Err(PolicyError::MissingQuoteKind(_))
    ));

    let mut bad_origin = policy(1, None);
    bad_origin.origin_pins[0].https_origin = "https://trust.example/path?from=provider".to_owned();
    assert!(matches!(
        ValidatedAttestationPolicy::validate(bad_origin),
        Err(PolicyError::InvalidOrigin { .. })
    ));

    let mut encoded_traversal = policy(1, None);
    encoded_traversal.trust_data[0]
        .source
        .as_mut()
        .unwrap()
        .path = "/tpm/%2e%2e/provider.pem".to_owned();
    assert!(matches!(
        ValidatedAttestationPolicy::validate(encoded_traversal),
        Err(PolicyError::InvalidTrustData { .. })
    ));

    let mut weak_tier3 = policy(1, None);
    let amd = weak_tier3
        .quote_kinds
        .iter_mut()
        .find(|entry| entry.kind == HardwareQuoteKind::AmdSevSnpVcek)
        .unwrap();
    amd.enabled = true;
    amd.required_trust_data.insert("tpm-roots".to_owned());
    assert!(matches!(
        ValidatedAttestationPolicy::validate(weak_tier3),
        Err(PolicyError::MissingTier3Platform(_))
    ));

    let mut no_workload = policy(1, None);
    let amd = no_workload
        .quote_kinds
        .iter_mut()
        .find(|entry| entry.kind == HardwareQuoteKind::AmdSevSnpVcek)
        .unwrap();
    amd.enabled = true;
    amd.required_trust_data.insert("tpm-roots".to_owned());
    amd.required_trust_data.insert("tpm-pcrs".to_owned());
    amd.measurement_trust_data.insert("tpm-pcrs".to_owned());
    amd.platforms.insert("azure-sev-snp".to_owned());
    amd.required_measurement_layers
        .insert(AttestationMeasurementLayer::Cpu);
    assert!(matches!(
        ValidatedAttestationPolicy::validate(no_workload),
        Err(PolicyError::InvalidTier3MeasurementLayers { .. })
    ));
}

#[test]
fn absent_catalog_policy_is_tier1_only_and_bindings_cannot_restore_hardware_fallback() {
    let tier1 =
        AttestationPolicyChain::from_catalog_records(None, Vec::new()).expect("Tier1 authority");
    assert!(tier1.is_empty());
    assert!(tier1.active_at(u64::MAX).is_none());

    assert_eq!(
        AttestationPolicyChain::from_catalog_records(None, vec![tpm_enclave_binding()])
            .unwrap_err(),
        PolicyError::BindingWithoutPolicy
    );
}

#[test]
fn enclave_bindings_are_unique_canonical_and_exact() {
    let validated = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();

    let original = tpm_enclave_binding();
    let mut unrelated = original.clone();
    unrelated.enclave_id = "22".repeat(32);
    let mut original_chain = AttestationPolicyChain::new();
    original_chain.append(validated.clone()).unwrap();
    original_chain
        .configure_enclave_bindings(vec![original.clone()])
        .unwrap();
    let mut expanded_chain = AttestationPolicyChain::new();
    expanded_chain.append(validated.clone()).unwrap();
    expanded_chain
        .configure_enclave_bindings(vec![original.clone(), unrelated])
        .unwrap();
    let before = original_chain
        .enclave_binding(
            &original.enclave_id,
            original.kind,
            original.platform.as_deref(),
        )
        .unwrap();
    let after = expanded_chain
        .enclave_binding(
            &original.enclave_id,
            original.kind,
            original.platform.as_deref(),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        serde_json::to_vec(before).unwrap(),
        serde_json::to_vec(after).unwrap()
    );

    let mut duplicate = AttestationPolicyChain::new();
    duplicate.append(validated.clone()).unwrap();
    assert!(matches!(
        duplicate.configure_enclave_bindings(vec![tpm_enclave_binding(), tpm_enclave_binding()]),
        Err(PolicyError::DuplicateEnclaveBinding { .. })
    ));

    let mut later = tpm_enclave_binding();
    later.enclave_id = "22".repeat(32);
    let mut earlier = tpm_enclave_binding();
    earlier.enclave_id = "11".repeat(32);
    let mut noncanonical = AttestationPolicyChain::new();
    noncanonical.append(validated).unwrap();
    assert_eq!(
        noncanonical
            .configure_enclave_bindings(vec![later, earlier])
            .unwrap_err(),
        PolicyError::NonCanonicalBindingOrder
    );
}

#[test]
fn tier3_enclave_binding_requires_exact_kind_platform_and_measurement_references() {
    let mut amd_policy = policy(1, None);
    amd_policy.trust_data.push(AttestationTrustDataRef {
        id: "amd-cpu".to_owned(),
        kind: AttestationTrustDataKind::Measurement,
        sha256: "44".repeat(32),
        media_type: "application/vnd.mayhem.cpu-measurement+json".to_owned(),
        max_bytes: 4096,
        valid_from_epoch: Some(10),
        valid_until_epoch: Some(100),
        source: Some(AttestationTrustDataSource {
            origin_pin: "tpm-roots-origin".to_owned(),
            path: "/amd/cpu.json".to_owned(),
        }),
    });
    let amd = amd_policy
        .quote_kinds
        .iter_mut()
        .find(|entry| entry.kind == HardwareQuoteKind::AmdSevSnpVcek)
        .unwrap();
    amd.enabled = true;
    amd.required_trust_data.extend([
        "amd-cpu".to_owned(),
        "tpm-pcrs".to_owned(),
        "tpm-roots".to_owned(),
    ]);
    amd.measurement_trust_data
        .extend(["amd-cpu".to_owned(), "tpm-pcrs".to_owned()]);
    amd.platforms.insert("azure-sev-snp".to_owned());
    amd.required_measurement_layers = BTreeSet::from([
        AttestationMeasurementLayer::Cpu,
        AttestationMeasurementLayer::Workload,
    ]);
    let validated = ValidatedAttestationPolicy::validate(amd_policy).unwrap();

    let exact = AdminEnclaveAttestationBinding {
        enclave_id: enclave_id(),
        kind: HardwareQuoteKind::AmdSevSnpVcek,
        platform: Some("azure-sev-snp".to_owned()),
        measurement_trust_data: BTreeMap::from([
            (AttestationMeasurementLayer::Cpu, "amd-cpu".to_owned()),
            (AttestationMeasurementLayer::Workload, "tpm-pcrs".to_owned()),
        ]),
    };
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated.clone()).unwrap();
    chain
        .configure_enclave_bindings(vec![exact.clone(), tpm_enclave_binding()])
        .unwrap();

    let mut incomplete = exact.clone();
    incomplete
        .measurement_trust_data
        .remove(&AttestationMeasurementLayer::Workload);
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated.clone()).unwrap();
    assert!(matches!(
        chain.configure_enclave_bindings(vec![incomplete]),
        Err(PolicyError::BindingMeasurementSetMismatch { .. })
    ));

    let mut wrong_platform = exact.clone();
    wrong_platform.platform = Some("provider-platform".to_owned());
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated.clone()).unwrap();
    assert!(matches!(
        chain.configure_enclave_bindings(vec![wrong_platform]),
        Err(PolicyError::BindingPlatformMismatch { .. })
    ));

    let mut wrong_reference = exact;
    wrong_reference.measurement_trust_data.insert(
        AttestationMeasurementLayer::Cpu,
        "provider-measurement".to_owned(),
    );
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated).unwrap();
    assert!(matches!(
        chain.configure_enclave_bindings(vec![wrong_reference]),
        Err(PolicyError::UnknownBindingTrustData { .. })
    ));
}

#[test]
fn route_selection_rejects_unknown_enclave_and_uses_only_the_supplied_ledger_epoch() {
    let first = ValidatedAttestationPolicy::validate(policy(1, None)).unwrap();
    let first_digest = first.digest().to_owned();
    let mut second_policy = policy(2, Some(first_digest));
    second_policy.issued_epoch = 50;
    second_policy.effective_epoch = 60;
    let second = ValidatedAttestationPolicy::validate(second_policy).unwrap();
    let mut chain = AttestationPolicyChain::new();
    chain.append(first).unwrap();
    chain.append(second).unwrap();
    chain
        .configure_enclave_bindings(vec![tpm_enclave_binding()])
        .unwrap();
    assert_eq!(chain.active_at(59).unwrap().policy().sequence, 1);
    assert_eq!(chain.active_at(60).unwrap().policy().sequence, 2);

    let active = chain.active_at(20).unwrap();
    let mut inventory = CollateralInventory::new();
    inventory
        .insert(active, "tpm-roots", root_bytes(), 20)
        .unwrap();
    inventory.insert(active, "tpm-pcrs", PCR_BYTES, 20).unwrap();
    let readiness = AttestationReadiness::evaluate(&chain, &inventory, &capabilities(), 20);
    assert!(matches!(
        readiness.bind_route(
            &"99".repeat(32),
            &device_id(),
            &HardwareQuoteRouteAdvertisement {
                kind: HardwareQuoteKind::Tpm2QuoteEk,
                declared_platform: Some("windows-tpm2".to_owned()),
            }
        ),
        Err(ReadinessError::UnknownEnclaveBinding { .. })
    ));
}
