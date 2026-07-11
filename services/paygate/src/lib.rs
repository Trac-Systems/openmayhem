#![forbid(unsafe_code)]

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    fs::OpenOptions,
    future::Future,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ed25519_dalek::{Signer, SigningKey};
use hmac::{Hmac, Mac};
use mayhem_proto::MoneyAu;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{net::TcpListener, sync::Mutex, time::sleep};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const SERVICE_NAME: &str = "mayhem-paygate";
pub const SERVICE_VERSION: u32 = 1;
pub const CREDIT_DENOM: &str = "au_usd";
pub const DEFAULT_BIND: &str = "127.0.0.1:11436";
pub const DEFAULT_CONTRACT_RPC_URL: &str = "http://127.0.0.1:49223/v1";
pub const DEFAULT_STRIPE_API_BASE_URL: &str = "https://api.stripe.com";
pub const DEFAULT_STRIPE_WEBHOOK_TOLERANCE_SECONDS: u64 = 300;
pub const DEFAULT_STRIPE_BACKFILL_INTERVAL_SECONDS: u64 = 300;
pub const DEFAULT_EPOCH_SECONDS: u64 = 3_600;
pub const AU_PER_USD_CENT: MoneyAu = 10_000_000_000_000_000;
pub const STRIPE_MIN_USD_CENTS: u64 = 50;
pub const DEFAULT_STRIPE_CURRENCY: &str = "usd";
pub const DEFAULT_STRIPE_LOCALE: &str = "en";
const HANDLED_STRIPE_EVENT_TYPES: &[&str] = &["payment_intent.succeeded", "charge.dispute.created"];

type HmacSha256 = Hmac<Sha256>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type Result<T> = std::result::Result<T, PaygateError>;

