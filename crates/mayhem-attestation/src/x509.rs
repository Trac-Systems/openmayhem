use std::collections::BTreeSet;

use p256::{
    elliptic_curve::{
        bigint::U256, ff::PrimeField, ops::Reduce, point::AffineCoordinates, sec1::ToEncodedPoint,
        Field, Group,
    },
    pkcs8::{DecodePublicKey, EncodePublicKey},
    FieldBytes, ProjectivePoint, PublicKey as P256PublicKey, Scalar,
};
use rsa::{
    pkcs1::EncodeRsaPublicKey,
    pkcs1v15::{Signature as RsaSignature, VerifyingKey as RsaVerifyingKey},
    signature::Verifier as _,
    traits::PublicKeyParts,
    RsaPublicKey,
};
use sha2::{Digest, Sha256, Sha384, Sha512};
use thiserror::Error;

const MAX_CERTIFICATE_BYTES: usize = 64 * 1024;
const MAX_CERTIFICATE_CHAIN_DEPTH: usize = 8;

const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const OID_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
const OID_EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
const OID_TCG_AT_TPM_MANUFACTURER: &[u8] = &[0x67, 0x81, 0x05, 0x02, 0x01];
const OID_TCG_AT_TPM_MODEL: &[u8] = &[0x67, 0x81, 0x05, 0x02, 0x02];
const OID_TCG_AT_TPM_VERSION: &[u8] = &[0x67, 0x81, 0x05, 0x02, 0x03];
const OID_TCG_KP_EK_CERTIFICATE: &[u8] = &[0x67, 0x81, 0x05, 0x08, 0x01];
const OID_RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum EkCertificateError {
    #[error("EK certificate chain must contain between 1 and {MAX_CERTIFICATE_CHAIN_DEPTH} certificates")]
    InvalidChainLength,
    #[error("admin policy contains no EK trust anchors")]
    NoTrustAnchors,
    #[error("X.509 certificate {index} is invalid: {reason}")]
    InvalidCertificate { index: usize, reason: String },
    #[error("X.509 certificate {index} is not valid at unix time {now_unix}")]
    CertificateTime { index: usize, now_unix: u64 },
    #[error("X.509 certificate {index} issuer does not match its parent")]
    IssuerMismatch { index: usize },
    #[error("X.509 certificate {index} signature is invalid or unsupported")]
    InvalidSignature { index: usize },
    #[error("X.509 certificate {index} is not an authorized CA")]
    InvalidCa { index: usize },
    #[error("X.509 certificate {index} violates a path-length constraint")]
    PathLength { index: usize },
    #[error("EK certificate chain does not terminate at an admin-pinned trust anchor")]
    Untrusted,
    #[error("EK certificate is a CA certificate")]
    LeafIsCa,
    #[error("EK certificate key usage is incompatible with its public key")]
    InvalidEkKeyUsage,
    #[error("EK certificate extended key usage does not authorize a TPM EK credential")]
    InvalidEkExtendedKeyUsage,
    #[error("EK certificate public key is unsupported: {0}")]
    UnsupportedEkPublicKey(String),
}

#[derive(Clone, Debug)]
pub(crate) enum EkPublicKey {
    Rsa(RsaPublicKey),
    EccP256(P256PublicKey),
}

#[derive(Clone, Debug)]
pub(crate) struct TrustedEkCertificate {
    pub(crate) public_key: EkPublicKey,
    pub(crate) canonical_spki_der: Vec<u8>,
    pub(crate) device_id: String,
}

