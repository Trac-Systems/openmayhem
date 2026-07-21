#![forbid(unsafe_code)]

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::thread::sleep;
use std::time::Duration;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use aes::{
    cipher::{AsyncStreamCipher, KeyIvInit},
    Aes128,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cfb_mode::Decryptor;
use ed25519_dalek::SigningKey;
use hmac::{Hmac, Mac};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mayhem_attestation::{
    issue_tpm_activate_credential_challenge_with_rng, ActivatedTpmIdentity, AttestationPolicyChain,
    AttestationReadiness, CollateralInventory, EvidenceBinding, QuoteKindReadinessStatus,
    ValidatedAttestationPolicy, VerifierCapabilities,
};
use mayhem_enclave::{
    build_hardware_attestation_report, build_tier1_attestation_report, measure_binary,
    HardwareAttestationOptions, RuntimeKeypair, Tier1AttestationOptions,
};
use mayhem_gateway::openai::{GatewayAttestationAuthority, GatewayAttestationCollateral};
use mayhem_gateway::{
    verify_attestation, verify_tier1_attestation, AttestationPolicyVerificationContext,
    AttestationVerificationRequest, EnclaveContractRecord, GatewayError,
    HardwareQuoteVerifierCommand, GATEWAY_ATTESTATION_VERIFIER_VERSION,
    MAX_HARDWARE_QUOTE_ENDORSEMENTS_BYTES, MAX_HARDWARE_QUOTE_EVIDENCE_BYTES,
    MAX_HARDWARE_QUOTE_METADATA_BYTES, MAX_HARDWARE_QUOTE_METADATA_DEPTH,
};
use mayhem_proto::{
    catalog_enclave_id, hardware_quote_binding, AdminAttestationPolicy,
    AdminEnclaveAttestationBinding, AttestationBody, AttestationMeasurementLayer,
    AttestationQuoteKindPolicy, AttestationRuntimeConfig, AttestationTrustDataKind,
    AttestationTrustDataRef, AttestationVerifierProfile, CatalogEnclaveIdentity, HardwareQuote,
    HardwareQuoteKind, HardwareQuoteRoutePolicyBinding, TpmActivateCredentialHello,
    TpmActivateCredentialResponse, TpmEkProfile, TpmHashAlgorithm, TpmPcrValue, TpmQuoteEvidence,
    ATTESTATION_ALG, ATTESTATION_POLICY_SCHEMA_VERSION, ATTESTATION_SCHEMA_VERSION,
    DEFAULT_MODEL_CLASS, TIER2_DEVICE_IDENTITY_TIER, TIER3_CONFIDENTIAL_COMPUTE_TIER,
    TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION, TPM_PCR_POLICY_SCHEMA_VERSION,
    TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use rsa::{
    pkcs1v15::SigningKey as RsaSigningKey,
    pkcs8::EncodePublicKey,
    signature::{SignatureEncoding, Signer as _},
    traits::PublicKeyParts,
    Oaep, RsaPrivateKey, RsaPublicKey,
};
use sha2::{Digest, Sha256};

const TEST_POLICY_EPOCH: u64 = 20;
const TEST_DEVICE_ID: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

#[cfg(unix)]
fn test_process_is_running(pid: &str) -> bool {
    let Ok(pid) = pid.parse::<u32>() else {
        return false;
    };
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let state = fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let (_, fields) = stat.rsplit_once(") ")?;
                let state = fields.split_ascii_whitespace().next()?.as_bytes();
                (state.len() == 1).then_some(state[0])
            });
        if state == Some(b'Z') {
            return false;
        }
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn test_enclave_id() -> String {
    catalog_enclave_id(&CatalogEnclaveIdentity {
        admin_pubkey: "admin-key".to_owned(),
        model_id: "mayhem/qwen3.5-4b@q4".to_owned(),
        artifact_root: "artifact-root-v1".to_owned(),
        artifact_sidecar_roots: BTreeMap::new(),
        manifest_hash: "manifest-hash-v1".to_owned(),
        binary_hash: String::new(),
    })
}

fn test_report() -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let binary = temp.path().join("mayhem-enclave-test-bin");
    fs::write(&binary, b"measured enclave binary").expect("write test binary");
    let binary_hash = measure_binary(&binary).expect("measure binary");
    let identity = CatalogEnclaveIdentity {
        admin_pubkey: "admin-key".to_owned(),
        model_id: "mayhem/qwen3.5-4b@q4".to_owned(),
        artifact_root: "artifact-root-v1".to_owned(),
        artifact_sidecar_roots: std::collections::BTreeMap::new(),
        manifest_hash: "manifest-hash-v1".to_owned(),
        binary_hash,
    };
    let enclave_id = catalog_enclave_id(&identity);
    let attestation = build_tier1_attestation_report(&Tier1AttestationOptions {
        identity: identity.clone(),
        runtime_keypair: RuntimeKeypair::from_seed([9_u8; 32]),
        provider_signing_seed: [7_u8; 32],
        binary_path: binary,
        boot_epoch: 100,
        report_ts: 200,
        nonce_u: "aa".repeat(32),
        runtime_config: AttestationRuntimeConfig::default(),
    })
    .expect("build report");
    let contract = EnclaveContractRecord {
        enclave_id,
        admin_pubkey: identity.admin_pubkey,
        model_id: identity.model_id,
        model_class: DEFAULT_MODEL_CLASS.to_owned(),
        artifact_root: identity.artifact_root,
        artifact_sidecar_roots: std::collections::BTreeMap::new(),
        manifest_hash: identity.manifest_hash,
        binary_hash: attestation.report.binary_hash.clone(),
        launch_measurements: serde_json::Value::Null,
        att_tier: 1,
        caps: serde_json::json!({}),
    };
    (temp, attestation.report, contract)
}

fn test_hardware_report(
    quote_kind: HardwareQuoteKind,
) -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
) {
    test_hardware_report_with_evidence_and_metadata(quote_kind, None, |body, binding| {
        match quote_kind {
            HardwareQuoteKind::AppleAppAttestJwt => test_apple_app_attest_evidence(body, binding),
            HardwareQuoteKind::NvidiaGb10DeviceJwt => {
                test_nvidia_gb10_device_evidence(body, binding, "NVIDIA GB10 DGX Spark")
            }
            HardwareQuoteKind::NvidiaNrasJwt => test_nvidia_evidence(binding, true),
            HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
                test_nvidia_offline_evidence(binding, true)
            }
            _ => "test-hardware-quote".to_owned(),
        }
    })
}

fn test_hardware_report_with_metadata(
    quote_kind: HardwareQuoteKind,
    metadata: serde_json::Value,
) -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
) {
    test_hardware_report_with_evidence_and_metadata(quote_kind, Some(metadata), |body, binding| {
        match quote_kind {
            HardwareQuoteKind::AppleAppAttestJwt => test_apple_app_attest_evidence(body, binding),
            HardwareQuoteKind::NvidiaGb10DeviceJwt => {
                test_nvidia_gb10_device_evidence(body, binding, "NVIDIA GB10 DGX Spark")
            }
            HardwareQuoteKind::NvidiaNrasJwt => test_nvidia_evidence(binding, true),
            HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
                test_nvidia_offline_evidence(binding, true)
            }
            _ => "test-hardware-quote".to_owned(),
        }
    })
}

fn golden_launch_measurements() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "effective_epoch": 0,
        "platform": "azure-h100-sev-snp-nvidia-cc",
        "layers": {
            "workload": {
                "vtpm_pcr_0": "ab".repeat(48)
            }
        }
    })
}

fn test_hardware_report_with_evidence(
    quote_kind: HardwareQuoteKind,
    evidence_for_binding: impl FnOnce(&AttestationBody, &str) -> String,
) -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
) {
    test_hardware_report_with_evidence_and_metadata(quote_kind, None, evidence_for_binding)
}

fn test_hardware_report_with_evidence_and_metadata(
    quote_kind: HardwareQuoteKind,
    metadata: Option<serde_json::Value>,
    evidence_for_binding: impl FnOnce(&AttestationBody, &str) -> String,
) -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let binary = temp.path().join("mayhem-enclave-test-bin");
    fs::write(&binary, b"measured hardware enclave binary").expect("write test binary");
    let binary_hash = measure_binary(&binary).expect("measure binary");
    let runtime_keypair = RuntimeKeypair::from_seed([9_u8; 32]);
    let provider_signing_key = SigningKey::from_bytes(&[7_u8; 32]);
    let att_tier = quote_kind.attestation_tier();
    let identity = CatalogEnclaveIdentity {
        admin_pubkey: "admin-key".to_owned(),
        model_id: "mayhem/qwen3.5-4b@q4".to_owned(),
        artifact_root: "artifact-root-v1".to_owned(),
        artifact_sidecar_roots: std::collections::BTreeMap::new(),
        manifest_hash: "manifest-hash-v1".to_owned(),
        binary_hash,
    };
    let body = AttestationBody {
        schema_version: ATTESTATION_SCHEMA_VERSION,
        alg: ATTESTATION_ALG.to_owned(),
        enclave_id: catalog_enclave_id(&identity),
        enclave_pubkey: runtime_keypair.public_key_hex(),
        provider_pubkey: hex::encode(provider_signing_key.verifying_key().to_bytes()),
        manifest_hash: identity.manifest_hash.clone(),
        binary_hash: identity.binary_hash.clone(),
        att_tier,
        hw_quote: None,
        boot_epoch: 100,
        report_ts: 200,
        nonce_u: "aa".repeat(32),
        runtime_config: AttestationRuntimeConfig::default(),
    };
    let binding = hardware_quote_binding(&body).expect("binding");
    let evidence_nonce = if matches!(
        quote_kind,
        HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
    ) {
        binding.clone()
    } else {
        policy_evidence_nonce(quote_kind, &body, &binding)
    };
    let quote = HardwareQuote {
        kind: quote_kind,
        evidence: evidence_for_binding(&body, &evidence_nonce),
        binding,
        endorsements: vec!["mock-root".to_owned()],
        metadata: metadata.unwrap_or_else(|| {
            if att_tier >= 3 {
                serde_json::json!({
                    "platform_id": "test-platform",
                    "region": "centralus"
                })
            } else {
                serde_json::Value::Null
            }
        }),
    };
    let attestation = build_hardware_attestation_report(&HardwareAttestationOptions {
        identity: identity.clone(),
        runtime_keypair,
        provider_signing_seed: [7_u8; 32],
        binary_path: binary,
        boot_epoch: body.boot_epoch,
        report_ts: body.report_ts,
        nonce_u: body.nonce_u.clone(),
        hw_quote: quote,
        runtime_config: body.runtime_config.clone(),
    })
    .expect("build hardware report");
    let contract = EnclaveContractRecord {
        enclave_id: body.enclave_id,
        admin_pubkey: identity.admin_pubkey,
        model_id: identity.model_id,
        model_class: DEFAULT_MODEL_CLASS.to_owned(),
        artifact_root: identity.artifact_root,
        artifact_sidecar_roots: std::collections::BTreeMap::new(),
        manifest_hash: identity.manifest_hash,
        binary_hash: attestation.report.binary_hash.clone(),
        launch_measurements: if attestation.report.att_tier >= 3 {
            golden_launch_measurements()
        } else {
            serde_json::Value::Null
        },
        att_tier: attestation.report.att_tier,
        caps: serde_json::json!({}),
    };
    (temp, attestation.report, contract)
}

