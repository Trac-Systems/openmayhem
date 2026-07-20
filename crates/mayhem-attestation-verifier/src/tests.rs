use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
    Engine as _,
};
use dcap_qvl::{verify::QuoteVerifier, QuoteCollateralV3};
use mayhem_attestation::{EvidenceBinding, ValidatedAttestationPolicy};
use mayhem_proto::{
    AdminAttestationPolicy, AdminEnclaveAttestationBinding, AttestationMeasurementLayer,
    AttestationOriginPin, AttestationQuoteKindPolicy, AttestationRuntimeConfig,
    AttestationTrustDataKind, AttestationTrustDataRef, AttestationTrustDataSource,
    AttestationVerifierProfile, HardwareQuote, HardwareQuoteKind, HardwareQuoteRoutePolicyBinding,
    TpmHashAlgorithm, TpmPcrValue, TpmQuoteEvidence, ATTESTATION_ALG,
    ATTESTATION_POLICY_SCHEMA_VERSION, ATTESTATION_SCHEMA_VERSION,
    TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use rsa::{
    pkcs1v15::SigningKey as RsaSigningKey,
    signature::{SignatureEncoding, Signer},
    traits::PublicKeyParts,
    RsaPrivateKey, RsaPublicKey,
};
use serde_json::{json, Value};
use sev::{firmware::guest::AttestationReport as SnpAttestationReport, parser::ByteParser};
use sha2::{Digest, Sha256, Sha512};

use super::*;

const AMD_NOW: u64 = 1_800_000_000;
const TDX_NOW: u64 = 1_752_915_600;
const MAA_NOW: u64 = 1_800_000_000;
const ENCLAVE_ID: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const NONCE: &str = "1111111111111111111111111111111111111111111111111111111111111111";

const AMD_REPORT: &[u8] = include_bytes!("../tests/fixtures/amd/report_milan.bin");
const AMD_VCEK: &[u8] = include_bytes!("../tests/fixtures/amd/vcek_milan.der");
const AMD_ARK: &[u8] = include_bytes!("../tests/fixtures/amd/ark_milan.der");
const AMD_ASK: &[u8] = include_bytes!("../tests/fixtures/amd/ask_milan.der");
const TDX_QUOTE: &[u8] = include_bytes!("../tests/fixtures/intel/tdx_quote.bin");
const TDX_COLLATERAL: &[u8] = include_bytes!("../tests/fixtures/intel/tdx_quote_collateral.json");
const TDX_ROOT: &[u8] = include_bytes!("../tests/fixtures/intel/TrustedRootCA.der");

#[test]
fn identity_is_bounded_platform_neutral_and_advertises_exact_profiles() {
    let identity = verifier_identity();
    assert_eq!(identity.verifier_id, VERIFIER_ID);
    assert_eq!(identity.version, VERIFIER_VERSION);
    assert_eq!(identity.max_input_bytes, MAX_VERIFIER_INPUT_BYTES);
    assert_eq!(
        identity.public_trust_source,
        "authenticated_admin_policy_input"
    );
    assert_eq!(
        identity.profiles,
        BTreeMap::from([
            (
                AttestationVerifierProfile::AmdSevSnpVcekV1,
                BTreeSet::from([1]),
            ),
            (
                AttestationVerifierProfile::IntelTdxDcapV1,
                BTreeSet::from([1]),
            ),
            (
                AttestationVerifierProfile::NvidiaNrasCompositeV1,
                BTreeSet::from([1]),
            ),
            (
                AttestationVerifierProfile::NvidiaNvtrustOfflineCompositeV1,
                BTreeSet::from([1]),
            ),
        ])
    );

    let encoded = serde_json::to_vec(&identity).unwrap();
    assert!(encoded.len() <= 4 * 1024);
    let encoded = String::from_utf8(encoded).unwrap();
    for forbidden in [
        "executable_sha256",
        "platform",
        "endpoint",
        "jwks",
        "trust_root",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "identity contains {forbidden}"
        );
    }
}

#[test]
fn verifier_identity_and_version_substitution_fail_closed() {
    let mut wrong_id = maa_request();
    wrong_id.managed_verifier.id = "provider-verifier".to_owned();
    assert_rejected(
        &wrong_id,
        MAA_NOW,
        "identity or version does not match this executable",
    );

    let mut wrong_version = maa_request();
    wrong_version.managed_verifier.version = VERIFIER_VERSION + 1;
    assert_rejected(
        &wrong_version,
        MAA_NOW,
        "identity or version does not match this executable",
    );
}