pub(crate) fn verify_ek_certificate_chain(
    chain_der: &[Vec<u8>],
    trust_anchor_der: &[Vec<u8>],
    now_unix: u64,
) -> Result<TrustedEkCertificate, EkCertificateError> {
    if chain_der.is_empty() || chain_der.len() > MAX_CERTIFICATE_CHAIN_DEPTH {
        return Err(EkCertificateError::InvalidChainLength);
    }
    if trust_anchor_der.is_empty() {
        return Err(EkCertificateError::NoTrustAnchors);
    }

    let chain = chain_der
        .iter()
        .enumerate()
        .map(|(index, der)| {
            parse_certificate(der)
                .map_err(|reason| EkCertificateError::InvalidCertificate { index, reason })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let anchors = trust_anchor_der
        .iter()
        .enumerate()
        .map(|(index, der)| {
            parse_certificate(der).map_err(|reason| EkCertificateError::InvalidCertificate {
                index: chain_der.len() + index,
                reason,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (index, certificate) in chain.iter().enumerate() {
        validate_time(certificate, index, now_unix)?;
        if index > 0 {
            validate_ca(certificate, index, index - 1)?;
            let child = &chain[index - 1];
            if child.issuer != certificate.subject {
                return Err(EkCertificateError::IssuerMismatch { index: index - 1 });
            }
            verify_certificate_signature(child, certificate)
                .map_err(|_| EkCertificateError::InvalidSignature { index: index - 1 })?;
        }
    }

    let leaf = &chain[0];
    if leaf.is_ca {
        return Err(EkCertificateError::LeafIsCa);
    }
    let public_key = extract_ek_public_key(leaf)?;
    validate_ek_usage(leaf, &public_key)?;

    let top = chain.last().expect("non-empty chain");
    let mut trusted = false;
    for (anchor_offset, anchor) in anchors.iter().enumerate() {
        let anchor_index = chain_der.len() + anchor_offset;
        validate_time(anchor, anchor_index, now_unix)?;
        let subordinate_cas = if top.raw == anchor.raw {
            chain.len().saturating_sub(2)
        } else {
            chain.len().saturating_sub(1)
        };
        validate_ca(anchor, anchor_index, subordinate_cas)?;
        if top.raw == anchor.raw {
            trusted = true;
            break;
        }
        if top.issuer == anchor.subject && verify_certificate_signature(top, anchor).is_ok() {
            trusted = true;
            break;
        }
    }
    if !trusted {
        return Err(EkCertificateError::Untrusted);
    }

    let (canonical_spki_der, device_identity) = match &public_key {
        EkPublicKey::Rsa(key) => (
            key.to_public_key_der()
                .map_err(|err| EkCertificateError::UnsupportedEkPublicKey(err.to_string()))?
                .as_bytes()
                .to_vec(),
            key.to_pkcs1_der()
                .map_err(|err| EkCertificateError::UnsupportedEkPublicKey(err.to_string()))?
                .as_bytes()
                .to_vec(),
        ),
        EkPublicKey::EccP256(key) => (
            key.to_public_key_der()
                .map_err(|err| EkCertificateError::UnsupportedEkPublicKey(err.to_string()))?
                .as_bytes()
                .to_vec(),
            key.to_encoded_point(false).as_bytes().to_vec(),
        ),
    };
    Ok(TrustedEkCertificate {
        public_key,
        canonical_spki_der,
        // The device identity is the stable public-key material, not the
        // replaceable vendor certificate that carries it.
        device_id: hex::encode(Sha256::digest(device_identity)),
    })
}

pub(crate) fn validate_ek_trust_anchors(
    trust_anchor_der: &[Vec<u8>],
    now_unix: u64,
) -> Result<(), EkCertificateError> {
    if trust_anchor_der.is_empty() {
        return Err(EkCertificateError::NoTrustAnchors);
    }
    for (index, der) in trust_anchor_der.iter().enumerate() {
        let anchor = parse_certificate(der)
            .map_err(|reason| EkCertificateError::InvalidCertificate { index, reason })?;
        validate_time(&anchor, index, now_unix)?;
        validate_ca(&anchor, index, 0)?;
        if RsaPublicKey::from_public_key_der(anchor.spki).is_err()
            && P256PublicKey::from_public_key_der(anchor.spki).is_err()
        {
            return Err(EkCertificateError::UnsupportedEkPublicKey(
                "trust-anchor key must be RSA or NIST P-256".to_owned(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum SignatureAlgorithm {
    RsaSha256,
    RsaSha384,
    RsaSha512,
    EcdsaSha256,
}

#[derive(Clone, Copy, Default)]
struct KeyUsage {
    key_encipherment: bool,
    key_agreement: bool,
    key_cert_sign: bool,
}

struct ParsedCertificate<'a> {
    raw: &'a [u8],
    tbs: &'a [u8],
    issuer: &'a [u8],
    subject: &'a [u8],
    spki: &'a [u8],
    signature_algorithm: SignatureAlgorithm,
    signature: &'a [u8],
    not_before: u64,
    not_after: u64,
    is_ca: bool,
    basic_constraints_present: bool,
    basic_constraints_critical: bool,
    path_len: Option<u32>,
    key_usage: Option<KeyUsage>,
    key_usage_critical: bool,
    extended_key_usage: Option<Vec<Vec<u8>>>,
}

fn parse_certificate(input: &[u8]) -> Result<ParsedCertificate<'_>, String> {
    if input.is_empty() || input.len() > MAX_CERTIFICATE_BYTES {
        return Err("certificate size is outside the accepted bounds".to_owned());
    }
    let outer = parse_single(input, 0x30)?;
    let mut outer_reader = DerReader::new(outer.value);
    let tbs = outer_reader.take(0x30)?;
    let outer_signature_algorithm = outer_reader.take(0x30)?;
    let signature = outer_reader.take(0x03)?;
    outer_reader.finish()?;
    if signature.value.first() != Some(&0) || signature.value.len() < 2 {
        return Err("certificate signature BIT STRING is malformed".to_owned());
    }

    let mut tbs_reader = DerReader::new(tbs.value);
    if tbs_reader.peek_tag() == Some(0xa0) {
        let version = tbs_reader.take(0xa0)?;
        let version = parse_single(version.value, 0x02)?;
        if version.value != [2] {
            return Err("only X.509 version 3 certificates are accepted".to_owned());
        }
    } else {
        return Err("only X.509 version 3 certificates are accepted".to_owned());
    }
    tbs_reader.take(0x02)?;
    let tbs_signature_algorithm = tbs_reader.take(0x30)?;
    if tbs_signature_algorithm.full != outer_signature_algorithm.full {
        return Err("inner and outer signature algorithms differ".to_owned());
    }
    let issuer = tbs_reader.take(0x30)?;
    let validity = tbs_reader.take(0x30)?;
    let subject = tbs_reader.take(0x30)?;
    let spki = tbs_reader.take(0x30)?;

    let (not_before, not_after) = parse_validity(validity.value)?;
    let mut is_ca = false;
    let mut basic_constraints_present = false;
    let mut basic_constraints_critical = false;
    let mut path_len = None;
    let mut key_usage = None;
    let mut key_usage_critical = false;
    let mut extended_key_usage = None;
    let mut seen_extensions = BTreeSet::new();

    while !tbs_reader.is_empty() {
        let tag = tbs_reader.peek_tag().ok_or("truncated TBSCertificate")?;
        match tag {
            0x81 | 0x82 => {
                tbs_reader.take(tag)?;
            }
            0xa3 => {
                let wrapper = tbs_reader.take(0xa3)?;
                let sequence = parse_single(wrapper.value, 0x30)?;
                let mut extensions = DerReader::new(sequence.value);
                while !extensions.is_empty() {
                    let extension = extensions.take(0x30)?;
                    let mut fields = DerReader::new(extension.value);
                    let oid = fields.take(0x06)?.value;
                    if !seen_extensions.insert(oid.to_vec()) {
                        return Err("duplicate X.509 extension".to_owned());
                    }
                    let critical = if fields.peek_tag() == Some(0x01) {
                        let value = fields.take(0x01)?.value;
                        parse_x509_boolean(value, "critical extension")?
                    } else {
                        false
                    };
                    let value = fields.take(0x04)?.value;
                    fields.finish()?;
                    match oid {
                        OID_BASIC_CONSTRAINTS => {
                            let parsed = parse_basic_constraints(value)?;
                            is_ca = parsed.0;
                            path_len = parsed.1;
                            basic_constraints_present = true;
                            basic_constraints_critical = critical;
                        }
                        OID_KEY_USAGE => {
                            key_usage = Some(parse_key_usage(value)?);
                            key_usage_critical = critical;
                        }
                        OID_EXTENDED_KEY_USAGE => {
                            extended_key_usage = Some(parse_extended_key_usage(value)?)
                        }
                        OID_SUBJECT_ALT_NAME if critical => parse_tpm_subject_alt_name(value)?,
                        OID_SUBJECT_ALT_NAME => {}
                        _ if critical => {
                            return Err("unsupported critical X.509 extension".to_owned());
                        }
                        _ => {}
                    }
                }
            }
            _ => return Err("unexpected TBSCertificate field".to_owned()),
        }
    }

    Ok(ParsedCertificate {
        raw: input,
        tbs: tbs.full,
        issuer: issuer.full,
        subject: subject.full,
        spki: spki.full,
        signature_algorithm: parse_signature_algorithm(outer_signature_algorithm.value)?,
        signature: &signature.value[1..],
        not_before,
        not_after,
        is_ca,
        basic_constraints_present,
        basic_constraints_critical,
        path_len,
        key_usage,
        key_usage_critical,
        extended_key_usage,
    })
}

fn extract_ek_public_key(
    certificate: &ParsedCertificate<'_>,
) -> Result<EkPublicKey, EkCertificateError> {
    if let Ok(key) = RsaPublicKey::from_public_key_der(certificate.spki) {
        if !(2048..=4096).contains(&key.n().bits()) {
            return Err(EkCertificateError::UnsupportedEkPublicKey(
                "RSA EK must be between 2048 and 4096 bits".to_owned(),
            ));
        }
        return Ok(EkPublicKey::Rsa(key));
    }
    if let Ok(key) = P256PublicKey::from_public_key_der(certificate.spki) {
        return Ok(EkPublicKey::EccP256(key));
    }
    Err(EkCertificateError::UnsupportedEkPublicKey(
        "expected RSA 2048-4096 or NIST P-256".to_owned(),
    ))
}

fn validate_ek_usage(
    leaf: &ParsedCertificate<'_>,
    public_key: &EkPublicKey,
) -> Result<(), EkCertificateError> {
    if !leaf.basic_constraints_present || leaf.is_ca {
        return Err(EkCertificateError::LeafIsCa);
    }
    let usage = leaf
        .key_usage
        .filter(|_| leaf.key_usage_critical)
        .ok_or(EkCertificateError::InvalidEkKeyUsage)?;
    let permitted = match public_key {
        EkPublicKey::Rsa(_) => usage.key_encipherment,
        EkPublicKey::EccP256(_) => usage.key_agreement,
    };
    if !permitted || usage.key_cert_sign {
        return Err(EkCertificateError::InvalidEkKeyUsage);
    }
    if !leaf.extended_key_usage.as_ref().is_some_and(|usages| {
        usages
            .iter()
            .any(|usage| usage == OID_TCG_KP_EK_CERTIFICATE)
    }) {
        return Err(EkCertificateError::InvalidEkExtendedKeyUsage);
    }
    Ok(())
}

fn validate_time(
    certificate: &ParsedCertificate<'_>,
    index: usize,
    now_unix: u64,
) -> Result<(), EkCertificateError> {
    if now_unix < certificate.not_before || now_unix > certificate.not_after {
        Err(EkCertificateError::CertificateTime { index, now_unix })
    } else {
        Ok(())
    }
}

fn validate_ca(
    certificate: &ParsedCertificate<'_>,
    index: usize,
    subordinate_cas: usize,
) -> Result<(), EkCertificateError> {
    if !certificate.basic_constraints_present
        || !certificate.basic_constraints_critical
        || !certificate.is_ca
        || !certificate.key_usage_critical
        || !certificate
            .key_usage
            .is_some_and(|usage| usage.key_cert_sign)
    {
        return Err(EkCertificateError::InvalidCa { index });
    }
    if certificate
        .path_len
        .is_some_and(|limit| subordinate_cas > limit as usize)
    {
        return Err(EkCertificateError::PathLength { index });
    }
    Ok(())
}

fn verify_certificate_signature(
    certificate: &ParsedCertificate<'_>,
    issuer: &ParsedCertificate<'_>,
) -> Result<(), ()> {
    match certificate.signature_algorithm {
        SignatureAlgorithm::RsaSha256 => verify_rsa::<Sha256>(certificate, issuer),
        SignatureAlgorithm::RsaSha384 => verify_rsa::<Sha384>(certificate, issuer),
        SignatureAlgorithm::RsaSha512 => verify_rsa::<Sha512>(certificate, issuer),
        SignatureAlgorithm::EcdsaSha256 => {
            let key = P256PublicKey::from_public_key_der(issuer.spki).map_err(|_| ())?;
            let (r, s) = parse_ecdsa_der_signature(certificate.signature).map_err(|_| ())?;
            verify_p256_signature(&key, certificate.tbs, &r, &s)
        }
    }
}

fn verify_rsa<D>(
    certificate: &ParsedCertificate<'_>,
    issuer: &ParsedCertificate<'_>,
) -> Result<(), ()>
where
    D: sha2::digest::Digest + sha2::digest::FixedOutputReset + rsa::pkcs8::AssociatedOid,
{
    let key = RsaPublicKey::from_public_key_der(issuer.spki).map_err(|_| ())?;
    let signature = RsaSignature::try_from(certificate.signature).map_err(|_| ())?;
    RsaVerifyingKey::<D>::new(key)
        .verify(certificate.tbs, &signature)
        .map_err(|_| ())
}

pub(crate) fn verify_p256_signature(
    key: &P256PublicKey,
    message: &[u8],
    r: &[u8; 32],
    s: &[u8; 32],
) -> Result<(), ()> {
    let r = Option::<Scalar>::from(Scalar::from_repr((*r).into())).ok_or(())?;
    let s = Option::<Scalar>::from(Scalar::from_repr((*s).into())).ok_or(())?;
    if bool::from(r.is_zero()) || bool::from(s.is_zero()) {
        return Err(());
    }
    let inverse = Option::<Scalar>::from(s.invert()).ok_or(())?;
    let digest = Sha256::digest(message);
    let z = <Scalar as Reduce<U256>>::reduce_bytes(FieldBytes::from_slice(&digest));
    let point = ProjectivePoint::GENERATOR * (z * inverse)
        + ProjectivePoint::from(*key.as_affine()) * (r * inverse);
    if bool::from(point.is_identity()) {
        return Err(());
    }
    let x = point.to_affine().x();
    let reduced_x = <Scalar as Reduce<U256>>::reduce_bytes(&x);
    if reduced_x == r {
        Ok(())
    } else {
        Err(())
    }
}

fn parse_ecdsa_der_signature(input: &[u8]) -> Result<([u8; 32], [u8; 32]), String> {
    let sequence = parse_single(input, 0x30)?;
    let mut fields = DerReader::new(sequence.value);
    let r = parse_positive_scalar(fields.take(0x02)?.value)?;
    let s = parse_positive_scalar(fields.take(0x02)?.value)?;
    fields.finish()?;
    Ok((r, s))
}

fn parse_positive_scalar(input: &[u8]) -> Result<[u8; 32], String> {
    if input.is_empty()
        || input.len() > 33
        || input[0] & 0x80 != 0
        || (input.len() > 1 && input[0] == 0 && input[1] & 0x80 == 0)
    {
        return Err("invalid ECDSA DER INTEGER".to_owned());
    }
    let input = input.strip_prefix(&[0]).unwrap_or(input);
    if input.len() > 32 {
        return Err("ECDSA scalar exceeds P-256".to_owned());
    }
    let mut scalar = [0u8; 32];
    scalar[32 - input.len()..].copy_from_slice(input);
    Ok(scalar)
}

fn parse_signature_algorithm(input: &[u8]) -> Result<SignatureAlgorithm, String> {
    let mut fields = DerReader::new(input);
    let oid = fields.take(0x06)?.value;
    let parameters = if fields.is_empty() {
        None
    } else {
        Some(fields.take_any()?)
    };
    fields.finish()?;
    match oid {
        OID_RSA_SHA256 | OID_RSA_SHA384 | OID_RSA_SHA512
            if parameters.is_some_and(|parameters| {
                parameters.tag != 0x05 || !parameters.value.is_empty()
            }) =>
        {
            Err("unsupported RSA certificate signature parameters".to_owned())
        }
        OID_RSA_SHA256 => Ok(SignatureAlgorithm::RsaSha256),
        OID_RSA_SHA384 => Ok(SignatureAlgorithm::RsaSha384),
        OID_RSA_SHA512 => Ok(SignatureAlgorithm::RsaSha512),
        OID_ECDSA_SHA256 if parameters.is_none() => Ok(SignatureAlgorithm::EcdsaSha256),
        OID_ECDSA_SHA256 => Err("ECDSA certificate signature parameters must be absent".to_owned()),
        _ => Err("unsupported certificate signature algorithm".to_owned()),
    }
}

fn parse_basic_constraints(input: &[u8]) -> Result<(bool, Option<u32>), String> {
    let sequence = parse_single(input, 0x30)?;
    let mut fields = DerReader::new(sequence.value);
    let is_ca = if fields.peek_tag() == Some(0x01) {
        let value = fields.take(0x01)?.value;
        parse_x509_boolean(value, "basicConstraints cA")?
    } else {
        false
    };
    let path_len = if fields.peek_tag() == Some(0x02) {
        Some(parse_positive_u32(fields.take(0x02)?.value)?)
    } else {
        None
    };
    fields.finish()?;
    if path_len.is_some() && !is_ca {
        return Err("path length is present on a non-CA certificate".to_owned());
    }
    Ok((is_ca, path_len))
}

fn parse_x509_boolean(input: &[u8], field: &str) -> Result<bool, String> {
    // Some signed TPM EK certificates explicitly encode DEFAULT FALSE or use
    // BER's 0x01 TRUE. The original TBS bytes are still verified unchanged.
    match input {
        [value] => Ok(*value != 0),
        _ => Err(format!("{field} BOOLEAN must contain exactly one byte")),
    }
}

fn parse_tpm_subject_alt_name(input: &[u8]) -> Result<(), String> {
    let sequence = parse_single(input, 0x30)?;
    let mut names = DerReader::new(sequence.value);
    if names.is_empty() {
        return Err("critical TPM subjectAltName is empty".to_owned());
    }

    let required = [
        OID_TCG_AT_TPM_MANUFACTURER,
        OID_TCG_AT_TPM_MODEL,
        OID_TCG_AT_TPM_VERSION,
    ];
    let mut seen = BTreeSet::new();
    while !names.is_empty() {
        let directory_name = names.take(0xa4).map_err(|_| {
            "critical TPM subjectAltName contains a non-directoryName entry".to_owned()
        })?;
        let name = parse_single(directory_name.value, 0x30)?;
        let mut rdns = DerReader::new(name.value);
        if rdns.is_empty() {
            return Err("critical TPM subjectAltName contains an empty directoryName".to_owned());
        }
        while !rdns.is_empty() {
            let rdn = rdns.take(0x31)?;
            let mut attributes = DerReader::new(rdn.value);
            if attributes.is_empty() {
                return Err("critical TPM subjectAltName contains an empty RDN".to_owned());
            }
            while !attributes.is_empty() {
                let attribute = attributes.take(0x30)?;
                let mut fields = DerReader::new(attribute.value);
                let oid = fields.take(0x06)?.value;
                let value = fields.take_any()?;
                fields.finish()?;
                if required.contains(&oid) {
                    if !seen.insert(oid.to_vec()) {
                        return Err(
                            "critical TPM subjectAltName repeats a TCG attribute".to_owned()
                        );
                    }
                    if value.tag != 0x0c
                        || value.value.is_empty()
                        || value.value.len() > 256
                        || std::str::from_utf8(value.value).is_err()
                    {
                        return Err(
                            "critical TPM subjectAltName has an invalid TCG attribute value"
                                .to_owned(),
                        );
                    }
                }
            }
        }
    }
    if required.iter().any(|oid| !seen.contains(*oid)) {
        return Err(
            "critical TPM subjectAltName must identify manufacturer, model, and version".to_owned(),
        );
    }
    Ok(())
}

fn parse_key_usage(input: &[u8]) -> Result<KeyUsage, String> {
    let bits = parse_single(input, 0x03)?;
    if !(2..=3).contains(&bits.value.len()) || bits.value[0] > 7 {
        return Err("malformed keyUsage BIT STRING".to_owned());
    }
    let unused = bits.value[0];
    let last = *bits.value.last().expect("keyUsage has data bytes");
    if unused > 0 && last & ((1u8 << unused) - 1) != 0 {
        return Err("non-zero unused keyUsage bits".to_owned());
    }
    let first = bits.value[1];
    Ok(KeyUsage {
        key_encipherment: first & 0x20 != 0,
        key_agreement: first & 0x08 != 0,
        key_cert_sign: first & 0x04 != 0,
    })
}

fn parse_extended_key_usage(input: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let sequence = parse_single(input, 0x30)?;
    let mut fields = DerReader::new(sequence.value);
    let mut usages = Vec::new();
    while !fields.is_empty() {
        usages.push(fields.take(0x06)?.value.to_vec());
    }
    if usages.is_empty() {
        return Err("empty extendedKeyUsage".to_owned());
    }
    Ok(usages)
}

fn parse_validity(input: &[u8]) -> Result<(u64, u64), String> {
    let mut fields = DerReader::new(input);
    let not_before = fields.take_any()?;
    let not_after = fields.take_any()?;
    fields.finish()?;
    let not_before = parse_x509_time(not_before.tag, not_before.value)?;
    let not_after = parse_x509_time(not_after.tag, not_after.value)?;
    if not_after < not_before {
        return Err("certificate validity is inverted".to_owned());
    }
    Ok((not_before, not_after))
}

fn parse_x509_time(tag: u8, input: &[u8]) -> Result<u64, String> {
    let (year, offset) = match tag {
        0x17 if input.len() == 13 && input[12] == b'Z' => {
            let short = decimal(input, 0, 2)?;
            ((if short >= 50 { 1900 } else { 2000 }) + short, 2)
        }
        0x18 if input.len() == 15 && input[14] == b'Z' => (decimal(input, 0, 4)?, 4),
        _ => return Err("unsupported X.509 time encoding".to_owned()),
    };
    let month = decimal(input, offset, 2)?;
    let day = decimal(input, offset + 2, 2)?;
    let hour = decimal(input, offset + 4, 2)?;
    let minute = decimal(input, offset + 6, 2)?;
    let second = decimal(input, offset + 8, 2)?;
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err("invalid X.509 calendar time".to_owned());
    }
    let days = days_from_civil(year as i64, month as i64, day as i64);
    if days < 0 {
        return Err("pre-epoch certificates are unsupported".to_owned());
    }
    Ok(days as u64 * 86_400 + hour as u64 * 3_600 + minute as u64 * 60 + second as u64)
}

fn decimal(input: &[u8], offset: usize, length: usize) -> Result<u32, String> {
    let digits = input
        .get(offset..offset + length)
        .ok_or("truncated X.509 time")?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err("non-decimal X.509 time".to_owned());
    }
    Ok(digits
        .iter()
        .fold(0u32, |value, digit| value * 10 + u32::from(*digit - b'0')))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn parse_positive_u32(input: &[u8]) -> Result<u32, String> {
    if input.is_empty()
        || input.len() > 5
        || input[0] & 0x80 != 0
        || (input.len() > 1 && input[0] == 0 && input[1] & 0x80 == 0)
    {
        return Err("invalid non-negative DER INTEGER".to_owned());
    }
    input.iter().try_fold(0u32, |value, byte| {
        value
            .checked_mul(256)
            .and_then(|value| value.checked_add(u32::from(*byte)))
            .ok_or_else(|| "DER INTEGER exceeds u32".to_owned())
    })
}

#[derive(Clone, Copy)]
struct DerElement<'a> {
    tag: u8,
    full: &'a [u8],
    value: &'a [u8],
}

struct DerReader<'a> {
    remaining: &'a [u8],
}

impl<'a> DerReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { remaining: input }
    }

    fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }

    fn peek_tag(&self) -> Option<u8> {
        self.remaining.first().copied()
    }

    fn take(&mut self, expected_tag: u8) -> Result<DerElement<'a>, String> {
        let element = self.take_any()?;
        if element.tag != expected_tag {
            return Err(format!(
                "unexpected DER tag {:02x}; expected {expected_tag:02x}",
                element.tag
            ));
        }
        Ok(element)
    }

    fn take_any(&mut self) -> Result<DerElement<'a>, String> {
        let (element, remaining) = parse_element(self.remaining)?;
        self.remaining = remaining;
        Ok(element)
    }

    fn finish(self) -> Result<(), String> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err("trailing DER data".to_owned())
        }
    }
}

