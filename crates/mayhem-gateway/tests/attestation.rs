#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;

use ed25519_dalek::SigningKey;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mayhem_enclave::{
    build_tier1_attestation_report, build_tier2_attestation_report, measure_binary, RuntimeKeypair,
    Tier1AttestationOptions, Tier2AttestationOptions, TIER2_ATTESTATION_TIER,
};
use mayhem_gateway::{
    verify_attestation, verify_tier1_attestation, AttestationVerificationRequest,
    EnclaveContractRecord, GatewayError,
};
use mayhem_proto::{
    catalog_enclave_id, hardware_quote_binding, AttestationBody, AttestationRuntimeConfig,
    CatalogEnclaveIdentity, HardwareQuote, HardwareQuoteKind, ATTESTATION_ALG,
    ATTESTATION_SCHEMA_VERSION,
};

fn test_report() -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
    BTreeSet<String>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let binary = temp.path().join("mayhem-enclave-test-bin");
    fs::write(&binary, b"measured enclave binary").expect("write test binary");
    let binary_hash = measure_binary(&binary).expect("measure binary");
    let identity = CatalogEnclaveIdentity {
        admin_pubkey: "admin-key".to_owned(),
        model_id: "mayhem/qwen3.5-4b@q4".to_owned(),
        artifact_root: "artifact-root-v1".to_owned(),
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
        artifact_root: identity.artifact_root,
        manifest_hash: identity.manifest_hash,
        binary_hash: attestation.report.binary_hash.clone(),
        att_tier: 1,
        caps: serde_json::json!({}),
    };
    let trusted = BTreeSet::from([attestation.report.binary_hash.clone()]);

    (temp, attestation.report, contract, trusted)
}

fn test_tier2_report(
    quote_kind: HardwareQuoteKind,
) -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
    BTreeSet<String>,
) {
    test_tier2_report_with_evidence(quote_kind, |_| "mock-hardware-quote".to_owned())
}