#[derive(Debug, Error)]
pub enum PaygateError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("hex error: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("Stripe error: {0}")]
    Stripe(String),
    #[error("Stripe signature error: {0}")]
    StripeSignature(String),
    #[error("contract post failed: {0}")]
    Contract(String),
    #[error("crypto error: {0}")]
    Crypto(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaygateConfig {
    pub bind: SocketAddr,
    pub contract_rpc_url: String,
    pub contract_dry_run: bool,
    pub epoch_seconds: u64,
    pub oracle_key_path: PathBuf,
    pub rails: RailConfig,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RailConfig {
    pub stripe: StripeSettings,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RailSettings {
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StripeSettings {
    pub enabled: bool,
    pub mode: StripeMode,
    pub secret_key: Option<String>,
    pub webhook_secret: Option<String>,
    pub api_base_url: String,
    pub event_store_path: PathBuf,
    pub webhook_tolerance_seconds: u64,
    pub backfill_enabled: bool,
    pub backfill_cursor_path: PathBuf,
    pub backfill_interval_seconds: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StripeMode {
    #[default]
    Test,
    Live,
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
    stripe: StripeConfigFile,
}

#[derive(Debug, Default, Deserialize)]
struct ServerConfigFile {
    bind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ContractConfigFile {
    rpc_url: Option<String>,
    dry_run: Option<bool>,
    epoch_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct OracleConfigFile {
    key_path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct StripeConfigFile {
    enabled: Option<bool>,
    mode: Option<String>,
    secret_key: Option<String>,
    webhook_secret: Option<String>,
    api_base_url: Option<String>,
    event_store_path: Option<PathBuf>,
    webhook_tolerance_seconds: Option<u64>,
    backfill_enabled: Option<bool>,
    backfill_cursor_path: Option<PathBuf>,
    backfill_interval_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct OracleKeypair {
    signing_key: SigningKey,
}

pub trait ContractPoster: Send + Sync {
    fn epoch_seconds_at<'a>(&'a self, at: u64, fallback: u64) -> BoxFuture<'a, Result<u64>>;

    fn post_fiat_deposit<'a>(
        &'a self,
        oracle: &'a OracleKeypair,
        feature: FiatDepositFeature,
    ) -> BoxFuture<'a, Result<ContractPostResult>>;

    fn post_fiat_chargeback<'a>(
        &'a self,
        oracle: &'a OracleKeypair,
        feature: FiatChargebackFeature,
    ) -> BoxFuture<'a, Result<ContractPostResult>>;
}

#[derive(Clone)]
pub struct PaygateState {
    config: Arc<PaygateConfig>,
    oracle: OracleKeypair,
    oracle_public_key: String,
    http: reqwest::Client,
    stripe_events: Arc<Mutex<StripeEventStore>>,
    contract: Arc<dyn ContractPoster>,
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
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct HealthRails {
    stripe: HealthStripeRail,
}

#[derive(Debug, Serialize)]
struct HealthStripeRail {
    enabled: bool,
    mode: &'static str,
    api_configured: bool,
    webhook_configured: bool,
    backfill_enabled: bool,
}

#[derive(Debug, Serialize)]
struct HealthControls {
    admin_controls_economy: bool,
    admin_sets_terms: bool,
    admin_sets_prices: bool,
    admin_sets_rules: bool,
    admin_sets_params: bool,
    admin_sets_provider_payout_targets: bool,
    admin_can_ban_providers: bool,
    providers_set_prices: bool,
    providers_set_rules: bool,
    providers_set_params: bool,
    providers_set_payout_terms: bool,
    providers_submit_models: bool,
    providers_create_canonical_rooms: bool,
    providers_only_join_admin_rooms: bool,
    provider_payout_targets_admin_verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct StripeCreatePaymentIntentRequest {
    pub who: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub au: MoneyAu,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StripeCreateCheckoutSessionRequest {
    pub who: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub au: MoneyAu,
    pub success_url: String,
    pub cancel_url: String,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StripeCreatePaymentIntentResponse {
    pub ok: bool,
    pub rail: &'static str,
    pub processor_rail: &'static str,
    pub denom: &'static str,
    pub who: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub au: MoneyAu,
    pub payment_intent: StripePaymentIntentSummary,
}

#[derive(Debug, Serialize)]
pub struct StripeCreateCheckoutSessionResponse {
    pub ok: bool,
    pub rail: &'static str,
    pub processor_rail: &'static str,
    pub denom: &'static str,
    pub who: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub au: MoneyAu,
    pub checkout_session: StripeCheckoutSessionSummary,
    pub copy_paste: CheckoutCopyPaste,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripePaymentIntentSummary {
    pub id: String,
    pub client_secret: Option<String>,
    pub amount: u64,
    pub currency: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripeCheckoutSessionSummary {
    pub id: String,
    pub url: String,
    pub amount_total: Option<u64>,
    pub currency: Option<String>,
    pub payment_intent: Option<String>,
    pub payment_status: Option<String>,
    pub status: Option<String>,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckoutCopyPaste {
    pub checkout_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FiatDepositFeature {
    pub op: &'static str,
    pub rail: &'static str,
    pub who: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub au: MoneyAu,
    pub ext_ref_hash: String,
    pub fiat_currency: String,
    pub fiat_amount_minor: u64,
    pub epoch: u64,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct FiatChargebackFeature {
    pub op: &'static str,
    pub rail: &'static str,
    pub who: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub au: MoneyAu,
    pub ext_ref_hash: String,
    pub dispute_ref_hash: String,
    pub fiat_currency: String,
    pub fiat_amount_minor: u64,
    pub epoch: u64,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContractPostResult {
    pub tx: String,
    pub command_hash: Option<String>,
    pub result: Value,
}

#[derive(Clone, Debug, Deserialize)]
struct StripeEventEnvelope {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    created: Option<u64>,
    data: StripeEventData,
}

#[derive(Clone, Debug, Deserialize)]
struct StripeEventData {
    object: Value,
}

#[derive(Clone, Debug, Deserialize)]
struct StripePaymentIntentObject {
    id: String,
    #[serde(default)]
    amount: Option<u64>,
    #[serde(default)]
    amount_received: Option<u64>,
    #[serde(default)]
    latest_charge: Option<Value>,
    #[serde(default)]
    charges: Option<Value>,
    currency: String,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
struct StripeWebhookResponse {
    ok: bool,
    event_id: String,
    event_type: String,
    duplicate: bool,
    credited: bool,
    clawed_back: bool,
    ignored: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    payment_intent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    charge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dispute: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mayhem_proto::optional_decimal_u128"
    )]
    au: Option<MoneyAu>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract: Option<ContractPostResult>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StripeBackfillCursor {
    schema_version: u32,
    last_created: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StripeBackfillReport {
    pub ok: bool,
    pub fetched: usize,
    pub processed: usize,
    pub duplicates: usize,
    pub credited: usize,
    pub clawed_back: usize,
    pub ignored: usize,
    pub cursor_path: PathBuf,
    pub previous_last_created: u64,
    pub last_created: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StripeEventRecord {
    #[serde(default = "default_stripe_event_record_kind")]
    kind: String,
    event_id: String,
    #[serde(default)]
    payment_intent: Option<String>,
    #[serde(default)]
    charge: Option<String>,
    #[serde(default)]
    dispute: Option<String>,
    who: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    au: MoneyAu,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    amount_minor: Option<u64>,
    ext_ref_hash: String,
    #[serde(default)]
    dispute_ref_hash: Option<String>,
    #[serde(default)]
    credited_at: Option<u64>,
    #[serde(default)]
    disputed_at: Option<u64>,
}

#[derive(Debug)]
struct StripeEventStore {
    seen: HashSet<String>,
    processing: HashSet<String>,
    deposits_by_payment_intent: HashMap<String, StripeEventRecord>,
    deposits_by_charge: HashMap<String, StripeEventRecord>,
    path: Option<PathBuf>,
}

#[derive(Debug)]
enum StripeEventBegin {
    Started,
    Duplicate,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

#[derive(Clone)]
pub struct PeerRpcContractPoster {
    rpc_url: String,
    dry_run: bool,
    http: reqwest::Client,
}

impl Default for PaygateConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 11_436)),
            contract_rpc_url: DEFAULT_CONTRACT_RPC_URL.to_owned(),
            contract_dry_run: false,
            epoch_seconds: DEFAULT_EPOCH_SECONDS,
            oracle_key_path: default_oracle_key_path(),
            rails: RailConfig::default(),
        }
    }
}

impl Default for StripeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: StripeMode::Test,
            secret_key: None,
            webhook_secret: None,
            api_base_url: DEFAULT_STRIPE_API_BASE_URL.to_owned(),
            event_store_path: default_stripe_event_store_path(),
            webhook_tolerance_seconds: DEFAULT_STRIPE_WEBHOOK_TOLERANCE_SECONDS,
            backfill_enabled: true,
            backfill_cursor_path: default_stripe_backfill_cursor_path(),
            backfill_interval_seconds: DEFAULT_STRIPE_BACKFILL_INTERVAL_SECONDS,
        }
    }
}

impl StripeMode {
    fn as_str(self) -> &'static str {
        match self {
            StripeMode::Test => "test",
            StripeMode::Live => "live",
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
        if self.epoch_seconds == 0 {
            return Err(PaygateError::InvalidConfig(
                "contract.epoch_seconds cannot be zero".to_owned(),
            ));
        }
        if self.rails.stripe.enabled {
            let secret_key = self.rails.stripe.secret_key.as_deref().unwrap_or("");
            if secret_key.is_empty() {
                return Err(PaygateError::InvalidConfig(
                    "stripe.secret_key is required when Stripe is enabled".to_owned(),
                ));
            }
            validate_stripe_secret_key_mode(secret_key, self.rails.stripe.mode)?;
            if self
                .rails
                .stripe
                .webhook_secret
                .as_deref()
                .unwrap_or("")
                .is_empty()
            {
                return Err(PaygateError::InvalidConfig(
                    "stripe.webhook_secret is required when Stripe is enabled".to_owned(),
                ));
            }
            if self.rails.stripe.webhook_tolerance_seconds == 0 {
                return Err(PaygateError::InvalidConfig(
                    "stripe.webhook_tolerance_seconds cannot be zero".to_owned(),
                ));
            }
            if self.rails.stripe.backfill_enabled
                && self.rails.stripe.backfill_interval_seconds == 0
            {
                return Err(PaygateError::InvalidConfig(
                    "stripe.backfill_interval_seconds cannot be zero when backfill is enabled"
                        .to_owned(),
                ));
            }
            if self.rails.stripe.mode == StripeMode::Live {
                if self.rails.stripe.api_base_url.trim_end_matches('/')
                    != DEFAULT_STRIPE_API_BASE_URL
                {
                    return Err(PaygateError::InvalidConfig(
                        "Stripe live mode requires the official Stripe API base URL".to_owned(),
                    ));
                }
                if self.contract_dry_run {
                    return Err(PaygateError::InvalidConfig(
                        "contract.dry_run is forbidden in Stripe live mode".to_owned(),
                    ));
                }
            }
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
        if let Some(dry_run) = file.contract.dry_run {
            self.contract_dry_run = dry_run;
        }
        if let Some(epoch_seconds) = file.contract.epoch_seconds {
            self.epoch_seconds = epoch_seconds;
        }
        if let Some(path) = file.oracle.key_path {
            self.oracle_key_path = expand_home(path);
        }
        if let Some(enabled) = file.stripe.enabled {
            self.rails.stripe.enabled = enabled;
        }
        if let Some(mode) = file.stripe.mode {
            self.rails.stripe.mode = parse_stripe_mode("stripe.mode", &mode)?;
        }
        if let Some(secret_key) = file.stripe.secret_key {
            self.rails.stripe.secret_key = Some(secret_key);
        }
        if let Some(webhook_secret) = file.stripe.webhook_secret {
            self.rails.stripe.webhook_secret = Some(webhook_secret);
        }
        if let Some(api_base_url) = file.stripe.api_base_url {
            self.rails.stripe.api_base_url = api_base_url;
        }
        if let Some(path) = file.stripe.event_store_path {
            self.rails.stripe.event_store_path = expand_home(path);
        }
        if let Some(tolerance) = file.stripe.webhook_tolerance_seconds {
            self.rails.stripe.webhook_tolerance_seconds = tolerance;
        }
        if let Some(enabled) = file.stripe.backfill_enabled {
            self.rails.stripe.backfill_enabled = enabled;
        }
        if let Some(path) = file.stripe.backfill_cursor_path {
            self.rails.stripe.backfill_cursor_path = expand_home(path);
        }
        if let Some(seconds) = file.stripe.backfill_interval_seconds {
            self.rails.stripe.backfill_interval_seconds = seconds;
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
        if let Ok(dry_run) = env::var("MAYHEM_PAYGATE_CONTRACT_DRY_RUN") {
            self.contract_dry_run = parse_bool("MAYHEM_PAYGATE_CONTRACT_DRY_RUN", &dry_run)?;
        }
        if let Ok(epoch_seconds) = env::var("MAYHEM_PAYGATE_EPOCH_SECONDS") {
            self.epoch_seconds = parse_u64("MAYHEM_PAYGATE_EPOCH_SECONDS", &epoch_seconds)?;
        }
        if let Ok(path) = env::var("MAYHEM_PAYGATE_ORACLE_KEY_PATH") {
            self.oracle_key_path = expand_home(PathBuf::from(path));
        }
        if let Ok(enabled) = env::var("MAYHEM_PAYGATE_STRIPE_ENABLED") {
            self.rails.stripe.enabled = parse_bool("MAYHEM_PAYGATE_STRIPE_ENABLED", &enabled)?;
        }
        if let Ok(mode) = env::var("MAYHEM_STRIPE_MODE") {
            self.rails.stripe.mode = parse_stripe_mode("MAYHEM_STRIPE_MODE", &mode)?;
        }
        if let Ok(secret_key) = env::var("MAYHEM_STRIPE_SECRET_KEY") {
            self.rails.stripe.secret_key = Some(secret_key);
        }
        if let Ok(webhook_secret) = env::var("MAYHEM_STRIPE_WEBHOOK_SECRET") {
            self.rails.stripe.webhook_secret = Some(webhook_secret);
        }
        if let Ok(api_base_url) = env::var("MAYHEM_STRIPE_API_BASE_URL") {
            self.rails.stripe.api_base_url = api_base_url;
        }
        if let Ok(path) = env::var("MAYHEM_PAYGATE_STRIPE_EVENTS_PATH") {
            self.rails.stripe.event_store_path = expand_home(PathBuf::from(path));
        }
        if let Ok(tolerance) = env::var("MAYHEM_STRIPE_WEBHOOK_TOLERANCE_SECONDS") {
            self.rails.stripe.webhook_tolerance_seconds =
                parse_u64("MAYHEM_STRIPE_WEBHOOK_TOLERANCE_SECONDS", &tolerance)?;
        }
        if let Ok(enabled) = env::var("MAYHEM_STRIPE_BACKFILL_ENABLED") {
            self.rails.stripe.backfill_enabled =
                parse_bool("MAYHEM_STRIPE_BACKFILL_ENABLED", &enabled)?;
        }
        if let Ok(path) = env::var("MAYHEM_STRIPE_BACKFILL_CURSOR_PATH") {
            self.rails.stripe.backfill_cursor_path = expand_home(PathBuf::from(path));
        }
        if let Ok(seconds) = env::var("MAYHEM_STRIPE_BACKFILL_INTERVAL_SECONDS") {
            self.rails.stripe.backfill_interval_seconds =
                parse_u64("MAYHEM_STRIPE_BACKFILL_INTERVAL_SECONDS", &seconds)?;
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

    pub fn sign_tx_hex(&self, tx_hex: &str) -> Result<String> {
        let tx = hex::decode(tx_hex)?;
        Ok(self.sign_hex(&tx))
    }
}

impl PaygateState {
    pub fn new(config: PaygateConfig, oracle: OracleKeypair) -> Self {
        let http = reqwest::Client::new();
        let contract = Arc::new(PeerRpcContractPoster::new(
            config.contract_rpc_url.clone(),
            config.contract_dry_run,
            http.clone(),
        ));
        Self::with_parts(config, oracle, http, StripeEventStore::memory(), contract)
    }

    pub fn try_new(config: PaygateConfig, oracle: OracleKeypair) -> Result<Self> {
        config.validate()?;
        let http = reqwest::Client::new();
        let contract = Arc::new(PeerRpcContractPoster::new(
            config.contract_rpc_url.clone(),
            config.contract_dry_run,
            http.clone(),
        ));
        let stripe_events = StripeEventStore::load(&config.rails.stripe.event_store_path)?;
        Ok(Self::with_parts(
            config,
            oracle,
            http,
            stripe_events,
            contract,
        ))
    }

    pub fn try_new_with_contract_poster(
        config: PaygateConfig,
        oracle: OracleKeypair,
        contract: Arc<dyn ContractPoster>,
    ) -> Result<Self> {
        config.validate()?;
        let http = reqwest::Client::new();
        let stripe_events = StripeEventStore::load(&config.rails.stripe.event_store_path)?;
        Ok(Self::with_parts(
            config,
            oracle,
            http,
            stripe_events,
            contract,
        ))
    }

    fn with_parts(
        config: PaygateConfig,
        oracle: OracleKeypair,
        http: reqwest::Client,
        stripe_events: StripeEventStore,
        contract: Arc<dyn ContractPoster>,
    ) -> Self {
        let oracle_public_key = oracle.public_key_hex();
        Self {
            config: Arc::new(config),
            oracle,
            oracle_public_key,
            http,
            stripe_events: Arc::new(Mutex::new(stripe_events)),
            contract,
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
                dry_run: self.config.contract_dry_run,
            },
            rails: HealthRails {
                stripe: HealthStripeRail {
                    enabled: self.config.rails.stripe.enabled,
                    mode: self.config.rails.stripe.mode.as_str(),
                    api_configured: self.config.rails.stripe.secret_key.is_some(),
                    webhook_configured: self.config.rails.stripe.webhook_secret.is_some(),
                    backfill_enabled: self.config.rails.stripe.backfill_enabled,
                },
            },
            controls: HealthControls {
                admin_controls_economy: true,
                admin_sets_terms: true,
                admin_sets_prices: true,
                admin_sets_rules: true,
                admin_sets_params: true,
                admin_sets_provider_payout_targets: true,
                admin_can_ban_providers: true,
                providers_set_prices: false,
                providers_set_rules: false,
                providers_set_params: false,
                providers_set_payout_terms: false,
                providers_submit_models: false,
                providers_create_canonical_rooms: false,
                providers_only_join_admin_rooms: true,
                provider_payout_targets_admin_verified: true,
            },
        }
    }
}

impl PeerRpcContractPoster {
    pub fn new(rpc_url: String, dry_run: bool, http: reqwest::Client) -> Self {
        Self {
            rpc_url,
            dry_run,
            http,
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.rpc_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    async fn read_state_value(&self, key: &str) -> Result<Option<Value>> {
        let url = format!(
            "{}?key={}&confirmed=false",
            self.endpoint("state"),
            query_escape(key)
        );
        let state: Value = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(state.get("value").cloned().filter(|value| !value.is_null()))
    }

    async fn read_param_u64_at(&self, key: &str, fallback: u64, at: u64) -> Result<u64> {
        let Some(record) = self.read_state_value(&format!("params/{key}")).await? else {
            return Ok(fallback);
        };
        let active = match record.get("pending") {
            Some(pending)
                if !pending.is_null()
                    && pending
                        .get("effective_at")
                        .and_then(Value::as_u64)
                        .is_some_and(|effective_at| effective_at <= at) =>
            {
                pending
            }
            _ => record.get("current").ok_or_else(|| {
                PaygateError::Contract(format!("params/{key} missing current entry"))
            })?,
        };
        active.get("value").and_then(Value::as_u64).ok_or_else(|| {
            PaygateError::Contract(format!("params/{key} active value is not a u64"))
        })
    }

    async fn post_admin_feature_value(
        &self,
        feature_key: &str,
        value: Value,
    ) -> Result<ContractPostResult> {
        if self.dry_run {
            return Err(PaygateError::Contract(
                "admin feature submission cannot run with contract.dry_run enabled".to_owned(),
            ));
        }
        let submitted: Value = self
            .http
            .post(self.endpoint("contract/feature"))
            .json(&json!({
                "feature": "mayhem",
                "key": feature_key,
                "value": value,
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let applied = submitted.get("ok").and_then(Value::as_bool) == Some(true)
            && submitted.get("status").and_then(Value::as_str) == Some("applied")
            && submitted.get("key").and_then(Value::as_str) == Some(feature_key);
        if !applied {
            return Err(PaygateError::Contract(format!(
                "admin feature was not applied: {submitted}"
            )));
        }
        let feature_result = submitted.get("result").ok_or_else(|| {
            PaygateError::Contract("applied feature response missing result".to_owned())
        })?;
        let result_feature_key = format!("mayhem_{feature_key}");
        if feature_result.get("ok").and_then(Value::as_bool) != Some(true)
            || feature_result.get("status").and_then(Value::as_str) != Some("applied")
            || feature_result.get("feature_key").and_then(Value::as_str)
                != Some(result_feature_key.as_str())
        {
            return Err(PaygateError::Contract(format!(
                "admin feature result was not applied: {feature_result}"
            )));
        }
        let result = feature_result.get("result").cloned().ok_or_else(|| {
            PaygateError::Contract("applied feature response missing contract result".to_owned())
        })?;
        if result.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(PaygateError::Contract(format!(
                "admin feature contract result was not successful: {result}"
            )));
        }
        let tx = submitted
            .get("hash")
            .and_then(Value::as_str)
            .ok_or_else(|| PaygateError::Contract("feature response missing hash".to_owned()))?
            .to_owned();
        Ok(ContractPostResult {
            tx,
            command_hash: None,
            result,
        })
    }

    async fn require_state_fields(&self, key: &str, expected: &[(&str, Value)]) -> Result<()> {
        let record = self.read_state_value(key).await?.ok_or_else(|| {
            PaygateError::Contract(format!(
                "applied feature did not create expected ledger state {key}"
            ))
        })?;
        for (field, expected_value) in expected {
            if record.get(*field) != Some(expected_value) {
                return Err(PaygateError::Contract(format!(
                    "ledger state {key} field {field} mismatch"
                )));
            }
        }
        Ok(())
    }
}

impl ContractPoster for PeerRpcContractPoster {
    fn epoch_seconds_at<'a>(&'a self, at: u64, fallback: u64) -> BoxFuture<'a, Result<u64>> {
        Box::pin(async move {
            let epoch_seconds = self
                .read_param_u64_at("epoch_seconds", fallback, at)
                .await?;
            if epoch_seconds == 0 {
                return Err(PaygateError::Contract(
                    "params/epoch_seconds active value cannot be zero".to_owned(),
                ));
            }
            Ok(epoch_seconds)
        })
    }

    fn post_fiat_deposit<'a>(
        &'a self,
        _oracle: &'a OracleKeypair,
        feature: FiatDepositFeature,
    ) -> BoxFuture<'a, Result<ContractPostResult>> {
        Box::pin(async move {
            let feature_key = format!("dep/fiat/{}", feature.ext_ref_hash);
            let result = self
                .post_admin_feature_value(&feature_key, serde_json::to_value(&feature)?)
                .await?;
            self.require_state_fields(
                &feature_key,
                &[
                    ("rail", json!("fiat")),
                    ("processor_rail", json!(feature.rail)),
                    ("who", json!(&feature.who)),
                    ("au", json!(feature.au.to_string())),
                    ("ext_ref_hash", json!(&feature.ext_ref_hash)),
                    ("fiat_currency", json!(&feature.fiat_currency)),
                    ("fiat_amount_minor", json!(feature.fiat_amount_minor)),
                    ("epoch", json!(feature.epoch)),
                    ("at", json!(feature.at)),
                    ("credited_at", json!(&feature_key)),
                    ("credited_by_role", json!("admin")),
                ],
            )
            .await?;
            Ok(result)
        })
    }

    fn post_fiat_chargeback<'a>(
        &'a self,
        _oracle: &'a OracleKeypair,
        feature: FiatChargebackFeature,
    ) -> BoxFuture<'a, Result<ContractPostResult>> {
        Box::pin(async move {
            let feature_key = format!(
                "dep/fiat/{}/chargeback/{}",
                feature.ext_ref_hash, feature.dispute_ref_hash
            );
            let result = self
                .post_admin_feature_value(&feature_key, serde_json::to_value(&feature)?)
                .await?;
            self.require_state_fields(
                &feature_key,
                &[
                    ("rail", json!("fiat")),
                    ("processor_rail", json!(feature.rail)),
                    ("who", json!(&feature.who)),
                    ("au", json!(feature.au.to_string())),
                    ("ext_ref_hash", json!(&feature.ext_ref_hash)),
                    ("dispute_ref_hash", json!(&feature.dispute_ref_hash)),
                    ("fiat_currency", json!(&feature.fiat_currency)),
                    ("fiat_amount_minor", json!(feature.fiat_amount_minor)),
                    ("epoch", json!(feature.epoch)),
                    ("at", json!(feature.at)),
                    ("credited_at", json!(&feature_key)),
                    ("credited_by_role", json!("admin")),
                ],
            )
            .await?;
            Ok(result)
        })
    }
}

pub fn paygate_router(state: PaygateState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/health", get(health))
        .route(
            "/stripe/payment-intents",
            post(create_stripe_payment_intent),
        )
        .route(
            "/v1/stripe/payment-intents",
            post(create_stripe_payment_intent),
        )
        .route(
            "/stripe/checkout-sessions",
            post(create_stripe_checkout_session),
        )
        .route(
            "/v1/stripe/checkout-sessions",
            post(create_stripe_checkout_session),
        )
        .route("/stripe/return", get(stripe_return))
        .route("/v1/stripe/return", get(stripe_return))
        .route("/stripe/cancel", get(stripe_cancel))
        .route("/v1/stripe/cancel", get(stripe_cancel))
        .route("/stripe/webhook", post(stripe_webhook))
        .route("/v1/stripe/webhook", post(stripe_webhook))
        .with_state(Arc::new(state))
}

pub async fn serve(bind: SocketAddr, state: PaygateState) -> std::io::Result<()> {
    if state.config.rails.stripe.enabled && state.config.rails.stripe.backfill_enabled {
        let backfill_state = state.clone();
        tokio::spawn(async move {
            stripe_backfill_loop(backfill_state).await;
        });
    }
    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, paygate_router(state)).await
}

async fn health(State(state): State<Arc<PaygateState>>) -> Json<HealthResponse> {
    Json(state.health())
}

async fn create_stripe_payment_intent(
    State(state): State<Arc<PaygateState>>,
    Json(request): Json<StripeCreatePaymentIntentRequest>,
) -> Response {
    match create_payment_intent(&state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn create_stripe_checkout_session(
    State(state): State<Arc<PaygateState>>,
    Json(request): Json<StripeCreateCheckoutSessionRequest>,
) -> Response {
    match create_checkout_session(&state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn stripe_return() -> Html<&'static str> {
    Html("Mayhem payment submitted. You can return to the CLI while credit confirmation settles.")
}

async fn stripe_cancel() -> Html<&'static str> {
    Html("Mayhem payment cancelled. You can return to the CLI.")
}

async fn stripe_webhook(
    State(state): State<Arc<PaygateState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match handle_stripe_webhook(&state, &headers, &body).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn create_payment_intent(
    state: &PaygateState,
    request: StripeCreatePaymentIntentRequest,
) -> Result<StripeCreatePaymentIntentResponse> {
    let stripe = &state.config.rails.stripe;
    if !stripe.enabled {
        return Err(PaygateError::InvalidRequest(
            "Stripe processor is not enabled".to_owned(),
        ));
    }
    validate_safe_key_part("who", &request.who)?;
    let currency = normalize_stripe_currency(request.currency.as_deref())?;
    let amount_cents = au_to_stripe_minor(request.au, &currency)?;
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;
    let intent =
        stripe_create_payment_intent(&state.http, stripe, secret_key, &request, amount_cents)
            .await?;
    if !intent.currency.eq_ignore_ascii_case(&currency) {
        return Err(PaygateError::Stripe(
            "PaymentIntent response currency did not match request".to_owned(),
        ));
    }
    Ok(StripeCreatePaymentIntentResponse {
        ok: true,
        rail: "fiat",
        processor_rail: "stripe",
        denom: CREDIT_DENOM,
        who: request.who,
        au: request.au,
        payment_intent: intent,
    })
}

async fn create_checkout_session(
    state: &PaygateState,
    request: StripeCreateCheckoutSessionRequest,
) -> Result<StripeCreateCheckoutSessionResponse> {
    let stripe = &state.config.rails.stripe;
    if !stripe.enabled {
        return Err(PaygateError::InvalidRequest(
            "Stripe processor is not enabled".to_owned(),
        ));
    }
    validate_safe_key_part("who", &request.who)?;
    validate_checkout_url("success_url", &request.success_url)?;
    validate_checkout_url("cancel_url", &request.cancel_url)?;
    let currency = normalize_stripe_currency(request.currency.as_deref())?;
    let _locale = normalize_stripe_locale(request.locale.as_deref())?;
    let amount_cents = au_to_stripe_minor(request.au, &currency)?;
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;
    let session =
        stripe_create_checkout_session(&state.http, stripe, secret_key, &request, amount_cents)
            .await?;
    if !session
        .currency
        .as_deref()
        .unwrap_or(&currency)
        .eq_ignore_ascii_case(&currency)
    {
        return Err(PaygateError::Stripe(
            "Checkout Session response currency did not match request".to_owned(),
        ));
    }
    let copy_paste = checkout_copy_paste(&session.url);
    Ok(StripeCreateCheckoutSessionResponse {
        ok: true,
        rail: "fiat",
        processor_rail: "stripe",
        denom: CREDIT_DENOM,
        who: request.who,
        au: request.au,
        checkout_session: session,
        copy_paste,
    })
}

async fn stripe_create_payment_intent(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    request: &StripeCreatePaymentIntentRequest,
    amount_cents: u64,
) -> Result<StripePaymentIntentSummary> {
    let currency = normalize_stripe_currency(request.currency.as_deref())?;
    let form = [
        ("amount".to_owned(), amount_cents.to_string()),
        ("currency".to_owned(), currency.clone()),
        (
            "automatic_payment_methods[enabled]".to_owned(),
            "true".to_owned(),
        ),
        ("metadata[mayhem_who]".to_owned(), request.who.to_owned()),
        ("metadata[mayhem_au]".to_owned(), request.au.to_string()),
        ("metadata[mayhem_denom]".to_owned(), CREDIT_DENOM.to_owned()),
        ("metadata[mayhem_fiat_currency]".to_owned(), currency),
        (
            "metadata[mayhem_fiat_amount_minor]".to_owned(),
            amount_cents.to_string(),
        ),
    ];
    let mut builder = http
        .post(format!(
            "{}/v1/payment_intents",
            stripe.api_base_url.trim_end_matches('/')
        ))
        .basic_auth(secret_key, Some(""))
        .form(&form);
    if let Some(idempotency_key) = &request.idempotency_key {
        if !idempotency_key.is_empty() {
            builder = builder.header("Idempotency-Key", idempotency_key);
        }
    }
    let response = builder.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(PaygateError::Stripe(format!(
            "Stripe returned {status}: {body}"
        )));
    }
    let value: Value = serde_json::from_str(&body)?;
    stripe_payment_intent_summary(value)
}

async fn stripe_create_checkout_session(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    request: &StripeCreateCheckoutSessionRequest,
    amount_cents: u64,
) -> Result<StripeCheckoutSessionSummary> {
    let currency = normalize_stripe_currency(request.currency.as_deref())?;
    let locale = normalize_stripe_locale(request.locale.as_deref())?;
    let form = [
        ("mode".to_owned(), "payment".to_owned()),
        ("success_url".to_owned(), request.success_url.to_owned()),
        ("cancel_url".to_owned(), request.cancel_url.to_owned()),
        ("locale".to_owned(), locale),
        (
            "line_items[0][price_data][currency]".to_owned(),
            currency.clone(),
        ),
        (
            "line_items[0][price_data][product_data][name]".to_owned(),
            "Mayhem credits".to_owned(),
        ),
        (
            "line_items[0][price_data][product_data][description]".to_owned(),
            "Mayhem credit top-up".to_owned(),
        ),
        (
            "line_items[0][price_data][unit_amount]".to_owned(),
            amount_cents.to_string(),
        ),
        ("line_items[0][quantity]".to_owned(), "1".to_owned()),
        ("client_reference_id".to_owned(), request.who.to_owned()),
        ("metadata[mayhem_who]".to_owned(), request.who.to_owned()),
        ("metadata[mayhem_au]".to_owned(), request.au.to_string()),
        ("metadata[mayhem_denom]".to_owned(), CREDIT_DENOM.to_owned()),
        (
            "metadata[mayhem_fiat_currency]".to_owned(),
            currency.clone(),
        ),
        (
            "metadata[mayhem_fiat_amount_minor]".to_owned(),
            amount_cents.to_string(),
        ),
        (
            "payment_intent_data[metadata][mayhem_who]".to_owned(),
            request.who.to_owned(),
        ),
        (
            "payment_intent_data[metadata][mayhem_au]".to_owned(),
            request.au.to_string(),
        ),
        (
            "payment_intent_data[metadata][mayhem_denom]".to_owned(),
            CREDIT_DENOM.to_owned(),
        ),
        (
            "payment_intent_data[metadata][mayhem_fiat_currency]".to_owned(),
            currency,
        ),
        (
            "payment_intent_data[metadata][mayhem_fiat_amount_minor]".to_owned(),
            amount_cents.to_string(),
        ),
    ];
    let mut builder = http
        .post(format!(
            "{}/v1/checkout/sessions",
            stripe.api_base_url.trim_end_matches('/')
        ))
        .basic_auth(secret_key, Some(""))
        .form(&form);
    if let Some(idempotency_key) = &request.idempotency_key {
        if !idempotency_key.is_empty() {
            builder = builder.header("Idempotency-Key", idempotency_key);
        }
    }
    let response = builder.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(PaygateError::Stripe(format!(
            "Stripe returned {status}: {body}"
        )));
    }
    let value: Value = serde_json::from_str(&body)?;
    stripe_checkout_session_summary(value)
}

fn stripe_payment_intent_summary(value: Value) -> Result<StripePaymentIntentSummary> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent response missing id".to_owned()))?;
    let amount = value
        .get("amount")
        .and_then(Value::as_u64)
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent response missing amount".to_owned()))?;
    let currency = value
        .get("currency")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            PaygateError::Stripe("PaymentIntent response missing currency".to_owned())
        })?;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent response missing status".to_owned()))?;
    Ok(StripePaymentIntentSummary {
        id: id.to_owned(),
        client_secret: value
            .get("client_secret")
            .and_then(Value::as_str)
            .map(str::to_owned),
        amount,
        currency: currency.to_owned(),
        status: status.to_owned(),
    })
}

fn stripe_checkout_session_summary(value: Value) -> Result<StripeCheckoutSessionSummary> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| PaygateError::Stripe("Checkout Session response missing id".to_owned()))?;
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| PaygateError::Stripe("Checkout Session response missing url".to_owned()))?;
    validate_hosted_checkout_url("checkout_session.url", url, "checkout.stripe.com")?;
    Ok(StripeCheckoutSessionSummary {
        id: id.to_owned(),
        url: url.to_owned(),
        amount_total: value.get("amount_total").and_then(Value::as_u64),
        currency: value
            .get("currency")
            .and_then(Value::as_str)
            .map(str::to_owned),
        payment_intent: value
            .get("payment_intent")
            .and_then(Value::as_str)
            .map(str::to_owned),
        payment_status: value
            .get("payment_status")
            .and_then(Value::as_str)
            .map(str::to_owned),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned),
        expires_at: value.get("expires_at").and_then(Value::as_u64),
    })
}

async fn handle_stripe_webhook(
    state: &PaygateState,
    headers: &HeaderMap,
    payload: &[u8],
) -> Result<StripeWebhookResponse> {
    let stripe = &state.config.rails.stripe;
    if !stripe.enabled {
        return Err(PaygateError::InvalidRequest(
            "Stripe processor is not enabled".to_owned(),
        ));
    }
    let webhook_secret = stripe
        .webhook_secret
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.webhook_secret missing".to_owned()))?;
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| PaygateError::StripeSignature("Stripe-Signature missing".to_owned()))?;
    verify_stripe_signature(
        payload,
        signature,
        webhook_secret,
        unix_epoch_seconds()?,
        stripe.webhook_tolerance_seconds,
    )?;
    let event: StripeEventEnvelope = serde_json::from_slice(payload)?;
    handle_stripe_event(state, event).await
}

async fn handle_stripe_event(
    state: &PaygateState,
    event: StripeEventEnvelope,
) -> Result<StripeWebhookResponse> {
    let handles_event = HANDLED_STRIPE_EVENT_TYPES.contains(&event.event_type.as_str());
    if !handles_event {
        return Ok(ignored_stripe_event_response(event));
    }
    if stripe_event_is_unrelated(state, &event).await? {
        return Ok(ignored_stripe_event_response(event));
    }

    {
        let mut store = state.stripe_events.lock().await;
        if matches!(store.begin(&event.id), StripeEventBegin::Duplicate) {
            return Ok(StripeWebhookResponse {
                ok: true,
                event_id: event.id,
                event_type: event.event_type,
                duplicate: true,
                credited: false,
                clawed_back: false,
                ignored: false,
                payment_intent: None,
                charge: None,
                dispute: None,
                au: None,
                contract: None,
            });
        }
    }

    let result = match event.event_type.as_str() {
        "payment_intent.succeeded" => handle_stripe_payment_intent_succeeded(state, &event).await,
        "charge.dispute.created" => handle_stripe_dispute_created(state, &event).await,
        _ => {
            state.stripe_events.lock().await.fail(&event.id);
            return Ok(ignored_stripe_event_response(event));
        }
    };
    match result {
        Ok((record, contract)) => {
            let mut store = state.stripe_events.lock().await;
            store.complete(record.clone())?;
            Ok(StripeWebhookResponse {
                ok: true,
                event_id: record.event_id.clone(),
                event_type: event.event_type,
                duplicate: false,
                credited: record.kind == "deposit",
                clawed_back: record.kind == "chargeback",
                ignored: false,
                payment_intent: record.payment_intent,
                charge: record.charge,
                dispute: record.dispute,
                au: Some(record.au),
                contract: Some(contract),
            })
        }
        Err(err) => {
            let mut store = state.stripe_events.lock().await;
            store.fail(&event.id);
            Err(err)
        }
    }
}

fn ignored_stripe_event_response(event: StripeEventEnvelope) -> StripeWebhookResponse {
    StripeWebhookResponse {
        ok: true,
        event_id: event.id,
        event_type: event.event_type,
        duplicate: false,
        credited: false,
        clawed_back: false,
        ignored: true,
        payment_intent: None,
        charge: None,
        dispute: None,
        au: None,
        contract: None,
    }
}

async fn stripe_event_is_unrelated(
    state: &PaygateState,
    event: &StripeEventEnvelope,
) -> Result<bool> {
    match event.event_type.as_str() {
        "payment_intent.succeeded" => {
            let metadata = match event.data.object.get("metadata") {
                None | Some(Value::Null) => return Ok(true),
                Some(Value::Object(metadata)) => metadata,
                Some(_) => {
                    return Err(PaygateError::Stripe(
                        "PaymentIntent metadata must be an object".to_owned(),
                    ))
                }
            };
            Ok(!metadata.keys().any(|key| key.starts_with("mayhem_")))
        }
        "charge.dispute.created" => {
            let object = &event.data.object;
            let charge = stripe_expandable_id(object.get("charge"));
            let payment_intent = stripe_expandable_id(object.get("payment_intent"));
            if charge.is_none() && payment_intent.is_none() {
                return Ok(false);
            }
            let store = state.stripe_events.lock().await;
            Ok(store
                .lookup_deposit(payment_intent.as_deref(), charge.as_deref())
                .is_none())
        }
        _ => Ok(false),
    }
}

pub async fn run_stripe_backfill_once(state: &PaygateState) -> Result<StripeBackfillReport> {
    let stripe = &state.config.rails.stripe;
    if !stripe.enabled {
        return Err(PaygateError::InvalidRequest(
            "Stripe processor is not enabled".to_owned(),
        ));
    }
    if !stripe.backfill_enabled {
        return Ok(StripeBackfillReport {
            ok: true,
            fetched: 0,
            processed: 0,
            duplicates: 0,
            credited: 0,
            clawed_back: 0,
            ignored: 0,
            cursor_path: stripe.backfill_cursor_path.clone(),
            previous_last_created: 0,
            last_created: 0,
        });
    }
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;
    let cursor = StripeBackfillCursor::load(&stripe.backfill_cursor_path)?;
    let previous_last_created = cursor.last_created;
    let mut events =
        stripe_fetch_backfill_events(&state.http, stripe, secret_key, cursor.last_created).await?;
    events.sort_by(|left, right| {
        (left.created.unwrap_or(0), left.id.as_str())
            .cmp(&(right.created.unwrap_or(0), right.id.as_str()))
    });

    let mut report = StripeBackfillReport {
        ok: true,
        fetched: events.len(),
        processed: 0,
        duplicates: 0,
        credited: 0,
        clawed_back: 0,
        ignored: 0,
        cursor_path: stripe.backfill_cursor_path.clone(),
        previous_last_created,
        last_created: previous_last_created,
    };

    for event in events {
        report.last_created = report.last_created.max(event.created.unwrap_or(0));
        let response = handle_stripe_event(state, event).await?;
        report.processed += 1;
        if response.duplicate {
            report.duplicates += 1;
        }
        if response.credited {
            report.credited += 1;
        }
        if response.clawed_back {
            report.clawed_back += 1;
        }
        if response.ignored {
            report.ignored += 1;
        }
    }

    StripeBackfillCursor {
        schema_version: 1,
        last_created: report.last_created,
    }
    .save(&stripe.backfill_cursor_path)?;
    Ok(report)
}

async fn stripe_backfill_loop(state: PaygateState) {
    loop {
        if let Err(err) = run_stripe_backfill_once(&state).await {
            eprintln!("mayhem-paygate Stripe backfill failed: {err}");
        }
        let seconds = state.config.rails.stripe.backfill_interval_seconds.max(1);
        sleep(Duration::from_secs(seconds)).await;
    }
}

async fn stripe_fetch_backfill_events(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    created_gte: u64,
) -> Result<Vec<StripeEventEnvelope>> {
    let mut events = Vec::new();
    let mut seen = HashSet::new();
    for event_type in HANDLED_STRIPE_EVENT_TYPES {
        let mut starting_after: Option<String> = None;
        let mut pages = 0_u32;
        loop {
            pages += 1;
            if pages > 1_000 {
                return Err(PaygateError::Stripe(
                    "Stripe events backfill exceeded pagination limit".to_owned(),
                ));
            }
            let mut url = reqwest::Url::parse(&format!(
                "{}/v1/events",
                stripe.api_base_url.trim_end_matches('/')
            ))
            .map_err(|err| PaygateError::Stripe(format!("Stripe events URL invalid: {err}")))?;
            {
                let mut query = url.query_pairs_mut();
                query.append_pair("limit", "100");
                query.append_pair("type", event_type);
                query.append_pair("created[gte]", &created_gte.to_string());
                if let Some(cursor) = starting_after.as_deref() {
                    query.append_pair("starting_after", cursor);
                }
            }
            let response = http
                .get(url)
                .basic_auth(secret_key, Some(""))
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                return Err(PaygateError::Stripe(format!(
                    "Stripe events backfill returned {status}: {body}"
                )));
            }
            let value: Value = serde_json::from_str(&body)?;
            let data = value.get("data").and_then(Value::as_array).ok_or_else(|| {
                PaygateError::Stripe("Stripe events response missing data".to_owned())
            })?;
            let mut last_id = None;
            for raw in data {
                let event: StripeEventEnvelope = serde_json::from_value(raw.clone())?;
                last_id = Some(event.id.clone());
                if seen.insert(event.id.clone()) {
                    events.push(event);
                }
            }
            if value.get("has_more").and_then(Value::as_bool) != Some(true) {
                break;
            }
            let Some(cursor) = last_id else {
                break;
            };
            starting_after = Some(cursor);
        }
    }
    Ok(events)
}

async fn handle_stripe_payment_intent_succeeded(
    state: &PaygateState,
    event: &StripeEventEnvelope,
) -> Result<(StripeEventRecord, ContractPostResult)> {
    let object: StripePaymentIntentObject = serde_json::from_value(event.data.object.clone())?;
    let currency = normalize_stripe_currency(Some(&object.currency))?;
    let amount_cents = object
        .amount_received
        .or(object.amount)
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent missing amount".to_owned()))?;
    let au_from_amount = MoneyAu::from(amount_cents)
        .checked_mul(AU_PER_USD_CENT)
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent amount overflow".to_owned()))?;
    let who = object
        .metadata
        .get("mayhem_who")
        .ok_or_else(|| {
            PaygateError::Stripe("PaymentIntent missing mayhem_who metadata".to_owned())
        })?
        .to_owned();
    validate_safe_key_part("mayhem_who", &who)?;
    let au = object
        .metadata
        .get("mayhem_au")
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent missing mayhem_au metadata".to_owned()))?
        .parse::<MoneyAu>()
        .map_err(|_| PaygateError::Stripe("PaymentIntent mayhem_au is invalid".to_owned()))?;
    if au != au_from_amount {
        return Err(PaygateError::Stripe(
            "PaymentIntent amount does not match mayhem_au metadata".to_owned(),
        ));
    }
    let denom = object
        .metadata
        .get("mayhem_denom")
        .map(String::as_str)
        .unwrap_or(CREDIT_DENOM);
    if denom != CREDIT_DENOM {
        return Err(PaygateError::Stripe(
            "PaymentIntent denomination must be au_usd".to_owned(),
        ));
    }
    let metadata_currency = object.metadata.get("mayhem_fiat_currency").ok_or_else(|| {
        PaygateError::Stripe("PaymentIntent missing fiat currency metadata".to_owned())
    })?;
    if !metadata_currency.eq_ignore_ascii_case(&currency) {
        return Err(PaygateError::Stripe(
            "PaymentIntent fiat currency metadata mismatch".to_owned(),
        ));
    }
    let metadata_amount = object
        .metadata
        .get("mayhem_fiat_amount_minor")
        .ok_or_else(|| {
            PaygateError::Stripe("PaymentIntent missing fiat amount metadata".to_owned())
        })?;
    let metadata_amount = metadata_amount.parse::<u64>().map_err(|_| {
        PaygateError::Stripe("PaymentIntent fiat amount metadata is invalid".to_owned())
    })?;
    if metadata_amount != amount_cents {
        return Err(PaygateError::Stripe(
            "PaymentIntent fiat amount metadata mismatch".to_owned(),
        ));
    }
    let at = event.created.unwrap_or(unix_epoch_seconds()?);
    let epoch_seconds = state
        .contract
        .epoch_seconds_at(at, state.config.epoch_seconds)
        .await?;
    let ext_ref_hash = stripe_ext_ref_hash(&object.id);
    let charge = payment_intent_charge_id(&object);
    let feature = FiatDepositFeature {
        op: "fiat_deposit",
        rail: "stripe",
        who: who.clone(),
        au,
        ext_ref_hash: ext_ref_hash.clone(),
        fiat_currency: currency.clone(),
        fiat_amount_minor: amount_cents,
        epoch: epoch_for_at(at, epoch_seconds),
        at,
    };
    let contract = state
        .contract
        .post_fiat_deposit(&state.oracle, feature)
        .await?;
    Ok((
        StripeEventRecord {
            kind: "deposit".to_owned(),
            event_id: event.id.clone(),
            payment_intent: Some(object.id),
            charge,
            dispute: None,
            who,
            au,
            currency: Some(currency),
            amount_minor: Some(amount_cents),
            ext_ref_hash,
            dispute_ref_hash: None,
            credited_at: Some(at),
            disputed_at: None,
        },
        contract,
    ))
}

async fn handle_stripe_dispute_created(
    state: &PaygateState,
    event: &StripeEventEnvelope,
) -> Result<(StripeEventRecord, ContractPostResult)> {
    let object = &event.data.object;
    let dispute = json_string_field(object, "id")?;
    let amount_cents = json_u64_field(object, "amount")?;
    let currency = json_string_field(object, "currency")?;
    let currency = normalize_stripe_currency(Some(&currency))?;
    let charge = stripe_expandable_id(object.get("charge"));
    let payment_intent = stripe_expandable_id(object.get("payment_intent"));
    if charge.is_none() && payment_intent.is_none() {
        return Err(PaygateError::Stripe(
            "Dispute missing charge/payment_intent reference".to_owned(),
        ));
    }
    let deposit = {
        let store = state.stripe_events.lock().await;
        store.lookup_deposit(payment_intent.as_deref(), charge.as_deref())
    }
    .ok_or_else(|| PaygateError::Stripe("Dispute original deposit not found".to_owned()))?;
    if let Some(deposit_currency) = deposit.currency.as_deref() {
        if !deposit_currency.eq_ignore_ascii_case(&currency) {
            return Err(PaygateError::Stripe(
                "Dispute currency does not match original deposit".to_owned(),
            ));
        }
    }
    let au = MoneyAu::from(amount_cents)
        .checked_mul(AU_PER_USD_CENT)
        .ok_or_else(|| PaygateError::Stripe("Dispute amount overflow".to_owned()))?;
    if au == 0 {
        return Err(PaygateError::Stripe(
            "Dispute amount must be positive".to_owned(),
        ));
    }
    if au > deposit.au {
        return Err(PaygateError::Stripe(
            "Dispute amount exceeds original deposit".to_owned(),
        ));
    }
    let at = event
        .created
        .or_else(|| object.get("created").and_then(Value::as_u64))
        .unwrap_or(unix_epoch_seconds()?);
    let epoch_seconds = state
        .contract
        .epoch_seconds_at(at, state.config.epoch_seconds)
        .await?;
    let dispute_ref_hash = stripe_dispute_ref_hash(
        &dispute,
        charge
            .as_deref()
            .or(payment_intent.as_deref())
            .unwrap_or(""),
    );
    let feature = FiatChargebackFeature {
        op: "fiat_chargeback",
        rail: "stripe",
        who: deposit.who.clone(),
        au,
        ext_ref_hash: deposit.ext_ref_hash.clone(),
        dispute_ref_hash: dispute_ref_hash.clone(),
        fiat_currency: currency.clone(),
        fiat_amount_minor: amount_cents,
        epoch: epoch_for_at(at, epoch_seconds),
        at,
    };
    let contract = state
        .contract
        .post_fiat_chargeback(&state.oracle, feature)
        .await?;
    Ok((
        StripeEventRecord {
            kind: "chargeback".to_owned(),
            event_id: event.id.clone(),
            payment_intent: payment_intent.or(deposit.payment_intent),
            charge: charge.or(deposit.charge),
            dispute: Some(dispute),
            who: deposit.who,
            au,
            currency: Some(currency),
            amount_minor: Some(amount_cents),
            ext_ref_hash: deposit.ext_ref_hash,
            dispute_ref_hash: Some(dispute_ref_hash),
            credited_at: None,
            disputed_at: Some(at),
        },
        contract,
    ))
}

pub fn verify_stripe_signature(
    payload: &[u8],
    header: &str,
    secret: &str,
    now: u64,
    tolerance_seconds: u64,
) -> Result<()> {
    if tolerance_seconds == 0 {
        return Err(PaygateError::StripeSignature(
            "timestamp tolerance cannot be zero".to_owned(),
        ));
    }
    let (timestamp, signatures) = parse_stripe_signature_header(header)?;
    let age = now.abs_diff(timestamp);
    if age > tolerance_seconds {
        return Err(PaygateError::StripeSignature(
            "signature timestamp outside tolerance".to_owned(),
        ));
    }
    let mut signed_payload = timestamp.to_string().into_bytes();
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(payload);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|err| PaygateError::Crypto(err.to_string()))?;
    mac.update(&signed_payload);
    let expected = mac.finalize().into_bytes();
    for signature in signatures {
        let candidate = hex::decode(signature)?;
        if expected.as_slice().ct_eq(candidate.as_slice()).into() {
            return Ok(());
        }
    }
    Err(PaygateError::StripeSignature(
        "no matching v1 signature".to_owned(),
    ))
}

pub fn stripe_signature_header(secret: &str, payload: &[u8], timestamp: u64) -> Result<String> {
    let mut signed_payload = timestamp.to_string().into_bytes();
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(payload);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|err| PaygateError::Crypto(err.to_string()))?;
    mac.update(&signed_payload);
    let signature = hex::encode(mac.finalize().into_bytes());
    Ok(format!("t={timestamp},v1={signature}"))
}

fn parse_stripe_signature_header(header: &str) -> Result<(u64, Vec<&str>)> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        match key {
            "t" => {
                timestamp =
                    Some(value.parse::<u64>().map_err(|_| {
                        PaygateError::StripeSignature("invalid timestamp".to_owned())
                    })?);
            }
            "v1" => signatures.push(value),
            _ => {}
        }
    }
    let timestamp =
        timestamp.ok_or_else(|| PaygateError::StripeSignature("missing timestamp".to_owned()))?;
    if signatures.is_empty() {
        return Err(PaygateError::StripeSignature(
            "missing v1 signature".to_owned(),
        ));
    }
    Ok((timestamp, signatures))
}