fn parse_single(input: &[u8], expected_tag: u8) -> Result<DerElement<'_>, String> {
    let (element, remaining) = parse_element(input)?;
    if !remaining.is_empty() {
        return Err("trailing DER data".to_owned());
    }
    if element.tag != expected_tag {
        return Err(format!(
            "unexpected DER tag {:02x}; expected {expected_tag:02x}",
            element.tag
        ));
    }
    Ok(element)
}

fn parse_element(input: &[u8]) -> Result<(DerElement<'_>, &[u8]), String> {
    let tag = *input.first().ok_or("truncated DER tag")?;
    if tag & 0x1f == 0x1f {
        return Err("high-tag-number DER is unsupported".to_owned());
    }
    let first_length = *input.get(1).ok_or("truncated DER length")?;
    let (header_len, value_len) = if first_length & 0x80 == 0 {
        (2usize, usize::from(first_length))
    } else {
        let count = usize::from(first_length & 0x7f);
        if count == 0 || count > std::mem::size_of::<usize>() {
            return Err("invalid DER length".to_owned());
        }
        let length_bytes = input.get(2..2 + count).ok_or("truncated DER length")?;
        if length_bytes[0] == 0 {
            return Err("non-minimal DER length".to_owned());
        }
        let value_len = length_bytes.iter().try_fold(0usize, |value, byte| {
            value
                .checked_mul(256)
                .and_then(|value| value.checked_add(usize::from(*byte)))
                .ok_or_else(|| "DER length overflow".to_owned())
        })?;
        if value_len < 128 {
            return Err("non-minimal DER length".to_owned());
        }
        (2 + count, value_len)
    };
    let end = header_len
        .checked_add(value_len)
        .ok_or("DER length overflow")?;
    let full = input.get(..end).ok_or("truncated DER value")?;
    let value = &full[header_len..];
    Ok((
        DerElement { tag, full, value },
        input.get(end..).expect("validated DER end"),
    ))
}

