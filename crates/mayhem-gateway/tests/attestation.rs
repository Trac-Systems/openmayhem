#![forbid(unsafe_code)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::thread::sleep;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mayhem_enclave::{
    build_hardware_attestation_report, build_tier1_attestation_report, measure_binary,
    HardwareAttestationOptions, RuntimeKeypair, Tier1AttestationOptions,
};
use mayhem_gateway::{
    verify_attestation, verify_tier1_attestation, AttestationVerificationRequest,
    EnclaveContractRecord, GatewayError, HardwareQuoteVerifierCommand,
    MAX_HARDWARE_QUOTE_ENDORSEMENTS_BYTES, MAX_HARDWARE_QUOTE_EVIDENCE_BYTES,
    MAX_HARDWARE_QUOTE_METADATA_BYTES, MAX_HARDWARE_QUOTE_METADATA_DEPTH,
};
use mayhem_proto::{
    catalog_enclave_id, hardware_quote_binding, AttestationBody, AttestationRuntimeConfig,
    CatalogEnclaveIdentity, HardwareQuote, HardwareQuoteKind, ATTESTATION_ALG,
    ATTESTATION_SCHEMA_VERSION, DEFAULT_MODEL_CLASS, TIER2_DEVICE_IDENTITY_TIER,
    TIER3_CONFIDENTIAL_COMPUTE_TIER,
};

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
    test_hardware_report_with_evidence_and_metadata(quote_kind, None, |_, _| {
        "test-hardware-quote".to_owned()
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
    test_hardware_report_with_evidence_and_metadata(quote_kind, Some(metadata), |_, _| {
        "test-hardware-quote".to_owned()
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
    let quote = HardwareQuote {
        kind: quote_kind,
        evidence: evidence_for_binding(&body, &binding),
        binding,
        endorsements: vec!["mock-root".to_owned()],
        metadata: metadata.unwrap_or_else(|| {
            if att_tier >= 3 {
                serde_json::json!({
                    "platform_id": "azure-h100-sev-snp-nvidia-cc",
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

fn test_nvidia_offline_jwks() -> serde_json::Value {
    serde_json::json!({
        "jwks": test_nvidia_jwks(),
        "issuers": ["https://local.verifier.attestation.nvidia.com"]
    })
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
    let request = AttestationVerificationRequest::new(
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
    assert!(!verified.report_head.is_empty());
}

#[test]
fn verification_rejects_wrong_provider_pubkey_on_default_path() {
    let (_temp, report, contract) = test_report();
    let wrong_provider = "dd".repeat(32);
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &wrong_provider,
        210,
    );

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
    let request = AttestationVerificationRequest::new(
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
    let request = AttestationVerificationRequest::new(
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
fn verifies_apple_app_attest_tier2_identity_with_trusted_jwks() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::AppleAppAttestJwt,
        test_apple_app_attest_evidence,
    );
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_apple_app_attest_jwks = Some(&jwks);

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
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_apple_app_attest_jwks = Some(&jwks);

    let err = verify_tier1_attestation(&request)
        .expect_err("case-variant binding claim must be rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("eat_nonce")
    ));
}

#[test]
fn apple_app_attest_tier2_identity_requires_trusted_jwks() {
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

    let err = verify_tier1_attestation(&request).expect_err("Apple App Attest needs trusted JWKS");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteTrustRootMissing { kind }
            if kind == "apple_app_attest_jwt"
    ));
}

#[test]
fn verifies_nvidia_gb10_tier2_device_identity_with_trusted_jwks() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaGb10DeviceJwt,
        |body, binding| test_nvidia_gb10_device_evidence(body, binding, "NVIDIA GB10 DGX Spark"),
    );
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_gb10_device_jwks = Some(&jwks);

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
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_gb10_device_jwks = Some(&jwks);

    let err =
        verify_tier1_attestation(&request).expect_err("non-GB10 device identity must be rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { kind, reason }
            if kind == "nvidia_gb10_device_jwt" && reason.contains("GB10")
    ));
}

#[test]
fn nvidia_nras_tier3_report_requires_admin_verifier_even_with_trusted_jwks() {
    let (_temp, report, contract) =
        test_hardware_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |_, binding| {
            test_nvidia_evidence(binding, true)
        });
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_nras_jwks = Some(&jwks);

    let err = verify_tier1_attestation(&request)
        .expect_err("NVIDIA NRAS alone is not enough for Tier-3 prompt confidentiality");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("admin hardware quote verifier")
    ));
}

#[test]
fn nvidia_nras_tier3_report_requires_trusted_jwks() {
    let (_temp, report, contract) =
        test_hardware_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |_, binding| {
            test_nvidia_evidence(binding, true)
        });
    let request = AttestationVerificationRequest::new(
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
            if reason.contains("admin hardware quote verifier")
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
    let request = AttestationVerificationRequest::new(
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
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_nras_jwks = Some(&jwks);

    let err = verify_tier1_attestation(&request)
        .expect_err("signed failed NVIDIA appraisal must be rejected");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("admin hardware quote verifier")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_requires_admin_verifier_even_with_trusted_jwks() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |_, binding| test_nvidia_offline_evidence(binding, true),
    );
    let jwks = test_nvidia_offline_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_offline_jwks = Some(&jwks);

    let err = verify_tier1_attestation(&request)
        .expect_err("offline NVIDIA token alone is not enough for Tier-3 prompt confidentiality");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("admin hardware quote verifier")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_requires_trusted_jwks() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |_, binding| test_nvidia_offline_evidence(binding, true),
    );
    let request = AttestationVerificationRequest::new(
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
            if reason.contains("admin hardware quote verifier")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_rejects_failed_measurements() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |_, binding| test_nvidia_offline_evidence(binding, false),
    );
    let jwks = test_nvidia_offline_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_offline_jwks = Some(&jwks);

    let err = verify_tier1_attestation(&request)
        .expect_err("failed offline NVIDIA measurements must reject");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { kind, reason }
            if kind == "nvidia_nvtrust_offline_jwt" && reason.contains("measurements")
    ));
}