#[test]
fn amd_native_profile_verifies_real_report_vcek_and_admin_roots() {
    let verdict = verify(amd_request(), AMD_NOW);
    assert!(verdict.ok, "{:?}", verdict.reason);
    assert!(verdict.roots.contains(&"amd_sev_snp_vcek".to_owned()));
    assert!(verdict.matched_measurements["vendor"].contains_key("snp_launch_measurement"));
    assert!(verdict.matched_measurements["workload"].contains_key("snp_launch_measurement"));
}

#[test]
fn amd_native_profile_rejects_report_tamper_stale_vcek_and_root_substitution() {
    let mut tampered = amd_request();
    mutate_cpu_b64(&mut tampered, "report_b64");
    assert_rejected(&tampered, AMD_NOW, "SNP report");

    assert_rejected(&amd_request(), 2_000_000_000, "stale");

    let mut substituted = amd_request();
    let mut bytes = BASE64.decode(&substituted.quote.endorsements[0]).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    substituted.quote.endorsements[0] = BASE64.encode(bytes);
    substituted.report.hw_quote = Some(substituted.quote.clone());
    assert_rejected(&substituted, AMD_NOW, "not authenticated by admin policy");
}

#[test]
fn intel_tdx_profile_verifies_real_quote_collateral_and_admin_root() {
    let verdict = verify(tdx_request(), TDX_NOW);
    assert!(verdict.ok, "{:?}", verdict.reason);
    assert!(verdict.roots.contains(&"intel_tdx_dcap".to_owned()));
    assert!(verdict.matched_measurements["vendor"].contains_key("mr_seam"));
    assert!(verdict.matched_measurements["workload"].contains_key("mr_td"));
}

#[test]
fn intel_tdx_profile_rejects_quote_tamper_stale_collateral_and_wrong_measurement() {
    let mut tampered = tdx_request();
    mutate_cpu_b64(&mut tampered, "quote_b64");
    assert_rejected(&tampered, TDX_NOW, "TDX quote");

    assert_rejected(
        &tdx_request(),
        TDX_NOW + 60 * 60 * 24 * 400,
        "verification failed",
    );

    let mut measurement = tdx_request();
    measurement.policy_measurement_collateral.insert(
        AttestationMeasurementLayer::Workload,
        json!({"mr_td": ["ff".repeat(48)]}),
    );
    assert_rejected(&measurement, TDX_NOW, "not in the admin golden set");
}

#[test]
fn azure_maa_vtpm_and_both_nvidia_composites_verify_all_required_layers() {
    for request in [nras_request(), maa_request()] {
        let verdict = verify(request, MAA_NOW);
        assert!(verdict.ok, "{:?}", verdict.reason);
        for root in [
            "azure_maa_jwt_jwks_issuer_nonce_claims",
            "nvidia_gpu_cert_chain",
            "nvidia_driver_rim",
            "nvidia_vbios_rim",
        ] {
            assert!(verdict.roots.contains(&root.to_owned()), "missing {root}");
        }
        assert!(verdict.matched_measurements["vendor"].contains_key("snp_launch_measurement"));
        assert!(verdict.matched_measurements["workload"].contains_key("vtpm_pcr_0"));
    }
}

#[test]
fn azure_maa_rejects_tamper_staleness_binding_platform_and_provider_authority() {
    let mut signature = maa_request();
    mutate_maa_jwt_signature(&mut signature);
    assert_rejected(&signature, MAA_NOW, "signature is invalid");

    assert_rejected(&maa_request(), MAA_NOW + 7_200, "stale");

    let mut binding = maa_request();
    binding.quote.binding = "99".repeat(32);
    binding.evidence_binding.quote_binding = binding.quote.binding.clone();
    binding.report.hw_quote = Some(binding.quote.clone());
    recompute_expected_binding(&mut binding);
    assert_rejected(&binding, MAA_NOW, "does not bind");

    let mut platform = maa_request();
    rebind_platform(&mut platform, "onprem-snp");
    assert_rejected(
        &platform,
        MAA_NOW,
        "only for an admin-approved Azure platform",
    );

    let mut authority = maa_request();
    authority.quote.metadata = json!({"jwks": {"keys": []}});
    authority.report.hw_quote = Some(authority.quote.clone());
    assert_rejected(&authority, MAA_NOW, "attempts to supply authority");
}