const TEST_NVIDIA_NRAS_KID: &str = "nras-test-kid";
const TEST_NVIDIA_NRAS_PRIVATE_KEY_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIG2AgEAMBAGByqGSM49AgEGBSuBBAAiBIGeMIGbAgEBBDCAHpFQ62QnGCEvYh/p
E9QmR1C9aLcDItRbslbmhen/h1tt8AyMhskeenT+rAyyPhGhZANiAAQLW5ZJePZz
MIPAxMtZXkEWbDF0zo9f2n4+T1h/2sh/fviblc/VTyrv10GEtIi5qiOy85Pf1RRw
8lE5IPUWpgu553SteKigiKLUPeNpbqmYZUkWGh3MLfVzLmx85ii2vMU=
-----END PRIVATE KEY-----"#;

fn test_nvidia_jwks() -> serde_json::Value {
    serde_json::json!({
        "keys": [{
            "kty": "EC",
            "crv": "P-384",
            "x": "C1uWSXj2czCDwMTLWV5BFmwxdM6PX9p-Pk9Yf9rIf374m5XP1U8q79dBhLSIuaoj",
            "y": "svOT39UUcPJROSD1FqYLued0rXiooIii1D3jaW6pmGVJFhodzC31cy5sfOYotrzF",
            "kid": TEST_NVIDIA_NRAS_KID,
            "use": "sig"
        }]
    })
}

fn verifier_profile(kind: HardwareQuoteKind) -> AttestationVerifierProfile {
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

struct TestPolicy {
    chain: AttestationPolicyChain,
    collateral: CollateralInventory,
    route_binding: HardwareQuoteRoutePolicyBinding,
    device_id: String,
}

impl TestPolicy {
    fn context(&self) -> AttestationPolicyVerificationContext<'_> {
        AttestationPolicyVerificationContext::new(
            &self.chain,
            &self.collateral,
            &self.route_binding,
            TEST_POLICY_EPOCH,
            &self.device_id,
        )
    }
}

fn test_policy(kind: HardwareQuoteKind) -> TestPolicy {
    test_policy_with_managed_executable(kind, None)
}

fn foreign_managed_verifier_target() -> &'static str {
    let current = mayhem_gateway::managed_verifier_target()
        .expect("managed verifier tests require a supported release target");
    mayhem_gateway::MANAGED_VERIFIER_TARGETS
        .iter()
        .copied()
        .find(|target| *target != current)
        .expect("managed verifier target matrix contains a foreign target")
}

fn test_policy_with_managed_executable(
    kind: HardwareQuoteKind,
    managed_executable: Option<&[u8]>,
) -> TestPolicy {
    test_policy_with_managed_identity(
        kind,
        managed_executable,
        "org.mayhem.test-managed-verifier",
        3,
    )
}

fn test_policy_with_managed_identity(
    kind: HardwareQuoteKind,
    managed_executable: Option<&[u8]>,
    verifier_id: &str,
    verifier_version: u32,
) -> TestPolicy {
    let target = mayhem_gateway::managed_verifier_target()
        .expect("managed verifier tests require a supported release target");
    test_policy_with_managed_identity_for_target(
        kind,
        managed_executable,
        verifier_id,
        verifier_version,
        target,
        target,
        true,
    )
}

fn test_policy_with_managed_identity_for_target(
    kind: HardwareQuoteKind,
    managed_executable: Option<&[u8]>,
    verifier_id: &str,
    verifier_version: u32,
    policy_target: &str,
    manifest_target: &str,
    include_release_target_matrix: bool,
) -> TestPolicy {
    let key_bytes = serde_json::to_vec(&test_nvidia_jwks()).unwrap();
    let pcr_bytes = br#"{"schema_version":2,"hash_algorithm":"sha256","pcrs":[0]}"#.to_vec();
    let root_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        include_str!("../../mayhem-attestation/tests/fixtures/tpm-root.der.b64").trim(),
    )
    .unwrap();
    let workload_measurement_bytes =
        br#"{"schema_version":1,"layers":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#
            .to_vec();
    let cpu_measurement_bytes =
        br#"{"schema_version":1,"layers":{"cpu":{"platform":"test-cpu-root"}}}"#.to_vec();
    let gpu_measurement_bytes =
        br#"{"schema_version":1,"layers":{"gpu":{"platform":"test-gpu-root"}}}"#.to_vec();

    let mut trust_data = Vec::new();
    let mut required_trust_data = BTreeSet::new();
    let mut measurement_trust_data = BTreeSet::new();
    let mut enclave_measurement_trust_data = BTreeMap::new();
    let mut collateral_bytes = Vec::new();
    if kind == HardwareQuoteKind::Tpm2QuoteEk {
        for (id, trust_kind, media_type, bytes) in [
            (
                "tpm-root",
                AttestationTrustDataKind::TrustAnchor,
                "application/pkix-cert",
                root_bytes,
            ),
            (
                "tpm-pcrs",
                AttestationTrustDataKind::Measurement,
                "application/vnd.mayhem.tpm-pcr-policy+json",
                pcr_bytes,
            ),
        ] {
            let digest = hex::encode(Sha256::digest(&bytes));
            trust_data.push(AttestationTrustDataRef {
                id: id.to_owned(),
                kind: trust_kind,
                sha256: digest,
                media_type: media_type.to_owned(),
                max_bytes: bytes.len() as u64,
                valid_from_epoch: Some(1),
                valid_until_epoch: Some(100),
                source: None,
            });
            required_trust_data.insert(id.to_owned());
            if trust_kind == AttestationTrustDataKind::Measurement {
                measurement_trust_data.insert(id.to_owned());
            }
            collateral_bytes.push((id.to_owned(), bytes));
        }
    } else {
        let digest = hex::encode(Sha256::digest(&key_bytes));
        trust_data.push(AttestationTrustDataRef {
            id: "vendor-jwks".to_owned(),
            kind: AttestationTrustDataKind::VerificationKey,
            sha256: digest,
            media_type: "application/jwk-set+json".to_owned(),
            max_bytes: key_bytes.len() as u64,
            valid_from_epoch: Some(1),
            valid_until_epoch: Some(100),
            source: None,
        });
        required_trust_data.insert("vendor-jwks".to_owned());
        collateral_bytes.push(("vendor-jwks".to_owned(), key_bytes));
        let mut tier3_measurements = Vec::new();
        if matches!(
            kind,
            HardwareQuoteKind::AmdSevSnpVcek
                | HardwareQuoteKind::IntelTdxDcap
                | HardwareQuoteKind::NvidiaNrasJwt
                | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
        ) {
            tier3_measurements.push((
                AttestationMeasurementLayer::Cpu,
                "golden-cpu",
                cpu_measurement_bytes,
            ));
        }
        if matches!(
            kind,
            HardwareQuoteKind::NvidiaNrasJwt | HardwareQuoteKind::NvidiaNvtrustOfflineJwt
        ) {
            tier3_measurements.push((
                AttestationMeasurementLayer::Gpu,
                "golden-gpu",
                gpu_measurement_bytes,
            ));
        }
        if kind.attestation_tier() >= TIER3_CONFIDENTIAL_COMPUTE_TIER {
            tier3_measurements.push((
                AttestationMeasurementLayer::Workload,
                "golden-workload",
                workload_measurement_bytes,
            ));
        }
        for (layer, id, bytes) in tier3_measurements {
            let digest = hex::encode(Sha256::digest(&bytes));
            trust_data.push(AttestationTrustDataRef {
                id: id.to_owned(),
                kind: AttestationTrustDataKind::Measurement,
                sha256: digest,
                media_type: "application/vnd.mayhem.launch-measurements+json".to_owned(),
                max_bytes: bytes.len() as u64,
                valid_from_epoch: Some(1),
                valid_until_epoch: Some(100),
                source: None,
            });
            required_trust_data.insert(id.to_owned());
            measurement_trust_data.insert(id.to_owned());
            enclave_measurement_trust_data.insert(layer, id.to_owned());
            collateral_bytes.push((id.to_owned(), bytes));
        }
    }
    if let Some(executable) = managed_executable {
        let executable = executable.to_vec();
        let profiles = BTreeMap::from([(verifier_profile(kind), BTreeSet::from([1_u32]))]);
        let targets = if include_release_target_matrix {
            mayhem_gateway::MANAGED_VERIFIER_TARGETS.to_vec()
        } else {
            vec![policy_target]
        };
        for target in targets {
            let target_executable = if target == policy_target {
                executable.clone()
            } else {
                format!("foreign managed verifier fixture for {target}").into_bytes()
            };
            let executable_sha256 = hex::encode(Sha256::digest(&target_executable));
            let target_manifest = if target == policy_target {
                manifest_target
            } else {
                target
            };
            let manifest = serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "target": target_manifest,
                "verifier_id": verifier_id,
                "version": verifier_version,
                "executable_sha256": executable_sha256,
                "profiles": profiles
            }))
            .unwrap();
            for (id, media_type, bytes) in [
                (
                    mayhem_gateway::managed_verifier_executable_trust_data_id(kind, target),
                    mayhem_gateway::MANAGED_VERIFIER_EXECUTABLE_MEDIA_TYPE,
                    target_executable,
                ),
                (
                    mayhem_gateway::managed_verifier_manifest_trust_data_id(kind, target),
                    mayhem_gateway::MANAGED_VERIFIER_MANIFEST_MEDIA_TYPE,
                    manifest,
                ),
            ] {
                let digest = hex::encode(Sha256::digest(&bytes));
                trust_data.push(AttestationTrustDataRef {
                    id: id.clone(),
                    kind: AttestationTrustDataKind::VerificationKey,
                    sha256: digest,
                    media_type: media_type.to_owned(),
                    max_bytes: bytes.len() as u64,
                    valid_from_epoch: Some(1),
                    valid_until_epoch: Some(100),
                    source: None,
                });
                if target == policy_target {
                    collateral_bytes.push((id, bytes));
                }
            }
        }
    }

    let policy = AdminAttestationPolicy {
        schema_version: ATTESTATION_POLICY_SCHEMA_VERSION,
        sequence: 1,
        previous_policy_digest: None,
        issued_epoch: 1,
        effective_epoch: 1,
        expires_epoch: None,
        min_verifier_version: 3,
        emergency_disabled_quote_kinds: BTreeSet::new(),
        origin_pins: Vec::new(),
        trust_data,
        quote_kinds: HardwareQuoteKind::ALL
            .into_iter()
            .map(|candidate| AttestationQuoteKindPolicy {
                kind: candidate,
                enabled: candidate == kind,
                verifier_profile: verifier_profile(candidate),
                evidence_schema_version: 1,
                required_trust_data: if candidate == kind {
                    required_trust_data.clone()
                } else {
                    BTreeSet::new()
                },
                measurement_trust_data: if candidate == kind {
                    measurement_trust_data.clone()
                } else {
                    BTreeSet::new()
                },
                platforms: if candidate == kind
                    && (candidate == HardwareQuoteKind::Tpm2QuoteEk
                        || candidate.attestation_tier() >= TIER3_CONFIDENTIAL_COMPUTE_TIER)
                {
                    BTreeSet::from(["test-platform".to_owned()])
                } else {
                    BTreeSet::new()
                },
                required_measurement_layers: if candidate == kind
                    && candidate.attestation_tier() >= TIER3_CONFIDENTIAL_COMPUTE_TIER
                {
                    enclave_measurement_trust_data.keys().copied().collect()
                } else {
                    BTreeSet::new()
                },
            })
            .collect(),
    };
    let validated = ValidatedAttestationPolicy::validate(policy).unwrap();
    let enclave_id = test_enclave_id();
    let platform = if kind == HardwareQuoteKind::Tpm2QuoteEk
        || kind.attestation_tier() >= TIER3_CONFIDENTIAL_COMPUTE_TIER
    {
        Some("test-platform".to_owned())
    } else {
        None
    };
    let route_binding = HardwareQuoteRoutePolicyBinding {
        enclave_id: enclave_id.clone(),
        device_id: TEST_DEVICE_ID.to_owned(),
        kind,
        evidence_schema_version: if kind == HardwareQuoteKind::Tpm2QuoteEk {
            TPM_QUOTE_EVIDENCE_SCHEMA_VERSION
        } else {
            1
        },
        policy_sequence: validated.policy().sequence,
        policy_digest: validated.digest().to_owned(),
        platform: platform.clone(),
    };
    let mut collateral = CollateralInventory::new();
    for (id, bytes) in collateral_bytes {
        collateral
            .insert(&validated, &id, bytes, TEST_POLICY_EPOCH)
            .unwrap();
    }
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated).unwrap();
    chain
        .configure_enclave_bindings(vec![AdminEnclaveAttestationBinding {
            enclave_id,
            kind,
            platform,
            measurement_trust_data: enclave_measurement_trust_data,
        }])
        .unwrap();
    TestPolicy {
        chain,
        collateral,
        route_binding,
        device_id: TEST_DEVICE_ID.to_owned(),
    }
}

