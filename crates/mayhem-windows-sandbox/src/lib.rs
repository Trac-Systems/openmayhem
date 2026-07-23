use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use thiserror::Error;

pub const CRATE_NAME: &str = "mayhem-windows-sandbox";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsSandboxConfig {
    pub read_only_dirs: Vec<PathBuf>,
    pub materialized_read_only_dirs: Vec<PathBuf>,
    pub writable_dirs: Vec<PathBuf>,
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsSandboxRunReport {
    pub status_code: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowsSandboxStderr {
    #[default]
    Inherit,
    Piped,
    Null,
}

#[derive(Clone, Debug)]
pub struct WindowsSandboxCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, Option<OsString>>,
    pub env_clear: bool,
    pub current_dir: Option<PathBuf>,
    pub stderr: WindowsSandboxStderr,
    pub executable_read_only_dirs: Vec<PathBuf>,
    pub allow_code_generation: bool,
}

#[derive(Debug, Error)]
pub enum WindowsSandboxError {
    #[error("Windows sandbox is not supported on this platform")]
    UnsupportedPlatform,
    #[error("Windows sandbox command cannot be empty")]
    EmptyCommand,
    #[error("invalid Windows sandbox configuration: {0}")]
    InvalidConfig(String),
    #[error("Windows sandbox error: {0}")]
    Windows(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, WindowsSandboxError>;

#[cfg(windows)]
mod platform;

#[cfg(windows)]
pub use platform::{run_appcontainer, spawn_appcontainer, WindowsSandboxChild};

#[cfg(not(windows))]
pub struct WindowsSandboxChild {
    pub stdin: Option<std::fs::File>,
    pub stdout: Option<std::fs::File>,
    pub stderr: Option<std::fs::File>,
}

#[cfg(not(windows))]
pub fn run_appcontainer(
    _config: &WindowsSandboxConfig,
    _command: &[String],
) -> Result<WindowsSandboxRunReport> {
    Err(WindowsSandboxError::UnsupportedPlatform)
}

#[cfg(not(windows))]
pub fn spawn_appcontainer(
    _config: &WindowsSandboxConfig,
    _command: &WindowsSandboxCommand,
) -> Result<WindowsSandboxChild> {
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