impl StripeEventStore {
    fn memory() -> Self {
        Self {
            seen: HashSet::new(),
            processing: HashSet::new(),
            deposits_by_payment_intent: HashMap::new(),
            deposits_by_charge: HashMap::new(),
            path: None,
        }
    }

    fn load(path: &Path) -> Result<Self> {
        let mut store = Self {
            seen: HashSet::new(),
            processing: HashSet::new(),
            deposits_by_payment_intent: HashMap::new(),
            deposits_by_charge: HashMap::new(),
            path: Some(path.to_path_buf()),
        };
        if !path.exists() {
            return Ok(store);
        }
        let text = fs::read_to_string(path)?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: StripeEventRecord = serde_json::from_str(line)?;
            store.seen.insert(record.event_id.clone());
            store.index_record(record);
        }
        Ok(store)
    }

    fn begin(&mut self, event_id: &str) -> StripeEventBegin {
        if self.seen.contains(event_id) || self.processing.contains(event_id) {
            StripeEventBegin::Duplicate
        } else {
            self.processing.insert(event_id.to_owned());
            StripeEventBegin::Started
        }
    }

    fn complete(&mut self, record: StripeEventRecord) -> Result<()> {
        self.processing.remove(&record.event_id);
        if self.seen.insert(record.event_id.clone()) {
            if let Some(path) = &self.path {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        fs::create_dir_all(parent)?;
                    }
                }
                let mut file = OpenOptions::new().create(true).append(true).open(path)?;
                writeln!(file, "{}", serde_json::to_string(&record)?)?;
                file.flush()?;
            }
            self.index_record(record);
        }
        Ok(())
    }

    fn fail(&mut self, event_id: &str) {
        self.processing.remove(event_id);
    }

    fn index_record(&mut self, record: StripeEventRecord) {
        if record.kind != "deposit" {
            return;
        }
        if let Some(payment_intent) = &record.payment_intent {
            self.deposits_by_payment_intent
                .insert(payment_intent.clone(), record.clone());
        }
        if let Some(charge) = &record.charge {
            self.deposits_by_charge.insert(charge.clone(), record);
        }
    }

    fn lookup_deposit(
        &self,
        payment_intent: Option<&str>,
        charge: Option<&str>,
    ) -> Option<StripeEventRecord> {
        payment_intent
            .and_then(|id| self.deposits_by_payment_intent.get(id).cloned())
            .or_else(|| charge.and_then(|id| self.deposits_by_charge.get(id).cloned()))
    }
}

