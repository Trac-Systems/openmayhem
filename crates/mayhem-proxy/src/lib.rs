#![forbid(unsafe_code)]

//! Proxy-only control and connector components. A catalog record is neither runtime
//! capacity evidence nor authorization to accept or settle an inference request.

pub mod attempts;
pub mod capacity;
pub mod catalog;
pub mod cli;
pub mod connector;
pub mod discovery;
pub mod endpoint;
pub mod execution;
pub mod financial;
pub mod matching;
pub mod metering;
pub mod receipts;
pub mod semantics;
pub mod supervisor;
pub mod worker;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid proxy discovery data: {0}")]
    Invalid(String),
    #[error("proxy catalog identity does not match this network/contract")]
    Identity,
    #[error("proxy catalog refresh changed; discard this stale response")]
    StaleRefresh,
    #[error("proxy catalog refresh is already in progress")]
    RefreshBusy,
    #[error("proxy catalog database: {0}")]
    Database(#[from] redb::Error),
    #[error("proxy catalog filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid proxy catalog JSON")]
    Json(#[from] serde_json::Error),
    #[error("proxy discovery transport failed")]
    Transport(#[source] reqwest::Error),
    #[error("proxy discovery HTTP status {status}: {code}")]
    Http { status: u16, code: String },
    #[error("proxy catalog blocking task failed")]
    Task,
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

pub(crate) fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(message))
    }
}

pub(crate) fn db<T, E: Into<redb::Error>>(result: std::result::Result<T, E>) -> Result<T> {
    result.map_err(|error| Error::Database(error.into()))
}