fn policy_evidence_nonce(
    kind: HardwareQuoteKind,
    body: &AttestationBody,
    quote_binding: &str,
) -> String {
    let policy = test_policy(kind);
    let binding = EvidenceBinding::new(
        &policy.route_binding,
        &body.nonce_u,
        &body.enclave_id,
        &policy.device_id,
        quote_binding,
    )
    .unwrap();
    hex::encode(binding.digest().unwrap())
}

fn policy_request<'a>(
    report: &'a mayhem_proto::AttestationReport,
    contract: &'a EnclaveContractRecord,
    expected_nonce: &'a str,
    expected_provider_pubkey: &'a str,
    now_ts: u64,
) -> AttestationVerificationRequest<'a> {
    let mut request = AttestationVerificationRequest::new(
        report,
        contract,
        expected_nonce,
        expected_provider_pubkey,
        now_ts,
    );
    if let Some(quote) = report.hw_quote.as_ref() {
        let policy = Box::leak(Box::new(test_policy(quote.kind)));
        let context = Box::leak(Box::new(policy.context()));
        request.attestation_policy = Some(context);
    }
    request
}

fn native_tpm_policy(
    root_der: &[u8],
    pcr_policy: &[u8],
    enclave_id: String,
    device_id: String,
) -> TestPolicy {
    let trust_data = [
        (
            "native-tpm-root",
            AttestationTrustDataKind::TrustAnchor,
            "application/pkix-cert",
            root_der,
        ),
        (
            "native-tpm-pcrs",
            AttestationTrustDataKind::Measurement,
            "application/vnd.mayhem.tpm-pcr-policy+json",
            pcr_policy,
        ),
    ]
    .into_iter()
    .map(|(id, kind, media_type, bytes)| AttestationTrustDataRef {
        id: id.to_owned(),
        kind,
        sha256: hex::encode(Sha256::digest(bytes)),
        media_type: media_type.to_owned(),
        max_bytes: bytes.len() as u64,
        valid_from_epoch: Some(1),
        valid_until_epoch: Some(100),
        source: None,
    })
    .collect::<Vec<_>>();
    let required = BTreeSet::from(["native-tpm-root".to_owned(), "native-tpm-pcrs".to_owned()]);
    let policy = AdminAttestationPolicy {
        schema_version: ATTESTATION_POLICY_SCHEMA_VERSION,
        sequence: 1,
        previous_policy_digest: None,
        issued_epoch: 1,
        effective_epoch: 1,
        expires_epoch: None,
        min_verifier_version: GATEWAY_ATTESTATION_VERIFIER_VERSION,
        emergency_disabled_quote_kinds: BTreeSet::new(),
        origin_pins: Vec::new(),
        trust_data,
        quote_kinds: HardwareQuoteKind::ALL
            .into_iter()
            .map(|kind| AttestationQuoteKindPolicy {
                kind,
                enabled: kind == HardwareQuoteKind::Tpm2QuoteEk,
                verifier_profile: verifier_profile(kind),
                evidence_schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
                required_trust_data: if kind == HardwareQuoteKind::Tpm2QuoteEk {
                    required.clone()
                } else {
                    BTreeSet::new()
                },
                measurement_trust_data: if kind == HardwareQuoteKind::Tpm2QuoteEk {
                    BTreeSet::from(["native-tpm-pcrs".to_owned()])
                } else {
                    BTreeSet::new()
                },
                platforms: if kind == HardwareQuoteKind::Tpm2QuoteEk {
                    BTreeSet::from(["test-tpm2".to_owned()])
                } else {
                    BTreeSet::new()
                },
                required_measurement_layers: BTreeSet::new(),
            })
            .collect(),
    };
    let validated = ValidatedAttestationPolicy::validate(policy).unwrap();
    let route_binding = HardwareQuoteRoutePolicyBinding {
        enclave_id: enclave_id.clone(),
        device_id: device_id.clone(),
        kind: HardwareQuoteKind::Tpm2QuoteEk,
        evidence_schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
        policy_sequence: 1,
        policy_digest: validated.digest().to_owned(),
        platform: Some("test-tpm2".to_owned()),
    };
    let mut collateral = CollateralInventory::new();
    collateral
        .insert(&validated, "native-tpm-root", root_der, TEST_POLICY_EPOCH)
        .unwrap();
    collateral
        .insert(&validated, "native-tpm-pcrs", pcr_policy, TEST_POLICY_EPOCH)
        .unwrap();
    let mut chain = AttestationPolicyChain::new();
    chain.append(validated).unwrap();
    chain
        .configure_enclave_bindings(vec![AdminEnclaveAttestationBinding {
            enclave_id,
            kind: HardwareQuoteKind::Tpm2QuoteEk,
            platform: Some("test-tpm2".to_owned()),
            measurement_trust_data: BTreeMap::new(),
        }])
        .unwrap();
    TestPolicy {
        chain,
        collateral,
        route_binding,
        device_id,
    }
}

struct NativeTpmFixture {
    _temp: tempfile::TempDir,
    report: mayhem_proto::AttestationReport,
    contract: EnclaveContractRecord,
    policy: TestPolicy,
    activated: ActivatedTpmIdentity,
    catalog_policy: AdminAttestationPolicy,
    catalog_binding: AdminEnclaveAttestationBinding,
    root_der: Vec<u8>,
    pcr_policy: Vec<u8>,
}