#[test]
fn azure_maa_rejects_wrong_pcr_hcl_ak_and_unpinned_jwks() {
    let mut pcr = maa_request();
    pcr.policy_measurement_collateral.insert(
        AttestationMeasurementLayer::Workload,
        json!({"vtpm_pcr_0": ["cc".repeat(32)]}),
    );
    assert_rejected(&pcr, MAA_NOW, "not in the admin golden");

    let mut ak = maa_request();
    mutate_tpm_ak_name(&mut ak);
    assert_rejected(&ak, MAA_NOW, "AK public area does not derive");

    let mut jwks = maa_request();
    jwks.quote.endorsements[0] = BASE64.encode(br#"{"keys":[]}"#);
    jwks.report.hw_quote = Some(jwks.quote.clone());
    assert_rejected(&jwks, MAA_NOW, "not authenticated by admin policy");
}

#[test]
fn strict_input_rejects_unknown_fields_and_oversized_or_malformed_payloads() {
    let mut value = serde_json::to_value(maa_request()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("provider_root".to_owned(), json!("forbidden"));
    let verdict = verify_json_at(&serde_json::to_vec(&value).unwrap(), MAA_NOW);
    assert!(!verdict.ok);
    assert!(verdict.reason.unwrap().contains("unknown field"));

    let malformed = vec![b' '; MAX_VERIFIER_INPUT_BYTES + 1];
    let verdict = verify_json_at(&malformed, MAA_NOW);
    assert!(!verdict.ok);
    assert!(verdict.reason.unwrap().contains("invalid"));
}

#[test]
fn gateway_environment_can_only_confirm_authenticated_stdin() {
    let request = maa_request();
    assert!(validate_environment_values(&request, |name| match name {
        "MAYHEM_HW_VERIFY_KIND" => Some(request.kind.as_str().to_owned()),
        "MAYHEM_HW_VERIFY_BINDING" => Some(request.expected_binding.clone()),
        "MAYHEM_HW_VERIFY_PLATFORM" => request.declared_platform.clone(),
        "MAYHEM_HW_VERIFY_ATTESTATION_TIER" => Some("3".to_owned()),
        _ => None,
    })
    .is_ok());
    let error = validate_environment_values(&request, |name| {
        (name == "MAYHEM_HW_VERIFY_PLATFORM").then(|| "provider-chosen".to_owned())
    })
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("disagrees with authenticated stdin"));
}

fn verify(request: VerifyRequest, now: u64) -> VerifierVerdict {
    verify_json_at(&serde_json::to_vec(&request).unwrap(), now)
}

fn assert_rejected(request: &VerifyRequest, now: u64, expected: &str) {
    let verdict = verify(request.clone(), now);
    assert!(!verdict.ok, "unexpected acceptance");
    let reason = verdict.reason.unwrap_or_default();
    assert!(
        reason.contains(expected),
        "expected {expected:?} in rejection, got {reason:?}"
    );
}

fn amd_request() -> VerifyRequest {
    static REQUEST: OnceLock<VerifyRequest> = OnceLock::new();
    REQUEST
        .get_or_init(|| {
            let report = SnpAttestationReport::from_bytes(AMD_REPORT).unwrap();
            let quote_binding = hex::encode(&report.report_data[..32]);
            let device_id = device_digest(&report.chip_id).unwrap();
            let launch = hex::encode(report.measurement);
            let evidence = json!({
                "schema_version": 1,
                "platform": "onprem-sev-snp",
                "cpu": {
                    "profile": "amd_sev_snp_vcek_v1",
                    "report_b64": BASE64.encode(AMD_REPORT),
                    "vcek_der_b64": BASE64.encode(AMD_VCEK),
                }
            })
            .to_string();
            build_request(
                HardwareQuoteKind::AmdSevSnpVcek,
                "onprem-sev-snp",
                quote_binding,
                device_id,
                evidence,
                BTreeMap::from([
                    (
                        AttestationMeasurementLayer::Cpu,
                        json!({"snp_launch_measurement": [launch.clone()]}),
                    ),
                    (
                        AttestationMeasurementLayer::Workload,
                        json!({"snp_launch_measurement": [launch]}),
                    ),
                ]),
                vec![
                    RootMaterial::plain(
                        "amd-milan-ark",
                        AttestationTrustDataKind::TrustAnchor,
                        "application/pkix-cert",
                        AMD_ARK,
                    ),
                    RootMaterial::plain(
                        "amd-milan-ask",
                        AttestationTrustDataKind::TrustAnchor,
                        "application/pkix-cert",
                        AMD_ASK,
                    ),
                ],
                AMD_NOW,
            )
        })
        .clone()
}

fn tdx_request() -> VerifyRequest {
    static REQUEST: OnceLock<VerifyRequest> = OnceLock::new();
    REQUEST
        .get_or_init(|| {
            let collateral: QuoteCollateralV3 = serde_json::from_slice(TDX_COLLATERAL).unwrap();
            let verified = QuoteVerifier::new(TDX_ROOT.to_vec())
                .verify(TDX_QUOTE, &collateral, TDX_NOW)
                .unwrap();
            assert_eq!(verified.status, "UpToDate");
            let td = verified.report.as_td10().unwrap();
            let quote_binding = hex::encode(&td.report_data[..32]);
            let device_id = device_digest(&verified.ppid).unwrap();
            let evidence = json!({
                "schema_version": 1,
                "platform": "onprem-tdx",
                "cpu": {
                    "profile": "intel_tdx_dcap_v1",
                    "quote_b64": BASE64.encode(TDX_QUOTE),
                    "collateral": collateral,
                }
            })
            .to_string();
            build_request(
                HardwareQuoteKind::IntelTdxDcap,
                "onprem-tdx",
                quote_binding,
                device_id,
                evidence,
                BTreeMap::from([
                    (
                        AttestationMeasurementLayer::Cpu,
                        json!({"mr_seam": [hex::encode(td.mr_seam)]}),
                    ),
                    (
                        AttestationMeasurementLayer::Workload,
                        json!({"mr_td": [hex::encode(td.mr_td)]}),
                    ),
                ]),
                vec![RootMaterial::plain(
                    "intel-dcap-root",
                    AttestationTrustDataKind::TrustAnchor,
                    "application/pkix-cert",
                    TDX_ROOT,
                )],
                TDX_NOW,
            )
        })
        .clone()
}

fn maa_request() -> VerifyRequest {
    static REQUEST: OnceLock<VerifyRequest> = OnceLock::new();
    REQUEST
        .get_or_init(|| build_maa_request(HardwareQuoteKind::NvidiaNvtrustOfflineJwt))
        .clone()
}

fn nras_request() -> VerifyRequest {
    static REQUEST: OnceLock<VerifyRequest> = OnceLock::new();
    REQUEST
        .get_or_init(|| build_maa_request(HardwareQuoteKind::NvidiaNrasJwt))
        .clone()
}

fn build_maa_request(kind: HardwareQuoteKind) -> VerifyRequest {
    let quote_binding = "33".repeat(32);
    let launch = "ab".repeat(48);
    let chip_id = "cd".repeat(64);
    let pcr = "42".repeat(32);

    let mut rng = ChaCha20Rng::from_seed([71; 32]);
    let issuer_private = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let ak_private = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let issuer_public = RsaPublicKey::from(&issuer_private);
    let ak_public = RsaPublicKey::from(&ak_private);
    let jwks = json!({
        "keys": [{
            "kty": "RSA",
            "kid": "maa-test-key",
            "n": b64url(&issuer_public.n().to_bytes_be()),
            "e": b64url(&issuer_public.e().to_bytes_be()),
            "alg": "RS256",
            "use": "sig",
            "key_ops": ["verify"],
        }]
    });
    let ak_jwk = json!({
        "kty": "RSA",
        "kid": "HCLAkPub",
        "n": b64url(&ak_public.n().to_bytes_be()),
        "e": b64url(&ak_public.e().to_bytes_be()),
        "key_ops": ["sign"],
    });
    let workload_quote = make_tpm_quote(&ak_private, &quote_binding, &pcr);
    let user_claims = format!("{{\"user-claims\": {{\"nonce\": \"{quote_binding}\"}}}}");
    let claims = json!({
        "iat": MAA_NOW - 1,
        "nbf": MAA_NOW - 1,
        "exp": MAA_NOW + 3_600,
        "iss": "https://unit.attest.azure.net",
        "x-ms-attestation-type": "sevsnpvm",
        "x-ms-compliance-status": "azure-compliant-cvm",
        "x-ms-policy-hash": b64url(&[7u8; 32]),
        "x-ms-runtime": {
            "keys": [ak_jwk],
            "user-data": hex::encode_upper(Sha512::digest(user_claims.as_bytes())),
            "vm-configuration": {
                "secure-boot": true,
                "tpm-enabled": true,
                "tpm-persisted": true,
            }
        },
        "x-ms-sevsnpvm-bootloader-svn": 3,
        "x-ms-sevsnpvm-chip-family": "Genoa",
        "x-ms-sevsnpvm-chipid": chip_id,
        "x-ms-sevsnpvm-is-debuggable": false,
        "x-ms-sevsnpvm-launchmeasurement": launch,
        "x-ms-sevsnpvm-microcode-svn": 9,
        "x-ms-sevsnpvm-migration-allowed": false,
        "x-ms-sevsnpvm-snpfw-svn": 7,
        "x-ms-sevsnpvm-tee-svn": 5,
        "x-ms-sevsnpvm-vmpl": 0,
    });
    let jwt = sign_jwt(&issuer_private, &claims);
    let cpu = json!({
        "profile": "azure_maa_snp_v1",
        "maa_jwt": jwt,
        "workload_quote": workload_quote,
    });
    let evidence = json!({
        "platform_id": "azure-ncc",
        "mayhem_cpu_evidence": cpu,
    })
    .to_string();
    let jwks_bytes = serde_json::to_vec(&jwks).unwrap();
    build_request(
        kind,
        "azure-ncc",
        quote_binding,
        hex::encode(Sha256::digest(hex::decode("cd".repeat(64)).unwrap())),
        evidence,
        BTreeMap::from([
            (
                AttestationMeasurementLayer::Cpu,
                json!({"snp_launch_measurement": ["ab".repeat(48)]}),
            ),
            (
                AttestationMeasurementLayer::Gpu,
                json!({"gpu_measurement": ["55".repeat(32)]}),
            ),
            (
                AttestationMeasurementLayer::Workload,
                json!({"vtpm_pcr_0": [pcr]}),
            ),
        ]),
        vec![RootMaterial::sourced(
            "azure-maa-jwks",
            AttestationTrustDataKind::VerificationKey,
            "application/jwk-set+json",
            &jwks_bytes,
            "azure-maa",
            "https://unit.attest.azure.net",
            "/certs",
        )],
        MAA_NOW,
    )
}

#[derive(Clone)]
struct RootMaterial {
    id: String,
    kind: AttestationTrustDataKind,
    media_type: String,
    bytes: Vec<u8>,
    origin: Option<(String, String, String)>,
}

impl RootMaterial {
    fn plain(id: &str, kind: AttestationTrustDataKind, media_type: &str, bytes: &[u8]) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            media_type: media_type.to_owned(),
            bytes: bytes.to_vec(),
            origin: None,
        }
    }

    fn sourced(
        id: &str,
        kind: AttestationTrustDataKind,
        media_type: &str,
        bytes: &[u8],
        origin_id: &str,
        origin: &str,
        path: &str,
    ) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            media_type: media_type.to_owned(),
            bytes: bytes.to_vec(),
            origin: Some((origin_id.to_owned(), origin.to_owned(), path.to_owned())),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_request(
    kind: HardwareQuoteKind,
    platform: &str,
    quote_binding: String,
    device_id: String,
    evidence: String,
    layers: BTreeMap<AttestationMeasurementLayer, Value>,
    roots: Vec<RootMaterial>,
    now: u64,
) -> VerifyRequest {
    let mut trust_data = Vec::new();
    let mut required_trust_data = BTreeSet::new();
    let mut measurement_trust_data = BTreeSet::new();
    let mut layer_ids = BTreeMap::new();
    let mut origins = BTreeMap::new();
    let mut endorsements = Vec::new();

    for root in roots {
        let source = root.origin.as_ref().map(|(id, origin, path)| {
            origins.insert(
                id.clone(),
                AttestationOriginPin {
                    id: id.clone(),
                    https_origin: origin.clone(),
                },
            );
            AttestationTrustDataSource {
                origin_pin: id.clone(),
                path: path.clone(),
            }
        });
        required_trust_data.insert(root.id.clone());
        endorsements.push(BASE64.encode(&root.bytes));
        trust_data.push(AttestationTrustDataRef {
            id: root.id,
            kind: root.kind,
            sha256: hex::encode(Sha256::digest(&root.bytes)),
            media_type: root.media_type,
            max_bytes: root.bytes.len() as u64,
            valid_from_epoch: Some(1),
            valid_until_epoch: None,
            source,
        });
    }
    for (layer, document) in &layers {
        let id = format!("measurement-{}", layer_name(*layer));
        let bytes = serde_json::to_vec(document).unwrap();
        required_trust_data.insert(id.clone());
        measurement_trust_data.insert(id.clone());
        layer_ids.insert(*layer, id.clone());
        trust_data.push(AttestationTrustDataRef {
            id,
            kind: AttestationTrustDataKind::Measurement,
            sha256: hex::encode(Sha256::digest(&bytes)),
            media_type: "application/vnd.mayhem.tier3-measurements+json".to_owned(),
            max_bytes: bytes.len() as u64,
            valid_from_epoch: Some(1),
            valid_until_epoch: None,
            source: None,
        });
    }
    let policy = AdminAttestationPolicy {
        schema_version: ATTESTATION_POLICY_SCHEMA_VERSION,
        sequence: 1,
        previous_policy_digest: None,
        issued_epoch: 1,
        effective_epoch: 1,
        expires_epoch: None,
        min_verifier_version: 1,
        emergency_disabled_quote_kinds: BTreeSet::new(),
        origin_pins: origins.into_values().collect(),
        trust_data,
        quote_kinds: HardwareQuoteKind::ALL
            .into_iter()
            .map(|candidate| {
                let active = candidate == kind;
                AttestationQuoteKindPolicy {
                    kind: candidate,
                    enabled: active,
                    verifier_profile: profile(candidate),
                    evidence_schema_version: 1,
                    required_trust_data: if active {
                        required_trust_data.clone()
                    } else {
                        BTreeSet::new()
                    },
                    measurement_trust_data: if active {
                        measurement_trust_data.clone()
                    } else {
                        BTreeSet::new()
                    },
                    platforms: if active {
                        BTreeSet::from([platform.to_owned()])
                    } else {
                        BTreeSet::new()
                    },
                    required_measurement_layers: if active {
                        layers.keys().copied().collect()
                    } else {
                        BTreeSet::new()
                    },
                }
            })
            .collect(),
    };
    let validated = ValidatedAttestationPolicy::validate(policy.clone()).unwrap();
    let route = HardwareQuoteRoutePolicyBinding {
        enclave_id: ENCLAVE_ID.to_owned(),
        device_id: device_id.clone(),
        kind,
        evidence_schema_version: 1,
        policy_sequence: 1,
        policy_digest: validated.digest().to_owned(),
        platform: Some(platform.to_owned()),
    };
    let binding = EvidenceBinding::new(
        &route,
        NONCE,
        ENCLAVE_ID,
        device_id.clone(),
        quote_binding.clone(),
    )
    .unwrap();
    let expected_binding = hex::encode(binding.digest().unwrap());
    let quote = HardwareQuote {
        kind,
        evidence,
        binding: quote_binding,
        endorsements,
        metadata: Value::Null,
    };
    let report = mayhem_proto::AttestationReport {
        schema_version: ATTESTATION_SCHEMA_VERSION,
        alg: ATTESTATION_ALG.to_owned(),
        enclave_id: ENCLAVE_ID.to_owned(),
        enclave_pubkey: "enclave-key".to_owned(),
        provider_pubkey: "provider-key".to_owned(),
        manifest_hash: "aa".repeat(32),
        binary_hash: "bb".repeat(32),
        att_tier: 3,
        hw_quote: Some(quote.clone()),
        boot_epoch: 1,
        report_ts: now,
        nonce_u: NONCE.to_owned(),
        runtime_config: AttestationRuntimeConfig::default(),
        sig_enclave: "sig-enclave".to_owned(),
        sig_provider: "sig-provider".to_owned(),
    };
    let vendor = layers
        .get(&AttestationMeasurementLayer::Cpu)
        .cloned()
        .unwrap();
    let workload = layers
        .get(&AttestationMeasurementLayer::Workload)
        .cloned()
        .unwrap();
    VerifyRequest {
        schema_version: 1,
        kind,
        expected_binding,
        declared_platform: Some(platform.to_owned()),
        evidence_binding: EvidenceBindingInput {
            kind,
            evidence_schema_version: 1,
            policy_sequence: 1,
            policy_digest: validated.digest().to_owned(),
            platform: Some(platform.to_owned()),
            nonce: NONCE.to_owned(),
            enclave_id: ENCLAVE_ID.to_owned(),
            device_id,
            quote_binding: quote.binding.clone(),
        },
        admin_policy: AdminPolicyInput {
            digest: validated.digest().to_owned(),
            record: policy,
        },
        admin_enclave_binding: AdminEnclaveAttestationBinding {
            enclave_id: ENCLAVE_ID.to_owned(),
            kind,
            platform: Some(platform.to_owned()),
            measurement_trust_data: layer_ids,
        },
        policy_measurement_collateral: layers,
        managed_verifier: ManagedVerifierInput {
            id: "mayhem-attestation-verifier".to_owned(),
            version: 1,
            executable_sha256: "dd".repeat(32),
        },
        golden_measurement_layers: json!({
            "vendor": vendor,
            "workload": workload,
        }),
        quote,
        report,
        contract: ContractInput {
            enclave_id: ENCLAVE_ID.to_owned(),
            admin_pubkey: "admin".to_owned(),
            model_id: "test-model".to_owned(),
            model_class: "test".to_owned(),
            artifact_root: "01".repeat(32),
            artifact_sidecar_roots: BTreeMap::new(),
            manifest_hash: "aa".repeat(32),
            binary_hash: "bb".repeat(32),
            launch_measurements: json!({}),
            att_tier: 3,
            caps: json!({}),
        },
    }
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

fn layer_name(layer: AttestationMeasurementLayer) -> &'static str {
    match layer {
        AttestationMeasurementLayer::Cpu => "cpu",
        AttestationMeasurementLayer::Gpu => "gpu",
        AttestationMeasurementLayer::Workload => "workload",
    }
}