#[test]
fn nvidia_nvtrust_offline_cc_quote_rejects_gb10_device_identity_claims() {
    let (_temp, report, contract) = test_hardware_report_with_evidence(
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
        |body, binding| test_nvidia_gb10_device_evidence(body, binding, "NVIDIA GB10 DGX Spark"),
    );
    let jwks = serde_json::json!({
        "jwks": test_nvidia_jwks(),
        "issuers": ["https://nras.attestation.nvidia.com"]
    });
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_offline_jwks = Some(&jwks);

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
    let request = AttestationVerificationRequest::new(
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
    let request = AttestationVerificationRequest::new(
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
    let request = AttestationVerificationRequest::new(
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
    let request = AttestationVerificationRequest::new(
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
    let request = AttestationVerificationRequest::new(
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
    request.hardware_quote_verifier_command = Some(verifier);
    request
}

#[cfg(unix)]
#[test]
fn tpm2_ek_tier2_requires_external_verifier() {
    let (_temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": "ab".repeat(32) }),
    );
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("TPM EK needs admin verifier");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("TPM 2.0 EK quotes require")
    ));
}

#[cfg(unix)]
#[test]
fn external_verifier_timeout_terminates_descendant_processes() {
    let device_key = "ab".repeat(32);
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": device_key }),
    );
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
        if !Command::new("kill")
            .args(["-0", &child_pid])
            .output()
            .expect("inspect verifier descendant")
            .status
            .success()
        {
            break;
        }
        sleep(Duration::from_millis(20));
    }
    assert!(
        !Command::new("kill")
            .args(["-0", &child_pid])
            .output()
            .expect("inspect terminated verifier descendant")
            .status
            .success(),
        "external verifier descendant {child_pid} survived timeout"
    );
}

#[cfg(unix)]
#[test]
fn external_tpm2_ek_verifier_accepts_source_build_with_valid_device_proof() {
    let device_key = "ab".repeat(32);
    let (temp, report, mut contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": device_key }),
    );
    contract.binary_hash = "11".repeat(32);
    let script = write_verifier_script(
        &temp,
        &format!(
            r#"{{"ok":true,"kind":"tpm2_quote_ek","att_tier":2,"roots":["tpm2_ek_cert_chain"],"device_key":"{}"}}"#,
            "ab".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let verified = verify_tier1_attestation(&request)
        .expect("TPM proof, not a runtime allowlist, admits Tier 2");

    assert_eq!(verified.att_tier, TIER2_DEVICE_IDENTITY_TIER);
    assert_ne!(report.binary_hash, contract.binary_hash);
}

#[cfg(unix)]
#[test]
fn external_tpm2_ek_verifier_rejects_wrong_device_key() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": "ab".repeat(32) }),
    );
    let script = write_verifier_script(
        &temp,
        &format!(
            r#"{{"ok":true,"kind":"tpm2_quote_ek","att_tier":2,"roots":["tpm_manufacturer_root"],"device_key":"{}"}}"#,
            "cd".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("TPM verifier-confirmed EK must match provider metadata");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("does not match the verifier-confirmed EK")
    ));
}

#[cfg(unix)]
#[test]
fn external_tpm2_ek_verifier_rejects_replayed_wrong_binding() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": "ab".repeat(32) }),
    );
    let script = write_verifier_script(
        &temp,
        &format!(
            r#"{{"ok":true,"kind":"tpm2_quote_ek","att_tier":2,"binding":"{}","roots":["tpm_manufacturer_root"],"device_key":"{}"}}"#,
            "cd".repeat(32),
            "ab".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("TPM verifier verdict must be bound to this Mayhem attestation nonce");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteBindingMismatch { .. }
    ));
}