impl StripeBackfillCursor {
    fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                schema_version: 1,
                last_created: 0,
            });
        }
        let cursor: Self = serde_json::from_str(&fs::read_to_string(path)?)?;
        if cursor.schema_version != 1 {
            return Err(PaygateError::InvalidConfig(format!(
                "unsupported Stripe backfill cursor schema_version {}",
                cursor.schema_version
            )));
        }
        Ok(cursor)
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }
}

impl From<PaygateError> for ApiError {
    fn from(err: PaygateError) -> Self {
        let status = match err {
            PaygateError::InvalidConfig(_) => StatusCode::SERVICE_UNAVAILABLE,
            PaygateError::InvalidRequest(_)
            | PaygateError::StripeSignature(_)
            | PaygateError::Stripe(_) => StatusCode::BAD_REQUEST,
            PaygateError::Contract(_)
            | PaygateError::Io(_)
            | PaygateError::Http(_)
            | PaygateError::Toml(_)
            | PaygateError::Json(_)
            | PaygateError::Hex(_)
            | PaygateError::Crypto(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "ok": false,
                "error": self.message,
            })),
        )
            .into_response()
    }
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

fn parse_stripe_mode(field: &str, value: &str) -> Result<StripeMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "test" | "sandbox" => Ok(StripeMode::Test),
        "live" | "production" | "prod" => Ok(StripeMode::Live),
        _ => Err(PaygateError::InvalidConfig(format!(
            "{field} must be test or live"
        ))),
    }
}

