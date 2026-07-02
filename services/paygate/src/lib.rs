#![forbid(unsafe_code)]

use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{extract::State, routing::get, Json, Router};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::TcpListener;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const SERVICE_NAME: &str = "mayhem-paygate";
pub const SERVICE_VERSION: u32 = 1;
pub const CREDIT_DENOM: &str = "mu_usd";
pub const DEFAULT_BIND: &str = "127.0.0.1:11436";
pub const DEFAULT_CONTRACT_RPC_URL: &str = "http://127.0.0.1:49223/v1";

pub type Result<T> = std::result::Result<T, PaygateError>;

#[derive(Debug, Error)]
pub enum PaygateError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("hex error: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("crypto error: {0}")]
    Crypto(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaygateConfig {
    pub bind: SocketAddr,
    pub contract_rpc_url: String,
    pub oracle_key_path: PathBuf,
    pub rails: RailConfig,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RailConfig {
    pub stripe: RailSettings,
    pub coinbase: RailSettings,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RailSettings {
    pub enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    server: ServerConfigFile,
    #[serde(default)]
    contract: ContractConfigFile,
    #[serde(default)]
    oracle: OracleConfigFile,
    #[serde(default)]
    stripe: RailConfigFile,
    #[serde(default)]
    coinbase: RailConfigFile,
}

#[derive(Debug, Default, Deserialize)]
struct ServerConfigFile {
    bind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ContractConfigFile {
    rpc_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OracleConfigFile {
    key_path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct RailConfigFile {
    enabled: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct OracleKeypair {
    signing_key: SigningKey,
}

#[derive(Clone, Debug)]
pub struct PaygateState {
    config: Arc<PaygateConfig>,
    oracle_public_key: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    version: u32,
    denom: &'static str,
    oracle_pubkey: String,
    contract: HealthContract,
    rails: HealthRails,
    controls: HealthControls,
}

#[derive(Debug, Serialize)]
struct HealthContract {
    rpc_configured: bool,
}

#[derive(Debug, Serialize)]
struct HealthRails {
    stripe: RailSettings,
    coinbase: RailSettings,
}

#[derive(Debug, Serialize)]
struct HealthControls {
    admin_sets_terms: bool,
    providers_set_prices: bool,
    providers_submit_models: bool,
}

impl Default for PaygateConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND
                .parse()
                .expect("default paygate bind address is valid"),
            contract_rpc_url: DEFAULT_CONTRACT_RPC_URL.to_owned(),
            oracle_key_path: default_oracle_key_path(),
            rails: RailConfig::default(),
        }
    }
}

impl PaygateConfig {
    pub fn from_sources(config_path: Option<&Path>) -> Result<Self> {
        let mut config = Self::default();
        if let Some(path) = config_path {
            config.apply_toml_file(path)?;
        }
        config.apply_env()?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self> {
        let mut config = Self::default();
        let file: ConfigFile = toml::from_str(input)?;
        config.apply_file(file)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.contract_rpc_url.trim().is_empty() {
            return Err(PaygateError::InvalidConfig(
                "contract.rpc_url cannot be empty".to_owned(),
            ));
        }
        if self.oracle_key_path.as_os_str().is_empty() {
            return Err(PaygateError::InvalidConfig(
                "oracle.key_path cannot be empty".to_owned(),
            ));
        }
        Ok(())
    }

    fn apply_toml_file(&mut self, path: &Path) -> Result<()> {
        let text = fs::read_to_string(path)?;
        let file: ConfigFile = toml::from_str(&text)?;
        self.apply_file(file)
    }

    fn apply_file(&mut self, file: ConfigFile) -> Result<()> {
        if let Some(bind) = file.server.bind {
            self.bind = parse_socket_addr("server.bind", &bind)?;
        }
        if let Some(rpc_url) = file.contract.rpc_url {
            self.contract_rpc_url = rpc_url;
        }
        if let Some(path) = file.oracle.key_path {
            self.oracle_key_path = expand_home(path);
        }
        if let Some(enabled) = file.stripe.enabled {
            self.rails.stripe.enabled = enabled;
        }
        if let Some(enabled) = file.coinbase.enabled {
            self.rails.coinbase.enabled = enabled;
        }
        Ok(())
    }

    fn apply_env(&mut self) -> Result<()> {
        if let Ok(bind) = env::var("MAYHEM_PAYGATE_BIND") {
            self.bind = parse_socket_addr("MAYHEM_PAYGATE_BIND", &bind)?;
        }
        if let Ok(rpc_url) = env::var("MAYHEM_CONTRACT_RPC_URL") {
            self.contract_rpc_url = rpc_url;
        }
        if let Ok(path) = env::var("MAYHEM_PAYGATE_ORACLE_KEY_PATH") {
            self.oracle_key_path = expand_home(PathBuf::from(path));
        }
        if let Ok(enabled) = env::var("MAYHEM_PAYGATE_STRIPE_ENABLED") {
            self.rails.stripe.enabled = parse_bool("MAYHEM_PAYGATE_STRIPE_ENABLED", &enabled)?;
        }
        if let Ok(enabled) = env::var("MAYHEM_PAYGATE_COINBASE_ENABLED") {
            self.rails.coinbase.enabled = parse_bool("MAYHEM_PAYGATE_COINBASE_ENABLED", &enabled)?;
        }
        Ok(())
    }
}

impl OracleKeypair {
    pub fn from_seed_hex(seed_hex: &str) -> Result<Self> {
        let seed = decode_seed(seed_hex)?;
        Ok(Self {
            signing_key: SigningKey::from_bytes(&seed),
        })
    }

    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            let seed_hex = fs::read_to_string(path)?;
            return Self::from_seed_hex(seed_hex.trim());
        }

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let seed = random_seed()?;
        let seed_hex = hex::encode(seed);
        match write_new_seed_file(path, &seed_hex) {
            Ok(()) => Self::from_seed_hex(&seed_hex),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let seed_hex = fs::read_to_string(path)?;
                Self::from_seed_hex(seed_hex.trim())
            }
            Err(err) => Err(err.into()),
        }
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign_hex(&self, payload: &[u8]) -> String {
        hex::encode(self.signing_key.sign(payload).to_bytes())
    }
}

impl PaygateState {
    pub fn new(config: PaygateConfig, oracle: OracleKeypair) -> Self {
        Self {
            config: Arc::new(config),
            oracle_public_key: oracle.public_key_hex(),
        }
    }

    pub fn oracle_public_key(&self) -> &str {
        &self.oracle_public_key
    }

    fn health(&self) -> HealthResponse {
        HealthResponse {
            ok: true,
            service: SERVICE_NAME,
            version: SERVICE_VERSION,
            denom: CREDIT_DENOM,
            oracle_pubkey: self.oracle_public_key.clone(),
            contract: HealthContract {
                rpc_configured: !self.config.contract_rpc_url.trim().is_empty(),
            },
            rails: HealthRails {
                stripe: self.config.rails.stripe.clone(),
                coinbase: self.config.rails.coinbase.clone(),
            },
            controls: HealthControls {
                admin_sets_terms: true,
                providers_set_prices: false,
                providers_submit_models: false,
            },
        }
    }
}

pub fn paygate_router(state: PaygateState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/health", get(health))
        .with_state(Arc::new(state))
}

pub async fn serve(bind: SocketAddr, state: PaygateState) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, paygate_router(state)).await
}

async fn health(State(state): State<Arc<PaygateState>>) -> Json<HealthResponse> {
    Json(state.health())
}

fn parse_socket_addr(field: &str, value: &str) -> Result<SocketAddr> {
    value
        .parse()
        .map_err(|err| PaygateError::InvalidConfig(format!("{field} is invalid: {err}")))
}

fn parse_bool(field: &str, value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(PaygateError::InvalidConfig(format!(
            "{field} must be a boolean"
        ))),
    }
}

fn decode_seed(seed_hex: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(seed_hex)?;
    bytes
        .try_into()
        .map_err(|_| PaygateError::InvalidConfig("oracle seed must be 32 bytes".to_owned()))
}

fn random_seed() -> Result<[u8; 32]> {
    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed).map_err(|err| PaygateError::Crypto(err.to_string()))?;
    Ok(seed)
}