fn sign_jwt(key: &RsaPrivateKey, claims: &Value) -> String {
    let header = json!({
        "alg": "RS256",
        "jku": "https://unit.attest.azure.net/certs",
        "kid": "maa-test-key",
        "typ": "JWT",
    });
    let header = b64url(&serde_json::to_vec(&header).unwrap());
    let claims = b64url(&serde_json::to_vec(claims).unwrap());
    let input = format!("{header}.{claims}");
    let signature = RsaSigningKey::<Sha256>::new(key.clone()).sign(input.as_bytes());
    format!("{input}.{}", b64url(&signature.to_vec()))
}

fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn make_tpm_quote(ak_private: &RsaPrivateKey, quote_binding: &str, pcr: &str) -> TpmQuoteEvidence {
    let public = make_rsa_ak_public(&RsaPublicKey::from(ak_private));
    let mut reader = &public[..];
    let length = usize::from(u16::from_be_bytes([reader[0], reader[1]]));
    reader = &reader[2..2 + length];
    let mut name = 0x000bu16.to_be_bytes().to_vec();
    name.extend_from_slice(&Sha256::digest(reader));
    let pcr_value = TpmPcrValue {
        hash_algorithm: TpmHashAlgorithm::Sha256,
        index: 0,
        digest: pcr.to_owned(),
    };
    let pcr_digest = Sha256::digest(hex::decode(pcr).unwrap());
    let attest = make_tpm_attest(&name, &hex::decode(quote_binding).unwrap(), &pcr_digest);
    let signature = RsaSigningKey::<Sha256>::new(ak_private.clone()).sign(&attest);
    let mut encoded_signature = 0x0014u16.to_be_bytes().to_vec();
    encoded_signature.extend_from_slice(&0x000bu16.to_be_bytes());
    encoded_signature.extend(tpm2b(&signature.to_vec()));
    TpmQuoteEvidence {
        schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
        ak_public_b64: BASE64.encode(public),
        ak_name_b64: BASE64.encode(name),
        quote_attest_b64: BASE64.encode(attest),
        quote_signature_b64: BASE64.encode(encoded_signature),
        pcr_values: vec![pcr_value],
    }
}