#[cfg(unix)]
#[test]
fn external_verifier_must_echo_binding_and_attestation_tier() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": "ab".repeat(32) }),
    );
    let script = write_raw_verifier_script(
        &temp,
        &format!(
            r#"{{"ok":true,"kind":"tpm2_quote_ek","att_tier":2,"roots":["tpm_manufacturer_root"],"device_key":"{}"}}"#,
            "ab".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("external verifier binding echo is mandatory");
    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("binding")
    ));

    let binding = report
        .hw_quote
        .as_ref()
        .expect("hardware quote")
        .binding
        .clone();
    let script = write_raw_verifier_script(
        &temp,
        &format!(
            r#"{{"ok":true,"kind":"tpm2_quote_ek","binding":"{binding}","roots":["tpm_manufacturer_root"],"device_key":"{}"}}"#,
            "ab".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
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
        &format!(
            r#"{{"ok":true,"binding":"{binding}","att_tier":2,"roots":["tpm_manufacturer_root"],"device_key":"{}"}}"#,
            "ab".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
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
fn external_tpm2_ek_verifier_rejects_missing_contract_device_key_metadata() {
    let (temp, report, contract) =
        test_hardware_report_with_metadata(HardwareQuoteKind::Tpm2QuoteEk, serde_json::json!({}));
    let script = write_verifier_script(
        &temp,
        &format!(
            r#"{{"ok":true,"kind":"tpm2_quote_ek","att_tier":2,"roots":["tpm2_ek_cert_chain"],"device_key":"{}"}}"#,
            "ab".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("TPM verifier-confirmed EK must match provider-submitted contract metadata");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("must carry the EK device_key")
    ));
}

#[cfg(unix)]
#[test]
fn external_tpm2_ek_verifier_rejects_public_key_only_root_label() {
    let (temp, report, contract) = test_hardware_report_with_metadata(
        HardwareQuoteKind::Tpm2QuoteEk,
        serde_json::json!({ "device_key": "ab".repeat(32) }),
    );
    let script = write_verifier_script(
        &temp,
        &format!(
            r#"{{"ok":true,"kind":"tpm2_quote_ek","att_tier":2,"roots":["tpm2_ek_public_sha256"],"device_key":"{}"}}"#,
            "ab".repeat(32)
        ),
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("Tier 2 TPM must be manufacturer/EK-cert rooted, not EK-public-only");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("do not satisfy")
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
        timeout: Duration::from_secs(15),
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
        timeout: Duration::from_secs(15),
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
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    verify_tier1_attestation(&request)
        .expect("Intel TDX/DCAP CPU root is supported as a best-effort CPU CC root");
}

#[cfg(unix)]
#[test]
fn external_azure_maa_cpu_path_requires_azure_scope_gpu_roots_and_workload_pcr() {
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
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    verify_tier1_attestation(&request).expect(
        "Azure-scoped MAA CPU path is accepted only after verifier validates JWT/JWKS/issuer/nonce/claims and workload PCR",
    );
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
        timeout: Duration::from_secs(15),
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
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","azure_maa_jwt_jwks_issuer_nonce_claims"],"matched_measurements":{"workload":{"vtpm_pcr_0":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
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
        r#"{"ok":true,"kind":"nvidia_nvtrust_offline_jwt","att_tier":3,"roots":["nvidia_gpu_cert_chain","nvidia_driver_rim","nvidia_vbios_rim","azure_maa_jwt_jwks_issuer_nonce_claims"],"matched_measurements":{"vendor":{"snp_launch_digest":"abababababababababababababababababababababababababababababababababababababababababababababababab"}}}"#,
    );
    let verifier = HardwareQuoteVerifierCommand {
        command: script,
        timeout: Duration::from_secs(15),
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
        timeout: Duration::from_secs(15),
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
fn external_verifier_requires_tier3_quote_platform_hint_but_does_not_trust_it() {
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
        timeout: Duration::from_secs(15),
    };
    let request = request_with_external_verifier(&report, &contract, &verifier);

    let err = verify_tier1_attestation(&request)
        .expect_err("Tier-3 quote submissions must carry a platform hint for admin workflow");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteInvalid { reason, .. }
            if reason.contains("platform_id")
    ));

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
        timeout: Duration::from_secs(15),
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
        timeout: Duration::from_secs(15),
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
    let request = AttestationVerificationRequest::new(
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
            if reason.contains("admin hardware quote verifier")
    ));
}

#[test]
fn verification_accepts_source_built_runtime_without_admin_approval() {
    let (_temp, report, mut contract) = test_report();
    contract.binary_hash = "11".repeat(32);
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let verified = verify_tier1_attestation(&request)
        .expect("a measured source build must not require admin admission");
    assert_eq!(verified.enclave_id, contract.enclave_id);
    assert_ne!(report.binary_hash, contract.binary_hash);
}

#[test]
fn verification_rejects_wrong_manifest() {
    let (_temp, report, mut contract) = test_report();
    contract.manifest_hash = "manifest-hash-v2".to_owned();
    let request = AttestationVerificationRequest::new(
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
    let request = AttestationVerificationRequest::new(
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
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &report.nonce_u,
        &report.provider_pubkey,
        100_000,
    );

    let err = verify_tier1_attestation(&request).expect_err("report timestamp must be fresh");

    assert!(matches!(err, GatewayError::ReportStale { .. }));
}
