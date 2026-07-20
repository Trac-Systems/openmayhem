use std::collections::BTreeMap;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use mayhem_attestation::ValidatedAttestationPolicy;
use mayhem_proto::TpmQuoteEvidence;
use rsa::{
    pkcs1v15::{Signature as RsaSignature, VerifyingKey as RsaVerifyingKey},
    signature::Verifier as _,
    traits::PublicKeyParts,
    BigUint, RsaPublicKey,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use subtle::ConstantTimeEq;
use url::Url;

use crate::{
    collateral::{AuthenticatedCollateral, AuthenticatedItem},
    device_digest, evidence_gap, reject,
    tpm::{self, VerifiedAkPublic},
    Result, VerifiedCpu, VerifyError, VerifyRequest,
};

const MAX_MAA_JWT_BYTES: usize = 512 * 1024;
const MAX_JWKS_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JwtHeader {
    alg: String,
    jku: String,
    kid: String,
    #[serde(default)]
    typ: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Jwk {
    kty: String,
    kid: String,
    n: String,
    e: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default, rename = "use")]
    usage: Option<String>,
    #[serde(default)]
    key_ops: Vec<String>,
    #[serde(default)]
    x5c: Vec<String>,
    #[serde(default)]
    x5t: Option<String>,
}

pub(crate) fn verify(
    request: &VerifyRequest,
    policy: &ValidatedAttestationPolicy,
    collateral: &AuthenticatedCollateral,
    maa_jwt: &str,
    workload_quote: &TpmQuoteEvidence,
    now_unix: u64,
) -> Result<VerifiedCpu> {
    let platform = request
        .declared_platform
        .as_deref()
        .ok_or_else(|| VerifyError::Rejected("MAA evidence has no platform".into()))?;
    if !is_azure_platform(platform) {
        return reject("Azure MAA is accepted only for an admin-approved Azure platform");
    }

    let (header_segment, claims_segment, signature_segment) = split_jwt(maa_jwt)?;
    let header = decode_json::<JwtHeader>(header_segment, "MAA JWT header")?;
    if header.alg != "RS256" {
        return reject("MAA JWT algorithm is not RS256");
    }
    if header.kid.is_empty() {
        return reject("MAA JWT has no key id");
    }
    if header.typ.as_deref().is_some_and(|typ| typ != "JWT") {
        return reject("MAA JWT typ is not JWT");
    }
    let jwks_item = authenticated_jwks(policy, request, collateral, &header.jku)?;
    let jwks = parse_jwks(jwks_item)?;
    let signing_key = select_signing_key(&jwks, &header.kid)?;
    verify_jwt_signature(
        &signing_key,
        header_segment,
        claims_segment,
        signature_segment,
    )?;

    let claims = decode_json::<Value>(claims_segment, "MAA JWT claims")?;
    validate_temporal_claims(&claims, now_unix)?;
    validate_issuer(&claims, &header.jku)?;
    validate_platform_claims(&claims)?;
    validate_runtime_binding(&claims, &request.quote.binding)?;

    let chip_id = claim_hex(&claims, "x-ms-sevsnpvm-chipid", 64)?;
    let device_id = device_digest(&chip_id)?;
    if device_id != request.evidence_binding.device_id {
        return reject("MAA-signed SNP chip identity does not match the selected route device");
    }
    let launch = claim_hex(&claims, "x-ms-sevsnpvm-launchmeasurement", 48)?;
    let workload_policy = request
        .policy_measurement_collateral
        .get(&mayhem_proto::AttestationMeasurementLayer::Workload)
        .ok_or_else(|| {
            VerifyError::Rejected("admin policy has no Azure workload measurement layer".into())
        })?;
    let workload =
        tpm::verify_workload_quote(workload_quote, &request.quote.binding, workload_policy)?;
    bind_hcl_ak(&claims, &workload.ak_public)?;

    let mut cpu_measurements =
        BTreeMap::from([("snp_launch_measurement".to_owned(), hex::encode(&launch))]);
    if let Some(policy_hash) = claims
        .get("x-ms-policy-hash")
        .and_then(Value::as_str)
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .filter(|value| value.len() == 32)
    {
        cpu_measurements.insert("maa_policy_hash".to_owned(), hex::encode(policy_hash));
    }
    let chip_family = claim_string(&claims, "x-ms-sevsnpvm-chip-family")?;
    let tcb = format!(
        "bl{}-tee{}-snp{}-ucode{}",
        claim_u64(&claims, "x-ms-sevsnpvm-bootloader-svn")?,
        claim_u64(&claims, "x-ms-sevsnpvm-tee-svn")?,
        claim_u64(&claims, "x-ms-sevsnpvm-snpfw-svn")?,
        claim_u64(&claims, "x-ms-sevsnpvm-microcode-svn")?
    );

    Ok(VerifiedCpu {
        roots: vec!["azure_maa_jwt_jwks_issuer_nonce_claims".to_owned()],
        cpu_measurements,
        workload_measurements: workload.measurements,
        device_id,
        snp_chip_family: Some(chip_family.to_owned()),
        snp_chip_id: Some(hex::encode(chip_id)),
        snp_tcb: Some(tcb),
    })
}

