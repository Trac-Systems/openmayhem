use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mayhem_proto::{
    AttestationTrustDataKind, AttestationVerifierProfile, HardwareQuote, HardwareQuoteKind,
    TpmHashAlgorithm, TpmPcrPolicy, TpmPcrValue, TpmQuoteEvidence, TPM_PCR_POLICY_SCHEMA_VERSION,
    TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
};
use p256::PublicKey as P256PublicKey;
use rsa::{
    pkcs1v15::{Signature as RsaSignature, VerifyingKey as RsaVerifyingKey},
    pss::{Signature as RsaPssSignature, VerifyingKey as RsaPssVerifyingKey},
    signature::Verifier as _,
    BigUint, RsaPublicKey,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::{
    x509::{
        validate_ek_trust_anchors, verify_ek_certificate_chain, verify_p256_signature,
        EkCertificateError, EkPublicKey,
    },
    ActivatedTpmIdentity, CollateralInventory, EvidenceBinding, EvidenceBindingError,
    ValidatedAttestationPolicy,
};

const TPM_GENERATED_VALUE: u32 = 0xff54_4347;
const TPM_ST_ATTEST_QUOTE: u16 = 0x8018;
const TPM_ALG_RSA: u16 = 0x0001;
const TPM_ALG_SHA256: u16 = 0x000b;
const TPM_ALG_NULL: u16 = 0x0010;
const TPM_ALG_RSASSA: u16 = 0x0014;
const TPM_ALG_RSAPSS: u16 = 0x0016;
const TPM_ALG_ECDSA: u16 = 0x0018;
const TPM_ALG_ECC: u16 = 0x0023;
const TPM_ECC_NIST_P256: u16 = 0x0003;
const TPM_SHA256_DIGEST_BYTES: usize = 32;
const TPM_SHA256_NAME_BYTES: usize = 2 + TPM_SHA256_DIGEST_BYTES;
const TPM_PC_CLIENT_PCR_SELECT_BYTES: usize = 3;
const TPM_MAX_PCR_INDEX: u8 = 23;

const TPMA_OBJECT_FIXED_TPM: u32 = 0x0000_0002;
const TPMA_OBJECT_FIXED_PARENT: u32 = 0x0000_0010;
const TPMA_OBJECT_SENSITIVE_DATA_ORIGIN: u32 = 0x0000_0020;
const TPMA_OBJECT_RESTRICTED: u32 = 0x0001_0000;
const TPMA_OBJECT_DECRYPT: u32 = 0x0002_0000;
const TPMA_OBJECT_SIGN_ENCRYPT: u32 = 0x0004_0000;
const REQUIRED_AK_ATTRIBUTES: u32 = TPMA_OBJECT_FIXED_TPM
    | TPMA_OBJECT_FIXED_PARENT
    | TPMA_OBJECT_SENSITIVE_DATA_ORIGIN
    | TPMA_OBJECT_RESTRICTED
    | TPMA_OBJECT_SIGN_ENCRYPT;

pub const MAX_TPM_QUOTE_EVIDENCE_BYTES: usize = 256 * 1024;
const MAX_TPM_ENDORSEMENT_CERTIFICATES: usize = 8;
const MAX_TPM_ENDORSEMENT_B64_BYTES: usize = 88 * 1024;
const MAX_TPM_AK_PUBLIC_B64_BYTES: usize = 8 * 1024;
const MAX_TPM_NAME_B64_BYTES: usize = 256;
const MAX_TPM_ATTEST_B64_BYTES: usize = 8 * 1024;
const MAX_TPM_SIGNATURE_B64_BYTES: usize = 2 * 1024;

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum TpmQuoteError {
    #[error("TPM verifier requires an enabled TPM quote policy")]
    PolicyNotEnabled,
    #[error("TPM policy uses verifier profile {0:?}")]
    PolicyProfileMismatch(AttestationVerifierProfile),
    #[error("TPM policy evidence schema {actual} is unsupported; expected {expected}")]
    PolicySchemaMismatch { expected: u32, actual: u32 },
    #[error("TPM verifier materials do not match the session policy binding")]
    PolicyBindingMismatch,
    #[error("TPM policy trust data {0} is missing from the authenticated collateral inventory")]
    MissingCollateral(String),
    #[error("TPM trust anchor {0} must be one DER application/pkix-cert")]
    InvalidTrustAnchor(String),
    #[error("TPM policy requires unsupported trust-data kind {kind:?} for {id}")]
    UnsupportedTrustDataKind {
        id: String,
        kind: AttestationTrustDataKind,
    },
    #[error("TPM policy must reference exactly one PCR measurement document")]
    InvalidPcrPolicyCount,
    #[error("TPM PCR policy is invalid: {0}")]
    InvalidPcrPolicy(String),
    #[error("hardware quote kind is not TPM2 EK")]
    QuoteKindMismatch,
    #[error("TPM quote binding does not match the expected attestation report")]
    QuoteBindingMismatch,
    #[error("TPM quote metadata must be null; verifier inputs come only from admin policy")]
    ProviderMetadataRejected,
    #[error("TPM quote evidence JSON is invalid: {0}")]
    InvalidEvidenceJson(String),
    #[error("TPM quote evidence exceeds verifier bounds")]
    EvidenceTooLarge,
    #[error("TPM evidence schema {actual} is unsupported; expected {expected}")]
    EvidenceSchemaMismatch { expected: u32, actual: u32 },
    #[error("TPM evidence field {0} is not valid base64")]
    InvalidBase64(&'static str),
    #[error("TPM EK certificate chain validation failed: {0}")]
    EkCertificate(#[from] EkCertificateError),
    #[error("TPM EK certificate device identity does not match the selected route")]
    DeviceMismatch,
    #[error("TPM EK certificate public key does not match the activated EK")]
    ActivatedEkMismatch,
    #[error("TPM AK public area is invalid: {0}")]
    InvalidAkPublic(String),
    #[error("TPM AK Name is invalid")]
    InvalidAkName,
    #[error("TPM AK public area does not derive the supplied AK Name")]
    AkNameMismatch,
    #[error("TPM AK differs from the key bound by ActivateCredential")]
    ActivatedAkMismatch,
    #[error("TPM quote attestation is invalid: {0}")]
    InvalidAttestation(String),
    #[error("TPM quote extraData is not bound to this policy, nonce, enclave, and device")]
    NonceBindingMismatch,
    #[error("TPM quote signature encoding or algorithm is invalid")]
    InvalidSignatureEncoding,
    #[error("TPM quote signature does not verify with the activated AK")]
    InvalidSignature,
    #[error("TPM quote PCR selection differs from immutable admin policy")]
    PcrSelectionMismatch,
    #[error("TPM quote PCR values are incomplete, duplicated, or malformed")]
    InvalidPcrValues,
    #[error("TPM quote PCR digest does not match the selected PCR values")]
    PcrDigestMismatch,
    #[error("TPM evidence binding is invalid: {0}")]
    EvidenceBinding(#[from] EvidenceBindingError),
}

#[derive(Clone, Debug)]
pub struct TpmVerificationMaterials {
    trust_anchor_der: Vec<Vec<u8>>,
    pcr_policy: TpmPcrPolicy,
    policy_sequence: u64,
    policy_digest: String,
    evidence_schema_version: u32,
}

impl TpmVerificationMaterials {
    pub fn from_policy(
        policy: &ValidatedAttestationPolicy,
        collateral: &CollateralInventory,
        now_unix: u64,
    ) -> Result<Self, TpmQuoteError> {
        let kind_policy = policy
            .quote_kind(HardwareQuoteKind::Tpm2QuoteEk)
            .expect("validated policy contains every quote kind");
        if !kind_policy.enabled {
            return Err(TpmQuoteError::PolicyNotEnabled);
        }
        if kind_policy.verifier_profile != AttestationVerifierProfile::Tpm2EkActivateCredentialV1 {
            return Err(TpmQuoteError::PolicyProfileMismatch(
                kind_policy.verifier_profile,
            ));
        }
        if kind_policy.evidence_schema_version != TPM_QUOTE_EVIDENCE_SCHEMA_VERSION {
            return Err(TpmQuoteError::PolicySchemaMismatch {
                expected: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
                actual: kind_policy.evidence_schema_version,
            });
        }

        let mut trust_anchor_der = Vec::new();
        for id in &kind_policy.required_trust_data {
            let reference = policy
                .trust_data(id)
                .expect("validated kind policy references existing trust data");
            let bytes = collateral
                .get(&reference.sha256)
                .ok_or_else(|| TpmQuoteError::MissingCollateral(id.clone()))?
                .bytes();
            match reference.kind {
                AttestationTrustDataKind::TrustAnchor => {
                    if reference.media_type != "application/pkix-cert"
                        || bytes.is_empty()
                        || bytes.len() > reference.max_bytes as usize
                    {
                        return Err(TpmQuoteError::InvalidTrustAnchor(id.clone()));
                    }
                    trust_anchor_der.push(bytes.to_vec());
                }
                AttestationTrustDataKind::Measurement => {}
                kind => {
                    return Err(TpmQuoteError::UnsupportedTrustDataKind {
                        id: id.clone(),
                        kind,
                    })
                }
            }
        }
        if trust_anchor_der.is_empty() {
            return Err(TpmQuoteError::InvalidTrustAnchor(
                "no trust anchor".to_owned(),
            ));
        }
        validate_ek_trust_anchors(&trust_anchor_der, now_unix)?;

        if kind_policy.measurement_trust_data.len() != 1 {
            return Err(TpmQuoteError::InvalidPcrPolicyCount);
        }
        let measurement_id = kind_policy
            .measurement_trust_data
            .iter()
            .next()
            .expect("checked one measurement");
        let reference = policy
            .trust_data(measurement_id)
            .expect("validated measurement references existing trust data");
        if reference.media_type != "application/vnd.mayhem.tpm-pcr-policy+json" {
            return Err(TpmQuoteError::InvalidPcrPolicy(
                "measurement media type is not the TPM PCR policy format".to_owned(),
            ));
        }
        let bytes = collateral
            .get(&reference.sha256)
            .ok_or_else(|| TpmQuoteError::MissingCollateral(measurement_id.clone()))?
            .bytes();
        let pcr_policy = serde_json::from_slice::<TpmPcrPolicy>(bytes)
            .map_err(|err| TpmQuoteError::InvalidPcrPolicy(err.to_string()))?;
        validate_pcr_policy(&pcr_policy)?;
        Ok(Self {
            trust_anchor_der,
            pcr_policy,
            policy_sequence: policy.policy().sequence,
            policy_digest: policy.digest().to_owned(),
            evidence_schema_version: kind_policy.evidence_schema_version,
        })
    }

    pub fn pcr_policy(&self) -> &TpmPcrPolicy {
        &self.pcr_policy
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedTpmQuote {
    pub device_id: String,
    pub ak_name_b64: String,
    pub selected_pcrs: BTreeSet<u8>,
    pub policy_sequence: u64,
    pub policy_digest: String,
    pub evidence_schema_version: u32,
    pub enclave_id: String,
    pub nonce: String,
}

pub fn verify_tpm_hardware_quote(
    quote: &HardwareQuote,
    expected: &EvidenceBinding,
    activated: &ActivatedTpmIdentity,
    materials: &TpmVerificationMaterials,
    now_unix: u64,
) -> Result<VerifiedTpmQuote, TpmQuoteError> {
    if quote.kind != HardwareQuoteKind::Tpm2QuoteEk
        || expected.kind != HardwareQuoteKind::Tpm2QuoteEk
    {
        return Err(TpmQuoteError::QuoteKindMismatch);
    }
    if expected.policy_sequence != materials.policy_sequence
        || expected.policy_digest != materials.policy_digest
        || expected.evidence_schema_version != materials.evidence_schema_version
    {
        return Err(TpmQuoteError::PolicyBindingMismatch);
    }
    if quote.binding != expected.quote_binding {
        return Err(TpmQuoteError::QuoteBindingMismatch);
    }
    if !quote.metadata.is_null() {
        return Err(TpmQuoteError::ProviderMetadataRejected);
    }
    if expected.evidence_schema_version != TPM_QUOTE_EVIDENCE_SCHEMA_VERSION {
        return Err(TpmQuoteError::EvidenceSchemaMismatch {
            expected: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            actual: expected.evidence_schema_version,
        });
    }
    if quote.evidence.len() > MAX_TPM_QUOTE_EVIDENCE_BYTES {
        return Err(TpmQuoteError::EvidenceTooLarge);
    }
    if quote.endorsements.is_empty()
        || quote.endorsements.len() > MAX_TPM_ENDORSEMENT_CERTIFICATES
        || quote
            .endorsements
            .iter()
            .any(|certificate| certificate.len() > MAX_TPM_ENDORSEMENT_B64_BYTES)
    {
        return Err(TpmQuoteError::EvidenceTooLarge);
    }
    let evidence = serde_json::from_str::<TpmQuoteEvidence>(&quote.evidence)
        .map_err(|err| TpmQuoteError::InvalidEvidenceJson(err.to_string()))?;
    if evidence.schema_version != TPM_QUOTE_EVIDENCE_SCHEMA_VERSION {
        return Err(TpmQuoteError::EvidenceSchemaMismatch {
            expected: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            actual: evidence.schema_version,
        });
    }

    let chain_der = quote
        .endorsements
        .iter()
        .map(|certificate| {
            BASE64
                .decode(certificate)
                .map_err(|_| TpmQuoteError::InvalidBase64("endorsements"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let trusted_ek =
        verify_ek_certificate_chain(&chain_der, &materials.trust_anchor_der, now_unix)?;
    if trusted_ek.device_id != expected.device_id {
        return Err(TpmQuoteError::DeviceMismatch);
    }
    let ek_spki_sha256 = hex::encode(Sha256::digest(&trusted_ek.canonical_spki_der));
    if ek_spki_sha256 != activated.ek_public_sha256() {
        return Err(TpmQuoteError::ActivatedEkMismatch);
    }
    match trusted_ek.public_key {
        EkPublicKey::Rsa(_) | EkPublicKey::EccP256(_) => {}
    }

    let ak_public = decode_b64(
        "ak_public_b64",
        &evidence.ak_public_b64,
        MAX_TPM_AK_PUBLIC_B64_BYTES,
    )?;
    let ak_name = decode_b64("ak_name_b64", &evidence.ak_name_b64, MAX_TPM_NAME_B64_BYTES)?;
    validate_tpm_name(&ak_name)?;
    if evidence.ak_name_b64 != activated.ak_name_b64() {
        return Err(TpmQuoteError::ActivatedAkMismatch);
    }
    let ak = parse_ak_public(&ak_public)?;
    let derived_name = derive_tpm_name(&ak_public)?;
    if !constant_time_equal(&derived_name, &ak_name) {
        return Err(TpmQuoteError::AkNameMismatch);
    }

    let attest = decode_b64(
        "quote_attest_b64",
        &evidence.quote_attest_b64,
        MAX_TPM_ATTEST_B64_BYTES,
    )?;
    let parsed_attest = parse_quote_attestation(&attest)?;
    let expected_extra_data = expected.digest()?;
    if !constant_time_equal(&parsed_attest.extra_data, &expected_extra_data) {
        return Err(TpmQuoteError::NonceBindingMismatch);
    }
    verify_pcrs(&parsed_attest, &evidence.pcr_values, &materials.pcr_policy)?;

    let signature = decode_b64(
        "quote_signature_b64",
        &evidence.quote_signature_b64,
        MAX_TPM_SIGNATURE_B64_BYTES,
    )?;
    verify_quote_signature(&ak, &attest, &signature)?;
    activated
        .bind_verified_quote(&evidence.ak_name_b64)
        .map_err(|_| TpmQuoteError::ActivatedAkMismatch)?;

    Ok(VerifiedTpmQuote {
        device_id: trusted_ek.device_id,
        ak_name_b64: evidence.ak_name_b64,
        selected_pcrs: parsed_attest.selected_pcrs,
        policy_sequence: expected.policy_sequence,
        policy_digest: expected.policy_digest.clone(),
        evidence_schema_version: expected.evidence_schema_version,
        enclave_id: expected.enclave_id.clone(),
        nonce: expected.nonce.clone(),
    })
}

fn validate_pcr_policy(policy: &TpmPcrPolicy) -> Result<(), TpmQuoteError> {
    if policy.schema_version != TPM_PCR_POLICY_SCHEMA_VERSION {
        return Err(TpmQuoteError::InvalidPcrPolicy(format!(
            "schema {} is unsupported",
            policy.schema_version
        )));
    }
    if policy.hash_algorithm != TpmHashAlgorithm::Sha256 {
        return Err(TpmQuoteError::InvalidPcrPolicy(
            "only SHA-256 PCR banks are supported".to_owned(),
        ));
    }
    if policy.pcrs.is_empty() || policy.pcrs.iter().any(|index| *index > TPM_MAX_PCR_INDEX) {
        return Err(TpmQuoteError::InvalidPcrPolicy(
            "PCR selection must contain indices 0 through 23".to_owned(),
        ));
    }
    Ok(())
}

enum AkPublic {
    Rsa {
        key: RsaPublicKey,
        scheme: Option<u16>,
    },
    EccP256 {
        key: P256PublicKey,
        scheme: Option<u16>,
    },
}

fn parse_ak_public(encoded: &[u8]) -> Result<AkPublic, TpmQuoteError> {
    let mut outer = TpmReader::new(encoded);
    let public_area = outer.tpm2b("TPM2B_PUBLIC", 2..=4096)?;
    outer.finish("TPM2B_PUBLIC")?;
    let mut reader = TpmReader::new(public_area);
    let key_type = reader.u16("AK type")?;
    let name_algorithm = reader.u16("AK name algorithm")?;
    if name_algorithm != TPM_ALG_SHA256 {
        return Err(invalid_ak("AK name algorithm must be SHA-256"));
    }
    let attributes = reader.u32("AK object attributes")?;
    if attributes & REQUIRED_AK_ATTRIBUTES != REQUIRED_AK_ATTRIBUTES
        || attributes & TPMA_OBJECT_DECRYPT != 0
    {
        return Err(invalid_ak(
            "AK must be fixed, restricted, signing-only, and TPM-generated",
        ));
    }
    reader.tpm2b("AK auth policy", 0..=64)?;
    parse_null_symmetric(&mut reader)?;
    let scheme = parse_signing_scheme(&mut reader)?;

    match key_type {
        TPM_ALG_RSA => {
            let key_bits = reader.u16("AK RSA key bits")?;
            if !(2048..=4096).contains(&key_bits) {
                return Err(invalid_ak("AK RSA key must be 2048 through 4096 bits"));
            }
            let exponent = match reader.u32("AK RSA exponent")? {
                0 => 65_537,
                exponent if exponent >= 3 && exponent % 2 == 1 => exponent,
                _ => return Err(invalid_ak("AK RSA exponent is invalid")),
            };
            let modulus = reader.tpm2b("AK RSA modulus", 256..=512)?;
            if modulus.len() != usize::from(key_bits / 8) {
                return Err(invalid_ak("AK RSA modulus length does not match keyBits"));
            }
            reader.finish("TPMT_PUBLIC")?;
            let key = RsaPublicKey::new(BigUint::from_bytes_be(modulus), BigUint::from(exponent))
                .map_err(|err| invalid_ak(&err.to_string()))?;
            if scheme.is_some_and(|algorithm| !matches!(algorithm, TPM_ALG_RSASSA | TPM_ALG_RSAPSS))
            {
                return Err(invalid_ak("AK RSA signing scheme is unsupported"));
            }
            Ok(AkPublic::Rsa { key, scheme })
        }
        TPM_ALG_ECC => {
            let curve = reader.u16("AK ECC curve")?;
            if curve != TPM_ECC_NIST_P256 {
                return Err(invalid_ak("only NIST P-256 AKs are supported"));
            }
            if reader.u16("AK ECC KDF")? != TPM_ALG_NULL {
                return Err(invalid_ak("AK ECC KDF must be TPM_ALG_NULL"));
            }
            let x = reader.tpm2b("AK ECC X", 32..=32)?;
            let y = reader.tpm2b("AK ECC Y", 32..=32)?;
            reader.finish("TPMT_PUBLIC")?;
            let mut sec1 = Vec::with_capacity(65);
            sec1.push(0x04);
            sec1.extend_from_slice(x);
            sec1.extend_from_slice(y);
            let public = P256PublicKey::from_sec1_bytes(&sec1)
                .map_err(|err| invalid_ak(&err.to_string()))?;
            if scheme.is_some_and(|algorithm| algorithm != TPM_ALG_ECDSA) {
                return Err(invalid_ak("AK ECC signing scheme is unsupported"));
            }
            Ok(AkPublic::EccP256 {
                key: public,
                scheme,
            })
        }
        _ => Err(invalid_ak("AK type must be RSA or ECC")),
    }
}

fn derive_tpm_name(encoded_public: &[u8]) -> Result<Vec<u8>, TpmQuoteError> {
    let mut reader = TpmReader::new(encoded_public);
    let public_area = reader.tpm2b("TPM2B_PUBLIC", 2..=4096)?;
    reader.finish("TPM2B_PUBLIC")?;
    let mut name = TPM_ALG_SHA256.to_be_bytes().to_vec();
    name.extend_from_slice(&Sha256::digest(public_area));
    Ok(name)
}

fn parse_null_symmetric(reader: &mut TpmReader<'_>) -> Result<(), TpmQuoteError> {
    if reader.u16("AK symmetric algorithm")? != TPM_ALG_NULL {
        return Err(invalid_ak(
            "restricted signing AK symmetric algorithm must be TPM_ALG_NULL",
        ));
    }
    Ok(())
}

fn parse_signing_scheme(reader: &mut TpmReader<'_>) -> Result<Option<u16>, TpmQuoteError> {
    let scheme = reader.u16("AK signing scheme")?;
    if scheme == TPM_ALG_NULL {
        return Ok(None);
    }
    if !matches!(scheme, TPM_ALG_RSASSA | TPM_ALG_RSAPSS | TPM_ALG_ECDSA) {
        return Err(invalid_ak("AK signing scheme is unsupported"));
    }
    if reader.u16("AK signing hash")? != TPM_ALG_SHA256 {
        return Err(invalid_ak("AK signing hash must be SHA-256"));
    }
    Ok(Some(scheme))
}

struct ParsedQuoteAttestation {
    extra_data: Vec<u8>,
    selected_pcrs: BTreeSet<u8>,
    pcr_digest: Vec<u8>,
}

fn parse_quote_attestation(input: &[u8]) -> Result<ParsedQuoteAttestation, TpmQuoteError> {
    let mut reader = TpmReader::new(input);
    if reader.u32("TPMS_ATTEST magic")? != TPM_GENERATED_VALUE {
        return Err(invalid_attest("invalid TPM_GENERATED magic"));
    }
    if reader.u16("TPMS_ATTEST type")? != TPM_ST_ATTEST_QUOTE {
        return Err(invalid_attest("attestation is not a TPM quote"));
    }
    let qualified_signer = reader.tpm2b("qualifiedSigner", 0..=128)?;
    validate_tpm_name(qualified_signer)?;
    let extra_data = reader.tpm2b("extraData", 0..=64)?.to_vec();
    reader.take("clockInfo", 8 + 4 + 4)?;
    let safe = reader.u8("clockInfo.safe")?;
    if safe > 1 {
        return Err(invalid_attest("clockInfo.safe is not a TPMI_YES_NO"));
    }
    reader.take("firmwareVersion", 8)?;
    if reader.u32("PCR selection count")? != 1 {
        return Err(invalid_attest(
            "exactly one SHA-256 PCR bank must be quoted",
        ));
    }
    if reader.u16("PCR bank hash")? != TPM_ALG_SHA256 {
        return Err(invalid_attest("quoted PCR bank must be SHA-256"));
    }
    let select_size = usize::from(reader.u8("PCR select size")?);
    if select_size != TPM_PC_CLIENT_PCR_SELECT_BYTES {
        return Err(invalid_attest("PCR select bitmap must cover PCRs 0-23"));
    }
    let bitmap = reader.take("PCR select bitmap", select_size)?;
    let mut selected_pcrs = BTreeSet::new();
    for (byte_index, byte) in bitmap.iter().enumerate() {
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                selected_pcrs.insert((byte_index * 8 + bit) as u8);
            }
        }
    }
    if selected_pcrs.is_empty() {
        return Err(invalid_attest("PCR selection is empty"));
    }
    let pcr_digest = reader
        .tpm2b(
            "quoted PCR digest",
            TPM_SHA256_DIGEST_BYTES..=TPM_SHA256_DIGEST_BYTES,
        )?
        .to_vec();
    reader.finish("TPMS_ATTEST")?;
    Ok(ParsedQuoteAttestation {
        extra_data,
        selected_pcrs,
        pcr_digest,
    })
}

fn verify_pcrs(
    attest: &ParsedQuoteAttestation,
    values: &[TpmPcrValue],
    policy: &TpmPcrPolicy,
) -> Result<(), TpmQuoteError> {
    let expected_selection = policy.pcrs.clone();
    if attest.selected_pcrs != expected_selection {
        return Err(TpmQuoteError::PcrSelectionMismatch);
    }
    let mut observed = BTreeMap::new();
    for value in values {
        if value.hash_algorithm != TpmHashAlgorithm::Sha256
            || value.index > TPM_MAX_PCR_INDEX
            || observed.contains_key(&value.index)
            || !valid_sha256(&value.digest)
        {
            return Err(TpmQuoteError::InvalidPcrValues);
        }
        observed.insert(
            value.index,
            hex::decode(&value.digest).map_err(|_| TpmQuoteError::InvalidPcrValues)?,
        );
    }
    if observed.keys().copied().collect::<BTreeSet<_>>() != expected_selection {
        return Err(TpmQuoteError::InvalidPcrValues);
    }
    let mut concatenated = Vec::with_capacity(observed.len() * TPM_SHA256_DIGEST_BYTES);
    for actual in observed.values() {
        concatenated.extend_from_slice(actual);
    }
    let digest = Sha256::digest(&concatenated);
    if !constant_time_equal(&digest, &attest.pcr_digest) {
        return Err(TpmQuoteError::PcrDigestMismatch);
    }
    Ok(())
}

fn verify_quote_signature(
    ak: &AkPublic,
    attest: &[u8],
    encoded: &[u8],
) -> Result<(), TpmQuoteError> {
    let mut reader = TpmReader::new(encoded);
    let algorithm = reader
        .u16("TPMT_SIGNATURE algorithm")
        .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
    let hash = reader
        .u16("TPMT_SIGNATURE hash")
        .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
    if hash != TPM_ALG_SHA256 {
        return Err(TpmQuoteError::InvalidSignatureEncoding);
    }
    match (ak, algorithm) {
        (AkPublic::Rsa { key, scheme }, TPM_ALG_RSASSA)
            if scheme.is_none_or(|value| value == TPM_ALG_RSASSA) =>
        {
            let signature = reader
                .tpm2b("RSA quote signature", 256..=512)
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            reader
                .finish("TPMT_SIGNATURE")
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            let signature = RsaSignature::try_from(signature)
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            RsaVerifyingKey::<Sha256>::new(key.clone())
                .verify(attest, &signature)
                .map_err(|_| TpmQuoteError::InvalidSignature)
        }
        (AkPublic::Rsa { key, scheme }, TPM_ALG_RSAPSS)
            if scheme.is_none_or(|value| value == TPM_ALG_RSAPSS) =>
        {
            let signature = reader
                .tpm2b("RSA-PSS quote signature", 256..=512)
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            reader
                .finish("TPMT_SIGNATURE")
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            let signature = RsaPssSignature::try_from(signature)
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            RsaPssVerifyingKey::<Sha256>::new(key.clone())
                .verify(attest, &signature)
                .map_err(|_| TpmQuoteError::InvalidSignature)
        }
        (AkPublic::EccP256 { key, scheme }, TPM_ALG_ECDSA)
            if scheme.is_none_or(|value| value == TPM_ALG_ECDSA) =>
        {
            let r = reader
                .tpm2b("ECDSA quote R", 1..=32)
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            let s = reader
                .tpm2b("ECDSA quote S", 1..=32)
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            reader
                .finish("TPMT_SIGNATURE")
                .map_err(|_| TpmQuoteError::InvalidSignatureEncoding)?;
            let mut padded_r = [0u8; 32];
            let mut padded_s = [0u8; 32];
            padded_r[32 - r.len()..].copy_from_slice(r);
            padded_s[32 - s.len()..].copy_from_slice(s);
            verify_p256_signature(key, attest, &padded_r, &padded_s)
                .map_err(|_| TpmQuoteError::InvalidSignature)
        }
        _ => Err(TpmQuoteError::InvalidSignatureEncoding),
    }
}

fn validate_tpm_name(name: &[u8]) -> Result<(), TpmQuoteError> {
    if name.len() == TPM_SHA256_NAME_BYTES && name[..2] == TPM_ALG_SHA256.to_be_bytes() {
        Ok(())
    } else {
        Err(TpmQuoteError::InvalidAkName)
    }
}

fn decode_b64(
    field: &'static str,
    encoded: &str,
    maximum_encoded_bytes: usize,
) -> Result<Vec<u8>, TpmQuoteError> {
    if encoded.len() > maximum_encoded_bytes {
        return Err(TpmQuoteError::EvidenceTooLarge);
    }
    BASE64
        .decode(encoded)
        .map_err(|_| TpmQuoteError::InvalidBase64(field))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn invalid_ak(reason: &str) -> TpmQuoteError {
    TpmQuoteError::InvalidAkPublic(reason.to_owned())
}

fn invalid_attest(reason: &str) -> TpmQuoteError {
    TpmQuoteError::InvalidAttestation(reason.to_owned())
}

struct TpmReader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> TpmReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn u8(&mut self, field: &str) -> Result<u8, TpmQuoteError> {
        Ok(self.take(field, 1)?[0])
    }

    fn u16(&mut self, field: &str) -> Result<u16, TpmQuoteError> {
        let bytes = self.take(field, 2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self, field: &str) -> Result<u32, TpmQuoteError> {
        let bytes = self.take(field, 4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn take(&mut self, field: &str, length: usize) -> Result<&'a [u8], TpmQuoteError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| invalid_attest(&format!("{field} length overflow")))?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| invalid_attest(&format!("{field} is truncated")))?;
        self.offset = end;
        Ok(value)
    }

    fn tpm2b(
        &mut self,
        field: &str,
        accepted: std::ops::RangeInclusive<usize>,
    ) -> Result<&'a [u8], TpmQuoteError> {
        let length = usize::from(self.u16(field)?);
        if !accepted.contains(&length) {
            return Err(invalid_attest(&format!("{field} length is invalid")));
        }
        self.take(field, length)
    }

    fn finish(&self, structure: &str) -> Result<(), TpmQuoteError> {
        if self.offset == self.input.len() {
            Ok(())
        } else {
            Err(invalid_attest(&format!("{structure} has trailing bytes")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mayhem_proto::HardwareQuoteRoutePolicyBinding;
    use p256::{
        elliptic_curve::{
            bigint::U256, ff::PrimeField, ops::Reduce, point::AffineCoordinates,
            sec1::ToEncodedPoint,
        },
        pkcs8::DecodePublicKey,
        FieldBytes, ProjectivePoint, Scalar,
    };
    use rand_chacha::ChaCha20Rng;
    use rand_core::SeedableRng;
    use rsa::{
        pkcs1v15::SigningKey as RsaSigningKey,
        pss::SigningKey as RsaPssSigningKey,
        signature::{RandomizedSigner, SignatureEncoding, Signer},
        traits::PublicKeyParts,
        RsaPrivateKey,
    };

    const VERIFY_AT_UNIX: u64 = 1_800_000_000;
    const ROOT_B64: &str = include_str!("../tests/fixtures/tpm-root.der.b64");
    const INTERMEDIATE_B64: &str = include_str!("../tests/fixtures/tpm-intermediate.der.b64");
    const RSA_EK_B64: &str = include_str!("../tests/fixtures/tpm-ek-rsa.der.b64");
    const ECC_EK_B64: &str = include_str!("../tests/fixtures/tpm-ek-ecc.der.b64");

    struct QuoteFixture {
        quote: HardwareQuote,
        expected: EvidenceBinding,
        activated: ActivatedTpmIdentity,
        materials: TpmVerificationMaterials,
        ak_private: RsaPrivateKey,
        ak_public: Vec<u8>,
        ak_name: Vec<u8>,
        pcr_values: Vec<TpmPcrValue>,
    }

    #[test]
    fn trusted_rsa_and_ecc_ek_certificates_extract_canonical_keys() {
        let root = fixture_der(ROOT_B64);
        let intermediate = fixture_der(INTERMEDIATE_B64);
        let rsa = verify_ek_certificate_chain(
            &[fixture_der(RSA_EK_B64), intermediate.clone()],
            std::slice::from_ref(&root),
            VERIFY_AT_UNIX,
        )
        .unwrap();
        assert!(matches!(rsa.public_key, EkPublicKey::Rsa(_)));
        assert_eq!(rsa.device_id.len(), 64);

        let ecc = verify_ek_certificate_chain(
            &[fixture_der(ECC_EK_B64), intermediate],
            &[root],
            VERIFY_AT_UNIX,
        )
        .unwrap();
        assert!(matches!(ecc.public_key, EkPublicKey::EccP256(_)));
        assert!(P256PublicKey::from_public_key_der(&ecc.canonical_spki_der).is_ok());
    }

    #[test]
    fn ek_chain_rejects_tampering_expiry_and_provider_chosen_anchor() {
        let root = fixture_der(ROOT_B64);
        let intermediate = fixture_der(INTERMEDIATE_B64);
        let mut leaf = fixture_der(RSA_EK_B64);
        *leaf.last_mut().unwrap() ^= 1;
        assert!(matches!(
            verify_ek_certificate_chain(
                &[leaf, intermediate.clone()],
                std::slice::from_ref(&root),
                VERIFY_AT_UNIX,
            ),
            Err(EkCertificateError::InvalidSignature { index: 0 })
        ));

        assert!(matches!(
            verify_ek_certificate_chain(
                &[fixture_der(RSA_EK_B64), intermediate.clone()],
                std::slice::from_ref(&root),
                2_000_000_000,
            ),
            Err(EkCertificateError::CertificateTime { index: 0, .. })
        ));

        assert!(verify_ek_certificate_chain(
            &[fixture_der(RSA_EK_B64), intermediate],
            &[fixture_der(ECC_EK_B64)],
            VERIFY_AT_UNIX,
        )
        .is_err());
    }

    #[test]
    fn nonce_bound_quote_verifies_ak_signature_pcrs_and_all_session_bindings() {
        let fixture = quote_fixture();
        let verified = verify_tpm_hardware_quote(
            &fixture.quote,
            &fixture.expected,
            &fixture.activated,
            &fixture.materials,
            VERIFY_AT_UNIX,
        )
        .unwrap();
        assert_eq!(verified.device_id, fixture.expected.device_id);
        assert_eq!(verified.selected_pcrs, BTreeSet::from([0, 7]));
        assert_eq!(verified.policy_sequence, 7);
        assert_eq!(verified.enclave_id, "canonical-enclave");
    }

    #[test]
    fn activated_ak_verifies_fresh_session_binding() {
        let fixture = quote_fixture();
        let mut fresh = fixture.expected.clone();
        fresh.nonce = "44".repeat(32);
        fresh.quote_binding = "55".repeat(32);
        let fresh_quote = quote_for_expected(&fixture, &fresh);

        assert_ne!(fixture.activated.quote_binding(), fresh.quote_binding);
        verify_tpm_hardware_quote(
            &fresh_quote,
            &fresh,
            &fixture.activated,
            &fixture.materials,
            VERIFY_AT_UNIX,
        )
        .unwrap();

        let mut wrong_current_binding = fresh_quote.clone();
        wrong_current_binding.binding = "66".repeat(32);
        assert_eq!(
            verify_tpm_hardware_quote(
                &wrong_current_binding,
                &fresh,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::QuoteBindingMismatch)
        );

        let mut stale_evidence = fresh.clone();
        stale_evidence.nonce = "77".repeat(32);
        assert_eq!(
            verify_tpm_hardware_quote(
                &fresh_quote,
                &stale_evidence,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::NonceBindingMismatch)
        );
    }

    #[test]
    fn quote_rejects_wrong_activated_ek_and_ak() {
        let fixture = quote_fixture();
        let wrong_ek = ActivatedTpmIdentity::test_identity(
            "ff".repeat(32),
            fixture.activated.ak_name_b64().to_owned(),
            fixture.activated.quote_binding().to_owned(),
        );
        assert_eq!(
            verify_tpm_hardware_quote(
                &fixture.quote,
                &fixture.expected,
                &wrong_ek,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::ActivatedEkMismatch)
        );

        let mut wrong_ak_name = fixture.ak_name.clone();
        *wrong_ak_name.last_mut().unwrap() ^= 1;
        let wrong_ak = ActivatedTpmIdentity::test_identity(
            fixture.activated.ek_public_sha256().to_owned(),
            BASE64.encode(wrong_ak_name),
            fixture.activated.quote_binding().to_owned(),
        );
        assert_eq!(
            verify_tpm_hardware_quote(
                &fixture.quote,
                &fixture.expected,
                &wrong_ak,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::ActivatedAkMismatch)
        );
    }

    #[test]
    fn ecc_ak_public_name_and_quote_signature_verify() {
        let fixture = quote_fixture();
        let secret = Option::<Scalar>::from(Scalar::from_repr([7u8; 32].into())).unwrap();
        let public =
            P256PublicKey::from_affine((ProjectivePoint::GENERATOR * secret).to_affine()).unwrap();
        let ak_public = make_ecc_ak_public(&public);
        let ak_name = derive_tpm_name(&ak_public).unwrap();
        let attest = make_attest(
            &ak_name,
            &fixture.expected.digest().unwrap(),
            &[0, 7],
            &pcr_digest(&fixture.pcr_values),
        );
        let evidence = TpmQuoteEvidence {
            schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            ak_public_b64: BASE64.encode(ak_public),
            ak_name_b64: BASE64.encode(&ak_name),
            quote_attest_b64: BASE64.encode(&attest),
            quote_signature_b64: BASE64.encode(sign_quote_ecc(secret, &attest)),
            pcr_values: fixture.pcr_values.clone(),
        };
        let mut quote = fixture.quote.clone();
        quote.evidence = serde_json::to_string(&evidence).unwrap();
        let activated = ActivatedTpmIdentity::test_identity(
            fixture.activated.ek_public_sha256().to_owned(),
            BASE64.encode(ak_name),
            quote.binding.clone(),
        );

        verify_tpm_hardware_quote(
            &quote,
            &fixture.expected,
            &activated,
            &fixture.materials,
            VERIFY_AT_UNIX,
        )
        .unwrap();
    }

    #[test]
    fn rsa_pss_ak_quote_signature_verifies() {
        let fixture = quote_fixture();
        let ak_public = make_rsa_ak_public_with_scheme(
            &RsaPublicKey::from(&fixture.ak_private),
            TPM_ALG_RSAPSS,
        );
        let ak_name = derive_tpm_name(&ak_public).unwrap();
        let attest = make_attest(
            &ak_name,
            &fixture.expected.digest().unwrap(),
            &[0, 7],
            &pcr_digest(&fixture.pcr_values),
        );
        let evidence = TpmQuoteEvidence {
            schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            ak_public_b64: BASE64.encode(ak_public),
            ak_name_b64: BASE64.encode(&ak_name),
            quote_attest_b64: BASE64.encode(&attest),
            quote_signature_b64: BASE64.encode(sign_quote_pss(&fixture.ak_private, &attest)),
            pcr_values: fixture.pcr_values.clone(),
        };
        let mut quote = fixture.quote.clone();
        quote.evidence = serde_json::to_string(&evidence).unwrap();
        let activated = ActivatedTpmIdentity::test_identity(
            fixture.activated.ek_public_sha256().to_owned(),
            BASE64.encode(ak_name),
            quote.binding.clone(),
        );

        verify_tpm_hardware_quote(
            &quote,
            &fixture.expected,
            &activated,
            &fixture.materials,
            VERIFY_AT_UNIX,
        )
        .unwrap();
    }

    #[test]
    fn quote_rejects_policy_nonce_enclave_device_kind_and_version_substitution() {
        let fixture = quote_fixture();
        for mut changed in [
            {
                let mut value = fixture.expected.clone();
                value.nonce = "99".repeat(32);
                value
            },
            {
                let mut value = fixture.expected.clone();
                value.enclave_id = "substituted-enclave".to_owned();
                value
            },
        ] {
            assert_eq!(
                verify_tpm_hardware_quote(
                    &fixture.quote,
                    &changed,
                    &fixture.activated,
                    &fixture.materials,
                    VERIFY_AT_UNIX,
                ),
                Err(TpmQuoteError::NonceBindingMismatch)
            );
            changed.nonce = "11".repeat(32);
        }

        for changed in [
            {
                let mut value = fixture.expected.clone();
                value.policy_digest = "98".repeat(32);
                value
            },
            {
                let mut value = fixture.expected.clone();
                value.policy_sequence += 1;
                value
            },
            {
                let mut value = fixture.expected.clone();
                value.evidence_schema_version += 1;
                value
            },
        ] {
            assert_eq!(
                verify_tpm_hardware_quote(
                    &fixture.quote,
                    &changed,
                    &fixture.activated,
                    &fixture.materials,
                    VERIFY_AT_UNIX,
                ),
                Err(TpmQuoteError::PolicyBindingMismatch)
            );
        }

        let mut changed = fixture.expected.clone();
        changed.device_id = "97".repeat(32);
        assert_eq!(
            verify_tpm_hardware_quote(
                &fixture.quote,
                &changed,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::DeviceMismatch)
        );

        let mut changed = fixture.expected.clone();
        changed.kind = HardwareQuoteKind::AppleAppAttestJwt;
        assert_eq!(
            verify_tpm_hardware_quote(
                &fixture.quote,
                &changed,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::QuoteKindMismatch)
        );

        let mut changed = fixture.quote.clone();
        let mut evidence = quote_evidence(&changed);
        evidence.schema_version = 2;
        changed.evidence = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            verify_tpm_hardware_quote(
                &changed,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::EvidenceSchemaMismatch {
                expected: 1,
                actual: 2
            })
        );
    }

    #[test]
    fn quote_rejects_signature_ak_name_pcr_digest_and_selection_but_accepts_value_changes() {
        let fixture = quote_fixture();

        let mut bad_signature = fixture.quote.clone();
        let mut evidence = quote_evidence(&bad_signature);
        let mut signature = BASE64.decode(&evidence.quote_signature_b64).unwrap();
        *signature.last_mut().unwrap() ^= 1;
        evidence.quote_signature_b64 = BASE64.encode(signature);
        bad_signature.evidence = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            verify_tpm_hardware_quote(
                &bad_signature,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::InvalidSignature)
        );

        let mut bad_public = fixture.quote.clone();
        let mut evidence = quote_evidence(&bad_public);
        let mut other_public = fixture.ak_public.clone();
        *other_public.last_mut().unwrap() ^= 2;
        evidence.ak_public_b64 = BASE64.encode(other_public);
        bad_public.evidence = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            verify_tpm_hardware_quote(
                &bad_public,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::AkNameMismatch)
        );

        let mut bad_name = fixture.quote.clone();
        let mut evidence = quote_evidence(&bad_name);
        let mut other_name = fixture.ak_name.clone();
        *other_name.last_mut().unwrap() ^= 1;
        evidence.ak_name_b64 = BASE64.encode(&other_name);
        bad_name.evidence = serde_json::to_string(&evidence).unwrap();
        let activated = ActivatedTpmIdentity::test_identity(
            fixture.activated.ek_public_sha256().to_owned(),
            evidence.ak_name_b64,
            fixture.quote.binding.clone(),
        );
        assert_eq!(
            verify_tpm_hardware_quote(
                &bad_name,
                &fixture.expected,
                &activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::AkNameMismatch)
        );

        let mut bad_value = fixture.quote.clone();
        let mut evidence = quote_evidence(&bad_value);
        evidence.pcr_values[0].digest = "55".repeat(32);
        bad_value.evidence = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            verify_tpm_hardware_quote(
                &bad_value,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::PcrDigestMismatch)
        );

        let mut changed_values = fixture.pcr_values.clone();
        changed_values[0].digest = "55".repeat(32);
        let attest = make_attest(
            &fixture.ak_name,
            &fixture.expected.digest().unwrap(),
            &[0, 7],
            &pcr_digest(&changed_values),
        );
        let mut changed_quote = fixture.quote.clone();
        changed_quote.evidence = serde_json::to_string(&TpmQuoteEvidence {
            schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            ak_public_b64: BASE64.encode(&fixture.ak_public),
            ak_name_b64: BASE64.encode(&fixture.ak_name),
            quote_attest_b64: BASE64.encode(&attest),
            quote_signature_b64: BASE64.encode(sign_quote(&fixture.ak_private, &attest)),
            pcr_values: changed_values,
        })
        .unwrap();
        verify_tpm_hardware_quote(
            &changed_quote,
            &fixture.expected,
            &fixture.activated,
            &fixture.materials,
            VERIFY_AT_UNIX,
        )
        .unwrap();

        let mut bad_selection = fixture.quote.clone();
        let mut evidence = quote_evidence(&bad_selection);
        let pcr_digest = pcr_digest(&fixture.pcr_values[..1]);
        let attest = make_attest(
            &fixture.ak_name,
            &fixture.expected.digest().unwrap(),
            &[0],
            &pcr_digest,
        );
        evidence.quote_attest_b64 = BASE64.encode(&attest);
        evidence.quote_signature_b64 = BASE64.encode(sign_quote(&fixture.ak_private, &attest));
        bad_selection.evidence = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            verify_tpm_hardware_quote(
                &bad_selection,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::PcrSelectionMismatch)
        );

        let mut bad_digest = fixture.quote.clone();
        let mut evidence = quote_evidence(&bad_digest);
        let attest = make_attest(
            &fixture.ak_name,
            &fixture.expected.digest().unwrap(),
            &[0, 7],
            &[0; 32],
        );
        evidence.quote_attest_b64 = BASE64.encode(&attest);
        evidence.quote_signature_b64 = BASE64.encode(sign_quote(&fixture.ak_private, &attest));
        bad_digest.evidence = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            verify_tpm_hardware_quote(
                &bad_digest,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::PcrDigestMismatch)
        );
    }

    #[test]
    fn provider_metadata_roots_and_policy_fields_are_rejected() {
        let fixture = quote_fixture();
        let mut metadata = fixture.quote.clone();
        metadata.metadata = serde_json::json!({"root": "provider-selected"});
        assert_eq!(
            verify_tpm_hardware_quote(
                &metadata,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::ProviderMetadataRejected)
        );

        let mut provider_policy = fixture.quote.clone();
        let evidence =
            serde_json::from_str::<serde_json::Value>(&provider_policy.evidence).unwrap();
        let mut object = evidence.as_object().unwrap().clone();
        object.insert(
            "trust_roots".to_owned(),
            serde_json::json!([fixture_der(ROOT_B64)]),
        );
        provider_policy.evidence = serde_json::Value::Object(object).to_string();
        assert!(matches!(
            verify_tpm_hardware_quote(
                &provider_policy,
                &fixture.expected,
                &fixture.activated,
                &fixture.materials,
                VERIFY_AT_UNIX,
            ),
            Err(TpmQuoteError::InvalidEvidenceJson(_))
        ));
    }

    fn quote_fixture() -> QuoteFixture {
        let root = fixture_der(ROOT_B64);
        let intermediate = fixture_der(INTERMEDIATE_B64);
        let leaf = fixture_der(RSA_EK_B64);
        let trusted = verify_ek_certificate_chain(
            &[leaf.clone(), intermediate.clone()],
            std::slice::from_ref(&root),
            VERIFY_AT_UNIX,
        )
        .unwrap();
        let mut rng = ChaCha20Rng::from_seed([41; 32]);
        let ak_private = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let ak_public = make_rsa_ak_public(&RsaPublicKey::from(&ak_private));
        let ak_name = derive_tpm_name(&ak_public).unwrap();
        let pcr_values = vec![
            TpmPcrValue {
                hash_algorithm: TpmHashAlgorithm::Sha256,
                index: 0,
                digest: "a0".repeat(32),
            },
            TpmPcrValue {
                hash_algorithm: TpmHashAlgorithm::Sha256,
                index: 7,
                digest: "b7".repeat(32),
            },
        ];
        let pcr_policy = TpmPcrPolicy {
            schema_version: TPM_PCR_POLICY_SCHEMA_VERSION,
            hash_algorithm: TpmHashAlgorithm::Sha256,
            pcrs: pcr_values.iter().map(|value| value.index).collect(),
        };
        let route = HardwareQuoteRoutePolicyBinding {
            enclave_id: "canonical-enclave".to_owned(),
            device_id: trusted.device_id.clone(),
            kind: HardwareQuoteKind::Tpm2QuoteEk,
            evidence_schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            policy_sequence: 7,
            policy_digest: "22".repeat(32),
            platform: Some("windows-tpm2".to_owned()),
        };
        let expected = EvidenceBinding::new(
            &route,
            "11".repeat(32),
            "canonical-enclave",
            trusted.device_id.clone(),
            "33".repeat(32),
        )
        .unwrap();
        let digest = pcr_digest(&pcr_values);
        let attest = make_attest(&ak_name, &expected.digest().unwrap(), &[0, 7], &digest);
        let evidence = TpmQuoteEvidence {
            schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            ak_public_b64: BASE64.encode(&ak_public),
            ak_name_b64: BASE64.encode(&ak_name),
            quote_attest_b64: BASE64.encode(&attest),
            quote_signature_b64: BASE64.encode(sign_quote(&ak_private, &attest)),
            pcr_values: pcr_values.clone(),
        };
        let quote = HardwareQuote {
            kind: HardwareQuoteKind::Tpm2QuoteEk,
            evidence: serde_json::to_string(&evidence).unwrap(),
            binding: expected.quote_binding.clone(),
            endorsements: vec![BASE64.encode(leaf), BASE64.encode(intermediate)],
            metadata: serde_json::Value::Null,
        };
        let activated = ActivatedTpmIdentity::test_identity(
            hex::encode(Sha256::digest(&trusted.canonical_spki_der)),
            BASE64.encode(&ak_name),
            expected.quote_binding.clone(),
        );
        QuoteFixture {
            quote,
            expected,
            activated,
            materials: TpmVerificationMaterials {
                trust_anchor_der: vec![root],
                pcr_policy,
                policy_sequence: route.policy_sequence,
                policy_digest: route.policy_digest,
                evidence_schema_version: route.evidence_schema_version,
            },
            ak_private,
            ak_public,
            ak_name,
            pcr_values,
        }
    }

    fn quote_for_expected(fixture: &QuoteFixture, expected: &EvidenceBinding) -> HardwareQuote {
        let attest = make_attest(
            &fixture.ak_name,
            &expected.digest().unwrap(),
            &[0, 7],
            &pcr_digest(&fixture.pcr_values),
        );
        let evidence = TpmQuoteEvidence {
            schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            ak_public_b64: BASE64.encode(&fixture.ak_public),
            ak_name_b64: BASE64.encode(&fixture.ak_name),
            quote_attest_b64: BASE64.encode(&attest),
            quote_signature_b64: BASE64.encode(sign_quote(&fixture.ak_private, &attest)),
            pcr_values: fixture.pcr_values.clone(),
        };
        HardwareQuote {
            kind: HardwareQuoteKind::Tpm2QuoteEk,
            evidence: serde_json::to_string(&evidence).unwrap(),
            binding: expected.quote_binding.clone(),
            endorsements: fixture.quote.endorsements.clone(),
            metadata: serde_json::Value::Null,
        }
    }

    fn make_rsa_ak_public(key: &RsaPublicKey) -> Vec<u8> {
        make_rsa_ak_public_with_scheme(key, TPM_ALG_RSASSA)
    }

    fn make_rsa_ak_public_with_scheme(key: &RsaPublicKey, scheme: u16) -> Vec<u8> {
        let mut public = Vec::new();
        public.extend_from_slice(&TPM_ALG_RSA.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        public.extend_from_slice(&REQUIRED_AK_ATTRIBUTES.to_be_bytes());
        public.extend_from_slice(&0u16.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_NULL.to_be_bytes());
        public.extend_from_slice(&scheme.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        public.extend_from_slice(&(key.n().bits() as u16).to_be_bytes());
        public.extend_from_slice(&0u32.to_be_bytes());
        let modulus = key.n().to_bytes_be();
        public.extend_from_slice(&(modulus.len() as u16).to_be_bytes());
        public.extend_from_slice(&modulus);
        let mut encoded = (public.len() as u16).to_be_bytes().to_vec();
        encoded.extend(public);
        encoded
    }

    fn make_ecc_ak_public(key: &P256PublicKey) -> Vec<u8> {
        let point = key.to_encoded_point(false);
        let mut public = Vec::new();
        public.extend_from_slice(&TPM_ALG_ECC.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        public.extend_from_slice(&REQUIRED_AK_ATTRIBUTES.to_be_bytes());
        public.extend_from_slice(&0u16.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_NULL.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_ECDSA.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        public.extend_from_slice(&TPM_ECC_NIST_P256.to_be_bytes());
        public.extend_from_slice(&TPM_ALG_NULL.to_be_bytes());
        public.extend(tpm2b_test(point.x().unwrap()));
        public.extend(tpm2b_test(point.y().unwrap()));
        let mut encoded = (public.len() as u16).to_be_bytes().to_vec();
        encoded.extend(public);
        encoded
    }

    fn make_attest(
        ak_name: &[u8],
        extra_data: &[u8],
        selected: &[u8],
        pcr_digest: &[u8],
    ) -> Vec<u8> {
        let mut attest = Vec::new();
        attest.extend_from_slice(&TPM_GENERATED_VALUE.to_be_bytes());
        attest.extend_from_slice(&TPM_ST_ATTEST_QUOTE.to_be_bytes());
        attest.extend(tpm2b_test(ak_name));
        attest.extend(tpm2b_test(extra_data));
        attest.extend_from_slice(&0u64.to_be_bytes());
        attest.extend_from_slice(&0u32.to_be_bytes());
        attest.extend_from_slice(&0u32.to_be_bytes());
        attest.push(1);
        attest.extend_from_slice(&1u64.to_be_bytes());
        attest.extend_from_slice(&1u32.to_be_bytes());
        attest.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        attest.push(TPM_PC_CLIENT_PCR_SELECT_BYTES as u8);
        let mut bitmap = [0u8; TPM_PC_CLIENT_PCR_SELECT_BYTES];
        for index in selected {
            bitmap[usize::from(*index / 8)] |= 1 << (*index % 8);
        }
        attest.extend_from_slice(&bitmap);
        attest.extend(tpm2b_test(pcr_digest));
        attest
    }

    fn pcr_digest(values: &[TpmPcrValue]) -> [u8; 32] {
        let mut ordered = values.to_vec();
        ordered.sort_by_key(|value| value.index);
        let mut concatenated = Vec::new();
        for value in ordered {
            concatenated.extend(hex::decode(value.digest).unwrap());
        }
        Sha256::digest(concatenated).into()
    }

    fn sign_quote(key: &RsaPrivateKey, attest: &[u8]) -> Vec<u8> {
        let signature = RsaSigningKey::<Sha256>::new(key.clone()).sign(attest);
        let mut encoded = TPM_ALG_RSASSA.to_be_bytes().to_vec();
        encoded.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        encoded.extend(tpm2b_test(&signature.to_vec()));
        encoded
    }

    fn sign_quote_pss(key: &RsaPrivateKey, attest: &[u8]) -> Vec<u8> {
        let mut rng = ChaCha20Rng::from_seed([51; 32]);
        let signature =
            RsaPssSigningKey::<Sha256>::new(key.clone()).sign_with_rng(&mut rng, attest);
        let mut encoded = TPM_ALG_RSAPSS.to_be_bytes().to_vec();
        encoded.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        encoded.extend(tpm2b_test(&signature.to_vec()));
        encoded
    }

    fn sign_quote_ecc(secret: Scalar, attest: &[u8]) -> Vec<u8> {
        let nonce = Option::<Scalar>::from(Scalar::from_repr([9u8; 32].into())).unwrap();
        let point = ProjectivePoint::GENERATOR * nonce;
        let r = <Scalar as Reduce<U256>>::reduce_bytes(&point.to_affine().x());
        let digest = Sha256::digest(attest);
        let z = <Scalar as Reduce<U256>>::reduce_bytes(FieldBytes::from_slice(&digest));
        let s = Option::<Scalar>::from(nonce.invert()).unwrap() * (z + r * secret);
        let mut encoded = TPM_ALG_ECDSA.to_be_bytes().to_vec();
        encoded.extend_from_slice(&TPM_ALG_SHA256.to_be_bytes());
        encoded.extend(tpm2b_test(r.to_repr().as_slice()));
        encoded.extend(tpm2b_test(s.to_repr().as_slice()));
        encoded
    }

    fn quote_evidence(quote: &HardwareQuote) -> TpmQuoteEvidence {
        serde_json::from_str(&quote.evidence).unwrap()
    }

    fn tpm2b_test(value: &[u8]) -> Vec<u8> {
        let mut encoded = (value.len() as u16).to_be_bytes().to_vec();
        encoded.extend_from_slice(value);
        encoded
    }

    fn fixture_der(encoded: &str) -> Vec<u8> {
        BASE64.decode(encoded.trim()).unwrap()
    }
}
