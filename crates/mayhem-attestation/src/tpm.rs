use std::fmt;

use aes::{
    cipher::{AsyncStreamCipher, KeyIvInit},
    Aes128,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cfb_mode::Encryptor;
use hmac::{Hmac, Mac};
use mayhem_proto::{
    TpmActivateCredentialChallenge, TpmActivateCredentialHello, TpmActivateCredentialResponse,
    TpmEkProfile, TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
};
use p256::{
    ecdh::EphemeralSecret,
    elliptic_curve::sec1::ToEncodedPoint,
    pkcs8::{DecodePublicKey, EncodePublicKey},
    PublicKey as P256PublicKey,
};
use rand_core::{CryptoRng, OsRng, RngCore};
use rsa::{traits::PublicKeyParts, Oaep, RsaPublicKey};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;
type Aes128CfbEncryptor = Encryptor<Aes128>;

const SHA256_TPM_ALG_ID: [u8; 2] = [0x00, 0x0b];
const TPM_CREDENTIAL_BYTES: usize = 32;
const TPM_CHALLENGE_ID_BYTES: usize = 32;
const TPM_AK_SHA256_NAME_BYTES: usize = 2 + 32;
const AES_128_KEY_BITS: usize = 128;
const SHA256_BITS: usize = 256;

pub const DEFAULT_TPM_CHALLENGE_TTL_SECS: u64 = 30;
pub const MAX_TPM_CHALLENGE_TTL_SECS: u64 = 300;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TpmActivationError {
    #[error("TPM activation schema {actual} is unsupported; expected {expected}")]
    UnsupportedSchema { expected: u32, actual: u32 },
    #[error("TPM quote binding must be a lowercase 32-byte digest")]
    InvalidQuoteBinding,
    #[error("TPM EK public key is not valid base64")]
    InvalidEkPublicEncoding,
    #[error("TPM EK public key does not match declared profile: {0}")]
    InvalidEkPublic(String),
    #[error("TPM RSA EK must be between 2048 and 4096 bits")]
    InvalidRsaEkSize,
    #[error("TPM AK name is not valid base64")]
    InvalidAkNameEncoding,
    #[error("TPM AK name must be a SHA-256 object name")]
    InvalidAkName,
    #[error(
        "TPM activation challenge TTL must be between 1 and {MAX_TPM_CHALLENGE_TTL_SECS} seconds"
    )]
    InvalidTtl,
    #[error("TPM activation challenge expiration overflow")]
    ExpirationOverflow,
    #[error("TPM MakeCredential failed: {0}")]
    MakeCredential(String),
    #[error("TPM activation response challenge id does not match")]
    ChallengeIdMismatch,
    #[error("TPM activation response AK name does not match")]
    AkNameMismatch,
    #[error("TPM activation response quote binding does not match")]
    QuoteBindingMismatch,
    #[error("TPM activated credential is not valid base64")]
    InvalidActivatedCredentialEncoding,
    #[error("TPM activated credential has the wrong length")]
    InvalidActivatedCredentialLength,
    #[error("TPM activation challenge expired at {expires_at_unix}")]
    Expired { expires_at_unix: u64 },
    #[error("TPM activated credential does not match the buyer challenge")]
    ActivatedCredentialMismatch,
    #[error("cryptographically verified TPM quote used a different AK name")]
    VerifiedQuoteAkMismatch,
}

pub struct PendingTpmActivation {
    challenge: TpmActivateCredentialChallenge,
    credential: Zeroizing<[u8; TPM_CREDENTIAL_BYTES]>,
}

impl fmt::Debug for PendingTpmActivation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingTpmActivation")
            .field("challenge", &self.challenge)
            .field("credential", &"<redacted>")
            .finish()
    }
}

impl PendingTpmActivation {
    pub fn challenge(&self) -> &TpmActivateCredentialChallenge {
        &self.challenge
    }