fn split_jwt(token: &str) -> Result<(&str, &str, &str)> {
    if token.is_empty() || token.len() > MAX_MAA_JWT_BYTES || !token.is_ascii() {
        return reject("MAA JWT is empty, non-ASCII, or exceeds its size limit");
    }
    let mut parts = token.split('.');
    let header = parts.next().unwrap_or_default();
    let claims = parts.next().unwrap_or_default();
    let signature = parts.next().unwrap_or_default();
    if header.is_empty() || claims.is_empty() || signature.is_empty() || parts.next().is_some() {
        return reject("MAA JWT must contain exactly three non-empty segments");
    }
    Ok((header, claims, signature))
}

fn decode_json<T: serde::de::DeserializeOwned>(encoded: &str, field: &str) -> Result<T> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| VerifyError::Rejected(format!("{field} is not base64url")))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| VerifyError::Rejected(format!("{field} is invalid JSON: {error}")))
}

fn authenticated_jwks<'a>(
    policy: &'a ValidatedAttestationPolicy,
    request: &VerifyRequest,
    collateral: &'a AuthenticatedCollateral,
    jku: &str,
) -> Result<&'a AuthenticatedItem> {
    validate_azure_jwks_url(jku)?;
    let matches = collateral
        .for_kind(policy, request.kind)
        .filter(|item| {
            item.source_url.as_deref() == Some(jku)
                && matches!(
                    item.media_type.as_str(),
                    "application/jwk-set+json" | "application/json"
                )
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [item] => Ok(*item),
        [] => evidence_gap("MAA JWT jku has no exact admin-authenticated JWKS collateral item"),
        _ => reject("MAA JWT jku resolves to ambiguous admin JWKS collateral"),
    }
}

fn validate_azure_jwks_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|error| VerifyError::Rejected(format!("MAA JWT jku is invalid: {error}")))?;
    let host = url
        .host_str()
        .ok_or_else(|| VerifyError::Rejected("MAA JWT jku has no host".into()))?;
    if url.scheme() != "https"
        || !host.ends_with(".attest.azure.net")
        || host == "attest.azure.net"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/certs"
    {
        return reject("MAA JWT jku is not an exact Azure Attestation HTTPS /certs URL");
    }
    Ok(url)
}

fn parse_jwks(item: &AuthenticatedItem) -> Result<JwkSet> {
    if item.bytes.is_empty() || item.bytes.len() > MAX_JWKS_BYTES {
        return reject("admin-authenticated MAA JWKS is empty or oversized");
    }
    let jwks = serde_json::from_slice::<JwkSet>(&item.bytes).map_err(|error| {
        VerifyError::Rejected(format!("admin-authenticated MAA JWKS is invalid: {error}"))
    })?;
    if jwks.keys.is_empty() || jwks.keys.len() > 64 {
        return reject("admin-authenticated MAA JWKS has an invalid key count");
    }
    Ok(jwks)
}