fn test_tier2_report_with_evidence(
    quote_kind: HardwareQuoteKind,
    evidence_for_binding: impl FnOnce(&str) -> String,
) -> (
    tempfile::TempDir,
    mayhem_proto::AttestationReport,
    EnclaveContractRecord,
    BTreeSet<String>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let binary = temp.path().join("mayhem-enclave-test-bin");
    fs::write(&binary, b"measured tier2 enclave binary").expect("write test binary");
    let binary_hash = measure_binary(&binary).expect("measure binary");
    let runtime_keypair = RuntimeKeypair::from_seed([9_u8; 32]);
    let provider_signing_key = SigningKey::from_bytes(&[7_u8; 32]);
    let identity = CatalogEnclaveIdentity {
        admin_pubkey: "admin-key".to_owned(),
        model_id: "mayhem/qwen3.5-4b@q4".to_owned(),
        artifact_root: "artifact-root-v1".to_owned(),
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
        att_tier: TIER2_ATTESTATION_TIER,
        hw_quote: None,
        boot_epoch: 100,
        report_ts: 200,
        nonce_u: "aa".repeat(32),
        runtime_config: AttestationRuntimeConfig::default(),
    };
    let binding = hardware_quote_binding(&body).expect("binding");
    let quote = HardwareQuote {
        kind: quote_kind,
        evidence: evidence_for_binding(&binding),
        binding,
        endorsements: vec!["mock-root".to_owned()],
    };
    let attestation = build_tier2_attestation_report(&Tier2AttestationOptions {
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
    .expect("build tier2 report");
    let contract = EnclaveContractRecord {
        enclave_id: body.enclave_id,
        admin_pubkey: identity.admin_pubkey,
        model_id: identity.model_id,
        artifact_root: identity.artifact_root,
        manifest_hash: identity.manifest_hash,
        binary_hash: attestation.report.binary_hash.clone(),
        att_tier: TIER2_ATTESTATION_TIER,
        caps: serde_json::json!({}),
    };
    let trusted = BTreeSet::from([attestation.report.binary_hash.clone()]);

    (temp, attestation.report, contract, trusted)
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

#[test]
fn verifies_signed_tier1_report() {
    let (_temp, report, contract, trusted) = test_report();
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
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
    let (_temp, report, contract, trusted) = test_report();
    let wrong_provider = "dd".repeat(32);
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
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
    let (_temp, report, mut contract, trusted) = test_report();
    contract.caps = serde_json::json!({ "tp_degree": 2 });
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
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
fn verifies_mock_tier2_report_when_explicitly_enabled() {
    let (_temp, report, contract, trusted) = test_tier2_report(HardwareQuoteKind::MockTier2);
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.allow_mock_hardware_quote = true;

    let verified = verify_tier1_attestation(&request).expect("mock tier2 report verifies");

    assert_eq!(verified.att_tier, 2);
    assert_eq!(verified.enclave_id, contract.enclave_id);
}

#[test]
fn tier2_report_requires_hardware_quote_verification() {
    let (_temp, report, contract, trusted) = test_tier2_report(HardwareQuoteKind::MockTier2);
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("mock quotes require opt-in");

    assert!(matches!(err, GatewayError::MockHardwareQuoteDisabled));
}

#[test]
fn verifies_nvidia_nras_tier2_report_with_trusted_jwks() {
    let (_temp, report, contract, trusted) =
        test_tier2_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |binding| {
            test_nvidia_evidence(binding, true)
        });
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.trusted_nvidia_nras_jwks = Some(&jwks);

    let verified =
        verify_tier1_attestation(&request).expect("signed NVIDIA NRAS tier2 report verifies");

    assert_eq!(verified.att_tier, 2);
    assert_eq!(verified.enclave_id, contract.enclave_id);
}

#[test]
fn nvidia_nras_tier2_report_requires_trusted_jwks() {
    let (_temp, report, contract, trusted) =
        test_tier2_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |binding| {
            test_nvidia_evidence(binding, true)
        });
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("NVIDIA NRAS quotes need trusted JWKS");

    assert!(matches!(
        err,
        GatewayError::HardwareQuoteTrustRootMissing { .. }
    ));
}

#[test]
fn nvidia_nras_tier2_report_rejects_signed_failed_appraisal() {
    let (_temp, report, contract, trusted) =
        test_tier2_report_with_evidence(HardwareQuoteKind::NvidiaNrasJwt, |binding| {
            test_nvidia_evidence(binding, false)
        });
    let jwks = test_nvidia_jwks();
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
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
            if reason.contains("measres")
    ));
}

#[test]
fn sev_snp_and_tdx_quote_kinds_fail_closed_until_vendor_verifiers_are_wired() {
    let (_temp, report, contract, trusted) = test_tier2_report(HardwareQuoteKind::IntelTdxDcap);
    let mut request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );
    request.allow_mock_hardware_quote = true;

    let err = verify_tier1_attestation(&request).expect_err("vendor quote verifier is required");

    assert!(matches!(err, GatewayError::HardwareQuoteUnsupported { .. }));
}

#[test]
fn verification_rejects_wrong_binary_hash() {
    let (_temp, report, contract, trusted) = test_report();
    let empty_release_set = BTreeSet::new();
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &empty_release_set,
        &report.nonce_u,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("binary must be trusted");

    assert!(matches!(err, GatewayError::BinaryHashNotTrusted { .. }));
    assert_eq!(trusted.len(), 1);
}

#[test]
fn verification_rejects_wrong_manifest() {
    let (_temp, report, mut contract, trusted) = test_report();
    contract.manifest_hash = "manifest-hash-v2".to_owned();
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
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
    let (_temp, report, contract, trusted) = test_report();
    let stale_nonce = "bb".repeat(32);
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
        &stale_nonce,
        &report.provider_pubkey,
        210,
    );

    let err = verify_tier1_attestation(&request).expect_err("nonce must be challenge-fresh");

    assert!(matches!(err, GatewayError::NonceMismatch { .. }));
}

#[test]
fn verification_rejects_stale_report_timestamp() {
    let (_temp, report, contract, trusted) = test_report();
    let request = AttestationVerificationRequest::new(
        &report,
        &contract,
        &trusted,
        &report.nonce_u,
        &report.provider_pubkey,
        100_000,
    );

    let err = verify_tier1_attestation(&request).expect_err("report timestamp must be fresh");

    assert!(matches!(err, GatewayError::ReportStale { .. }));
}