    pub fn complete(
        self,
        response: &TpmActivateCredentialResponse,
        now_unix: u64,
    ) -> Result<ActivatedTpmIdentity, TpmActivationError> {
        validate_schema(response.schema_version)?;
        if now_unix >= self.challenge.expires_at_unix {
            return Err(TpmActivationError::Expired {
                expires_at_unix: self.challenge.expires_at_unix,
            });
        }
        if response.challenge_id != self.challenge.challenge_id {
            return Err(TpmActivationError::ChallengeIdMismatch);
        }
        if response.ak_name_b64 != self.challenge.ak_name_b64 {
            return Err(TpmActivationError::AkNameMismatch);
        }
        if response.quote_binding != self.challenge.quote_binding {
            return Err(TpmActivationError::QuoteBindingMismatch);
        }
        let activated = Zeroizing::new(
            BASE64
                .decode(&response.activated_secret_b64)
                .map_err(|_| TpmActivationError::InvalidActivatedCredentialEncoding)?,
        );
        if activated.len() != TPM_CREDENTIAL_BYTES {
            return Err(TpmActivationError::InvalidActivatedCredentialLength);
        }
        if !bool::from(activated.as_slice().ct_eq(self.credential.as_slice())) {
            return Err(TpmActivationError::ActivatedCredentialMismatch);
        }
        Ok(ActivatedTpmIdentity {
            challenge_id: self.challenge.challenge_id,
            ek_public_sha256: self.challenge.ek_public_sha256,
            ak_name_b64: self.challenge.ak_name_b64,
            quote_binding: self.challenge.quote_binding,
            activated_at_unix: now_unix,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivatedTpmIdentity {
    challenge_id: String,
    ek_public_sha256: String,
    ak_name_b64: String,
    quote_binding: String,
    activated_at_unix: u64,
}

impl ActivatedTpmIdentity {
    #[cfg(test)]
    pub(crate) fn test_identity(
        ek_public_sha256: String,
        ak_name_b64: String,
        quote_binding: String,
    ) -> Self {
        Self {
            challenge_id: "test-activation".to_owned(),
            ek_public_sha256,
            ak_name_b64,
            quote_binding,
            activated_at_unix: 1,
        }
    }

    pub fn challenge_id(&self) -> &str {
        &self.challenge_id
    }

    pub fn ek_public_sha256(&self) -> &str {
        &self.ek_public_sha256
    }

    pub fn ak_name_b64(&self) -> &str {
        &self.ak_name_b64
    }

    pub fn quote_binding(&self) -> &str {
        &self.quote_binding
    }

    pub fn activated_at_unix(&self) -> u64 {
        self.activated_at_unix
    }

    /// Bind this activation result to a quote whose current evidence binding,
    /// signature, and PCR digest have already been cryptographically verified.
    pub fn bind_verified_quote(
        &self,
        verified_ak_name_b64: &str,
    ) -> Result<(), TpmActivationError> {
        if verified_ak_name_b64 != self.ak_name_b64 {
            return Err(TpmActivationError::VerifiedQuoteAkMismatch);
        }
        Ok(())
    }
}

pub fn issue_tpm_activate_credential_challenge(
    hello: &TpmActivateCredentialHello,
    now_unix: u64,
) -> Result<(TpmActivateCredentialChallenge, PendingTpmActivation), TpmActivationError> {
    issue_tpm_activate_credential_challenge_with_rng(
        hello,
        now_unix,
        DEFAULT_TPM_CHALLENGE_TTL_SECS,
        &mut OsRng,
    )
}

/// Issue a TPM2 MakeCredential challenge bound to the supplied EK public key
/// and AK Name. Before treating the completed result as Tier-2 identity, the
/// caller must also match `ek_public_sha256` to a policy-trusted EK certificate
/// and cryptographically verify the nonce-bound AK quote.
pub fn issue_tpm_activate_credential_challenge_with_rng<R>(
    hello: &TpmActivateCredentialHello,
    now_unix: u64,
    ttl_secs: u64,
    rng: &mut R,
) -> Result<(TpmActivateCredentialChallenge, PendingTpmActivation), TpmActivationError>
where
    R: CryptoRng + RngCore,
{
    validate_schema(hello.schema_version)?;
    validate_quote_binding(&hello.quote_binding)?;
    if ttl_secs == 0 || ttl_secs > MAX_TPM_CHALLENGE_TTL_SECS {
        return Err(TpmActivationError::InvalidTtl);
    }
    let expires_at_unix = now_unix
        .checked_add(ttl_secs)
        .ok_or(TpmActivationError::ExpirationOverflow)?;
    let ek_der = BASE64
        .decode(&hello.ek_public_spki_der_b64)
        .map_err(|_| TpmActivationError::InvalidEkPublicEncoding)?;
    let ak_name = BASE64
        .decode(&hello.ak_name_b64)
        .map_err(|_| TpmActivationError::InvalidAkNameEncoding)?;
    validate_ak_name(&ak_name)?;

    let mut credential = Zeroizing::new([0u8; TPM_CREDENTIAL_BYTES]);
    rng.fill_bytes(credential.as_mut());
    let mut challenge_id = [0u8; TPM_CHALLENGE_ID_BYTES];
    rng.fill_bytes(&mut challenge_id);

    let made = make_credential(
        hello.ek_profile,
        &ek_der,
        &ak_name,
        credential.as_slice(),
        rng,
    )?;
    let ek_public_sha256 = hex::encode(Sha256::digest(&made.canonical_ek_spki_der));
    let challenge = TpmActivateCredentialChallenge {
        schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
        challenge_id: hex::encode(challenge_id),
        ek_public_sha256,
        ak_name_b64: hello.ak_name_b64.clone(),
        quote_binding: hello.quote_binding.clone(),
        credential_blob_b64: BASE64.encode(made.credential_blob),
        encrypted_secret_b64: BASE64.encode(made.encrypted_secret),
        issued_at_unix: now_unix,
        expires_at_unix,
    };
    let pending = PendingTpmActivation {
        challenge: challenge.clone(),
        credential,
    };
    Ok((challenge, pending))
}

struct MadeCredential {
    canonical_ek_spki_der: Vec<u8>,
    credential_blob: Vec<u8>,
    encrypted_secret: Vec<u8>,
}

fn make_credential<R>(
    profile: TpmEkProfile,
    ek_spki_der: &[u8],
    ak_name: &[u8],
    credential: &[u8],
    rng: &mut R,
) -> Result<MadeCredential, TpmActivationError>
where
    R: CryptoRng + RngCore,
{
    match profile {
        TpmEkProfile::RsaSha256Aes128Cfb => {
            make_rsa_credential(ek_spki_der, ak_name, credential, rng)
        }
        TpmEkProfile::EccP256Sha256Aes128Cfb => {
            make_ecc_credential(ek_spki_der, ak_name, credential, rng)
        }
    }
}

fn make_rsa_credential<R>(
    ek_spki_der: &[u8],
    ak_name: &[u8],
    credential: &[u8],
    rng: &mut R,
) -> Result<MadeCredential, TpmActivationError>
where
    R: CryptoRng + RngCore,
{
    let ek = RsaPublicKey::from_public_key_der(ek_spki_der)
        .map_err(|err| TpmActivationError::InvalidEkPublic(err.to_string()))?;
    if !(2048..=4096).contains(&ek.n().bits()) {
        return Err(TpmActivationError::InvalidRsaEkSize);
    }
    let canonical = ek
        .to_public_key_der()
        .map_err(|err| TpmActivationError::InvalidEkPublic(err.to_string()))?
        .as_bytes()
        .to_vec();
    let mut seed = Zeroizing::new([0u8; TPM_CREDENTIAL_BYTES]);
    rng.fill_bytes(seed.as_mut());
    let encrypted_seed = ek
        .encrypt(
            rng,
            Oaep::new_with_label::<Sha256, _>("IDENTITY\0"),
            seed.as_slice(),
        )
        .map_err(|err| TpmActivationError::MakeCredential(err.to_string()))?;
    Ok(MadeCredential {
        canonical_ek_spki_der: canonical,
        credential_blob: credential_blob(seed.as_slice(), ak_name, credential)?,
        encrypted_secret: tpm2b(&encrypted_seed)?,
    })
}

fn make_ecc_credential<R>(
    ek_spki_der: &[u8],
    ak_name: &[u8],
    credential: &[u8],
    rng: &mut R,
) -> Result<MadeCredential, TpmActivationError>
where
    R: CryptoRng + RngCore,
{
    let ek = P256PublicKey::from_public_key_der(ek_spki_der)
        .map_err(|err| TpmActivationError::InvalidEkPublic(err.to_string()))?;
    let canonical = ek
        .to_public_key_der()
        .map_err(|err| TpmActivationError::InvalidEkPublic(err.to_string()))?
        .as_bytes()
        .to_vec();
    let ephemeral = EphemeralSecret::random(rng);
    let ephemeral_public = P256PublicKey::from(&ephemeral);
    let shared = ephemeral.diffie_hellman(&ek);
    let ephemeral_point = ephemeral_public.to_encoded_point(false);
    let ek_point = ek.to_encoded_point(false);
    let ephemeral_x = ephemeral_point
        .x()
        .ok_or_else(|| TpmActivationError::MakeCredential("ephemeral X is missing".to_owned()))?;
    let ephemeral_y = ephemeral_point
        .y()
        .ok_or_else(|| TpmActivationError::MakeCredential("ephemeral Y is missing".to_owned()))?;
    let ek_x = ek_point
        .x()
        .ok_or_else(|| TpmActivationError::MakeCredential("EK X is missing".to_owned()))?;
    let seed = Zeroizing::new(kdfe_sha256(
        shared.raw_secret_bytes(),
        b"IDENTITY",
        ephemeral_x,
        ek_x,
        SHA256_BITS,
    ));
    let mut encrypted_secret_body = tpm2b(ephemeral_x)?;
    encrypted_secret_body.extend(tpm2b(ephemeral_y)?);
    Ok(MadeCredential {
        canonical_ek_spki_der: canonical,
        credential_blob: credential_blob(&seed, ak_name, credential)?,
        encrypted_secret: tpm2b(&encrypted_secret_body)?,
    })
}

fn credential_blob(
    seed: &[u8],
    ak_name: &[u8],
    credential: &[u8],
) -> Result<Vec<u8>, TpmActivationError> {
    let symmetric_key = Zeroizing::new(kdfa_sha256(
        seed,
        b"STORAGE",
        ak_name,
        &[],
        AES_128_KEY_BITS,
    ));
    let mut enc_identity = tpm2b(credential)?;
    Aes128CfbEncryptor::new_from_slices(&symmetric_key, &[0u8; 16])
        .map_err(|err| TpmActivationError::MakeCredential(err.to_string()))?
        .encrypt(&mut enc_identity);

    let integrity_key = Zeroizing::new(kdfa_sha256(seed, b"INTEGRITY", &[], &[], SHA256_BITS));
    let mut hmac = HmacSha256::new_from_slice(&integrity_key)
        .map_err(|err| TpmActivationError::MakeCredential(err.to_string()))?;
    hmac.update(&enc_identity);
    hmac.update(ak_name);
    let integrity = hmac.finalize().into_bytes();

    let mut id_object = tpm2b(&integrity)?;
    id_object.extend(tpm2b(&enc_identity)?);
    tpm2b(&id_object)
}

fn kdfa_sha256(
    key: &[u8],
    label: &[u8],
    context_u: &[u8],
    context_v: &[u8],
    bits: usize,
) -> Vec<u8> {
    let bytes = bits.div_ceil(8);
    let mut output = Vec::with_capacity(bytes);
    let mut counter = 1u32;
    while output.len() < bytes {
        let mut hmac = HmacSha256::new_from_slice(key).expect("HMAC accepts arbitrary key length");
        hmac.update(&counter.to_be_bytes());
        hmac.update(label);
        hmac.update(&[0]);
        hmac.update(context_u);
        hmac.update(context_v);
        hmac.update(&(bits as u32).to_be_bytes());
        output.extend_from_slice(&hmac.finalize().into_bytes());
        counter = counter.saturating_add(1);
    }
    output.truncate(bytes);
    output
}

fn kdfe_sha256(
    z: &[u8],
    label: &[u8],
    party_u_info: &[u8],
    party_v_info: &[u8],
    bits: usize,
) -> Vec<u8> {
    let bytes = bits.div_ceil(8);
    let mut output = Vec::with_capacity(bytes);
    let mut counter = 1u32;
    while output.len() < bytes {
        let mut hash = Sha256::new();
        hash.update(counter.to_be_bytes());
        hash.update(z);
        hash.update(label);
        hash.update([0]);
        hash.update(party_u_info);
        hash.update(party_v_info);
        output.extend_from_slice(&hash.finalize());
        counter = counter.saturating_add(1);
    }
    output.truncate(bytes);
    output
}

fn tpm2b(bytes: &[u8]) -> Result<Vec<u8>, TpmActivationError> {
    let length = u16::try_from(bytes.len()).map_err(|_| {
        TpmActivationError::MakeCredential("TPM2B value exceeds 65535 bytes".to_owned())
    })?;
    let mut encoded = Vec::with_capacity(2 + bytes.len());
    encoded.extend_from_slice(&length.to_be_bytes());
    encoded.extend_from_slice(bytes);
    Ok(encoded)
}

fn validate_schema(actual: u32) -> Result<(), TpmActivationError> {
    if actual == TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(TpmActivationError::UnsupportedSchema {
            expected: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
            actual,
        })
    }
}

fn validate_quote_binding(binding: &str) -> Result<(), TpmActivationError> {
    if binding.len() == 64
        && binding
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(TpmActivationError::InvalidQuoteBinding)
    }
}

fn validate_ak_name(name: &[u8]) -> Result<(), TpmActivationError> {
    if name.len() == TPM_AK_SHA256_NAME_BYTES && name[..2] == SHA256_TPM_ALG_ID {
        Ok(())
    } else {
        Err(TpmActivationError::InvalidAkName)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::AsyncStreamCipher;
    use cfb_mode::Decryptor;
    use p256::{ecdh::diffie_hellman, SecretKey as P256SecretKey};
    use rand_chacha::ChaCha20Rng;
    use rand_core::SeedableRng;
    use rsa::RsaPrivateKey;

    type Aes128CfbDecryptor = Decryptor<Aes128>;

    #[test]
    fn rsa_activate_credential_rejects_separate_ek_and_ak_proofs() {
        let mut key_rng = ChaCha20Rng::from_seed([7; 32]);
        let ek = RsaPrivateKey::new(&mut key_rng, 2048).unwrap();
        let other_ek = RsaPrivateKey::new(&mut key_rng, 2048).unwrap();
        let ek_der = RsaPublicKey::from(&ek).to_public_key_der().unwrap();
        let ak_name = make_ak_name(b"ak-on-certified-tpm");
        let other_ak_name = make_ak_name(b"software-ak");
        let hello = hello(
            TpmEkProfile::RsaSha256Aes128Cfb,
            ek_der.as_bytes(),
            &ak_name,
        );
        let mut challenge_rng = ChaCha20Rng::from_seed([9; 32]);
        let (challenge, pending) =
            issue_tpm_activate_credential_challenge_with_rng(&hello, 1_000, 30, &mut challenge_rng)
                .unwrap();

        assert!(activate_rsa(&other_ek, &ak_name, &challenge).is_none());
        assert!(activate_rsa(&ek, &other_ak_name, &challenge).is_none());
        let activated = activate_rsa(&ek, &ak_name, &challenge).unwrap();
        let response = response(&challenge, &activated);
        let identity = pending.complete(&response, 1_001).unwrap();
        assert_eq!(identity.bind_verified_quote(&hello.ak_name_b64), Ok(()));
        assert_eq!(
            identity
                .bind_verified_quote(&BASE64.encode(other_ak_name))
                .unwrap_err(),
            TpmActivationError::VerifiedQuoteAkMismatch
        );
    }

    #[test]
    fn activation_identity_reuses_ak_across_fresh_quote_bindings() {
        let activation_binding = "aa".repeat(32);
        let ak_name_b64 = BASE64.encode(make_ak_name(b"stable-ak"));
        let identity = ActivatedTpmIdentity::test_identity(
            "11".repeat(32),
            ak_name_b64.clone(),
            activation_binding.clone(),
        );

        assert_eq!(identity.quote_binding(), activation_binding);
        for current_session_binding in ["bb".repeat(32), "cc".repeat(32)] {
            assert_ne!(identity.quote_binding(), current_session_binding);
            assert_eq!(identity.bind_verified_quote(&ak_name_b64), Ok(()));
        }
        assert_eq!(
            identity
                .bind_verified_quote(&BASE64.encode(make_ak_name(b"different-ak")))
                .unwrap_err(),
            TpmActivationError::VerifiedQuoteAkMismatch
        );
    }

    #[test]
    fn ecc_p256_activate_credential_round_trip_binds_ak_name() {
        let mut key_rng = ChaCha20Rng::from_seed([11; 32]);
        let ek = P256SecretKey::random(&mut key_rng);
        let ek_der = ek.public_key().to_public_key_der().unwrap();
        let ak_name = make_ak_name(b"ecc-ak");
        let hello = hello(
            TpmEkProfile::EccP256Sha256Aes128Cfb,
            ek_der.as_bytes(),
            &ak_name,
        );
        let mut challenge_rng = ChaCha20Rng::from_seed([12; 32]);
        let (challenge, pending) =
            issue_tpm_activate_credential_challenge_with_rng(&hello, 2_000, 30, &mut challenge_rng)
                .unwrap();

        assert!(activate_ecc(&ek, &make_ak_name(b"other-ak"), &challenge).is_none());
        let activated = activate_ecc(&ek, &ak_name, &challenge).unwrap();
        let identity = pending
            .complete(&response(&challenge, &activated), 2_001)
            .unwrap();
        assert_eq!(identity.ak_name_b64(), BASE64.encode(ak_name));
    }

    #[test]
    fn activation_state_rejects_guess_expiry_and_transcript_changes() {
        let mut key_rng = ChaCha20Rng::from_seed([15; 32]);
        let ek = P256SecretKey::random(&mut key_rng);
        let ek_der = ek.public_key().to_public_key_der().unwrap();
        let ak_name = make_ak_name(b"state-ak");
        let hello = hello(
            TpmEkProfile::EccP256Sha256Aes128Cfb,
            ek_der.as_bytes(),
            &ak_name,
        );

        let mut first_rng = ChaCha20Rng::from_seed([16; 32]);
        let (challenge, pending) =
            issue_tpm_activate_credential_challenge_with_rng(&hello, 3_000, 30, &mut first_rng)
                .unwrap();
        let guessed = response(&challenge, &[0; TPM_CREDENTIAL_BYTES]);
        assert_eq!(
            pending.complete(&guessed, 3_001).unwrap_err(),
            TpmActivationError::ActivatedCredentialMismatch
        );

        let mut second_rng = ChaCha20Rng::from_seed([17; 32]);
        let (challenge, pending) =
            issue_tpm_activate_credential_challenge_with_rng(&hello, 3_000, 30, &mut second_rng)
                .unwrap();
        let activated = activate_ecc(&ek, &ak_name, &challenge).unwrap();
        assert_eq!(
            pending
                .complete(&response(&challenge, &activated), challenge.expires_at_unix)
                .unwrap_err(),
            TpmActivationError::Expired {
                expires_at_unix: challenge.expires_at_unix
            }
        );

        let mut third_rng = ChaCha20Rng::from_seed([18; 32]);
        let (challenge, pending) =
            issue_tpm_activate_credential_challenge_with_rng(&hello, 3_000, 30, &mut third_rng)
                .unwrap();
        let activated = activate_ecc(&ek, &ak_name, &challenge).unwrap();
        let mut changed = response(&challenge, &activated);
        changed.quote_binding = "bb".repeat(32);
        assert_eq!(
            pending.complete(&changed, 3_001).unwrap_err(),
            TpmActivationError::QuoteBindingMismatch
        );
    }

    fn hello(profile: TpmEkProfile, ek_der: &[u8], ak_name: &[u8]) -> TpmActivateCredentialHello {
        TpmActivateCredentialHello {
            schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
            ek_profile: profile,
            ek_public_spki_der_b64: BASE64.encode(ek_der),
            ak_name_b64: BASE64.encode(ak_name),
            quote_binding: "aa".repeat(32),
        }
    }

    fn response(
        challenge: &TpmActivateCredentialChallenge,
        activated: &[u8],
    ) -> TpmActivateCredentialResponse {
        TpmActivateCredentialResponse {
            schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
            challenge_id: challenge.challenge_id.clone(),
            ak_name_b64: challenge.ak_name_b64.clone(),
            quote_binding: challenge.quote_binding.clone(),
            activated_secret_b64: BASE64.encode(activated),
        }
    }

    fn make_ak_name(label: &[u8]) -> Vec<u8> {
        let mut name = SHA256_TPM_ALG_ID.to_vec();
        name.extend_from_slice(&Sha256::digest(label));
        name
    }

    fn activate_rsa(
        ek: &RsaPrivateKey,
        ak_name: &[u8],
        challenge: &TpmActivateCredentialChallenge,
    ) -> Option<Vec<u8>> {
        let encrypted_secret_blob = BASE64.decode(&challenge.encrypted_secret_b64).ok()?;
        let encrypted_secret = unwrap_tpm2b(&encrypted_secret_blob)?;
        let seed = ek
            .decrypt(
                Oaep::new_with_label::<Sha256, _>("IDENTITY\0"),
                encrypted_secret,
            )
            .ok()?;
        recover_credential(
            &seed,
            ak_name,
            &BASE64.decode(&challenge.credential_blob_b64).ok()?,
        )
    }

    fn activate_ecc(
        ek: &P256SecretKey,
        ak_name: &[u8],
        challenge: &TpmActivateCredentialChallenge,
    ) -> Option<Vec<u8>> {
        let encrypted_secret = BASE64.decode(&challenge.encrypted_secret_b64).ok()?;
        let point_body = unwrap_tpm2b(&encrypted_secret)?;
        let mut offset = 0;
        let x = take_tpm2b(point_body, &mut offset)?;
        let y = take_tpm2b(point_body, &mut offset)?;
        if offset != point_body.len() {
            return None;
        }
        let mut sec1 = Vec::with_capacity(1 + x.len() + y.len());
        sec1.push(0x04);
        sec1.extend_from_slice(x);
        sec1.extend_from_slice(y);
        let ephemeral = P256PublicKey::from_sec1_bytes(&sec1).ok()?;
        let shared = diffie_hellman(ek.to_nonzero_scalar(), ephemeral.as_affine());
        let ek_point = ek.public_key().to_encoded_point(false);
        let ek_x = ek_point.x()?;
        let seed = kdfe_sha256(shared.raw_secret_bytes(), b"IDENTITY", x, ek_x, SHA256_BITS);
        recover_credential(
            &seed,
            ak_name,
            &BASE64.decode(&challenge.credential_blob_b64).ok()?,
        )
    }

    fn recover_credential(seed: &[u8], ak_name: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
        let id_object = unwrap_tpm2b(blob)?;
        let mut offset = 0;
        let integrity = take_tpm2b(id_object, &mut offset)?;
        let enc_identity = take_tpm2b(id_object, &mut offset)?;
        if offset != id_object.len() {
            return None;
        }
        let integrity_key = kdfa_sha256(seed, b"INTEGRITY", &[], &[], SHA256_BITS);
        let mut hmac = HmacSha256::new_from_slice(&integrity_key).ok()?;
        hmac.update(enc_identity);
        hmac.update(ak_name);
        hmac.verify_slice(integrity).ok()?;

        let symmetric_key = kdfa_sha256(seed, b"STORAGE", ak_name, &[], AES_128_KEY_BITS);
        let mut credential = enc_identity.to_vec();
        Aes128CfbDecryptor::new_from_slices(&symmetric_key, &[0; 16])
            .ok()?
            .decrypt(&mut credential);
        unwrap_tpm2b(&credential).map(Vec::from)
    }

    fn unwrap_tpm2b(encoded: &[u8]) -> Option<&[u8]> {
        let mut offset = 0;
        let value = take_tpm2b(encoded, &mut offset)?;
        (offset == encoded.len()).then_some(value)
    }

    fn take_tpm2b<'a>(encoded: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
        let length_bytes = encoded.get(*offset..offset.checked_add(2)?)?;
        let length = u16::from_be_bytes([length_bytes[0], length_bytes[1]]) as usize;
        *offset += 2;
        let end = offset.checked_add(length)?;
        let value = encoded.get(*offset..end)?;
        *offset = end;
        Some(value)
    }
}