#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

    use super::{
        parse_certificate, parse_signature_algorithm, parse_tpm_subject_alt_name,
        parse_x509_boolean,
    };

    #[test]
    fn x509_boolean_accepts_der_and_vendor_ber_encodings() {
        assert_eq!(parse_x509_boolean(&[0xff], "test"), Ok(true));
        assert_eq!(parse_x509_boolean(&[0x01], "test"), Ok(true));
        assert_eq!(parse_x509_boolean(&[0x00], "test"), Ok(false));
    }

    #[test]
    fn x509_boolean_rejects_malformed_values() {
        assert!(parse_x509_boolean(&[], "test").is_err());
        assert!(parse_x509_boolean(&[0x01, 0xff], "test").is_err());
    }

    #[test]
    fn critical_tpm_subject_alt_name_requires_the_standard_tcg_identity() {
        let encoded = BASE64
            .decode("MESkQjBAMRYwFAYFZ4EFAgEMC2lkOjQxNEQ0NDAwMQ4wDAYFZ4EFAgIMA0FNRDEWMBQGBWeBBQIDDAtpZDowMDAzMDAwMQ==")
            .unwrap();
        assert!(parse_tpm_subject_alt_name(&encoded).is_ok());

        let mut missing_model = encoded;
        let model_oid = [0x06, 0x05, 0x67, 0x81, 0x05, 0x02, 0x02];
        let offset = missing_model
            .windows(model_oid.len())
            .position(|window| window == model_oid)
            .unwrap();
        missing_model[offset + model_oid.len() - 1] = 0x04;
        assert!(parse_tpm_subject_alt_name(&missing_model).is_err());
    }

    #[test]
    fn ecdsa_certificate_signature_parameters_must_be_absent() {
        let ecdsa_sha256 = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
        assert!(parse_signature_algorithm(&ecdsa_sha256).is_ok());

        let mut with_null = ecdsa_sha256.to_vec();
        with_null.extend_from_slice(&[0x05, 0x00]);
        assert!(parse_signature_algorithm(&with_null).is_err());
    }

    #[test]
    fn unprocessed_critical_extensions_are_rejected() {
        let mut certificate = BASE64
            .decode(include_str!("../tests/fixtures/tpm-ek-rsa.der.b64").trim())
            .unwrap();
        let basic_constraints_critical = [0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff];
        let offset = certificate
            .windows(basic_constraints_critical.len())
            .position(|window| window == basic_constraints_critical)
            .unwrap();
        certificate[offset + 4] = 0x0e;

        let error = parse_certificate(&certificate).err().unwrap();
        assert!(error.contains("unsupported critical X.509 extension"));
    }
}