fn parse_u64(field: &str, value: &str) -> Result<u64> {
    value
        .trim()
        .parse()
        .map_err(|err| PaygateError::InvalidConfig(format!("{field} is invalid: {err}")))
}

fn validate_stripe_secret_key_mode(secret_key: &str, mode: StripeMode) -> Result<()> {
    match mode {
        StripeMode::Test if secret_key.starts_with("sk_test_") => Ok(()),
        StripeMode::Live if secret_key.starts_with("sk_live_") => Ok(()),
        StripeMode::Test => Err(PaygateError::InvalidConfig(
            "Stripe test mode requires a sk_test_ secret key".to_owned(),
        )),
        StripeMode::Live => Err(PaygateError::InvalidConfig(
            "Stripe live mode requires a sk_live_ secret key".to_owned(),
        )),
    }
}

fn json_string_field(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| PaygateError::Stripe(format!("Stripe object missing {field}")))
}

fn json_u64_field(value: &Value, field: &str) -> Result<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| PaygateError::Stripe(format!("Stripe object missing {field}")))
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
    mayhem_home().join("paygate").join("oracle.seed")
}

fn default_stripe_event_store_path() -> PathBuf {
    mayhem_home().join("paygate").join("stripe-events.jsonl")
}

fn default_stripe_backfill_cursor_path() -> PathBuf {
    mayhem_home()
        .join("paygate")
        .join("stripe-backfill-cursor.json")
}