fn make_rsa_ak_public(key: &RsaPublicKey) -> Vec<u8> {
    const ATTRIBUTES: u32 = 0x0000_0002 | 0x0000_0010 | 0x0000_0020 | 0x0001_0000 | 0x0004_0000;
    let mut public = Vec::new();
    public.extend_from_slice(&0x0001u16.to_be_bytes());
    public.extend_from_slice(&0x000bu16.to_be_bytes());
    public.extend_from_slice(&ATTRIBUTES.to_be_bytes());
    public.extend_from_slice(&0u16.to_be_bytes());
    public.extend_from_slice(&0x0010u16.to_be_bytes());
    public.extend_from_slice(&0x0014u16.to_be_bytes());
    public.extend_from_slice(&0x000bu16.to_be_bytes());
    public.extend_from_slice(&(key.n().bits() as u16).to_be_bytes());
    public.extend_from_slice(&0u32.to_be_bytes());
    let modulus = key.n().to_bytes_be();
    public.extend(tpm2b(&modulus));
    tpm2b(&public)
}

fn make_tpm_attest(ak_name: &[u8], binding: &[u8], pcr_digest: &[u8]) -> Vec<u8> {
    let mut attest = Vec::new();
    attest.extend_from_slice(&0xff54_4347u32.to_be_bytes());
    attest.extend_from_slice(&0x8018u16.to_be_bytes());
    attest.extend(tpm2b(ak_name));
    attest.extend(tpm2b(binding));
    attest.extend_from_slice(&0u64.to_be_bytes());
    attest.extend_from_slice(&0u32.to_be_bytes());
    attest.extend_from_slice(&0u32.to_be_bytes());
    attest.push(1);
    attest.extend_from_slice(&1u64.to_be_bytes());
    attest.extend_from_slice(&1u32.to_be_bytes());
    attest.extend_from_slice(&0x000bu16.to_be_bytes());
    attest.push(3);
    attest.extend_from_slice(&[1, 0, 0]);
    attest.extend(tpm2b(pcr_digest));
    attest
}