fn native_tpm_fixture() -> NativeTpmFixture {
    const NOW: u64 = 1_800_000_000;
    let mut key_rng = ChaCha20Rng::from_seed([61; 32]);
    let root_key = RsaPrivateKey::new(&mut key_rng, 2048).unwrap();
    let ek_key = RsaPrivateKey::new(&mut key_rng, 2048).unwrap();
    let ak_key = RsaPrivateKey::new(&mut key_rng, 2048).unwrap();
    let root_name = x509_name("Mayhem test TPM root");
    let leaf_name = x509_name("Mayhem test TPM EK");
    let root_der = test_certificate(
        1,
        &root_name,
        &root_name,
        &RsaPublicKey::from(&root_key),
        &root_key,
        true,
    );
    let leaf_der = test_certificate(
        2,
        &root_name,
        &leaf_name,
        &RsaPublicKey::from(&ek_key),
        &root_key,
        false,
    );
    let device_id = hex::encode(Sha256::digest(&leaf_der));
    let pcr_value = "a0".repeat(32);
    let pcr_policy = serde_json::to_vec(&serde_json::json!({
        "schema_version": TPM_PCR_POLICY_SCHEMA_VERSION,
        "hash_algorithm": "sha256",
        "pcrs": [0]
    }))
    .unwrap();

    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("native-tpm-enclave");
    fs::write(&binary, b"native TPM measured runtime").unwrap();
    let binary_hash = measure_binary(&binary).unwrap();
    let identity = CatalogEnclaveIdentity {
        admin_pubkey: "admin-key".to_owned(),
        model_id: "mayhem/qwen3.5-4b@q4".to_owned(),
        artifact_root: "artifact-root-v1".to_owned(),
        artifact_sidecar_roots: BTreeMap::new(),
        manifest_hash: "manifest-hash-v1".to_owned(),
        binary_hash,
    };
    let runtime_keypair = RuntimeKeypair::from_seed([9; 32]);
    let provider_signing_key = SigningKey::from_bytes(&[7; 32]);
    let body = AttestationBody {
        schema_version: ATTESTATION_SCHEMA_VERSION,
        alg: ATTESTATION_ALG.to_owned(),
        enclave_id: catalog_enclave_id(&identity),
        enclave_pubkey: runtime_keypair.public_key_hex(),
        provider_pubkey: hex::encode(provider_signing_key.verifying_key().to_bytes()),
        manifest_hash: identity.manifest_hash.clone(),
        binary_hash: identity.binary_hash.clone(),
        att_tier: TIER2_DEVICE_IDENTITY_TIER,
        hw_quote: None,
        boot_epoch: 100,
        report_ts: NOW,
        nonce_u: "aa".repeat(32),
        runtime_config: AttestationRuntimeConfig::default(),
    };
    let policy = native_tpm_policy(&root_der, &pcr_policy, body.enclave_id.clone(), device_id);
    let catalog_policy = policy
        .chain
        .active_at(TEST_POLICY_EPOCH)
        .expect("native TPM policy is active")
        .policy()
        .clone();
    let catalog_binding = AdminEnclaveAttestationBinding {
        enclave_id: body.enclave_id.clone(),
        kind: HardwareQuoteKind::Tpm2QuoteEk,
        platform: Some("test-tpm2".to_owned()),
        measurement_trust_data: BTreeMap::new(),
    };
    let quote_binding = hardware_quote_binding(&body).unwrap();
    let evidence_binding = EvidenceBinding::new(
        &policy.route_binding,
        &body.nonce_u,
        &body.enclave_id,
        &policy.device_id,
        &quote_binding,
    )
    .unwrap();
    let ak_public = make_rsa_ak_public(&RsaPublicKey::from(&ak_key));
    let ak_name = tpm_name(&ak_public);
    let ek_spki = RsaPublicKey::from(&ek_key).to_public_key_der().unwrap();
    let hello = TpmActivateCredentialHello {
        schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
        ek_profile: TpmEkProfile::RsaSha256Aes128Cfb,
        ek_public_spki_der_b64: BASE64.encode(ek_spki.as_bytes()),
        ak_name_b64: BASE64.encode(&ak_name),
        quote_binding: quote_binding.clone(),
    };
    let mut challenge_rng = ChaCha20Rng::from_seed([62; 32]);
    let (challenge, pending) =
        issue_tpm_activate_credential_challenge_with_rng(&hello, NOW, 30, &mut challenge_rng)
            .unwrap();
    let activated_secret = activate_rsa_credential(&ek_key, &ak_name, &challenge);
    let activated = pending
        .complete(
            &TpmActivateCredentialResponse {
                schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
                challenge_id: challenge.challenge_id,
                ak_name_b64: challenge.ak_name_b64,
                quote_binding: challenge.quote_binding,
                activated_secret_b64: BASE64.encode(activated_secret),
            },
            NOW + 1,
        )
        .unwrap();

    let pcr_values = vec![TpmPcrValue {
        hash_algorithm: TpmHashAlgorithm::Sha256,
        index: 0,
        digest: pcr_value,
    }];
    let pcr_digest = Sha256::digest(hex::decode(&pcr_values[0].digest).unwrap());
    let quote_attest =
        make_tpm_quote_attest(&ak_name, &evidence_binding.digest().unwrap(), &pcr_digest);
    let evidence = TpmQuoteEvidence {
        schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
        ak_public_b64: BASE64.encode(ak_public),
        ak_name_b64: BASE64.encode(ak_name),
        quote_attest_b64: BASE64.encode(&quote_attest),
        quote_signature_b64: BASE64.encode(sign_tpm_quote(&ak_key, &quote_attest)),
        pcr_values,
    };
    let attestation = build_hardware_attestation_report(&HardwareAttestationOptions {
        identity: identity.clone(),
        runtime_keypair,
        provider_signing_seed: [7; 32],
        binary_path: binary,
        boot_epoch: body.boot_epoch,
        report_ts: body.report_ts,
        nonce_u: body.nonce_u.clone(),
        hw_quote: HardwareQuote {
            kind: HardwareQuoteKind::Tpm2QuoteEk,
            evidence: serde_json::to_string(&evidence).unwrap(),
            binding: quote_binding,
            endorsements: vec![BASE64.encode(leaf_der)],
            metadata: serde_json::Value::Null,
        },
        runtime_config: body.runtime_config,
    })
    .unwrap();
    let contract = EnclaveContractRecord {
        enclave_id: body.enclave_id,
        admin_pubkey: identity.admin_pubkey,
        model_id: identity.model_id,
        model_class: DEFAULT_MODEL_CLASS.to_owned(),
        artifact_root: identity.artifact_root,
        artifact_sidecar_roots: BTreeMap::new(),
        manifest_hash: identity.manifest_hash,
        binary_hash: "approved hashes are not runtime admission".to_owned(),
        launch_measurements: serde_json::Value::Null,
        att_tier: TIER2_DEVICE_IDENTITY_TIER,
        caps: serde_json::json!({}),
    };
    NativeTpmFixture {
        _temp: temp,
        report: attestation.report,
        contract,
        policy,
        activated,
        catalog_policy,
        catalog_binding,
        root_der,
        pcr_policy,
    }
}

fn make_rsa_ak_public(key: &RsaPublicKey) -> Vec<u8> {
    const REQUIRED_ATTRIBUTES: u32 =
        0x0000_0002 | 0x0000_0010 | 0x0000_0020 | 0x0001_0000 | 0x0004_0000;
    let mut public = Vec::new();
    public.extend_from_slice(&0x0001_u16.to_be_bytes());
    public.extend_from_slice(&0x000b_u16.to_be_bytes());
    public.extend_from_slice(&REQUIRED_ATTRIBUTES.to_be_bytes());
    public.extend_from_slice(&0_u16.to_be_bytes());
    public.extend_from_slice(&0x0010_u16.to_be_bytes());
    public.extend_from_slice(&0x0014_u16.to_be_bytes());
    public.extend_from_slice(&0x000b_u16.to_be_bytes());
    public.extend_from_slice(&(key.n().bits() as u16).to_be_bytes());
    public.extend_from_slice(&0_u32.to_be_bytes());
    public.extend(tpm2b(&key.n().to_bytes_be()));
    tpm2b(&public)
}

fn tpm_name(public: &[u8]) -> Vec<u8> {
    let mut name = 0x000b_u16.to_be_bytes().to_vec();
    name.extend_from_slice(&Sha256::digest(&public[2..]));
    name
}

fn make_tpm_quote_attest(ak_name: &[u8], extra_data: &[u8], pcr_digest: &[u8]) -> Vec<u8> {
    let mut attest = Vec::new();
    attest.extend_from_slice(&0xff54_4347_u32.to_be_bytes());
    attest.extend_from_slice(&0x8018_u16.to_be_bytes());
    attest.extend(tpm2b(ak_name));
    attest.extend(tpm2b(extra_data));
    attest.extend_from_slice(&0_u64.to_be_bytes());
    attest.extend_from_slice(&0_u32.to_be_bytes());
    attest.extend_from_slice(&0_u32.to_be_bytes());
    attest.push(1);
    attest.extend_from_slice(&1_u64.to_be_bytes());
    attest.extend_from_slice(&1_u32.to_be_bytes());
    attest.extend_from_slice(&0x000b_u16.to_be_bytes());
    attest.push(3);
    attest.extend_from_slice(&[1, 0, 0]);
    attest.extend(tpm2b(pcr_digest));
    attest
}

fn sign_tpm_quote(key: &RsaPrivateKey, attest: &[u8]) -> Vec<u8> {
    let signature = RsaSigningKey::<Sha256>::new(key.clone()).sign(attest);
    let mut encoded = 0x0014_u16.to_be_bytes().to_vec();
    encoded.extend_from_slice(&0x000b_u16.to_be_bytes());
    encoded.extend(tpm2b(&signature.to_vec()));
    encoded
}

fn activate_rsa_credential(
    ek: &RsaPrivateKey,
    ak_name: &[u8],
    challenge: &mayhem_proto::TpmActivateCredentialChallenge,
) -> Vec<u8> {
    let encrypted_secret_blob = BASE64.decode(&challenge.encrypted_secret_b64).unwrap();
    let encrypted_secret = unwrap_tpm2b(&encrypted_secret_blob);
    let seed = ek
        .decrypt(
            Oaep::new_with_label::<Sha256, _>("IDENTITY\0"),
            encrypted_secret,
        )
        .unwrap();
    let blob = BASE64.decode(&challenge.credential_blob_b64).unwrap();
    let object = unwrap_tpm2b(&blob);
    let mut offset = 0;
    let integrity = take_tpm2b(object, &mut offset);
    let encrypted_identity = take_tpm2b(object, &mut offset);
    assert_eq!(offset, object.len());
    let integrity_key = kdfa_sha256(&seed, b"INTEGRITY", &[], &[], 256);
    let mut hmac = Hmac::<Sha256>::new_from_slice(&integrity_key).unwrap();
    hmac.update(encrypted_identity);
    hmac.update(ak_name);
    hmac.verify_slice(integrity).unwrap();
    let storage_key = kdfa_sha256(&seed, b"STORAGE", ak_name, &[], 128);
    let mut credential = encrypted_identity.to_vec();
    Decryptor::<Aes128>::new_from_slices(&storage_key, &[0; 16])
        .unwrap()
        .decrypt(&mut credential);
    unwrap_tpm2b(&credential).to_vec()
}

fn kdfa_sha256(
    key: &[u8],
    label: &[u8],
    context_u: &[u8],
    context_v: &[u8],
    bits: usize,
) -> Vec<u8> {
    let mut output = Vec::new();
    let mut counter = 1_u32;
    while output.len() < bits.div_ceil(8) {
        let mut hmac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        hmac.update(&counter.to_be_bytes());
        hmac.update(label);
        hmac.update(&[0]);
        hmac.update(context_u);
        hmac.update(context_v);
        hmac.update(&(bits as u32).to_be_bytes());
        output.extend_from_slice(&hmac.finalize().into_bytes());
        counter += 1;
    }
    output.truncate(bits.div_ceil(8));
    output
}

fn unwrap_tpm2b(encoded: &[u8]) -> &[u8] {
    let mut offset = 0;
    let value = take_tpm2b(encoded, &mut offset);
    assert_eq!(offset, encoded.len());
    value
}

fn take_tpm2b<'a>(encoded: &'a [u8], offset: &mut usize) -> &'a [u8] {
    let length = u16::from_be_bytes([encoded[*offset], encoded[*offset + 1]]) as usize;
    *offset += 2;
    let value = &encoded[*offset..*offset + length];
    *offset += length;
    value
}

fn tpm2b(value: &[u8]) -> Vec<u8> {
    let mut encoded = (value.len() as u16).to_be_bytes().to_vec();
    encoded.extend_from_slice(value);
    encoded
}

