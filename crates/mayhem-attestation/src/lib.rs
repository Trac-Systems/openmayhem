#![forbid(unsafe_code)]

//! Fail-closed immutable attestation policy, exact enclave bindings,
//! collateral, readiness, and TPM credential activation primitives.
//!
//! This crate does not authenticate ledger authors or fetch collateral. Callers
//! must pass policy records already authenticated as canonical admin records,
//! and may fetch only the policy-derived sources exposed here.

mod binding;
mod collateral;
mod policy;
mod readiness;
mod tpm;
mod tpm_quote;
mod x509;

pub use collateral::{
    collateral_cache_relative_path, CollateralError, CollateralInventory, ValidatedCollateral,
    MAX_COLLATERAL_BYTES,
};
pub use mayhem_proto::AdminEnclaveAttestationBinding;
pub use policy::{
    AttestationPolicyChain, PolicyError, ValidatedAttestationPolicy, MAX_POLICY_BYTES,
};
pub use readiness::{
    AttestationReadiness, QuoteKindReadiness, QuoteKindReadinessStatus, ReadinessError,
    VerifierCapabilities,
};
pub use tpm::{
    issue_tpm_activate_credential_challenge, issue_tpm_activate_credential_challenge_with_rng,
    ActivatedTpmIdentity, PendingTpmActivation, TpmActivationError, DEFAULT_TPM_CHALLENGE_TTL_SECS,
    MAX_TPM_CHALLENGE_TTL_SECS,
};
pub use tpm_quote::{
    verify_tpm_hardware_quote, TpmQuoteError, TpmVerificationMaterials, VerifiedTpmQuote,
    MAX_TPM_QUOTE_EVIDENCE_BYTES,
};
pub use x509::EkCertificateError;

pub const CRATE_NAME: &str = "mayhem-attestation";
pub use binding::{EvidenceBinding, EvidenceBindingError};
