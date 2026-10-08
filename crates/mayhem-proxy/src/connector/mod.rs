//! Proxy upstream boundary. The parent remains responsible for validated public
//! requests, durable dispatch intent, capacity/offer acceptance and settlement.
//! This module never chooses a price, retries a POST or signs a receipt.

pub mod config;
pub mod failure;
pub mod framing;
pub mod http;

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("invalid proxy connection configuration: {0}")]
    Invalid(&'static str),
    #[error("proxy connection file could not be read")]
    File,
    #[error("proxy connection file must be a private, owner-controlled regular file")]
    FilePermissions,
    #[error("proxy credential is missing or invalid")]
    Credential,
    #[error("private proxy files require a platform-specific permission verifier unavailable in this build")]
    UnsupportedFilePermissions,
    #[error("proxy HTTP client could not be initialized")]
    Client,
}

pub type SetupResult<T> = Result<T, SetupError>;

fn require(ok: bool, message: &'static str) -> SetupResult<()> {
    if ok {
        Ok(())
    } else {
        Err(SetupError::Invalid(message))
    }
}