fn test_certificate(
    serial: u8,
    issuer: &[u8],
    subject: &[u8],
    public_key: &RsaPublicKey,
    signing_key: &RsaPrivateKey,
    is_ca: bool,
) -> Vec<u8> {
    let signature_algorithm = der_sequence(&[
        der(
            0x06,
            &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b],
        ),
        der(0x05, &[]),
    ]);
    let validity = der_sequence(&[der(0x18, b"20200101000000Z"), der(0x18, b"20490101000000Z")]);
    let basic_constraints = if is_ca {
        der_sequence(&[der(0x01, &[0xff])])
    } else {
        der_sequence(&[])
    };
    let key_usage = if is_ca {
        der(0x03, &[2, 0x04])
    } else {
        der(0x03, &[5, 0x20])
    };
    let mut extensions = vec![
        x509_extension(&[0x55, 0x1d, 0x13], true, &basic_constraints),
        x509_extension(&[0x55, 0x1d, 0x0f], true, &key_usage),
    ];
    if !is_ca {
        extensions.push(x509_extension(
            &[0x55, 0x1d, 0x25],
            false,
            &der_sequence(&[der(0x06, &[0x67, 0x81, 0x05, 0x08, 0x01])]),
        ));
    }
    let tbs = der_sequence(&[
        der(0xa0, &der(0x02, &[2])),
        der(0x02, &[serial]),
        signature_algorithm.clone(),
        issuer.to_vec(),
        validity,
        subject.to_vec(),
        public_key.to_public_key_der().unwrap().as_bytes().to_vec(),
        der(0xa3, &der_sequence(&extensions)),
    ]);
    let signature = RsaSigningKey::<Sha256>::new(signing_key.clone()).sign(&tbs);
    let mut signature_bits = vec![0];
    signature_bits.extend_from_slice(&signature.to_vec());
    der_sequence(&[tbs, signature_algorithm, der(0x03, &signature_bits)])
}

fn x509_name(common_name: &str) -> Vec<u8> {
    der_sequence(&[der(
        0x31,
        &der_sequence(&[
            der(0x06, &[0x55, 0x04, 0x03]),
            der(0x0c, common_name.as_bytes()),
        ]),
    )])
}

fn x509_extension(oid: &[u8], critical: bool, value: &[u8]) -> Vec<u8> {
    let mut fields = vec![der(0x06, oid)];
    if critical {
        fields.push(der(0x01, &[0xff]));
    }
    fields.push(der(0x04, value));
    der_sequence(&fields)
}

fn der_sequence(elements: &[Vec<u8>]) -> Vec<u8> {
    der(0x30, &elements.concat())
}

fn der(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut encoded = vec![tag];
    if value.len() < 128 {
        encoded.push(value.len() as u8);
    } else {
        let bytes = (value.len() as u64).to_be_bytes();
        let first = bytes.iter().position(|byte| *byte != 0).unwrap();
        encoded.push(0x80 | (bytes.len() - first) as u8);
        encoded.extend_from_slice(&bytes[first..]);
    }
    encoded.extend_from_slice(value);
    encoded
}

fn test_nvidia_signed_eat(claims: serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::ES384);
    header.kid = Some(TEST_NVIDIA_NRAS_KID.to_owned());
    encode(
        &header,
        &claims,
        &EncodingKey::from_ec_pem(TEST_NVIDIA_NRAS_PRIVATE_KEY_PEM.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn test_nvidia_evidence(binding: &str, gpu_measurement_success: bool) -> String {
    let exp = 4_102_444_800_u64;
    let overall = test_nvidia_signed_eat(serde_json::json!({
        "iss": "https://nras.attestation.nvidia.com",
        "sub": "NVIDIA-PLATFORM-ATTESTATION",
        "exp": exp,
        "eat_nonce": binding,
        "x-nvidia-overall-att-result": true
    }));
    let gpu = test_nvidia_signed_eat(serde_json::json!({
        "iss": "https://nras.attestation.nvidia.com",
        "sub": "NVIDIA-GPU-ATTESTATION",
        "exp": exp,
        "eat_nonce": binding,
        "x-nvidia-device-type": "gpu",
        "measres": if gpu_measurement_success { "Success" } else { "Failure" },
        "secboot": true,
        "dbgstat": "disabled",
        "hwmodel": "GH100 A01 GSP BROM",
        "ueid": "test-ueid",
        "oemid": "5703",
        "x-nvidia-gpu-driver-version": "575.32",
        "x-nvidia-gpu-vbios-version": "96.00.AF.00.01",
        "x-nvidia-gpu-arch-check": true,
        "x-nvidia-gpu-attestation-report-cert-chain-fwid-match": true,
        "x-nvidia-gpu-attestation-report-parsed": true,
        "x-nvidia-gpu-attestation-report-nonce-match": true,
        "x-nvidia-gpu-attestation-report-signature-verified": true,
        "x-nvidia-gpu-driver-rim-fetched": true,
        "x-nvidia-gpu-driver-rim-schema-validated": true,
        "x-nvidia-gpu-driver-rim-signature-verified": true,
        "x-nvidia-gpu-driver-rim-version-match": true,
        "x-nvidia-gpu-driver-rim-measurements-available": true,
        "x-nvidia-gpu-vbios-rim-fetched": true,
        "x-nvidia-gpu-vbios-rim-schema-validated": true,
        "x-nvidia-gpu-vbios-rim-version-match": true,
        "x-nvidia-gpu-vbios-rim-signature-verified": true,
        "x-nvidia-gpu-vbios-rim-measurements-available": true,
        "x-nvidia-gpu-vbios-index-no-conflict": true
    }));
    serde_json::json!({
        "detached_eat": [
            ["JWT", overall],
            { "GPU-0": gpu }
        ]
    })
    .to_string()
}

fn test_nvidia_offline_evidence(binding: &str, measurements_match: bool) -> String {
    let exp = 4_102_444_800_u64;
    let overall = test_nvidia_signed_eat(serde_json::json!({
        "iss": "https://local.verifier.attestation.nvidia.com",
        "sub": "NVIDIA-LOCAL-VERIFIER-ATTESTATION",
        "exp": exp,
        "eat_nonce": binding,
        "x-nvidia-overall-att-result": true
    }));
    let gpu = test_nvidia_signed_eat(serde_json::json!({
        "iss": "https://local.verifier.attestation.nvidia.com",
        "sub": "NVIDIA-GPU-LOCAL-VERIFIER",
        "exp": exp,
        "eat_nonce": binding,
        "x-nv-gpu-cert-chain-verified": true,
        "x-nv-gpu-cert-check-complete": true,
        "x-nv-gpu-measurement-available": true,
        "x-nv-gpu-root-cert-available": true,
        "x-nv-gpu-info-fetched": true,
        "x-nv-gpu-available": true,
        "x-nv-gpu-attestation-report-available": true,
        "x-nv-gpu-attestation-report-driver-version-match": true,
        "x-nv-gpu-attestation-report-vbios-version-match": true,
        "x-nv-gpu-attestation-report-verified": true,
        "x-nv-gpu-driver-rim-schema-fetched": true,
        "x-nv-gpu-driver-rim-cert-extracted": true,
        "x-nv-gpu-vbios-rim-cert-extracted": true,
        "x-nv-gpu-vbios-rim-driver-measurements-available": true,
        "x-nv-gpu-driver-rim-driver-measurements-available": true,
        "x-nvidia-gpu-arch-check": true,
        "x-nvidia-gpu-driver-rim-signature-verified": true,
        "x-nvidia-gpu-vbios-rim-signature-verified": true,
        "x-nvidia-gpu-attestation-report-parsed": true,
        "x-nv-gpu-nonce-match": true,
        "x-nv-gpu-measurements-match": measurements_match
    }));
    serde_json::json!({
        "detached_eat": [
            ["JWT", overall],
            { "GPU-0": gpu }
        ]
    })
    .to_string()
}

fn test_apple_app_attest_evidence(body: &AttestationBody, binding: &str) -> String {
    test_apple_app_attest_evidence_with_binding_claim(body, binding)
}

fn test_apple_app_attest_evidence_with_binding_claim(
    body: &AttestationBody,
    binding_claim: &str,
) -> String {
    test_nvidia_signed_eat(serde_json::json!({
        "iss": "https://appattest.apple.com",
        "sub": "APPLE-APP-ATTEST",
        "exp": 4_102_444_800_u64,
        "eat_nonce": binding_claim,
        "x-mayhem-attestation-mechanism": "apple_app_attest",
        "x-mayhem-enclave-id": body.enclave_id.clone(),
        "x-mayhem-binary-hash": body.binary_hash.clone(),
        "x-mayhem-prompt-confidentiality": false,
        "x-apple-app-attest-root-verified": true,
        "x-apple-app-attest-app-id-bound": true,
        "x-apple-app-attest-client-hash-bound": true,
        "x-apple-app-attest-signature-verified": true,
        "x-apple-app-attest-counter-valid": true
    }))
}

fn test_nvidia_gb10_device_evidence(body: &AttestationBody, binding: &str, model: &str) -> String {
    test_nvidia_signed_eat(serde_json::json!({
        "iss": "https://nras.attestation.nvidia.com",
        "sub": "NVIDIA-GB10-DEVICE-ATTESTATION",
        "exp": 4_102_444_800_u64,
        "eat_nonce": binding,
        "x-mayhem-enclave-id": body.enclave_id.clone(),
        "x-mayhem-binary-hash": body.binary_hash.clone(),
        "x-nvidia-device-type": "gpu",
        "hwmodel": model,
        "x-nvidia-gpu-attestation-report-cert-chain-validated": true,
        "x-nvidia-gpu-attestation-report-parsed": true,
        "x-nvidia-gpu-attestation-report-signature-verified": true,
        "x-nvidia-gpu-driver-rim-signature-verified": true,
        "x-nvidia-gpu-vbios-rim-signature-verified": true,
        "x-nvidia-gpu-driver-rim-measurements-available": true,
        "x-nvidia-gpu-vbios-rim-measurements-available": true,
        "x-nvidia-gpu-nonce-match": true,
        "x-nvidia-gpu-arch-check": true
    }))
}

#[test]
fn verifies_signed_tier1_report() {
    let (_temp, report, contract) = test_report();
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let verified = verify_attestation(&request).expect("valid report verifies");

    assert_eq!(verified.enclave_id, contract.enclave_id);
    assert_eq!(verified.provider_pubkey, report.provider_pubkey);
    assert_eq!(verified.enclave_pubkey, report.enclave_pubkey);
    assert_eq!(verified.runtime_binary_hash, report.binary_hash);
    assert!(!verified.report_head.is_empty());
}

#[test]
fn verification_rejects_wrong_provider_pubkey_on_default_path() {
    let (_temp, report, contract) = test_report();
    let wrong_provider = "dd".repeat(32);
    let request = policy_request(&report, &contract, &report.nonce_u, &wrong_provider, 210);

    let err = verify_attestation(&request).expect_err("provider binding must match registered key");

    assert!(matches!(
        err,
        GatewayError::ContractMismatch {
            field: "provider_pubkey",
            ..
        }
    ));
}

#[test]
fn verification_rejects_runtime_tp_degree_mismatch() {
    let (_temp, report, mut contract) = test_report();
    contract.caps = serde_json::json!({ "tp_degree": 2 });
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("tp_degree must match admin caps");

    assert!(matches!(
        err,
        GatewayError::ContractMismatch {
            field: "runtime_config.tp_degree",
            ..
        }
    ));
}

#[test]
fn verification_rejects_runtime_model_class_mismatch() {
    let (_temp, report, mut contract) = test_report();
    contract.model_class = "embedding".to_owned();
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("model_class must match admin record");

    assert!(matches!(
        err,
        GatewayError::ContractMismatch {
            field: "runtime_config.model_class",
            ..
        }
    ));
}

#[test]
fn verifies_apple_app_attest_tier2_identity_with_policy_jwks() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        test_apple_app_attest_evidence,
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let verified =
        verify_tier1_attestation(&request).expect("Apple App Attest Tier 2 identity verifies");

    assert_eq!(verified.att_tier, TIER2_DEVICE_IDENTITY_TIER);
    assert_eq!(verified.enclave_id, contract.enclave_id);
}

#[test]
fn apple_app_attest_rejects_case_variant_binding_claim() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        |body, binding| {
            test_apple_app_attest_evidence_with_binding_claim(body, &binding.to_ascii_uppercase())
        },
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("case-variant binding claim must be rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("eat_nonce")
    ));
}