fn select_signing_key(jwks: &JwkSet, kid: &str) -> Result<RsaPublicKey> {
    let matches = jwks
        .keys
        .iter()
        .filter(|key| key.kid == kid)
        .collect::<Vec<_>>();
    let key = match matches.as_slice() {
        [key] => *key,
        [] => return reject("MAA JWT kid is absent from admin-authenticated JWKS"),
        _ => return reject("MAA JWT kid is duplicated in admin-authenticated JWKS"),
    };
    if key.kty != "RSA"
        || key.alg.as_deref().is_some_and(|alg| alg != "RS256")
        || key.usage.as_deref().is_some_and(|usage| usage != "sig")
        || key.key_ops.iter().any(|operation| operation != "verify")
        || key.x5c.len() > 8
        || key.x5t.as_deref().is_some_and(str::is_empty)
    {
        return reject("MAA JWKS key is not an RS256 verification key");
    }
    rsa_key(key)
}

fn rsa_key(jwk: &Jwk) -> Result<RsaPublicKey> {
    let modulus = URL_SAFE_NO_PAD
        .decode(&jwk.n)
        .map_err(|_| VerifyError::Rejected("RSA JWK modulus is not base64url".into()))?;
    let exponent = URL_SAFE_NO_PAD
        .decode(&jwk.e)
        .map_err(|_| VerifyError::Rejected("RSA JWK exponent is not base64url".into()))?;
    if !(256..=512).contains(&modulus.len()) || exponent.is_empty() || exponent.len() > 8 {
        return reject("RSA JWK key size or exponent is invalid");
    }
    RsaPublicKey::new(
        BigUint::from_bytes_be(&modulus),
        BigUint::from_bytes_be(&exponent),
    )
    .map_err(|error| VerifyError::Rejected(format!("RSA JWK is invalid: {error}")))
}

fn verify_jwt_signature(
    key: &RsaPublicKey,
    header: &str,
    claims: &str,
    signature: &str,
) -> Result<()> {
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| VerifyError::Rejected("MAA JWT signature is not base64url".into()))?;
    let signature = RsaSignature::try_from(signature.as_slice())
        .map_err(|_| VerifyError::Rejected("MAA JWT signature length is invalid".into()))?;
    let signing_input = format!("{header}.{claims}");
    RsaVerifyingKey::<Sha256>::new(key.clone())
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| VerifyError::Rejected("MAA JWT signature is invalid".into()))
}

fn validate_temporal_claims(claims: &Value, now_unix: u64) -> Result<()> {
    let issued = claim_u64(claims, "iat")?;
    let not_before = claim_u64(claims, "nbf")?;
    let expires = claim_u64(claims, "exp")?;
    if issued > not_before || not_before >= expires {
        return reject("MAA JWT temporal claims are inconsistent");
    }
    if now_unix < not_before || now_unix >= expires {
        return reject("MAA JWT is stale or not yet valid");
    }
    Ok(())
}

fn validate_issuer(claims: &Value, jku: &str) -> Result<()> {
    let issuer = claim_string(claims, "iss")?;
    let url = validate_azure_jwks_url(jku)?;
    let expected = format!(
        "https://{}",
        url.host_str().expect("validated URL has a host")
    );
    if issuer.trim_end_matches('/') != expected {
        return reject("MAA JWT issuer does not match the admin-pinned JWKS origin");
    }
    Ok(())
}

fn validate_platform_claims(claims: &Value) -> Result<()> {
    if claim_string(claims, "x-ms-attestation-type")? != "sevsnpvm" {
        return reject("MAA JWT is not a SEV-SNP VM attestation");
    }
    if claim_string(claims, "x-ms-compliance-status")? != "azure-compliant-cvm" {
        return reject("MAA JWT does not assert azure-compliant-cvm");
    }
    for claim in [
        "x-ms-sevsnpvm-is-debuggable",
        "x-ms-sevsnpvm-migration-allowed",
    ] {
        if claim_bool(claims, claim)? {
            return reject(format!(
                "MAA JWT permits {}",
                claim.replace("x-ms-sevsnpvm-", "")
            ));
        }
    }
    if claim_u64(claims, "x-ms-sevsnpvm-vmpl")? != 0 {
        return reject("MAA JWT SEV-SNP VMPL is not zero");
    }
    let runtime = claim_object(claims, "x-ms-runtime")?;
    let configuration = runtime
        .get("vm-configuration")
        .and_then(Value::as_object)
        .ok_or_else(|| VerifyError::Rejected("MAA JWT has no VM configuration".into()))?;
    for property in ["secure-boot", "tpm-enabled", "tpm-persisted"] {
        if configuration.get(property).and_then(Value::as_bool) != Some(true) {
            return reject(format!("MAA JWT does not assert {property}"));
        }
    }
    Ok(())
}