fn tpm2b(bytes: &[u8]) -> Vec<u8> {
    let mut encoded = (bytes.len() as u16).to_be_bytes().to_vec();
    encoded.extend_from_slice(bytes);
    encoded
}

fn mutate_cpu_b64(request: &mut VerifyRequest, field: &str) {
    let mut value: Value = serde_json::from_str(&request.quote.evidence).unwrap();
    let cpu = value.get_mut("cpu").unwrap().as_object_mut().unwrap();
    let mut bytes = BASE64.decode(cpu[field].as_str().unwrap()).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    cpu.insert(field.to_owned(), json!(BASE64.encode(bytes)));
    request.quote.evidence = value.to_string();
    request.report.hw_quote = Some(request.quote.clone());
}

fn mutate_maa_jwt_signature(request: &mut VerifyRequest) {
    let mut value: Value = serde_json::from_str(&request.quote.evidence).unwrap();
    let jwt = value["mayhem_cpu_evidence"]["maa_jwt"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut parts = jwt.split('.').map(str::to_owned).collect::<Vec<_>>();
    let mut signature = URL_SAFE_NO_PAD.decode(&parts[2]).unwrap();
    signature[0] ^= 1;
    parts[2] = b64url(&signature);
    value["mayhem_cpu_evidence"]["maa_jwt"] = json!(parts.join("."));
    request.quote.evidence = value.to_string();
    request.report.hw_quote = Some(request.quote.clone());
}

fn mutate_tpm_ak_name(request: &mut VerifyRequest) {
    let mut value: Value = serde_json::from_str(&request.quote.evidence).unwrap();
    let quote = &mut value["mayhem_cpu_evidence"]["workload_quote"];
    let mut name = BASE64
        .decode(quote["ak_name_b64"].as_str().unwrap())
        .unwrap();
    *name.last_mut().unwrap() ^= 1;
    quote["ak_name_b64"] = json!(BASE64.encode(name));
    request.quote.evidence = value.to_string();
    request.report.hw_quote = Some(request.quote.clone());
}

fn rebind_platform(request: &mut VerifyRequest, platform: &str) {
    request.declared_platform = Some(platform.to_owned());
    request.evidence_binding.platform = Some(platform.to_owned());
    request.admin_enclave_binding.platform = Some(platform.to_owned());
    let kind_policy = request
        .admin_policy
        .record
        .quote_kinds
        .iter_mut()
        .find(|entry| entry.kind == request.kind)
        .unwrap();
    kind_policy.platforms = BTreeSet::from([platform.to_owned()]);
    let validated =
        ValidatedAttestationPolicy::validate(request.admin_policy.record.clone()).unwrap();
    request.admin_policy.record = validated.policy().clone();
    request.admin_policy.digest = validated.digest().to_owned();
    request.evidence_binding.policy_digest = validated.digest().to_owned();
    let mut evidence: Value = serde_json::from_str(&request.quote.evidence).unwrap();
    evidence["platform_id"] = json!(platform);
    request.quote.evidence = evidence.to_string();
    request.report.hw_quote = Some(request.quote.clone());
    recompute_expected_binding(request);
}

fn recompute_expected_binding(request: &mut VerifyRequest) {
    let route = HardwareQuoteRoutePolicyBinding {
        enclave_id: request.evidence_binding.enclave_id.clone(),
        device_id: request.evidence_binding.device_id.clone(),
        kind: request.kind,
        evidence_schema_version: request.evidence_binding.evidence_schema_version,
        policy_sequence: request.evidence_binding.policy_sequence,
        policy_digest: request.evidence_binding.policy_digest.clone(),
        platform: request.evidence_binding.platform.clone(),
    };
    let binding = EvidenceBinding::new(
        &route,
        request.evidence_binding.nonce.clone(),
        request.evidence_binding.enclave_id.clone(),
        request.evidence_binding.device_id.clone(),
        request.evidence_binding.quote_binding.clone(),
    )
    .unwrap();
    request.expected_binding = hex::encode(binding.digest().unwrap());
}