#[test]
fn apple_app_attest_rejects_directly_supplied_trust_even_with_policy() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        test_apple_app_attest_evidence,
    );
    let jwks = test_nvidia_jwks();
    let mut request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    request.trusted_apple_app_attest_jwks = Some(&jwks);
    let err = verify_tier1_attestation(&request)
        .expect_err("directly supplied verifier trust must be rejected");

    assert!(matches!(
        err,
        GatewayError::ProviderTrustMaterialRejected { kind }
            if kind == "apple_app_attest_jwt"
    ));
}

#[test]
fn hardware_attestation_requires_an_active_policy_context() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        test_apple_app_attest_evidence,
    );
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    assert!(matches!(
        verify_tier1_attestation(&request).unwrap_err(),
        GatewayError::HardwareQuotePolicyRequired { .. }
    ));
}

#[test]
fn hardware_attestation_rejects_policy_binding_mismatch() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        test_apple_app_attest_evidence,
    );
    let mut policy = test_policy(HardwareQuoteKind::AppleAppAttestJwt);
    policy.route_binding.policy_digest = "00".repeat(32);
    let context = policy.context();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("stale or mismatched")
    ));
}

#[test]
fn hardware_attestation_rejects_device_identity_substitution_after_route_binding() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        test_apple_app_attest_evidence,
    );
    let mut policy = test_policy(HardwareQuoteKind::AppleAppAttestJwt);
    policy.route_binding.device_id = "ee".repeat(32);
    let context = policy.context();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("device id does not match the immutable route policy binding")
    ));
}

#[test]
fn hardware_attestation_rejects_stale_policy_after_rollover() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        test_apple_app_attest_evidence,
    );
    let mut policy = test_policy(HardwareQuoteKind::AppleAppAttestJwt);
    let first = policy.chain.head().unwrap();
    let mut second = first.policy().clone();
    second.sequence = 2;
    second.previous_policy_digest = Some(first.digest().to_owned());
    second.issued_epoch = 21;
    second.effective_epoch = 30;
    policy
        .chain
        .append(ValidatedAttestationPolicy::validate(second).unwrap())
        .unwrap();
    let context = AttestationPolicyVerificationContext::new(
        &policy.chain,
        &policy.collateral,
        &policy.route_binding,
        30,
        &policy.device_id,
    );
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("stale or mismatched")
    ));
}

#[test]
fn verifies_nvidia_gb10_tier2_device_identity_with_policy_jwks() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaGb10DeviceJwt,
        |body, binding| test_nvidia_gb10_device_evidence(body, binding, "NVIDIA GB10 DGX Spark"),
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let verified =
        verify_tier1_attestation(&request).expect("NVIDIA GB10 device identity verifies");

    assert_eq!(verified.att_tier, TIER2_DEVICE_IDENTITY_TIER);
    assert_eq!(verified.enclave_id, contract.enclave_id);
}

#[test]
fn nvidia_gb10_tier2_device_identity_rejects_non_gb10_hardware() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaGb10DeviceJwt,
        |body, binding| test_nvidia_gb10_device_evidence(body, binding, "NVIDIA H100"),
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err =
        verify_tier1_attestation(&request).expect_err("non-GB10 device identity must be rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { kind, reason }
            if kind == "nvidia_gb10_device_jwt" && reason.contains("GB10")
    ));
}

#[test]
fn nvidia_nras_tier3_report_requires_authenticated_managed_verifier() {
    let (_temp, report, contract) =
        test_hardware_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |_, binding| {
            test_nvidia_evidence(binding, true)
        });
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("NVIDIA NRAS alone is not enough for Tier-3 prompt confidentiality");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("VerifierProfileUnavailable")
    ));
}

#[test]
fn nvidia_nras_tier3_report_requires_trusted_jwks() {
    let (_temp, report, contract) =
        test_hardware_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |_, binding| {
            test_nvidia_evidence(binding, true)
        });
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request)
        .expect_err("NVIDIA NRAS quotes need admin verifier for Tier 3");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("VerifierProfileUnavailable")
    ));
}

#[test]
fn hardware_quote_kind_must_match_report_tier() {
    let (_temp, mut report, mut contract) =
        test_hardware_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |_, binding| {
            test_nvidia_evidence(binding, true)
        });
    report.att_tier = TIER2_DEVICE_IDENTITY_TIER;
    contract.att_tier = TIER2_DEVICE_IDENTITY_TIER;
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request)
        .expect_err("NVIDIA CC quote must not be accepted as Tier 2 identity");

    assert!(matches!(
        err,
        GatewayError::ContractMismatch {
            field: "hw_quote.att_tier",
            expected,
            actual
        } if expected == TIER3_CONFIDENTIAL_COMPUTE_TIER.to_string()
            && actual == TIER2_DEVICE_IDENTITY_TIER.to_string()
    ));
}

#[test]
fn nvidia_nras_tier3_report_without_admin_verifier_fails_before_appraisal() {
    let (_temp, report, contract) =
        test_hardware_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |_, binding| {
            test_nvidia_evidence(binding, false)
        });
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("signed failed NVIDIA appraisal must be rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("VerifierProfileUnavailable")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_requires_authenticated_managed_verifier() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |_, binding| test_nvidia_offline_evidence(binding, true),
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("offline NVIDIA token alone is not enough for Tier-3 prompt confidentiality");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("VerifierProfileUnavailable")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_requires_trusted_jwks() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |_, binding| test_nvidia_offline_evidence(binding, true),
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request)
        .expect_err("offline NVIDIA CC needs admin verifier for Tier 3");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("VerifierProfileUnavailable")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_rejects_failed_measurements() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |_, binding| test_nvidia_offline_evidence(binding, false),
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("failed offline NVIDIA measurements must reject");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { kind, reason }
            if kind == "nvidia_nvtrust_offline_jwt"
                && reason.contains("VerifierProfileUnavailable")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_rejects_gb10_device_identity_claims() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |body, binding| test_nvidia_gb10_device_evidence(body, binding, "NVIDIA GB10 DGX Spark"),
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("GB10 device identity is not a confidential-compute quote");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { kind, .. }
            if kind == "nvidia_nvtrust_offline_jwt"
    ));
}

#[test]
fn hardware_quote_cannot_be_reused_for_a_new_attestation_nonce() {
    let (_temp, mut report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": "ab".repeat(32) }),
    );
    let original_binding = report
        .hw_quote
        .as_ref()
        .expect("hardware quote")
        .binding
        .clone();
    report.nonce_u = "bb".repeat(32);
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request)
        .expect_err("a quote from another session nonce must be rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteBindingMismatch { actual, .. }
            if actual == original_binding
    ));
}

#[test]
fn hardware_quote_rejects_oversized_evidence_and_metadata() {
    let (_temp, mut report, contract) = test_hardware_report(HardwareQuoteKind::AppleAppAttestJwt);
    report.hw_quote.as_mut().expect("hardware quote").evidence =
        "x".repeat(MAX_HARDWARE_QUOTE_EVIDENCE_BYTES + 1);
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err =
        verify_tier1_attestation(&request).expect_err("oversized quote evidence must be rejected");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("evidence") && reason.contains("maximum")
    ));

    let (_temp, mut report, contract) = test_hardware_report(HardwareQuoteKind::AppleAppAttestJwt);
    report.hw_quote.as_mut().expect("hardware quote").metadata =
        serde_json::json!("x".repeat(MAX_HARDWARE_QUOTE_METADATA_BYTES + 1));
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err =
        verify_tier1_attestation(&request).expect_err("oversized quote metadata must be rejected");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("metadata") && reason.contains("maximum")
    ));
}

#[test]
fn hardware_quote_rejects_excessive_metadata_depth_and_endorsement_bytes() {
    let (_temp, mut report, contract) = test_hardware_report(HardwareQuoteKind::AppleAppAttestJwt);
    let mut metadata = serde_json::Value::Null;
    for _ in 0..=MAX_HARDWARE_QUOTE_METADATA_DEPTH {
        metadata = serde_json::Value::Array(vec![metadata]);
    }
    report.hw_quote.as_mut().expect("hardware quote").metadata = metadata;
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("depth-unbounded quote metadata must be rejected");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("metadata depth")
    ));

    let (_temp, mut report, contract) = test_hardware_report(HardwareQuoteKind::AppleAppAttestJwt);
    report
        .hw_quote
        .as_mut()
        .expect("hardware quote")
        .endorsements = vec!["x".repeat(MAX_HARDWARE_QUOTE_ENDORSEMENTS_BYTES / 4); 5];
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let err = verify_tier1_attestation(&request)
        .expect_err("oversized quote endorsements must be rejected");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("endorsements") && reason.contains("maximum")
    ));
}

#[test]
fn native_tpm_quote_and_ek_chain_verify_without_external_helper() {
    let fixture = native_tpm_fixture();
    let context = fixture
        .policy
        .context()
        .with_activated_tpm_identity(&fixture.activated);
    let mut request = AttestationVerificationRequest::new(
        &fixture.report,
        &fixture.contract,
        &fixture.report.nonce_u,
        &fixture.report.provider_pubkey,
        fixture.report.report_ts + 10,
    );
    request.attestation_policy = Some(&context);

    let verified = verify_tier1_attestation(&request).expect("native TPM proof verifies");
    assert_eq!(verified.att_tier, TIER2_DEVICE_IDENTITY_TIER);
    assert_eq!(verified.runtime_binary_hash, fixture.report.binary_hash);
    assert_ne!(verified.runtime_binary_hash, fixture.contract.binary_hash);
}

