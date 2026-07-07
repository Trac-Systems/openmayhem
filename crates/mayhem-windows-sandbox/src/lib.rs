use std::path::PathBuf;

use thiserror::Error;

pub const CRATE_NAME: &str = "mayhem-windows-sandbox";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsSandboxConfig {
    pub sealed_store_dir: PathBuf,
    pub ipc_socket_path: PathBuf,
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsSandboxRunReport {
    pub status_code: u32,
}

#[derive(Debug, Error)]
pub enum WindowsSandboxError {
    #[error("Windows sandbox is not supported on this platform")]
    UnsupportedPlatform,
    #[error("Windows sandbox command cannot be empty")]
    EmptyCommand,
    #[error("Windows sandbox error: {0}")]
    Windows(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, WindowsSandboxError>;

#[cfg(windows)]
mod platform;

#[cfg(windows)]
pub use platform::run_appcontainer;

#[cfg(not(windows))]
pub fn run_appcontainer(
    _config: &WindowsSandboxConfig,
    _command: &[String],
) -> Result<WindowsSandboxRunReport> {
    Err(WindowsSandboxError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-windows-sandbox");
    }
}
