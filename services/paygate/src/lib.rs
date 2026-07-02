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
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ed25519_dalek::{Signer, SigningKey};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{net::TcpListener, sync::Mutex};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const SERVICE_NAME: &str = "mayhem-paygate";
pub const SERVICE_VERSION: u32 = 1;
pub const CREDIT_DENOM: &str = "mu_usd";
pub const DEFAULT_BIND: &str = "127.0.0.1:11436";
pub const DEFAULT_CONTRACT_RPC_URL: &str = "http://127.0.0.1:49223/v1";
pub const DEFAULT_STRIPE_API_BASE_URL: &str = "https://api.stripe.com";
pub const DEFAULT_STRIPE_WEBHOOK_TOLERANCE_SECONDS: u64 = 300;
pub const DEFAULT_EPOCH_SECONDS: u64 = 3_600;
pub const MU_PER_USD_CENT: u64 = 10_000;
pub const STRIPE_MIN_USD_CENTS: u64 = 50;

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
    pub contract_simulate: bool,
    pub epoch_seconds: u64,
    pub oracle_key_path: PathBuf,
    pub rails: RailConfig,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RailConfig {
    pub stripe: StripeSettings,
    pub coinbase: RailSettings,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RailSettings {
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StripeSettings {
    pub enabled: bool,
    pub secret_key: Option<String>,
    pub webhook_secret: Option<String>,
    pub api_base_url: String,
    pub event_store_path: PathBuf,
    pub webhook_tolerance_seconds: u64,
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
    simulate: Option<bool>,
    epoch_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct OracleConfigFile {
    key_path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct StripeConfigFile {
    enabled: Option<bool>,
    secret_key: Option<String>,
    webhook_secret: Option<String>,
    api_base_url: Option<String>,
    event_store_path: Option<PathBuf>,
    webhook_tolerance_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct RailConfigFile {
    enabled: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct OracleKeypair {
    signing_key: SigningKey,
}

pub trait ContractPoster: Send + Sync {
    fn post_fiat_deposit<'a>(
        &'a self,
        oracle: &'a OracleKeypair,
        feature: FiatDepositFeature,
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
    simulate: bool,
}

#[derive(Debug, Serialize)]
struct HealthRails {
    stripe: HealthStripeRail,
    coinbase: RailSettings,
}

#[derive(Debug, Serialize)]
struct HealthStripeRail {
    enabled: bool,
    api_configured: bool,
    webhook_configured: bool,
}

#[derive(Debug, Serialize)]
struct HealthControls {
    admin_sets_terms: bool,
    providers_set_prices: bool,
    providers_submit_models: bool,
}

#[derive(Debug, Deserialize)]
pub struct StripeCreatePaymentIntentRequest {
    pub who: String,
    pub mu: u64,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StripeCreatePaymentIntentResponse {
    pub ok: bool,
    pub rail: &'static str,
    pub denom: &'static str,
    pub who: String,
    pub mu: u64,
    pub payment_intent: StripePaymentIntentSummary,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripePaymentIntentSummary {
    pub id: String,
    pub client_secret: Option<String>,
    pub amount: u64,
    pub currency: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FiatDepositFeature {
    pub op: &'static str,
    pub rail: &'static str,
    pub who: String,
    pub mu: u64,
    pub ext_ref_hash: String,
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
    currency: String,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
struct StripeWebhookResponse {
    ok: bool,
    event_id: String,
    duplicate: bool,
    credited: bool,
    ignored: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    payment_intent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mu: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract: Option<ContractPostResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StripeEventRecord {
    event_id: String,
    payment_intent: String,
    who: String,
    mu: u64,
    ext_ref_hash: String,
    credited_at: u64,
}

#[derive(Debug)]
struct StripeEventStore {
    seen: HashSet<String>,
    processing: HashSet<String>,
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
    simulate: bool,
    http: reqwest::Client,
}

impl Default for PaygateConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND
                .parse()
                .expect("default paygate bind address is valid"),
            contract_rpc_url: DEFAULT_CONTRACT_RPC_URL.to_owned(),
            contract_simulate: false,
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
            secret_key: None,
            webhook_secret: None,
            api_base_url: DEFAULT_STRIPE_API_BASE_URL.to_owned(),
            event_store_path: default_stripe_event_store_path(),
            webhook_tolerance_seconds: DEFAULT_STRIPE_WEBHOOK_TOLERANCE_SECONDS,
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
            if self
                .rails
                .stripe
                .secret_key
                .as_deref()
                .unwrap_or("")
                .is_empty()
            {
                return Err(PaygateError::InvalidConfig(
                    "stripe.secret_key is required when Stripe is enabled".to_owned(),
                ));
            }
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
        if let Some(simulate) = file.contract.simulate {
            self.contract_simulate = simulate;
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
        if let Ok(simulate) = env::var("MAYHEM_PAYGATE_CONTRACT_SIM") {
            self.contract_simulate = parse_bool("MAYHEM_PAYGATE_CONTRACT_SIM", &simulate)?;
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
            config.contract_simulate,
            http.clone(),
        ));
        Self::with_parts(config, oracle, http, StripeEventStore::memory(), contract)
    }

    pub fn try_new(config: PaygateConfig, oracle: OracleKeypair) -> Result<Self> {
        let http = reqwest::Client::new();
        let contract = Arc::new(PeerRpcContractPoster::new(
            config.contract_rpc_url.clone(),
            config.contract_simulate,
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
                simulate: self.config.contract_simulate,
            },
            rails: HealthRails {
                stripe: HealthStripeRail {
                    enabled: self.config.rails.stripe.enabled,
                    api_configured: self.config.rails.stripe.secret_key.is_some(),
                    webhook_configured: self.config.rails.stripe.webhook_secret.is_some(),
                },
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

impl PeerRpcContractPoster {
    pub fn new(rpc_url: String, simulate: bool, http: reqwest::Client) -> Self {
        Self {
            rpc_url,
            simulate,
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
}

impl ContractPoster for PeerRpcContractPoster {
    fn post_fiat_deposit<'a>(
        &'a self,
        oracle: &'a OracleKeypair,
        feature: FiatDepositFeature,
    ) -> BoxFuture<'a, Result<ContractPostResult>> {
        Box::pin(async move {
            let nonce_response: Value = self
                .http
                .get(self.endpoint("contract/nonce"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let nonce = nonce_response
                .get("nonce")
                .and_then(Value::as_str)
                .ok_or_else(|| PaygateError::Contract("nonce response missing nonce".to_owned()))?;
            let prepared_command = json!({
                "type": "fiatDeposit",
                "value": feature,
            });
            let prepared: Value = self
                .http
                .post(self.endpoint("contract/tx/prepare"))
                .json(&json!({
                    "prepared_command": prepared_command,
                    "address": oracle.public_key_hex(),
                    "nonce": nonce,
                }))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let tx = prepared
                .get("tx")
                .and_then(Value::as_str)
                .ok_or_else(|| PaygateError::Contract("prepare response missing tx".to_owned()))?
                .to_owned();
            let command_hash = prepared
                .get("command_hash")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let signature = oracle.sign_tx_hex(&tx)?;
            let submitted: Value = self
                .http
                .post(self.endpoint("contract/tx"))
                .json(&json!({
                    "tx": tx,
                    "prepared_command": prepared_command,
                    "address": oracle.public_key_hex(),
                    "signature": signature,
                    "nonce": nonce,
                    "sim": self.simulate,
                }))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let result = submitted
                .get("result")
                .cloned()
                .unwrap_or_else(|| submitted.clone());
            let accepted = result.get("ok").and_then(Value::as_bool) == Some(true)
                || (result.get("local").and_then(Value::as_bool) == Some(true)
                    && result.get("txo").is_some());
            if !accepted {
                return Err(PaygateError::Contract(result.to_string()));
            }
            Ok(ContractPostResult {
                tx,
                command_hash,
                result,
            })
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
        .route("/stripe/webhook", post(stripe_webhook))
        .route("/v1/stripe/webhook", post(stripe_webhook))
        .with_state(Arc::new(state))
}

pub async fn serve(bind: SocketAddr, state: PaygateState) -> std::io::Result<()> {
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
            "Stripe rail is not enabled".to_owned(),
        ));
    }
    validate_safe_key_part("who", &request.who)?;
    let amount_cents = mu_to_usd_cents(request.mu)?;
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;
    let intent =
        stripe_create_payment_intent(&state.http, stripe, secret_key, &request, amount_cents)
            .await?;
    Ok(StripeCreatePaymentIntentResponse {
        ok: true,
        rail: "stripe",
        denom: CREDIT_DENOM,
        who: request.who,
        mu: request.mu,
        payment_intent: intent,
    })
}

async fn stripe_create_payment_intent(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    request: &StripeCreatePaymentIntentRequest,
    amount_cents: u64,
) -> Result<StripePaymentIntentSummary> {
    let form = [
        ("amount".to_owned(), amount_cents.to_string()),
        ("currency".to_owned(), "usd".to_owned()),
        (
            "automatic_payment_methods[enabled]".to_owned(),
            "true".to_owned(),
        ),
        ("metadata[mayhem_who]".to_owned(), request.who.to_owned()),
        ("metadata[mayhem_mu]".to_owned(), request.mu.to_string()),
        ("metadata[mayhem_denom]".to_owned(), CREDIT_DENOM.to_owned()),
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

async fn handle_stripe_webhook(
    state: &PaygateState,
    headers: &HeaderMap,
    payload: &[u8],
) -> Result<StripeWebhookResponse> {
    let stripe = &state.config.rails.stripe;
    if !stripe.enabled {
        return Err(PaygateError::InvalidRequest(
            "Stripe rail is not enabled".to_owned(),
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
    if event.event_type != "payment_intent.succeeded" {
        return Ok(StripeWebhookResponse {
            ok: true,
            event_id: event.id,
            duplicate: false,
            credited: false,
            ignored: true,
            payment_intent: None,
            mu: None,
            contract: None,
        });
    }

    {
        let mut store = state.stripe_events.lock().await;
        if matches!(store.begin(&event.id), StripeEventBegin::Duplicate) {
            return Ok(StripeWebhookResponse {
                ok: true,
                event_id: event.id,
                duplicate: true,
                credited: false,
                ignored: false,
                payment_intent: None,
                mu: None,
                contract: None,
            });
        }
    }

    let result = handle_stripe_payment_intent_succeeded(state, &event).await;
    match result {
        Ok((record, contract)) => {
            let mut store = state.stripe_events.lock().await;
            store.complete(record.clone())?;
            Ok(StripeWebhookResponse {
                ok: true,
                event_id: record.event_id,
                duplicate: false,
                credited: true,
                ignored: false,
                payment_intent: Some(record.payment_intent),
                mu: Some(record.mu),
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

async fn handle_stripe_payment_intent_succeeded(
    state: &PaygateState,
    event: &StripeEventEnvelope,
) -> Result<(StripeEventRecord, ContractPostResult)> {
    let object: StripePaymentIntentObject = serde_json::from_value(event.data.object.clone())?;
    if !object.currency.eq_ignore_ascii_case("usd") {
        return Err(PaygateError::Stripe(
            "PaymentIntent currency must be usd".to_owned(),
        ));
    }
    let amount_cents = object
        .amount_received
        .or(object.amount)
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent missing amount".to_owned()))?;
    let mu_from_amount = amount_cents
        .checked_mul(MU_PER_USD_CENT)
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent amount overflow".to_owned()))?;
    let who = object
        .metadata
        .get("mayhem_who")
        .ok_or_else(|| {
            PaygateError::Stripe("PaymentIntent missing mayhem_who metadata".to_owned())
        })?
        .to_owned();
    validate_safe_key_part("mayhem_who", &who)?;
    let mu = object
        .metadata
        .get("mayhem_mu")
        .ok_or_else(|| PaygateError::Stripe("PaymentIntent missing mayhem_mu metadata".to_owned()))?
        .parse::<u64>()
        .map_err(|_| PaygateError::Stripe("PaymentIntent mayhem_mu is invalid".to_owned()))?;
    if mu != mu_from_amount {
        return Err(PaygateError::Stripe(
            "PaymentIntent amount does not match mayhem_mu metadata".to_owned(),
        ));
    }
    let denom = object
        .metadata
        .get("mayhem_denom")
        .map(String::as_str)
        .unwrap_or(CREDIT_DENOM);
    if denom != CREDIT_DENOM {
        return Err(PaygateError::Stripe(
            "PaymentIntent denomination must be mu_usd".to_owned(),
        ));
    }
    let at = event.created.unwrap_or(unix_epoch_seconds()?);
    let ext_ref_hash = stripe_ext_ref_hash(&event.id, &object.id);
    let feature = FiatDepositFeature {
        op: "fiat_deposit",
        rail: "stripe",
        who: who.clone(),
        mu,
        ext_ref_hash: ext_ref_hash.clone(),
        epoch: epoch_for_at(at, state.config.epoch_seconds),
        at,
    };
    let contract = state
        .contract
        .post_fiat_deposit(&state.oracle, feature)
        .await?;
    Ok((
        StripeEventRecord {
            event_id: event.id.clone(),
            payment_intent: object.id,
            who,
            mu,
            ext_ref_hash,
            credited_at: at,
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
            path: None,
        }
    }

    fn load(path: &Path) -> Result<Self> {
        let mut store = Self {
            seen: HashSet::new(),
            processing: HashSet::new(),
            path: Some(path.to_path_buf()),
        };
        if !path.exists() {
            return Ok(store);
        }
        let text = fs::read_to_string(path)?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: StripeEventRecord = serde_json::from_str(line)?;
            store.seen.insert(record.event_id);
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
        }
        Ok(())
    }

    fn fail(&mut self, event_id: &str) {
        self.processing.remove(event_id);
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

fn parse_u64(field: &str, value: &str) -> Result<u64> {
    value
        .trim()
        .parse()
        .map_err(|err| PaygateError::InvalidConfig(format!("{field} is invalid: {err}")))
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

fn is_safe_key_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn mu_to_usd_cents(mu: u64) -> Result<u64> {
    if mu == 0 {
        return Err(PaygateError::InvalidRequest(
            "mu must be positive".to_owned(),
        ));
    }
    if mu % MU_PER_USD_CENT != 0 {
        return Err(PaygateError::InvalidRequest(
            "Stripe deposits must be whole USD cents".to_owned(),
        ));
    }
    let cents = mu / MU_PER_USD_CENT;
    if cents < STRIPE_MIN_USD_CENTS {
        return Err(PaygateError::InvalidRequest(
            "Stripe minimum deposit is 500000 mu_usd ($0.50)".to_owned(),
        ));
    }
    Ok(cents)
}

fn stripe_ext_ref_hash(event_id: &str, payment_intent: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mayhem-stripe-deposit-v1");
    hasher.update(event_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(payment_intent.as_bytes());
    hasher.finalize().to_hex().to_string()
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
            simulate = true
            epoch_seconds = 7200

            [oracle]
            key_path = "/tmp/mayhem-paygate-test.seed"

            [stripe]
            enabled = true
            secret_key = "sk_test_local"
            webhook_secret = "whsec_local"
            api_base_url = "http://127.0.0.1:19999"
            event_store_path = "/tmp/mayhem-paygate-stripe-events.jsonl"
            "#,
        )?;

        assert_eq!(config.bind, "127.0.0.1:19091".parse().unwrap());
        assert_eq!(config.contract_rpc_url, "http://127.0.0.1:49299/v1");
        assert!(config.contract_simulate);
        assert_eq!(config.epoch_seconds, 7200);
        assert_eq!(
            config.oracle_key_path,
            PathBuf::from("/tmp/mayhem-paygate-test.seed")
        );
        assert!(config.rails.stripe.enabled);
        assert_eq!(
            config.rails.stripe.secret_key.as_deref(),
            Some("sk_test_local")
        );
        assert_eq!(
            config.rails.stripe.webhook_secret.as_deref(),
            Some("whsec_local")
        );
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
    fn stripe_mu_to_cents_requires_cent_aligned_minimum() {
        assert_eq!(mu_to_usd_cents(500_000).unwrap(), 50);
        assert!(mu_to_usd_cents(499_999).is_err());
        assert!(mu_to_usd_cents(10_001).is_err());
    }
}