#[test]
fn native_tpm_catalog_authority_preflights_collateral_semantics() {
    let fixture = native_tpm_fixture();
    let collateral = |policy: &AdminAttestationPolicy, pcr_policy: Vec<u8>| {
        let root = policy
            .trust_data
            .iter()
            .find(|reference| reference.id == "native-tpm-root")
            .unwrap()
            .clone();
        let pcrs = policy
            .trust_data
            .iter()
            .find(|reference| reference.id == "native-tpm-pcrs")
            .unwrap()
            .clone();
        vec![
            GatewayAttestationCollateral {
                reference: root,
                bytes: fixture.root_der.clone(),
                observed_epoch: TEST_POLICY_EPOCH,
            },
            GatewayAttestationCollateral {
                reference: pcrs,
                bytes: pcr_policy,
                observed_epoch: TEST_POLICY_EPOCH,
            },
        ]
    };

    GatewayAttestationAuthority::from_catalog_records(
        Some(vec![fixture.catalog_policy.clone()]),
        vec![fixture.catalog_binding.clone()],
        collateral(&fixture.catalog_policy, fixture.pcr_policy.clone()),
        TEST_POLICY_EPOCH,
    )
    .expect("schema-2 native TPM collateral preflights");

    let stale_pcr_policy = br#"{"schema_version":1,"hash_algorithm":"sha256","pcrs":{"0":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#.to_vec();
    let mut stale_policy = fixture.catalog_policy.clone();
    let stale_reference = stale_policy
        .trust_data
        .iter_mut()
        .find(|reference| reference.id == "native-tpm-pcrs")
        .unwrap();
    stale_reference.sha256 = hex::encode(Sha256::digest(&stale_pcr_policy));
    stale_reference.max_bytes = stale_pcr_policy.len() as u64;
    let error = GatewayAttestationAuthority::from_catalog_records(
        Some(vec![stale_policy.clone()]),
        vec![fixture.catalog_binding.clone()],
        collateral(&stale_policy, stale_pcr_policy),
        TEST_POLICY_EPOCH,
    )
    .expect_err("schema-1 PCR collateral must fail before routing");
    assert!(error.contains("native TPM collateral preflight failed"));
    assert!(error.contains("PCR policy"));
}

#[test]
fn native_tpm_policy_rejects_version_three_buyers_before_routing() {
    let fixture = native_tpm_fixture();
    assert_eq!(GATEWAY_ATTESTATION_VERIFIER_VERSION, 4);
    let old_capabilities = VerifierCapabilities::with_evidence_schemas(
        3,
        [(
            AttestationVerifierProfile::Tpm2EkActivateCredentialV1,
            BTreeSet::from([TPM_QUOTE_EVIDENCE_SCHEMA_VERSION]),
        )],
    );
    let readiness = AttestationReadiness::evaluate(
        &fixture.policy.chain,
        &fixture.policy.collateral,
        &old_capabilities,
        TEST_POLICY_EPOCH,
    );
    assert_eq!(
        readiness
            .quote_kinds
            .get(&HardwareQuoteKind::Tpm2QuoteEk)
            .unwrap()
            .status,
        QuoteKindReadinessStatus::VerifierTooOld {
            required: 4,
            actual: 3,
        }
    );
}

#[test]
fn native_tpm_rejects_provider_supplied_policy_or_roots() {
    let mut fixture = native_tpm_fixture();
    fixture.report.hw_quote.as_mut().unwrap().metadata = serde_json::json!({
        "trust_roots": ["provider-selected"],
        "policy": "provider-selected"
    });
    let context = fixture
        .policy
        .context()
        .with_activated_tpm_identity(&fixture.activated);
    let mut request = AttestationVerificationRequest::new(
        &fixture.report,
        &fixture.contract,
        &fixture.report.nonce_u,
        &fixture.report.provider_pubkey,
        fixture.report.report_ts + 10,
    );
    request.attestation_policy = Some(&context);

    assert!(matches!(
        verify_tier1_attestation(&request).unwrap_err(),
        GatewayError::ProviderTrustMaterialRejected { .. }
    ));
}

#[cfg(unix)]
fn write_verifier_script(dir: &tempfile::TempDir, stdout_json: &str) -> std::path::PathBuf {
    let path = dir.path().join("verify-hardware.sh");
    let script = if stdout_json.contains(r#""binding""#) {
        let escaped = stdout_json.replace('\'', "'\\''");
        format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{escaped}'\n")
    } else {
        let suffix = stdout_json
            .strip_prefix('{')
            .expect("verifier fixture must be a JSON object")
            .replace('\'', "'\\''");
        format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '{{\"binding\":\"%s\",%s\\n' \"$MAYHEM_HW_VERIFY_BINDING\" '{suffix}'\n"
        )
    };
    fs::write(&path, script).expect("write verifier script");
    let mut permissions = fs::metadata(&path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod verifier script");
    path
}

#[cfg(unix)]
fn write_raw_verifier_script(dir: &tempfile::TempDir, stdout_json: &str) -> std::path::PathBuf {
    let path = dir.path().join("verify-hardware-raw.sh");
    let escaped = stdout_json.replace('\'', "'\\''");
    fs::write(
        &path,
        format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{escaped}'\n"),
    )
    .expect("write raw verifier script");
    let mut permissions = fs::metadata(&path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod raw verifier script");
    path
}

#[cfg(unix)]
fn request_with_external_verifier<'a>(
    report: &'a mayhem_proto::AttestationReport,
    contract: &'a EnclaveContractRecord,
    verifier: &'a HardwareQuoteVerifierCommand,
) -> AttestationVerificationRequest<'a> {
    let mut request = AttestationVerificationRequest::new(
        report,
        contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    let kind = report
        .hw_quote
        .as_ref()
        .expect("managed verifier request requires hardware quote")
        .kind;
    let executable = fs::read(&verifier.command).expect("read managed verifier fixture");
    let policy = Box::leak(Box::new(test_policy_with_managed_executable(
        kind,
        Some(&executable),
    )));
    let context = Box::leak(Box::new(policy.context()));
    request.attestation_policy = Some(context);
    request.hardware_quote_verifier_command = Some(verifier);
    request
}

#[cfg(unix)]
#[test]
fn tpm2_ek_tier2_requires_gateway_activation_identity() {
    let (_temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": "ab".repeat(32) }),
    );
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err =
        verify_tier1_attestation(&request).expect_err("TPM EK needs native activation identity");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("ActivateCredential")
    ));
}

#[cfg(unix)]
#[test]
fn external_verifier_timeout_terminates_descendant_processes() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let child_pid_path = temp.path().join("verifier-child.pid");
    let script = temp.path().join("verify-hardware-hang.sh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nsleep 30 &\nchild=$!\nprintf '%s\\n' \"$child\" > '{}'\ncat >/dev/null\nwait \"$child\"\n",
            child_pid_path.display()
        ),
    )
    .expect("write hanging verifier script");
    let mut permissions = fs::metadata(&script)
        .expect("hanging verifier metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).expect("chmod hanging verifier");
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        // The full attestation suite runs many external verifier fixtures in
        // parallel. Leave enough startup time to create the descendant before
        // exercising the timeout cleanup itself.
        timeout: Duration::from_secs(5),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let error = verify_tier1_attestation(&request).expect_err("verifier must time out");
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. } if reason.contains("timed out")
    ));
    let child_pid = fs::read_to_string(&child_pid_path)
        .expect("read verifier descendant pid")
        .trim()
        .to_owned();
    for _ in 0..50 {
        if !test_process_is_running(&child_pid) {
            break;
        }
        sleep(Duration::from_millis(20));
    }
    assert!(
        !test_process_is_running(&child_pid),
        "external verifier descendant {child_pid} survived timeout"
    );
}

#[cfg(unix)]
#[test]
fn managed_verifier_rejects_an_unbound_command_path() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let approved = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"intel_tdx_dcap","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let approved_bytes = fs::read(&approved).unwrap();
    let policy =
        test_policy_with_managed_executable(HardwareQuoteKind::IntelTdxDcap, Some(&approved_bytes));
    let context = policy.context();
    let unbound = temp.path().join("unbound-verifier.sh");
    fs::write(&unbound, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = fs::metadata(&unbound).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&unbound, permissions).unwrap();
    let verifier = HardwareQuoteVerifierCommand {
        command: unbound,
        timeout: Duration::from_secs(5),
    };
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);
    request.hardware_quote_verifier_command = Some(&verifier);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("not the executable authenticated by active policy")
    ));
}

#[cfg(unix)]
#[test]
fn managed_verifier_rejects_policy_bound_only_to_another_target() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"intel_tdx_dcap","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let executable = fs::read(&script).unwrap();
    let foreign_target = foreign_managed_verifier_target();
    let policy = test_policy_with_managed_identity_for_target(
        HardwareQuoteKind::IntelTdxDcap,
        Some(&executable),
        "org.mayhem.test-managed-verifier",
        3,
        foreign_target,
        foreign_target,
        false,
    );
    let context = policy.context();
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(5),
    };
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);
    request.hardware_quote_verifier_command = Some(&verifier);

    let error = verify_tier1_attestation(&request).unwrap_err();
    let current_target = mayhem_gateway::managed_verifier_target().unwrap();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains(current_target)
                && reason.contains("not bound by active policy")
    ));
}

#[cfg(unix)]
#[test]
fn managed_verifier_rejects_cross_target_manifest_substitution() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"intel_tdx_dcap","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let executable = fs::read(&script).unwrap();
    let current_target = mayhem_gateway::managed_verifier_target().unwrap();
    let foreign_target = foreign_managed_verifier_target();
    let policy = test_policy_with_managed_identity_for_target(
        HardwareQuoteKind::IntelTdxDcap,
        Some(&executable),
        "org.mayhem.test-managed-verifier",
        3,
        current_target,
        foreign_target,
        false,
    );
    let context = policy.context();
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(5),
    };
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);
    request.hardware_quote_verifier_command = Some(&verifier);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains(foreign_target)
                && reason.contains(current_target)
                && reason.contains("manifest target")
    ));
}

#[cfg(unix)]
#[test]
fn managed_verifier_rejects_cross_target_executable_substitution() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let approved = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"intel_tdx_dcap","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let approved_bytes = fs::read(&approved).unwrap();
    let policy =
        test_policy_with_managed_executable(HardwareQuoteKind::IntelTdxDcap, Some(&approved_bytes));
    let context = policy.context();
    let substituted = temp.path().join("foreign-target-verifier.sh");
    fs::write(
        &substituted,
        "#!/bin/sh\ncat >/dev/null\nprintf '{\"ok\":false}\\n'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&substituted).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&substituted, permissions).unwrap();
    let verifier = HardwareQuoteVerifierCommand {
        command: substituted,
        timeout: Duration::from_secs(5),
    };
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);
    request.hardware_quote_verifier_command = Some(&verifier);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("not the executable authenticated by active policy")
    ));
}