fn mayhem_home() -> PathBuf {
    env::var_os("MAYHEM_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".mayhem")))
        .unwrap_or_else(|| PathBuf::from(".mayhem"))
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

fn validate_safe_key_part(field: &str, value: &str) -> Result<()> {
    if is_safe_key_part(value) {
        Ok(())
    } else {
        Err(PaygateError::InvalidRequest(format!("{field} is invalid")))
    }
}

fn validate_checkout_url(field: &str, value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 2048
        || trimmed.bytes().any(|byte| byte.is_ascii_control())
        || !(trimmed.starts_with("https://")
            || trimmed.starts_with("http://127.0.0.1")
            || trimmed.starts_with("http://localhost"))
    {
        return Err(PaygateError::InvalidRequest(format!("{field} is invalid")));
    }
    Ok(())
}

fn validate_hosted_checkout_url(field: &str, value: &str, expected_host: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(value)
        .map_err(|_| PaygateError::InvalidRequest(format!("{field} is invalid")))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some(expected_host) {
        return Err(PaygateError::InvalidRequest(format!(
            "{field} must be an HTTPS URL on {expected_host}"
        )));
    }
    Ok(())
}

fn checkout_copy_paste(url: &str) -> CheckoutCopyPaste {
    CheckoutCopyPaste {
        checkout_url: url.to_owned(),
    }
}

fn normalize_stripe_currency(value: Option<&str>) -> Result<String> {
    let currency = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_STRIPE_CURRENCY)
        .to_ascii_lowercase();
    if !matches!(currency.as_str(), "usd" | "eur") {
        return Err(PaygateError::InvalidRequest(
            "Stripe currency must be usd or eur".to_owned(),
        ));
    }
    Ok(currency)
}

fn normalize_stripe_locale(value: Option<&str>) -> Result<String> {
    let locale = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_STRIPE_LOCALE)
        .to_ascii_lowercase();
    if locale != "en" {
        return Err(PaygateError::InvalidRequest(
            "Stripe checkout locale must be en".to_owned(),
        ));
    }
    Ok(locale)
}