fn write_new_seed_file(path: &Path, seed_hex: &str) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(seed_hex.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}

fn default_oracle_key_path() -> PathBuf {
    let home = env::var_os("MAYHEM_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".mayhem")))
        .unwrap_or_else(|| PathBuf::from(".mayhem"));
    home.join("paygate").join("oracle.seed")
}

fn expand_home(path: PathBuf) -> PathBuf {
    let path_str = path.to_string_lossy();
    if path_str == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path_str.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_config_overlays_defaults() -> Result<()> {
        let config = PaygateConfig::from_toml_str(
            r#"
            [server]
            bind = "127.0.0.1:19091"

            [contract]
            rpc_url = "http://127.0.0.1:49299/v1"

            [oracle]
            key_path = "/tmp/mayhem-paygate-test.seed"

            [stripe]
            enabled = true
            "#,
        )?;

        assert_eq!(config.bind, "127.0.0.1:19091".parse().unwrap());
        assert_eq!(config.contract_rpc_url, "http://127.0.0.1:49299/v1");
        assert_eq!(
            config.oracle_key_path,
            PathBuf::from("/tmp/mayhem-paygate-test.seed")
        );
        assert!(config.rails.stripe.enabled);
        assert!(!config.rails.coinbase.enabled);
        Ok(())
    }

    #[test]
    fn oracle_key_file_is_created_and_reused() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("nested").join("oracle.seed");

        let first = OracleKeypair::load_or_create(&path)?;
        let second = OracleKeypair::load_or_create(&path)?;

        assert_eq!(first.public_key_hex(), second.public_key_hex());
        let seed_hex = fs::read_to_string(&path)?;
        assert_eq!(seed_hex.trim().len(), 64);
        assert_ne!(seed_hex.trim(), "0".repeat(64));

        #[cfg(unix)]
        {
            let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        Ok(())
    }

    #[test]
    fn oracle_seed_must_be_thirty_two_bytes() {
        let err = OracleKeypair::from_seed_hex("abcd").expect_err("short seed rejected");
        assert!(err.to_string().contains("32 bytes"));
    }
}
