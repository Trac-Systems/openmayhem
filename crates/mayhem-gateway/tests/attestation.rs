#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;

use ed25519_dalek::SigningKey;
use mayhem_enclave::{
    build_tier1_attestation_report, build_tier2_attestation_report, measure_binary, RuntimeKeypair,
    Tier1AttestationOptions, Tier2AttestationOptions, TIER2_ATTESTATION_TIER,
};
use mayhem_gateway::{
    verify_tier1_attestation, AttestationVerificationRequest, EnclaveContractRecord, GatewayError,
};
use mayhem_proto::{
    catalog_enclave_id, hardware_quote_binding, AttestationBody, CatalogEnclaveIdentity,
    HardwareQuote, HardwareQuoteKind, ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION,
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
    };
    let quote = HardwareQuote {
        kind: quote_kind,
        evidence: "mock-hardware-quote".to_owned(),
        binding: hardware_quote_binding(&body).expect("binding"),
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
    };
    let trusted = BTreeSet::from([attestation.report.binary_hash.clone()]);

    (temp, attestation.report, contract, trusted)
}

#[test]
fn verifies_signed_tier1_report() {
    let (_temp, report, contract, trusted) = test_report();
    let mut request =
        AttestationVerificationRequest::new(&report, &contract, &trusted, &report.nonce_u, 210);
    request.expected_provider_pubkey = Some(&report.provider_pubkey);

    let verified = verify_tier1_attestation(&request).expect("valid report verifies");

    assert_eq!(verified.enclave_id, contract.enclave_id);
    assert_eq!(verified.provider_pubkey, report.provider_pubkey);
    assert_eq!(verified.enclave_pubkey, report.enclave_pubkey);
    assert!(!verified.report_head.is_empty());
}

#[test]
fn verifies_mock_tier2_report_when_explicitly_enabled() {
    let (_temp, report, contract, trusted) = test_tier2_report(HardwareQuoteKind::MockTier2);
    let mut request =
        AttestationVerificationRequest::new(&report, &contract, &trusted, &report.nonce_u, 210);
    request.allow_mock_hardware_quote = true;

    let verified = verify_tier1_attestation(&request).expect("mock tier2 report verifies");

    assert_eq!(verified.att_tier, 2);
    assert_eq!(verified.enclave_id, contract.enclave_id);
}

#[test]
fn tier2_report_requires_hardware_quote_verification() {
    let (_temp, report, contract, trusted) = test_tier2_report(HardwareQuoteKind::MockTier2);
    let request =
        AttestationVerificationRequest::new(&report, &contract, &trusted, &report.nonce_u, 210);

    let err = verify_tier1_attestation(&request).expect_err("mock quotes require opt-in");

    assert!(matches!(err, GatewayError::MockHardwareQuoteDisabled));
}

#[test]
fn real_tier2_quote_kinds_fail_closed_until_vendor_verifiers_are_wired() {
    let (_temp, report, contract, trusted) = test_tier2_report(HardwareQuoteKind::IntelTdxDcap);
    let mut request =
        AttestationVerificationRequest::new(&report, &contract, &trusted, &report.nonce_u, 210);
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
    let request =
        AttestationVerificationRequest::new(&report, &contract, &trusted, &report.nonce_u, 210);

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
    let request =
        AttestationVerificationRequest::new(&report, &contract, &trusted, &stale_nonce, 210);

    let err = verify_tier1_attestation(&request).expect_err("nonce must be challenge-fresh");

    assert!(matches!(err, GatewayError::NonceMismatch { .. }));
}

#[test]
fn verification_rejects_stale_report_timestamp() {
    let (_temp, report, contract, trusted) = test_report();
    let request =
        AttestationVerificationRequest::new(&report, &contract, &trusted, &report.nonce_u, 100_000);

    let err = verify_tier1_attestation(&request).expect_err("report timestamp must be fresh");

    assert!(matches!(err, GatewayError::ReportStale { .. }));
}
