use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mayhem_proto::{
    TpmHashAlgorithm, TpmPcrValue, TpmQuoteEvidence, TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
};
use p256::{
    ecdsa::{Signature as P256Signature, VerifyingKey as P256VerifyingKey},
    PublicKey as P256PublicKey,
};
use rsa::{
    pkcs1v15::{Signature as RsaSignature, VerifyingKey as RsaVerifyingKey},
    pss::{Signature as RsaPssSignature, VerifyingKey as RsaPssVerifyingKey},
    signature::Verifier as _,
    BigUint, RsaPublicKey,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{
    measurements::{expected_measurements, pcr_index},
    reject, Result, VerifyError,
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
const TPM_PC_CLIENT_PCR_SELECT_BYTES: usize = 3;
const TPM_MAX_PCR_INDEX: u8 = 23;
const TPM_SHA256_NAME_BYTES: usize = 34;
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

const MAX_AK_PUBLIC_BYTES: usize = 4 * 1024;
const MAX_AK_NAME_BYTES: usize = 128;
const MAX_ATTEST_BYTES: usize = 8 * 1024;
const MAX_SIGNATURE_BYTES: usize = 1024;

#[derive(Clone)]
pub(crate) enum VerifiedAkPublic {
    Rsa(RsaPublicKey),
    EccP256,
}

pub(crate) struct VerifiedWorkloadQuote {
    pub ak_public: VerifiedAkPublic,
    pub measurements: BTreeMap<String, String>,
}

pub(crate) fn verify_workload_quote(
    evidence: &TpmQuoteEvidence,
    expected_quote_binding: &str,
    workload_policy: &Value,
) -> Result<VerifiedWorkloadQuote> {
    if evidence.schema_version != TPM_QUOTE_EVIDENCE_SCHEMA_VERSION {
        return reject(format!(
            "vTPM quote schema {} is unsupported; expected {}",
            evidence.schema_version, TPM_QUOTE_EVIDENCE_SCHEMA_VERSION
        ));
    }
    let binding = hex::decode(expected_quote_binding)
        .map_err(|_| VerifyError::Rejected("vTPM quote binding is not hexadecimal".into()))?;
    if binding.len() != 32 {
        return reject("vTPM quote binding is not 32 bytes");
    }

    let encoded_public = decode_b64(
        "vTPM AK public area",
        &evidence.ak_public_b64,
        MAX_AK_PUBLIC_BYTES,
    )?;
    let ak_name = decode_b64("vTPM AK Name", &evidence.ak_name_b64, MAX_AK_NAME_BYTES)?;
    validate_tpm_name(&ak_name)?;
    let ak = parse_ak_public(&encoded_public)?;
    let derived_name = derive_tpm_name(&encoded_public)?;
    if !constant_time_equal(&derived_name, &ak_name) {
        return reject("vTPM AK public area does not derive the supplied AK Name");
    }

    let attest = decode_b64(
        "vTPM quote attestation",
        &evidence.quote_attest_b64,
        MAX_ATTEST_BYTES,
    )?;
    let parsed = parse_quote_attestation(&attest)?;
    if !constant_time_equal(&parsed.extra_data, &binding) {
        return reject("vTPM quote extraData does not bind the Mayhem hardware quote");
    }
    let expected_pcrs = expected_pcr_policy(workload_policy)?;
    let measurements = verify_pcrs(&parsed, &evidence.pcr_values, &expected_pcrs)?;

    let signature = decode_b64(
        "vTPM quote signature",
        &evidence.quote_signature_b64,
        MAX_SIGNATURE_BYTES,
    )?;
    verify_quote_signature(&ak, &attest, &signature)?;

    Ok(VerifiedWorkloadQuote {
        ak_public: match ak {
            AkPublic::Rsa { key, .. } => VerifiedAkPublic::Rsa(key),
            AkPublic::EccP256 { .. } => VerifiedAkPublic::EccP256,
        },
        measurements,
    })
}

fn expected_pcr_policy(workload_policy: &Value) -> Result<BTreeMap<u8, BTreeSet<String>>> {
    let measurements = expected_measurements(workload_policy, "workload")?;
    let mut pcrs = BTreeMap::new();
    for (name, allowed) in measurements {
        let index = pcr_index(&name).ok_or_else(|| {
            VerifyError::EvidenceGap(format!(
                "Azure workload policy measurement {name} is not a vTPM PCR"
            ))
        })?;
        if let Some(existing) = pcrs.insert(index, allowed.clone()) {
            if existing != allowed {
                return reject(format!(
                    "workload policy assigns conflicting golden values to vTPM PCR {index}"
                ));
            }
        }
    }
    if pcrs.is_empty() {
        return reject("Azure workload policy contains no vTPM PCR measurements");
    }
    Ok(pcrs)
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

fn parse_ak_public(encoded: &[u8]) -> Result<AkPublic> {
    let mut outer = TpmReader::new(encoded);
    let public_area = outer.tpm2b("TPM2B_PUBLIC", 2..=4096)?;
    outer.finish("TPM2B_PUBLIC")?;
    let mut reader = TpmReader::new(public_area);
    let key_type = reader.u16("AK type")?;
    if reader.u16("AK name algorithm")? != TPM_ALG_SHA256 {
        return reject("vTPM AK Name algorithm is not SHA-256");
    }
    let attributes = reader.u32("AK object attributes")?;
    if attributes & REQUIRED_AK_ATTRIBUTES != REQUIRED_AK_ATTRIBUTES
        || attributes & TPMA_OBJECT_DECRYPT != 0
    {
        return reject("vTPM AK is not fixed, restricted, TPM-generated, and signing-only");
    }
    reader.tpm2b("AK auth policy", 0..=64)?;
    if reader.u16("AK symmetric algorithm")? != TPM_ALG_NULL {
        return reject("vTPM restricted signing AK has a non-null symmetric algorithm");
    }
    let scheme = parse_signing_scheme(&mut reader)?;

    match key_type {
        TPM_ALG_RSA => {
            let key_bits = reader.u16("AK RSA key bits")?;
            if !(2048..=4096).contains(&key_bits) {
                return reject("vTPM RSA AK must be 2048 through 4096 bits");
            }
            let exponent = match reader.u32("AK RSA exponent")? {
                0 => 65_537,
                exponent if exponent >= 3 && exponent % 2 == 1 => exponent,
                _ => return reject("vTPM RSA AK exponent is invalid"),
            };
            let modulus = reader.tpm2b("AK RSA modulus", 256..=512)?;
            if modulus.len() != usize::from(key_bits / 8) {
                return reject("vTPM RSA AK modulus length differs from keyBits");
            }
            reader.finish("TPMT_PUBLIC")?;
            if scheme.is_some_and(|value| !matches!(value, TPM_ALG_RSASSA | TPM_ALG_RSAPSS)) {
                return reject("vTPM RSA AK signing scheme is unsupported");
            }
            let key = RsaPublicKey::new(BigUint::from_bytes_be(modulus), BigUint::from(exponent))
                .map_err(|error| {
                VerifyError::Rejected(format!("vTPM RSA AK is invalid: {error}"))
            })?;
            Ok(AkPublic::Rsa { key, scheme })
        }
        TPM_ALG_ECC => {
            if reader.u16("AK ECC curve")? != TPM_ECC_NIST_P256 {
                return reject("vTPM ECC AK is not NIST P-256");
            }
            if reader.u16("AK ECC KDF")? != TPM_ALG_NULL {
                return reject("vTPM ECC AK KDF is not null");
            }
            let x = reader.tpm2b("AK ECC X", 32..=32)?;
            let y = reader.tpm2b("AK ECC Y", 32..=32)?;
            reader.finish("TPMT_PUBLIC")?;
            if scheme.is_some_and(|value| value != TPM_ALG_ECDSA) {
                return reject("vTPM ECC AK signing scheme is unsupported");
            }
            let mut sec1 = Vec::with_capacity(65);
            sec1.push(4);
            sec1.extend_from_slice(x);
            sec1.extend_from_slice(y);
            let key = P256PublicKey::from_sec1_bytes(&sec1).map_err(|error| {
                VerifyError::Rejected(format!("vTPM P-256 AK is invalid: {error}"))
            })?;
            Ok(AkPublic::EccP256 { key, scheme })
        }
        _ => reject("vTPM AK type is neither RSA nor ECC"),
    }
}

fn parse_signing_scheme(reader: &mut TpmReader<'_>) -> Result<Option<u16>> {
    let scheme = reader.u16("AK signing scheme")?;
    if scheme == TPM_ALG_NULL {
        return Ok(None);
    }
    if !matches!(scheme, TPM_ALG_RSASSA | TPM_ALG_RSAPSS | TPM_ALG_ECDSA) {
        return reject("vTPM AK signing scheme is unsupported");
    }
    if reader.u16("AK signing hash")? != TPM_ALG_SHA256 {
        return reject("vTPM AK signing hash is not SHA-256");
    }
    Ok(Some(scheme))
}

fn derive_tpm_name(encoded_public: &[u8]) -> Result<Vec<u8>> {
    let mut reader = TpmReader::new(encoded_public);
    let public_area = reader.tpm2b("TPM2B_PUBLIC", 2..=4096)?;
    reader.finish("TPM2B_PUBLIC")?;
    let mut name = TPM_ALG_SHA256.to_be_bytes().to_vec();
    name.extend_from_slice(&Sha256::digest(public_area));
    Ok(name)
}

struct ParsedQuoteAttestation {
    extra_data: Vec<u8>,
    selected_pcrs: BTreeSet<u8>,
    pcr_digest: Vec<u8>,
}

fn parse_quote_attestation(input: &[u8]) -> Result<ParsedQuoteAttestation> {
    let mut reader = TpmReader::new(input);
    if reader.u32("TPMS_ATTEST magic")? != TPM_GENERATED_VALUE {
        return reject("vTPM quote has invalid TPM_GENERATED magic");
    }
    if reader.u16("TPMS_ATTEST type")? != TPM_ST_ATTEST_QUOTE {
        return reject("vTPM attestation is not a quote");
    }
    validate_tpm_name(reader.tpm2b("qualifiedSigner", 0..=128)?)?;
    let extra_data = reader.tpm2b("extraData", 0..=64)?.to_vec();
    reader.take("clockInfo", 8 + 4 + 4)?;
    if reader.u8("clockInfo.safe")? > 1 {
        return reject("vTPM quote clockInfo.safe is invalid");
    }
    reader.take("firmwareVersion", 8)?;
    if reader.u32("PCR selection count")? != 1 {
        return reject("vTPM quote must select exactly one PCR bank");
    }
    if reader.u16("PCR bank hash")? != TPM_ALG_SHA256 {
        return reject("vTPM quote PCR bank is not SHA-256");
    }
    let size = usize::from(reader.u8("PCR select size")?);
    if size != TPM_PC_CLIENT_PCR_SELECT_BYTES {
        return reject("vTPM quote PCR bitmap does not cover PCRs 0 through 23");
    }
    let bitmap = reader.take("PCR select bitmap", size)?;
    let mut selected_pcrs = BTreeSet::new();
    for (byte_index, byte) in bitmap.iter().enumerate() {
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                selected_pcrs.insert((byte_index * 8 + bit) as u8);
            }
        }
    }
    if selected_pcrs.is_empty() {
        return reject("vTPM quote PCR selection is empty");
    }
    let pcr_digest = reader.tpm2b("quoted PCR digest", 32..=32)?.to_vec();
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
    policy: &BTreeMap<u8, BTreeSet<String>>,
) -> Result<BTreeMap<String, String>> {
    let expected_selection = policy.keys().copied().collect::<BTreeSet<_>>();
    if attest.selected_pcrs != expected_selection {
        return reject("vTPM quote PCR selection differs from immutable admin policy");
    }
    let mut observed = BTreeMap::new();
    for value in values {
        if value.hash_algorithm != TpmHashAlgorithm::Sha256
            || value.index > TPM_MAX_PCR_INDEX
            || observed.contains_key(&value.index)
            || !valid_sha256(&value.digest)
        {
            return reject("vTPM PCR values are incomplete, duplicated, or malformed");
        }
        observed.insert(
            value.index,
            hex::decode(&value.digest)
                .map_err(|_| VerifyError::Rejected("vTPM PCR digest is invalid".into()))?,
        );
    }
    if observed.keys().copied().collect::<BTreeSet<_>>() != expected_selection {
        return reject("vTPM PCR values do not exactly cover the policy selection");
    }

    let mut concatenated = Vec::with_capacity(observed.len() * 32);
    let mut measurements = BTreeMap::new();
    for (index, actual) in &observed {
        let normalized = hex::encode(actual);
        if !policy
            .get(index)
            .is_some_and(|allowed| allowed.contains(&normalized))
        {
            return reject(format!(
                "vTPM PCR {index} is not in the admin golden workload set"
            ));
        }
        concatenated.extend_from_slice(actual);
        measurements.insert(format!("vtpm_pcr_{index}"), normalized);
    }
    let digest = Sha256::digest(&concatenated);
    if !constant_time_equal(&digest, &attest.pcr_digest) {
        return reject("vTPM quote PCR digest does not match the selected PCR values");
    }
    Ok(measurements)
}

fn verify_quote_signature(ak: &AkPublic, attest: &[u8], encoded: &[u8]) -> Result<()> {
    let mut reader = TpmReader::new(encoded);
    let algorithm = reader.u16("TPMT_SIGNATURE algorithm")?;
    if reader.u16("TPMT_SIGNATURE hash")? != TPM_ALG_SHA256 {
        return reject("vTPM quote signature hash is not SHA-256");
    }
    match (ak, algorithm) {
        (AkPublic::Rsa { key, scheme }, TPM_ALG_RSASSA)
            if scheme.is_none_or(|value| value == TPM_ALG_RSASSA) =>
        {
            let bytes = reader.tpm2b("RSA quote signature", 256..=512)?;
            reader.finish("TPMT_SIGNATURE")?;
            let signature = RsaSignature::try_from(bytes)
                .map_err(|_| VerifyError::Rejected("vTPM RSA signature is malformed".into()))?;
            RsaVerifyingKey::<Sha256>::new(key.clone())
                .verify(attest, &signature)
                .map_err(|_| VerifyError::Rejected("vTPM RSA quote signature is invalid".into()))
        }
        (AkPublic::Rsa { key, scheme }, TPM_ALG_RSAPSS)
            if scheme.is_none_or(|value| value == TPM_ALG_RSAPSS) =>
        {
            let bytes = reader.tpm2b("RSA-PSS quote signature", 256..=512)?;
            reader.finish("TPMT_SIGNATURE")?;
            let signature = RsaPssSignature::try_from(bytes)
                .map_err(|_| VerifyError::Rejected("vTPM RSA-PSS signature is malformed".into()))?;
            RsaPssVerifyingKey::<Sha256>::new(key.clone())
                .verify(attest, &signature)
                .map_err(|_| {
                    VerifyError::Rejected("vTPM RSA-PSS quote signature is invalid".into())
                })
        }
        (AkPublic::EccP256 { key, scheme }, TPM_ALG_ECDSA)
            if scheme.is_none_or(|value| value == TPM_ALG_ECDSA) =>
        {
            let r = reader.tpm2b("ECDSA quote R", 1..=32)?;
            let s = reader.tpm2b("ECDSA quote S", 1..=32)?;
            reader.finish("TPMT_SIGNATURE")?;
            let mut padded_r = [0u8; 32];
            let mut padded_s = [0u8; 32];
            padded_r[32 - r.len()..].copy_from_slice(r);
            padded_s[32 - s.len()..].copy_from_slice(s);
            let signature = P256Signature::from_scalars(padded_r, padded_s)
                .map_err(|_| VerifyError::Rejected("vTPM ECDSA signature is malformed".into()))?;
            P256VerifyingKey::from(*key)
                .verify(attest, &signature)
                .map_err(|_| VerifyError::Rejected("vTPM ECDSA quote signature is invalid".into()))
        }
        _ => reject("vTPM quote signature algorithm does not match the AK"),
    }
}

fn validate_tpm_name(name: &[u8]) -> Result<()> {
    if name.len() == TPM_SHA256_NAME_BYTES && name[..2] == TPM_ALG_SHA256.to_be_bytes() {
        Ok(())
    } else {
        reject("vTPM AK Name is not a SHA-256 TPM Name")
    }
}

fn decode_b64(field: &str, encoded: &str, maximum: usize) -> Result<Vec<u8>> {
    if encoded.len() > maximum.saturating_mul(2) {
        return reject(format!("{field} exceeds its encoded size limit"));
    }
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| VerifyError::Rejected(format!("{field} is not base64")))?;
    if bytes.is_empty() || bytes.len() > maximum {
        return reject(format!("{field} exceeds its decoded size limit"));
    }
    Ok(bytes)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

struct TpmReader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> TpmReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn u8(&mut self, field: &str) -> Result<u8> {
        Ok(self.take(field, 1)?[0])
    }

    fn u16(&mut self, field: &str) -> Result<u16> {
        let bytes = self.take(field, 2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self, field: &str) -> Result<u32> {
        let bytes = self.take(field, 4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn take(&mut self, field: &str, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| VerifyError::Rejected(format!("{field} length overflow")))?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| VerifyError::Rejected(format!("{field} is truncated")))?;
        self.offset = end;
        Ok(value)
    }

    fn tpm2b(
        &mut self,
        field: &str,
        accepted: std::ops::RangeInclusive<usize>,
    ) -> Result<&'a [u8]> {
        let length = usize::from(self.u16(field)?);
        if !accepted.contains(&length) {
            return reject(format!("{field} length is invalid"));
        }
        self.take(field, length)
    }

    fn finish(&self, structure: &str) -> Result<()> {
        if self.offset == self.input.len() {
            Ok(())
        } else {
            reject(format!("{structure} has trailing bytes"))
        }
    }
}