#[cfg(unix)]
#[test]
fn managed_verifier_identity_version_is_policy_bound_before_readiness() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"intel_tdx_dcap","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let executable = fs::read(&script).unwrap();
    let policy = test_policy_with_managed_identity(
        HardwareQuoteKind::IntelTdxDcap,
        Some(&executable),
        "org.mayhem.shipped-tdx-verifier",
        2,
    );
    let context = policy.context();
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(5),
    };
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.attestation_policy = Some(&context);
    request.hardware_quote_verifier_command = Some(&verifier);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("older than active policy minimum")
    ));
}

#[cfg(unix)]
#[test]
fn managed_verifier_environment_is_minimal_and_retains_required_bindings() {
    const SENTINEL: &str = "MAYHEM_GATEWAY_TEST_PARENT_SECRET";
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let script = temp.path().join("verify-hardware-environment.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
cat >/dev/null
if [ "${MAYHEM_GATEWAY_TEST_PARENT_SECRET+x}" = x ]; then
  exit 21
fi
if [ "$PWD" != "/" ] ||
   [ "$MAYHEM_HW_VERIFY_KIND" != "intel_tdx_dcap" ] ||
   [ "$MAYHEM_HW_VERIFY_PLATFORM" != "test-platform" ] ||
   [ "$MAYHEM_HW_VERIFY_ATTESTATION_TIER" != "3" ] ||
   [ -z "$MAYHEM_HW_VERIFY_BINDING" ]; then
  exit 22
fi
printf '{"ok":true,"kind":"intel_tdx_dcap","binding":"%s","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}\n' "$MAYHEM_HW_VERIFY_BINDING"
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(30),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);
    let previous = std::env::var_os(SENTINEL);
    std::env::set_var(SENTINEL, "must-not-reach-managed-verifier");
    let result = verify_tier1_attestation(&request);
    if let Some(previous) = previous {
        std::env::set_var(SENTINEL, previous);
    } else {
        std::env::remove_var(SENTINEL);
    }

    result.expect("managed verifier receives only sanitized runtime and binding variables");
}

#[cfg(unix)]
#[test]
fn managed_verifier_concurrently_drains_and_bounds_stdout_and_stderr() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let script = temp.path().join("verify-hardware-flood.sh");
    fs::write(
        &script,
        "#!/bin/sh\ncat >/dev/null\nhead -c 300000 /dev/zero | tr '\\0' x >&2\nhead -c 300000 /dev/zero | tr '\\0' y\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let error = verify_tier1_attestation(&request).unwrap_err();
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("output exceeded")
    ));
}

#[cfg(unix)]
#[test]
fn managed_verifier_must_echo_binding_kind_and_attestation_tier() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let script = write_raw_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"intel_tdx_dcap","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("external verifier binding echo is mandatory");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("binding")
    ));

    let script = write_raw_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"intel_tdx_dcap","binding":"wrong","roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("external verifier att_tier echo is mandatory");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("att_tier")
    ));

    let script = write_raw_verifier_script(
        &temp,
        r#"{"ok":true,"binding":"wrong","att_tier":3,"roots":["intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err =
        verify_tier1_attestation(&request).expect_err("external verifier kind echo is mandatory");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("kind")
    ));
}

#[cfg(unix)]
#[test]
fn external_nvidia_cc_verifier_admits_source_build_on_valid_roots_and_golden_measurement() {
    let (temp, report, mut contract) =
        test_hardware_report(HardwareQuoteKind::NvidiaNvtrustOfflineJwt);
    contract.binary_hash = "11".repeat(32);
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","amd_sev_snp_vcek"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let verified = verify_tier1_attestation(&request)
        .expect("external verifier accepts NVIDIA GPU + CPU CC + golden measurement");

    assert_eq!(verified.att_tier, TIER3_CONFIDENTIAL_COMPUTE_TIER);
    assert_ne!(report.binary_hash, contract.binary_hash);
}

#[cfg(unix)]
#[test]
fn external_nvidia_cc_verifier_rejects_gpu_only_h100_without_cpu_root() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::NvidiaNvtrustOfflineJwt);
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("NVIDIA GPU-only evidence is not Tier-3 prompt confidentiality");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("do not satisfy")
    ));
}

#[cfg(unix)]
#[test]
fn external_verifier_accepts_intel_tdx_cpu_root_for_nvidia_cc_best_effort() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::NvidiaNvtrustOfflineJwt);
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","intel_tdx_dcap"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    verify_tier1_attestation(&request)
        .expect("Intel TDX/DCAP CPU root is supported as a best-effort CPU CC root");
}

#[cfg(unix)]
#[test]
fn provider_metadata_cannot_promote_a_route_into_azure_maa_scope() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        serde_json::json!({
            "platform_id": "azure-ncc",
            "region": "centralus"
        }),
    );
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","azure_maa_jwt_jwks_issuer_nonce_claims"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let error = verify_tier1_attestation(&request)
        .expect_err("provider metadata cannot alter the admin-bound platform");
    assert!(matches!(
        error,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("do not satisfy")
    ));
}

#[cfg(unix)]
#[test]
fn external_azure_maa_cpu_path_is_not_universal_cpu_root() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        serde_json::json!({
            "platform_id": "onprem-qemu-v1"
        }),
    );
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","azure_maa_jwt_jwks_issuer_nonce_claims"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("MAA is not a universal CPU/VM root outside Azure platform entries");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("do not satisfy")
    ));
}

#[cfg(unix)]
#[test]
fn external_azure_maa_cpu_path_still_rejects_wrong_workload_pcr() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        serde_json::json!({
            "platform_id": "azure-ncc",
            "region": "centralus"
        }),
    );
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","amd_sev_snp_vcek"],"matched_measurements":{"workload":{"vtpm_pcr_0":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("MAA platform proof never skips Mayhem workload PCR matching");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("workload PCR/stack")
    ));
}

#[cfg(unix)]
#[test]
fn external_tier3_registration_requires_workload_measurement_layer() {
    let (temp, report, mut contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        serde_json::json!({
            "platform_id": "azure-ncc",
            "region": "centralus"
        }),
    );
    contract.launch_measurements = serde_json::json!({
        "schema_version": 1,
        "effective_epoch": 0,
        "platform": "azure-ncc",
        "layers": {
            "vendor": {
                "snp_launch_digest": "ab".repeat(48)
            }
        }
    });
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","amd_sev_snp_vcek"],"matched_measurements":{"vendor":{"snp_launch_digest":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("Tier-3 cannot register with only vendor-layer measurements");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("workload PCR/stack")
    ));
}

#[cfg(unix)]
#[test]
fn external_verifier_rejects_unknown_measurement_even_on_real_roots() {
    let (temp, report, contract) = test_hardware_report(HardwareQuoteKind::NvidiaNvtrustOfflineJwt);
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","amd_sev_snp_vcek"],"matched_measurements":{"workload":{"vtpm_pcr_0":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}},"platform_id":"provider-declared-azure-ncc","region":"centralus","snp_chip_family":"genoa","snp_chip_id":"chip-123","snp_tcb":"svn27","gpu_model":"H100","gpu_driver":"550.90","gpu_vbios":"96.00.00"}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("hardware-valid unknown image measurement is rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("does not match")
                && reason.contains("platform=provider-declared-azure-ncc")
                && reason.contains("region=centralus")
                && reason.contains("snp_chip_id=chip-123")
                && reason.contains("gpu_model=H100")
    ));
}

#[cfg(unix)]
#[test]
fn external_verifier_uses_policy_platform_and_ignores_provider_platform_hints() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        serde_json::Value::Null,
    );
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","amd_sev_snp_vcek"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    verify_tier1_attestation(&request)
        .expect("the exact admin policy platform is sufficient without provider metadata");

    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        serde_json::json!({
            "platform_id": "provider-can-lie-here",
            "region": "wrong-region"
        }),
    );
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","amd_sev_snp_vcek"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    verify_tier1_attestation(&request)
        .expect("declared platform/region hints do not influence Tier-3 trust acceptance");
}

#[cfg(unix)]
#[test]
fn external_verifier_rejects_tier3_enclave_without_golden_measurement() {
    let (temp, report, mut contract) =
        test_hardware_report(HardwareQuoteKind::NvidiaNvtrustOfflineJwt);
    contract.launch_measurements = serde_json::Value::Null;
    let script = write_verifier_script(
        &temp,
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","amd_sev_snp_vcek"],"matched_measurements":{"workload":{"vtpm_pcr_0":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(60),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("Tier-3 registration without golden measurement fails closed");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("no admin-published measurement layers")
    ));
}

#[test]
fn tier3_quote_kinds_fail_closed_without_admin_verifier() {
    let (_temp, report, contract) = test_hardware_report(HardwareQuoteKind::IntelTdxDcap);
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("admin quote verifier is required");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("VerifierProfileUnavailable")
    ));
}

#[test]
fn verification_accepts_source_built_runtime_without_admin_approval() {
    let (_temp, report, mut contract) = test_report();
    contract.binary_hash = "11".repeat(32);
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let verified = verify_tier1_attestation(&request)
        .expect("a measured source build must not require admin admission");
    assert_eq!(verified.enclave_id, contract.enclave_id);
    assert_eq!(verified.runtime_binary_hash, report.binary_hash);
    assert_ne!(report.binary_hash, contract.binary_hash);
}

#[test]
fn verification_rejects_wrong_manifest() {
    let (_temp, report, mut contract) = test_report();
    contract.manifest_hash = "manifest-hash-v2".to_owned();
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("manifest must match contract");

    assert!(matches!(
        err,
        GatewayError::ContractMismatch {
            field: "manifest_hash",
            ..
        }
    ));
}

#[test]
fn verification_rejects_stale_nonce() {
    let (_temp, report, contract) = test_report();
    let stale_nonce = "bb".repeat(32);
    let request = policy_request(
        &report,
        &contract,
        &stale_nonce,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("nonce must be challenge-fresh");

    assert!(matches!(err, GatewayError::NonceMismatch { .. }));
}

#[test]
fn verification_rejects_stale_report_timestamp() {
    let (_temp, report, contract) = test_report();
    let request = policy_request(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        100_000,
    );

    let err = verify_tier1_attestation(&request).expect_err("report timestamp must be fresh");

    assert!(matches!(err, GatewayError::ReportStale { .. }));
}