fn is_safe_key_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn au_to_stripe_minor(au: MoneyAu, currency: &str) -> Result<u64> {
    if au == 0 {
        return Err(PaygateError::InvalidRequest(
            "au must be positive".to_owned(),
        ));
    }
    if au % AU_PER_USD_CENT != 0 {
        return Err(PaygateError::InvalidRequest(format!(
            "Stripe {currency} deposits must be whole cents"
        )));
    }
    let cents = u64::try_from(au / AU_PER_USD_CENT)
        .map_err(|_| PaygateError::InvalidRequest("Stripe amount exceeds u64 cents".to_owned()))?;
    if cents < STRIPE_MIN_USD_CENTS {
        return Err(PaygateError::InvalidRequest(
            "Stripe minimum deposit is 500000000000000000 au_usd".to_owned(),
        ));
    }
    Ok(cents)
}

fn stripe_ext_ref_hash(payment_intent: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mayhem-stripe-deposit-v2");
    hasher.update(payment_intent.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn stripe_dispute_ref_hash(dispute: &str, source: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mayhem-stripe-dispute-v2");
    hasher.update(dispute.as_bytes());
    hasher.update(b"\0");
    hasher.update(source.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn stripe_expandable_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(id) if !id.is_empty() => Some(id.to_owned()),
        Value::Object(object) => object
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

fn payment_intent_charge_id(object: &StripePaymentIntentObject) -> Option<String> {
    stripe_expandable_id(object.latest_charge.as_ref()).or_else(|| {
        object
            .charges
            .as_ref()
            .and_then(|charges| charges.get("data"))
            .and_then(Value::as_array)
            .and_then(|data| data.first())
            .and_then(|charge| stripe_expandable_id(Some(charge)))
    })
}

fn default_stripe_event_record_kind() -> String {
    "deposit".to_owned()
}

fn query_escape(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                out.push('%');
                out.push(char::from(HEX[(byte >> 4) as usize]));
                out.push(char::from(HEX[(byte & 0x0f) as usize]));
            }
        }
    }
    out
}

fn epoch_for_at(at: u64, epoch_seconds: u64) -> u64 {
    at / epoch_seconds + 1
}

fn unix_epoch_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| PaygateError::InvalidRequest(err.to_string()))?
        .as_secs())
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
            dry_run = true
            epoch_seconds = 7200

            [oracle]
            key_path = "/tmp/mayhem-paygate-test.seed"

            [stripe]
            enabled = true
            mode = "test"
            secret_key = "sk_test_local"
            webhook_secret = "whsec_local"
            api_base_url = "http://127.0.0.1:19999"
            event_store_path = "/tmp/mayhem-paygate-stripe-events.jsonl"

            "#,
        )?;

        assert_eq!(config.bind, "127.0.0.1:19091".parse().unwrap());
        assert_eq!(config.contract_rpc_url, "http://127.0.0.1:49299/v1");
        assert!(config.contract_dry_run);
        assert_eq!(config.epoch_seconds, 7200);
        assert_eq!(
            config.oracle_key_path,
            PathBuf::from("/tmp/mayhem-paygate-test.seed")
        );
        assert!(config.rails.stripe.enabled);
        assert_eq!(config.rails.stripe.mode, StripeMode::Test);
        assert_eq!(
            config.rails.stripe.secret_key.as_deref(),
            Some("sk_test_local")
        );
        assert_eq!(
            config.rails.stripe.webhook_secret.as_deref(),
            Some("whsec_local")
        );
        Ok(())
    }

    #[test]
    fn stripe_mode_rejects_wrong_secret_key_prefix() {
        let test_err = PaygateConfig::from_toml_str(
            r#"
            [contract]
            dry_run = true

            [stripe]
            enabled = true
            mode = "test"
            secret_key = "sk_live_wrong"
            webhook_secret = "whsec_local"
            "#,
        )
        .expect_err("live key rejected in test mode");
        assert!(test_err
            .to_string()
            .contains("test mode requires a sk_test_"));

        let live_err = PaygateConfig::from_toml_str(
            r#"
            [contract]
            rpc_url = "https://contract.testnet.trac.network/v1"

            [stripe]
            enabled = true
            mode = "live"
            secret_key = "sk_test_wrong"
            webhook_secret = "whsec_local"
            "#,
        )
        .expect_err("test key rejected in live mode");
        assert!(live_err
            .to_string()
            .contains("live mode requires a sk_live_"));
    }

    #[test]
    fn stripe_live_mode_rejects_dry_run_and_unofficial_api_base() {
        let dry_run = PaygateConfig::from_toml_str(
            r#"
            [contract]
            rpc_url = "https://contract.testnet.trac.network/v1"
            dry_run = true

            [stripe]
            enabled = true
            mode = "live"
            secret_key = "sk_live_local"
            webhook_secret = "whsec_local"
            "#,
        )
        .expect_err("live Stripe refuses contract dry-run");
        assert!(dry_run.to_string().contains("dry_run is forbidden"));

        let local = PaygateConfig::from_toml_str(
            r#"
            [stripe]
            enabled = true
            mode = "live"
            secret_key = "sk_live_local"
            webhook_secret = "whsec_local"
            "#,
        )
        .expect("live Stripe may use the loopback peer after the service mainnet proof");
        assert_eq!(local.contract_rpc_url, DEFAULT_CONTRACT_RPC_URL);

        let api_base = PaygateConfig::from_toml_str(
            r#"
            [contract]
            rpc_url = "https://contract.testnet.trac.network/v1"

            [stripe]
            enabled = true
            mode = "live"
            secret_key = "sk_live_local"
            webhook_secret = "whsec_local"
            api_base_url = "http://127.0.0.1:19999"
            "#,
        )
        .expect_err("live Stripe refuses alternate API base");
        assert!(api_base
            .to_string()
            .contains("official Stripe API base URL"));
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

    #[test]
    fn stripe_signature_verification_accepts_only_fresh_v1_hmac() -> Result<()> {
        let payload = br#"{"id":"evt_test"}"#;
        let header = stripe_signature_header("whsec_test", payload, 1000)?;
        verify_stripe_signature(payload, &header, "whsec_test", 1100, 300)?;
        let bad = verify_stripe_signature(payload, &header, "wrong", 1100, 300)
            .expect_err("wrong secret rejected");
        assert!(bad.to_string().contains("no matching"));
        let old = verify_stripe_signature(payload, &header, "whsec_test", 1401, 300)
            .expect_err("old timestamp rejected");
        assert!(old.to_string().contains("outside tolerance"));
        Ok(())
    }

    #[test]
    fn stripe_au_to_cents_requires_cent_aligned_minimum() {
        assert_eq!(
            au_to_stripe_minor(500_000_000_000_000_000, "usd").unwrap(),
            50
        );
        assert_eq!(
            au_to_stripe_minor(500_000_000_000_000_000, "eur").unwrap(),
            50
        );
        assert!(au_to_stripe_minor(499_999_999_999_999_999, "usd").is_err());
        assert!(au_to_stripe_minor(10_001, "eur").is_err());
        assert_eq!(normalize_stripe_currency(None).unwrap(), "usd");
        assert_eq!(normalize_stripe_currency(Some("EUR")).unwrap(), "eur");
        assert!(normalize_stripe_currency(Some("gbp")).is_err());
        assert_eq!(normalize_stripe_locale(None).unwrap(), "en");
        assert!(normalize_stripe_locale(Some("de")).is_err());
    }

    #[test]
    fn hosted_checkout_urls_are_exact_supported_https_hosts() {
        let stripe = stripe_checkout_session_summary(json!({
            "id": "cs_test",
            "url": "https://checkout.stripe.com/c/pay/cs_test"
        }))
        .expect("hosted Stripe checkout URL accepted");
        assert_eq!(stripe.url, "https://checkout.stripe.com/c/pay/cs_test");

        assert!(stripe_checkout_session_summary(json!({
            "id": "cs_test",
            "url": "https://checkout.stripe.com.evil.example/c/pay/cs_test"
        }))
        .is_err());
    }
}