fn validate_runtime_binding(claims: &Value, quote_binding: &str) -> Result<()> {
    if quote_binding.len() != 64
        || !quote_binding
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return reject("Mayhem quote binding is not a lowercase 32-byte digest");
    }
    let runtime = claim_object(claims, "x-ms-runtime")?;
    let actual = runtime
        .get("user-data")
        .and_then(Value::as_str)
        .ok_or_else(|| VerifyError::Rejected("MAA JWT runtime has no user-data digest".into()))?;
    let claims_json = format!("{{\"user-claims\": {{\"nonce\": \"{quote_binding}\"}}}}");
    let expected = hex::encode_upper(Sha512::digest(claims_json.as_bytes()));
    if actual.len() != expected.len() || !bool::from(actual.as_bytes().ct_eq(expected.as_bytes())) {
        return reject("MAA JWT user-data does not bind the Mayhem hardware quote");
    }
    Ok(())
}

fn bind_hcl_ak(claims: &Value, workload_ak: &VerifiedAkPublic) -> Result<()> {
    let VerifiedAkPublic::Rsa(workload_ak) = workload_ak else {
        return evidence_gap(
            "Azure MAA HCLAkPub is RSA, but the workload quote used a non-RSA vTPM AK",
        );
    };
    let runtime = claim_object(claims, "x-ms-runtime")?;
    let keys = runtime
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| VerifyError::Rejected("MAA JWT runtime has no HCL keys".into()))?;
    let candidates = keys
        .iter()
        .filter(|key| key.get("kid").and_then(Value::as_str) == Some("HCLAkPub"))
        .collect::<Vec<_>>();
    let value = match candidates.as_slice() {
        [value] => *value,
        [] => return reject("MAA JWT runtime has no HCLAkPub"),
        _ => return reject("MAA JWT runtime has duplicate HCLAkPub keys"),
    };
    let hcl = serde_json::from_value::<Jwk>(value.clone())
        .map_err(|error| VerifyError::Rejected(format!("MAA HCLAkPub is invalid: {error}")))?;
    if hcl.key_ops != ["sign"] {
        return reject("MAA HCLAkPub is not an exact signing key");
    }
    let hcl = rsa_key(&hcl)?;
    if hcl.n() != workload_ak.n() || hcl.e() != workload_ak.e() {
        return reject("vTPM quote AK does not match the MAA-signed HCLAkPub");
    }
    Ok(())
}

fn claim_object<'a>(claims: &'a Value, name: &str) -> Result<&'a serde_json::Map<String, Value>> {
    claims
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| VerifyError::Rejected(format!("MAA JWT claim {name} is not an object")))
}

fn claim_string<'a>(claims: &'a Value, name: &str) -> Result<&'a str> {
    claims
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| VerifyError::Rejected(format!("MAA JWT claim {name} is not a string")))
}

fn claim_bool(claims: &Value, name: &str) -> Result<bool> {
    claims
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| VerifyError::Rejected(format!("MAA JWT claim {name} is not boolean")))
}

fn claim_u64(claims: &Value, name: &str) -> Result<u64> {
    claims.get(name).and_then(Value::as_u64).ok_or_else(|| {
        VerifyError::Rejected(format!("MAA JWT claim {name} is not an unsigned integer"))
    })
}

fn claim_hex(claims: &Value, name: &str, bytes: usize) -> Result<Vec<u8>> {
    let value = claim_string(claims, name)?;
    if value.len() != bytes * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return reject(format!(
            "MAA JWT claim {name} is not a {bytes}-byte hex value"
        ));
    }
    hex::decode(value)
        .map_err(|_| VerifyError::Rejected(format!("MAA JWT claim {name} is invalid hex")))
}

fn is_azure_platform(platform: &str) -> bool {
    let normalized = platform.trim().to_ascii_lowercase();
    normalized == "azure-ncc" || normalized.starts_with("azure-ncc-")
}
