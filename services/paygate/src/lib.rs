#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    body::{to_bytes, Body, Bytes},
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ed25519_dalek::{Signer, SigningKey};
use hmac::{Hmac, Mac};
use mayhem_proto::MoneyAu;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
pub const DEFAULT_STRIPE_CONNECT_RETURN_URL: &str = "https://dashboard.stripe.com/";
pub const DEFAULT_STRIPE_CONNECT_OAUTH_AUTHORIZE_URL: &str =
    "https://connect.stripe.com/oauth/authorize";
pub const DEFAULT_STRIPE_CONNECT_OAUTH_TOKEN_URL: &str = "https://connect.stripe.com/oauth/token";
pub const DEFAULT_STRIPE_CONNECT_CONSENT_TTL_SECONDS: u64 = 300;
pub const DEFAULT_STRIPE_INTERNAL_AUTH_TOLERANCE_SECONDS: u64 = 30;
pub const STRIPE_INTERNAL_AUTH_MAX_BODY_BYTES: usize = 1_000_000;
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
    pub connect_account_type: StripeConnectAccountType,
    pub connect_accounts_path: PathBuf,
    pub connect_return_url: String,
    pub connect_refresh_url: String,
    pub connect_oauth_client_id: Option<String>,
    pub connect_oauth_redirect_url: Option<String>,
    pub connect_oauth_token_url: String,
    pub connect_consents_path: PathBuf,
    pub connect_consent_ttl_seconds: u64,
    pub internal_auth_secret_path: PathBuf,
    pub internal_auth_tolerance_seconds: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StripeMode {
    #[default]
    Test,
    Live,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StripeConnectAccountType {
    #[default]
    Express,
    Custom,
    Standard,
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
    connect_account_type: Option<String>,
    connect_accounts_path: Option<PathBuf>,
    connect_return_url: Option<String>,
    connect_refresh_url: Option<String>,
    connect_oauth_client_id: Option<String>,
    connect_oauth_redirect_url: Option<String>,
    connect_oauth_token_url: Option<String>,
    connect_consents_path: Option<PathBuf>,
    connect_consent_ttl_seconds: Option<u64>,
    internal_auth_secret_path: Option<PathBuf>,
    internal_auth_tolerance_seconds: Option<u64>,
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
    stripe_connect: Arc<Mutex<StripeConnectStore>>,
    stripe_connect_consents: Arc<Mutex<StripeConnectConsentStore>>,
    stripe_internal_auth_secret: Option<Arc<Vec<u8>>>,
    stripe_internal_auth_nonces: Arc<Mutex<StripeInternalAuthNonceStore>>,
    contract: Arc<dyn ContractPoster>,
}

#[derive(Debug, Deserialize, Serialize)]
struct StripeInternalAuthNonceFile {
    schema_version: u32,
    nonces: BTreeMap<String, u64>,
}

#[derive(Debug)]
struct StripeInternalAuthNonceStore {
    path: Option<PathBuf>,
    nonces: BTreeMap<String, u64>,
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
    connect_account_type: &'static str,
    connect_configured: bool,
}

#[derive(Debug, Serialize)]
struct HealthControls {
    admin_controls_economy: bool,
    admin_sets_terms: bool,
    admin_sets_prices: bool,
    admin_sets_rules: bool,
    admin_sets_params: bool,
    admin_can_ban_providers: bool,
    providers_set_prices: bool,
    providers_set_rules: bool,
    providers_set_params: bool,
    providers_set_payout_terms: bool,
    providers_submit_models: bool,
    providers_create_canonical_rooms: bool,
    providers_only_join_admin_rooms: bool,
    providers_bind_verified_payout_targets: bool,
    payout_liabilities_revision_bound: bool,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StripeConnectOnboardRequest {
    pub provider: String,
    pub country: String,
    pub request_nonce: String,
    #[serde(default)]
    pub rotate: bool,
    #[serde(default)]
    pub previous_account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StripeConnectStatusRequest {
    pub provider: String,
    pub request_nonce: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StripeConnectRelinkRequest {
    pub provider: String,
    pub source_provider: String,
    pub account_id: String,
    pub context_revision: String,
    pub country: String,
    pub request_nonce: String,
    pub consent_expires_at: u64,
    pub source_consent_signature: String,
    pub target_service_signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StripeConnectRelinkReturnQuery {
    state: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, rename = "error_description")]
    _error_description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripeConnectAccountSummary {
    pub id: String,
    pub account_type: String,
    pub country: String,
    pub default_currency: String,
    pub details_submitted: bool,
    pub charges_enabled: bool,
    pub payouts_enabled: bool,
    pub transfers_enabled: bool,
    pub ready: bool,
    pub currently_due: Vec<String>,
    pub eventually_due: Vec<String>,
    pub disabled_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripeConnectOnboardingSummary {
    pub url: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripeConnectResponse {
    pub ok: bool,
    pub rail: &'static str,
    pub processor_rail: &'static str,
    pub provider: String,
    pub mode: &'static str,
    pub account: StripeConnectAccountSummary,
    pub onboarding: Option<StripeConnectOnboardingSummary>,
    pub copy_paste: Option<StripeConnectCopyPaste>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripeConnectCopyPaste {
    pub onboarding_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StripeConnectRelinkResponse {
    pub ok: bool,
    pub rail: &'static str,
    pub processor_rail: &'static str,
    pub provider: String,
    pub source_provider: String,
    pub mode: &'static str,
    pub status: &'static str,
    pub account: Option<StripeConnectAccountSummary>,
    pub onboarding: Option<StripeConnectOnboardingSummary>,
    pub copy_paste: Option<StripeConnectCopyPaste>,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct StripeConnectAccountRecord {
    schema_version: u32,
    mode: StripeMode,
    provider: String,
    account_id: String,
    account_type: StripeConnectAccountType,
    country: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_account_id: Option<String>,
    created_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StripeConnectConsentRecord {
    schema_version: u32,
    mode: StripeMode,
    provider: String,
    source_provider: String,
    account_id: String,
    account_type: StripeConnectAccountType,
    context_revision: String,
    country: String,
    request_nonce: String,
    source_consent_signature: String,
    target_service_signature: String,
    state: String,
    created_at: u64,
    expires_at: u64,
    #[serde(default)]
    completed_at: Option<u64>,
}

#[derive(Clone, Debug)]
struct VerifiedStripeConnectAccount {
    summary: StripeConnectAccountSummary,
    owner_provider: String,
    livemode: bool,
    metadata_mode: StripeMode,
}

#[derive(Debug)]
struct StripeConnectStore {
    accounts: HashMap<String, StripeConnectAccountRecord>,
    bindings: HashMap<String, StripeConnectAccountRecord>,
    requests: HashMap<String, StripeConnectAccountRecord>,
    path: Option<PathBuf>,
}

#[derive(Debug)]
struct StripeConnectConsentStore {
    challenges: HashMap<String, StripeConnectConsentRecord>,
    requests: HashMap<String, String>,
    processing: HashSet<String>,
    path: Option<PathBuf>,
}

#[derive(Debug)]
enum StripeEventBegin {
    Started,
    Duplicate,
}

#[derive(Debug)]
enum StripeConnectConsentBegin {
    Started(StripeConnectConsentRecord),
    Completed,
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
            connect_account_type: StripeConnectAccountType::Express,
            connect_accounts_path: default_stripe_connect_accounts_path(),
            connect_return_url: DEFAULT_STRIPE_CONNECT_RETURN_URL.to_owned(),
            connect_refresh_url: DEFAULT_STRIPE_CONNECT_RETURN_URL.to_owned(),
            connect_oauth_client_id: None,
            connect_oauth_redirect_url: None,
            connect_oauth_token_url: DEFAULT_STRIPE_CONNECT_OAUTH_TOKEN_URL.to_owned(),
            connect_consents_path: default_stripe_connect_consents_path(),
            connect_consent_ttl_seconds: DEFAULT_STRIPE_CONNECT_CONSENT_TTL_SECONDS,
            internal_auth_secret_path: default_stripe_internal_auth_secret_path(),
            internal_auth_tolerance_seconds: DEFAULT_STRIPE_INTERNAL_AUTH_TOLERANCE_SECONDS,
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

impl StripeConnectAccountType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Express => "express",
            Self::Custom => "custom",
            Self::Standard => "standard",
        }
    }
}

impl PaygateConfig {
    pub fn from_sources(config_path: Option<&Path>) -> Result<Self> {
        let mut config = Self::default();
        let mut stripe_mode_explicit = false;
        if let Some(path) = config_path {
            stripe_mode_explicit = config.apply_toml_file(path)?;
        }
        stripe_mode_explicit |= config.apply_env()?;
        config.require_explicit_stripe_mode(stripe_mode_explicit)?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self> {
        let mut config = Self::default();
        let file: ConfigFile = toml::from_str(input)?;
        let stripe_mode_explicit = file.stripe.mode.is_some();
        config.apply_file(file)?;
        config.require_explicit_stripe_mode(stripe_mode_explicit)?;
        config.validate()?;
        Ok(config)
    }

    fn require_explicit_stripe_mode(&self, mode_explicit: bool) -> Result<()> {
        if self.rails.stripe.enabled && !mode_explicit {
            return Err(PaygateError::InvalidConfig(
                "Stripe mode must be explicitly set to test or live when Stripe is enabled"
                    .to_owned(),
            ));
        }
        Ok(())
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
            if self
                .rails
                .stripe
                .connect_accounts_path
                .as_os_str()
                .is_empty()
            {
                return Err(PaygateError::InvalidConfig(
                    "stripe.connect_accounts_path cannot be empty".to_owned(),
                ));
            }
            if self
                .rails
                .stripe
                .connect_consents_path
                .as_os_str()
                .is_empty()
            {
                return Err(PaygateError::InvalidConfig(
                    "stripe.connect_consents_path cannot be empty".to_owned(),
                ));
            }
            if self.rails.stripe.connect_consent_ttl_seconds == 0 {
                return Err(PaygateError::InvalidConfig(
                    "stripe.connect_consent_ttl_seconds cannot be zero".to_owned(),
                ));
            }
            if self
                .rails
                .stripe
                .internal_auth_secret_path
                .as_os_str()
                .is_empty()
            {
                return Err(PaygateError::InvalidConfig(
                    "stripe.internal_auth_secret_path cannot be empty".to_owned(),
                ));
            }
            if self.rails.stripe.internal_auth_tolerance_seconds == 0
                || self.rails.stripe.internal_auth_tolerance_seconds > 300
            {
                return Err(PaygateError::InvalidConfig(
                    "stripe.internal_auth_tolerance_seconds must be between 1 and 300".to_owned(),
                ));
            }
            validate_connect_redirect_url(
                "stripe.connect_return_url",
                &self.rails.stripe.connect_return_url,
            )?;
            validate_connect_redirect_url(
                "stripe.connect_refresh_url",
                &self.rails.stripe.connect_refresh_url,
            )?;
            match (
                self.rails.stripe.connect_oauth_client_id.as_deref(),
                self.rails.stripe.connect_oauth_redirect_url.as_deref(),
            ) {
                (Some(client_id), Some(redirect_url)) => {
                    validate_stripe_connect_client_id(client_id)?;
                    validate_connect_redirect_url(
                        "stripe.connect_oauth_redirect_url",
                        redirect_url,
                    )?;
                }
                (None, None) => {}
                _ => {
                    return Err(PaygateError::InvalidConfig(
                        "stripe.connect_oauth_client_id and stripe.connect_oauth_redirect_url must be configured together"
                            .to_owned(),
                    ));
                }
            }
            validate_stripe_oauth_token_url(
                &self.rails.stripe.connect_oauth_token_url,
                self.rails.stripe.mode,
            )?;
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

    fn apply_toml_file(&mut self, path: &Path) -> Result<bool> {
        let text = fs::read_to_string(path)?;
        let file: ConfigFile = toml::from_str(&text)?;
        let stripe_mode_explicit = file.stripe.mode.is_some();
        self.apply_file(file)?;
        Ok(stripe_mode_explicit)
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
        if let Some(account_type) = file.stripe.connect_account_type {
            self.rails.stripe.connect_account_type =
                parse_stripe_connect_account_type("stripe.connect_account_type", &account_type)?;
        }
        if let Some(path) = file.stripe.connect_accounts_path {
            self.rails.stripe.connect_accounts_path = expand_home(path);
        }
        if let Some(url) = file.stripe.connect_return_url {
            self.rails.stripe.connect_return_url = url;
        }
        if let Some(url) = file.stripe.connect_refresh_url {
            self.rails.stripe.connect_refresh_url = url;
        }
        if let Some(client_id) = file.stripe.connect_oauth_client_id {
            self.rails.stripe.connect_oauth_client_id = Some(client_id);
        }
        if let Some(url) = file.stripe.connect_oauth_redirect_url {
            self.rails.stripe.connect_oauth_redirect_url = Some(url);
        }
        if let Some(url) = file.stripe.connect_oauth_token_url {
            self.rails.stripe.connect_oauth_token_url = url;
        }
        if let Some(path) = file.stripe.connect_consents_path {
            self.rails.stripe.connect_consents_path = expand_home(path);
        }
        if let Some(seconds) = file.stripe.connect_consent_ttl_seconds {
            self.rails.stripe.connect_consent_ttl_seconds = seconds;
        }
        if let Some(path) = file.stripe.internal_auth_secret_path {
            self.rails.stripe.internal_auth_secret_path = expand_home(path);
        }
        if let Some(seconds) = file.stripe.internal_auth_tolerance_seconds {
            self.rails.stripe.internal_auth_tolerance_seconds = seconds;
        }
        Ok(())
    }

    fn apply_env(&mut self) -> Result<bool> {
        let mut stripe_mode_explicit = false;
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
            stripe_mode_explicit = true;
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
        if let Ok(account_type) = env::var("MAYHEM_STRIPE_CONNECT_ACCOUNT_TYPE") {
            self.rails.stripe.connect_account_type = parse_stripe_connect_account_type(
                "MAYHEM_STRIPE_CONNECT_ACCOUNT_TYPE",
                &account_type,
            )?;
        }
        if let Ok(path) = env::var("MAYHEM_STRIPE_CONNECT_ACCOUNTS_PATH") {
            self.rails.stripe.connect_accounts_path = expand_home(PathBuf::from(path));
        }
        if let Ok(url) = env::var("MAYHEM_STRIPE_CONNECT_RETURN_URL") {
            self.rails.stripe.connect_return_url = url;
        }
        if let Ok(url) = env::var("MAYHEM_STRIPE_CONNECT_REFRESH_URL") {
            self.rails.stripe.connect_refresh_url = url;
        }
        if let Ok(client_id) = env::var("MAYHEM_STRIPE_CONNECT_OAUTH_CLIENT_ID") {
            self.rails.stripe.connect_oauth_client_id = Some(client_id);
        }
        if let Ok(url) = env::var("MAYHEM_STRIPE_CONNECT_OAUTH_REDIRECT_URL") {
            self.rails.stripe.connect_oauth_redirect_url = Some(url);
        }
        if let Ok(url) = env::var("MAYHEM_STRIPE_CONNECT_OAUTH_TOKEN_URL") {
            self.rails.stripe.connect_oauth_token_url = url;
        }
        if let Ok(path) = env::var("MAYHEM_STRIPE_CONNECT_CONSENTS_PATH") {
            self.rails.stripe.connect_consents_path = expand_home(PathBuf::from(path));
        }
        if let Ok(seconds) = env::var("MAYHEM_STRIPE_CONNECT_CONSENT_TTL_SECONDS") {
            self.rails.stripe.connect_consent_ttl_seconds =
                parse_u64("MAYHEM_STRIPE_CONNECT_CONSENT_TTL_SECONDS", &seconds)?;
        }
        if let Ok(path) = env::var("MAYHEM_PAYGATE_INTERNAL_AUTH_SECRET_FILE") {
            self.rails.stripe.internal_auth_secret_path = expand_home(PathBuf::from(path));
        }
        if let Ok(seconds) = env::var("MAYHEM_PAYGATE_INTERNAL_AUTH_TOLERANCE_SECONDS") {
            self.rails.stripe.internal_auth_tolerance_seconds =
                parse_u64("MAYHEM_PAYGATE_INTERNAL_AUTH_TOLERANCE_SECONDS", &seconds)?;
        }
        Ok(stripe_mode_explicit)
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
        let (stripe_internal_auth_secret, stripe_internal_auth_nonces) =
            if config.rails.stripe.enabled {
                match (
                    load_stripe_internal_auth_secret(
                        &config.rails.stripe.internal_auth_secret_path,
                    ),
                    StripeInternalAuthNonceStore::load(
                        &config.rails.stripe.internal_auth_secret_path,
                        config.rails.stripe.internal_auth_tolerance_seconds,
                    ),
                ) {
                    (Ok(secret), Ok(nonces)) => (Some(secret), nonces),
                    _ => (None, StripeInternalAuthNonceStore::memory()),
                }
            } else {
                (None, StripeInternalAuthNonceStore::memory())
            };
        Self::with_parts(
            stripe_internal_auth_secret,
            stripe_internal_auth_nonces,
            config,
            oracle,
            http,
            StripeEventStore::memory(),
            StripeConnectStore::memory(),
            StripeConnectConsentStore::memory(),
            contract,
        )
    }

    pub fn try_new(config: PaygateConfig, oracle: OracleKeypair) -> Result<Self> {
        config.validate()?;
        let stripe_internal_auth_secret = if config.rails.stripe.enabled {
            Some(load_stripe_internal_auth_secret(
                &config.rails.stripe.internal_auth_secret_path,
            )?)
        } else {
            None
        };
        let stripe_internal_auth_nonces = if config.rails.stripe.enabled {
            StripeInternalAuthNonceStore::load(
                &config.rails.stripe.internal_auth_secret_path,
                config.rails.stripe.internal_auth_tolerance_seconds,
            )?
        } else {
            StripeInternalAuthNonceStore::memory()
        };
        let http = reqwest::Client::new();
        let contract = Arc::new(PeerRpcContractPoster::new(
            config.contract_rpc_url.clone(),
            config.contract_dry_run,
            http.clone(),
        ));
        let stripe_events = StripeEventStore::load(&config.rails.stripe.event_store_path)?;
        let stripe_connect = StripeConnectStore::load(&config.rails.stripe.connect_accounts_path)?;
        let stripe_connect_consents =
            StripeConnectConsentStore::load(&config.rails.stripe.connect_consents_path)?;
        Ok(Self::with_parts(
            stripe_internal_auth_secret,
            stripe_internal_auth_nonces,
            config,
            oracle,
            http,
            stripe_events,
            stripe_connect,
            stripe_connect_consents,
            contract,
        ))
    }

    pub fn try_new_with_contract_poster(
        config: PaygateConfig,
        oracle: OracleKeypair,
        contract: Arc<dyn ContractPoster>,
    ) -> Result<Self> {
        config.validate()?;
        let stripe_internal_auth_secret = if config.rails.stripe.enabled {
            Some(load_stripe_internal_auth_secret(
                &config.rails.stripe.internal_auth_secret_path,
            )?)
        } else {
            None
        };
        let stripe_internal_auth_nonces = if config.rails.stripe.enabled {
            StripeInternalAuthNonceStore::load(
                &config.rails.stripe.internal_auth_secret_path,
                config.rails.stripe.internal_auth_tolerance_seconds,
            )?
        } else {
            StripeInternalAuthNonceStore::memory()
        };
        let http = reqwest::Client::new();
        let stripe_events = StripeEventStore::load(&config.rails.stripe.event_store_path)?;
        let stripe_connect = StripeConnectStore::load(&config.rails.stripe.connect_accounts_path)?;
        let stripe_connect_consents =
            StripeConnectConsentStore::load(&config.rails.stripe.connect_consents_path)?;
        Ok(Self::with_parts(
            stripe_internal_auth_secret,
            stripe_internal_auth_nonces,
            config,
            oracle,
            http,
            stripe_events,
            stripe_connect,
            stripe_connect_consents,
            contract,
        ))
    }

    fn with_parts(
        stripe_internal_auth_secret: Option<Vec<u8>>,
        stripe_internal_auth_nonces: StripeInternalAuthNonceStore,
        config: PaygateConfig,
        oracle: OracleKeypair,
        http: reqwest::Client,
        stripe_events: StripeEventStore,
        stripe_connect: StripeConnectStore,
        stripe_connect_consents: StripeConnectConsentStore,
        contract: Arc<dyn ContractPoster>,
    ) -> Self {
        let oracle_public_key = oracle.public_key_hex();
        Self {
            config: Arc::new(config),
            oracle,
            oracle_public_key,
            http,
            stripe_events: Arc::new(Mutex::new(stripe_events)),
            stripe_connect: Arc::new(Mutex::new(stripe_connect)),
            stripe_connect_consents: Arc::new(Mutex::new(stripe_connect_consents)),
            stripe_internal_auth_secret: stripe_internal_auth_secret.map(Arc::new),
            stripe_internal_auth_nonces: Arc::new(Mutex::new(stripe_internal_auth_nonces)),
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
                    connect_account_type: self.config.rails.stripe.connect_account_type.as_str(),
                    connect_configured: !self
                        .config
                        .rails
                        .stripe
                        .connect_accounts_path
                        .as_os_str()
                        .is_empty(),
                },
            },
            controls: HealthControls {
                admin_controls_economy: true,
                admin_sets_terms: true,
                admin_sets_prices: true,
                admin_sets_rules: true,
                admin_sets_params: true,
                admin_can_ban_providers: true,
                providers_set_prices: false,
                providers_set_rules: false,
                providers_set_params: false,
                providers_set_payout_terms: false,
                providers_submit_models: false,
                providers_create_canonical_rooms: false,
                providers_only_join_admin_rooms: true,
                providers_bind_verified_payout_targets: true,
                payout_liabilities_revision_bound: true,
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

fn internal_stripe_auth_message(
    timestamp: u64,
    nonce: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> String {
    format!(
        "mayhem-paygate-internal-request-v1\n{timestamp}\n{nonce}\n{method}\n{path}\n{}",
        hex::encode(Sha256::digest(body))
    )
}

fn internal_auth_error(message: &'static str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": message }))).into_response()
}

async fn require_internal_stripe_auth(
    state: Arc<PaygateState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(secret) = state.stripe_internal_auth_secret.as_ref() else {
        return internal_auth_error("Stripe internal request authentication is unavailable");
    };
    let Some(timestamp_text) = request
        .headers()
        .get("x-mayhem-paygate-timestamp")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return internal_auth_error("Missing Stripe internal request authentication");
    };
    let Some(nonce) = request
        .headers()
        .get("x-mayhem-paygate-nonce")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return internal_auth_error("Missing Stripe internal request authentication");
    };
    let Some(signature) = request
        .headers()
        .get("x-mayhem-paygate-signature")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return internal_auth_error("Missing Stripe internal request authentication");
    };
    let Ok(timestamp) = timestamp_text.parse::<u64>() else {
        return internal_auth_error("Invalid Stripe internal request authentication");
    };
    if nonce.len() != 64
        || nonce != nonce.to_ascii_lowercase()
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || signature.len() != 64
        || signature != signature.to_ascii_lowercase()
        || !signature.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return internal_auth_error("Invalid Stripe internal request authentication");
    }
    let now = match unix_epoch_seconds() {
        Ok(now) => now,
        Err(_) => {
            return internal_auth_error("Stripe internal request authentication is unavailable")
        }
    };
    let tolerance = state.config.rails.stripe.internal_auth_tolerance_seconds;
    if timestamp > now.saturating_add(tolerance) || now > timestamp.saturating_add(tolerance) {
        return internal_auth_error("Stale Stripe internal request authentication");
    }
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, STRIPE_INTERNAL_AUTH_MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return internal_auth_error("Invalid Stripe internal request body"),
    };
    let message = internal_stripe_auth_message(timestamp, &nonce, &method, &path, &body);
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return internal_auth_error("Stripe internal request authentication is unavailable");
    };
    mac.update(message.as_bytes());
    let Ok(signature_bytes) = hex::decode(&signature) else {
        return internal_auth_error("Invalid Stripe internal request authentication");
    };
    if mac.verify_slice(&signature_bytes).is_err() {
        return internal_auth_error("Invalid Stripe internal request authentication");
    }
    {
        let mut nonces = state.stripe_internal_auth_nonces.lock().await;
        match nonces.remember(nonce, timestamp, now, tolerance) {
            Ok(true) => {}
            Ok(false) => {
                return internal_auth_error("Replayed Stripe internal request authentication")
            }
            Err(_) => {
                return internal_auth_error("Stripe internal request authentication is unavailable")
            }
        }
    }
    next.run(Request::from_parts(parts, Body::from(body))).await
}

pub fn paygate_router(state: PaygateState) -> Router {
    let state = Arc::new(state);
    let internal_stripe = Router::new()
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
        .route(
            "/stripe/connect/onboard",
            post(create_stripe_connect_onboarding),
        )
        .route(
            "/v1/stripe/connect/onboard",
            post(create_stripe_connect_onboarding),
        )
        .route("/stripe/connect/status", post(read_stripe_connect_status))
        .route(
            "/v1/stripe/connect/status",
            post(read_stripe_connect_status),
        )
        .route("/stripe/connect/relink", post(create_stripe_connect_relink))
        .route(
            "/v1/stripe/connect/relink",
            post(create_stripe_connect_relink),
        )
        .route_layer(middleware::from_fn({
            let state = state.clone();
            move |request: Request, next: Next| {
                let state = state.clone();
                async move { require_internal_stripe_auth(state, request, next).await }
            }
        }));
    Router::new()
        .route("/health", get(health))
        .route("/v1/health", get(health))
        .merge(internal_stripe)
        .route(
            "/stripe/connect/relink/return",
            get(stripe_connect_relink_return),
        )
        .route(
            "/v1/stripe/connect/relink/return",
            get(stripe_connect_relink_return),
        )
        .route("/stripe/return", get(stripe_return))
        .route("/v1/stripe/return", get(stripe_return))
        .route("/stripe/cancel", get(stripe_cancel))
        .route("/v1/stripe/cancel", get(stripe_cancel))
        .route("/stripe/webhook", post(stripe_webhook))
        .route("/v1/stripe/webhook", post(stripe_webhook))
        .with_state(state)
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

async fn create_stripe_connect_onboarding(
    State(state): State<Arc<PaygateState>>,
    Json(request): Json<StripeConnectOnboardRequest>,
) -> Response {
    match create_connect_onboarding(&state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn read_stripe_connect_status(
    State(state): State<Arc<PaygateState>>,
    Json(request): Json<StripeConnectStatusRequest>,
) -> Response {
    match read_connect_status(&state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn create_stripe_connect_relink(
    State(state): State<Arc<PaygateState>>,
    Json(request): Json<StripeConnectRelinkRequest>,
) -> Response {
    match create_connect_relink(&state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn stripe_connect_relink_return(
    State(state): State<Arc<PaygateState>>,
    Query(query): Query<StripeConnectRelinkReturnQuery>,
) -> Response {
    match complete_connect_relink(&state, query).await {
        Ok(()) => Html(
            "Stripe account consent verified. Return to the Mayhem CLI while payout activation completes.",
        )
        .into_response(),
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

async fn create_connect_onboarding(
    state: &PaygateState,
    request: StripeConnectOnboardRequest,
) -> Result<StripeConnectResponse> {
    let stripe = require_stripe(state)?;
    validate_provider_id(&request.provider)?;
    validate_connect_request_nonce(&request.request_nonce)?;
    let country = normalize_stripe_country(&request.country)?;
    if request.rotate {
        let previous_account_id = request.previous_account_id.as_deref().ok_or_else(|| {
            PaygateError::InvalidRequest(
                "Stripe Connect rotation requires previous_account_id".to_owned(),
            )
        })?;
        validate_stripe_account_id(previous_account_id)?;
    } else if request.previous_account_id.is_some() {
        return Err(PaygateError::InvalidRequest(
            "previous_account_id is valid only for Stripe Connect rotation".to_owned(),
        ));
    }
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;

    let record = {
        let mut store = state.stripe_connect.lock().await;
        if request.rotate {
            let previous_account_id = request.previous_account_id.as_deref().ok_or_else(|| {
                PaygateError::InvalidRequest(
                    "Stripe Connect rotation requires previous_account_id".to_owned(),
                )
            })?;
            if let Some(record) = store.replay_rotation(
                stripe.mode,
                &request.provider,
                &request.request_nonce,
                previous_account_id,
                &country,
            )? {
                record
            } else {
                let existing = store
                    .get(stripe.mode, &request.provider)
                    .cloned()
                    .ok_or_else(|| {
                        PaygateError::InvalidRequest(
                            "provider has no Stripe Connect account to rotate".to_owned(),
                        )
                    })?;
                if existing.account_id != previous_account_id {
                    return Err(PaygateError::InvalidRequest(
                        "Stripe Connect rotation previous account is stale".to_owned(),
                    ));
                }
                if existing.country != country {
                    return Err(PaygateError::InvalidRequest(format!(
                        "provider Stripe Connect account is registered in {}",
                        existing.country
                    )));
                }
                let account = stripe_create_connect_account(
                    &state.http,
                    stripe,
                    secret_key,
                    &request.provider,
                    &country,
                    Some(&request.request_nonce),
                )
                .await?;
                verify_new_connect_account(
                    &account,
                    stripe,
                    &request.provider,
                    stripe.connect_account_type,
                    &country,
                )?;
                let record = StripeConnectAccountRecord {
                    schema_version: 1,
                    mode: stripe.mode,
                    provider: request.provider.clone(),
                    account_id: account.summary.id,
                    account_type: stripe.connect_account_type,
                    country,
                    request_nonce: Some(request.request_nonce.clone()),
                    previous_account_id: Some(previous_account_id.to_owned()),
                    created_at: unix_epoch_seconds()?,
                };
                store.insert_rotated(record.clone(), previous_account_id)?;
                record
            }
        } else if let Some(record) = store.get(stripe.mode, &request.provider).cloned() {
            if record.country != country {
                return Err(PaygateError::InvalidRequest(format!(
                    "provider already has a Stripe Connect account in {}",
                    record.country
                )));
            }
            record
        } else {
            let account = stripe_create_connect_account(
                &state.http,
                stripe,
                secret_key,
                &request.provider,
                &country,
                None,
            )
            .await?;
            verify_new_connect_account(
                &account,
                stripe,
                &request.provider,
                stripe.connect_account_type,
                &country,
            )?;
            let record = StripeConnectAccountRecord {
                schema_version: 1,
                mode: stripe.mode,
                provider: request.provider.clone(),
                account_id: account.summary.id,
                account_type: stripe.connect_account_type,
                country,
                request_nonce: Some(request.request_nonce.clone()),
                previous_account_id: None,
                created_at: unix_epoch_seconds()?,
            };
            store.insert_created(record.clone())?;
            record
        }
    };

    let account = retrieve_bound_connect_account(state, stripe, secret_key, &record).await?;
    let onboarding = stripe_create_connect_account_link(
        &state.http,
        stripe,
        secret_key,
        &record.account_id,
        &request.provider,
        &request.request_nonce,
    )
    .await?;
    let copy_paste = StripeConnectCopyPaste {
        onboarding_url: onboarding.url.clone(),
    };
    Ok(StripeConnectResponse {
        ok: true,
        rail: "fiat",
        processor_rail: "stripe",
        provider: request.provider,
        mode: stripe.mode.as_str(),
        account,
        onboarding: Some(onboarding),
        copy_paste: Some(copy_paste),
    })
}

async fn read_connect_status(
    state: &PaygateState,
    request: StripeConnectStatusRequest,
) -> Result<StripeConnectResponse> {
    let stripe = require_stripe(state)?;
    validate_provider_id(&request.provider)?;
    validate_connect_request_nonce(&request.request_nonce)?;
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;
    let record = state
        .stripe_connect
        .lock()
        .await
        .get(stripe.mode, &request.provider)
        .cloned()
        .ok_or_else(|| {
            PaygateError::InvalidRequest(
                "provider has not started Stripe Connect onboarding".to_owned(),
            )
        })?;
    let account = retrieve_bound_connect_account(state, stripe, secret_key, &record).await?;
    Ok(StripeConnectResponse {
        ok: true,
        rail: "fiat",
        processor_rail: "stripe",
        provider: request.provider,
        mode: stripe.mode.as_str(),
        account,
        onboarding: None,
        copy_paste: None,
    })
}

async fn retrieve_bound_connect_account(
    state: &PaygateState,
    stripe: &StripeSettings,
    secret_key: &str,
    record: &StripeConnectAccountRecord,
) -> Result<StripeConnectAccountSummary> {
    let account =
        stripe_retrieve_connect_account(&state.http, stripe, secret_key, &record.account_id)
            .await?;
    verify_connect_account_record(&account, stripe, record)?;
    let owner_record = state
        .stripe_connect
        .lock()
        .await
        .get_bound(stripe.mode, &account.owner_provider, &record.account_id)
        .cloned()
        .ok_or_else(|| {
            PaygateError::InvalidConfig(
                "Stripe account owner provider is not bound in the Connect store".to_owned(),
            )
        })?;
    if owner_record.account_id != record.account_id {
        return Err(PaygateError::InvalidConfig(
            "Stripe account owner provider is bound to a different account".to_owned(),
        ));
    }
    Ok(account.summary)
}

async fn create_connect_relink(
    state: &PaygateState,
    request: StripeConnectRelinkRequest,
) -> Result<StripeConnectRelinkResponse> {
    let stripe = require_stripe(state)?;
    validate_provider_id(&request.provider)?;
    validate_provider_id(&request.source_provider)?;
    validate_connect_request_nonce(&request.request_nonce)?;
    validate_stripe_account_id(&request.account_id)?;
    validate_connect_context_revision(&request.context_revision)?;
    validate_source_consent_signature(&request.source_consent_signature)?;
    validate_target_service_signature(&request.target_service_signature)?;
    if request.provider == request.source_provider {
        return Err(PaygateError::InvalidRequest(
            "Stripe relink requires a different source provider identity".to_owned(),
        ));
    }
    let country = normalize_stripe_country(&request.country)?;
    let now = unix_epoch_seconds()?;
    let latest_expiry = now
        .checked_add(stripe.connect_consent_ttl_seconds)
        .ok_or_else(|| PaygateError::Crypto("Stripe consent expiry overflow".to_owned()))?;
    if request.consent_expires_at <= now || request.consent_expires_at > latest_expiry {
        return Err(PaygateError::InvalidRequest(
            "Stripe relink source consent is stale or exceeds its bounded lifetime".to_owned(),
        ));
    }
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;
    let source_record = state
        .stripe_connect
        .lock()
        .await
        .get_bound(stripe.mode, &request.source_provider, &request.account_id)
        .cloned()
        .ok_or_else(|| {
            PaygateError::InvalidRequest(
                "source provider has no Stripe Connect account to relink".to_owned(),
            )
        })?;
    if source_record.account_id != request.account_id {
        return Err(PaygateError::InvalidRequest(
            "Stripe relink consent account does not match the source provider binding".to_owned(),
        ));
    }
    if source_record.country != country {
        return Err(PaygateError::InvalidRequest(format!(
            "source Stripe Connect account is registered in {}",
            source_record.country
        )));
    }
    let state_token = stripe_connect_relink_state(
        stripe,
        &request.provider,
        &request.source_provider,
        &request.account_id,
        &request.context_revision,
        &country,
        &request.request_nonce,
        request.consent_expires_at,
        &request.source_consent_signature,
        &request.target_service_signature,
    )?;
    let challenge = StripeConnectConsentRecord {
        schema_version: 1,
        mode: stripe.mode,
        provider: request.provider.clone(),
        source_provider: request.source_provider.clone(),
        account_id: request.account_id.clone(),
        account_type: source_record.account_type,
        context_revision: request.context_revision,
        country,
        request_nonce: request.request_nonce,
        source_consent_signature: request.source_consent_signature,
        target_service_signature: request.target_service_signature,
        state: state_token,
        created_at: now,
        expires_at: request.consent_expires_at,
        completed_at: None,
    };
    let challenge = state
        .stripe_connect_consents
        .lock()
        .await
        .insert_pending(challenge)?;

    let existing_target = {
        state
            .stripe_connect
            .lock()
            .await
            .get(stripe.mode, &request.provider)
            .cloned()
    };
    if let Some(existing) = existing_target {
        if existing.account_id != source_record.account_id {
            return Err(PaygateError::InvalidRequest(
                "provider already has a different Stripe Connect account".to_owned(),
            ));
        }
        let account = retrieve_bound_connect_account(state, stripe, secret_key, &existing).await?;
        state
            .stripe_connect_consents
            .lock()
            .await
            .complete(&challenge.state, now)?;
        return Ok(StripeConnectRelinkResponse {
            ok: true,
            rail: "fiat",
            processor_rail: "stripe",
            provider: request.provider,
            source_provider: request.source_provider,
            mode: stripe.mode.as_str(),
            status: "linked",
            account: Some(account),
            onboarding: None,
            copy_paste: None,
        });
    }

    let account = retrieve_bound_connect_account(state, stripe, secret_key, &source_record).await?;
    if !account.ready {
        return Err(PaygateError::InvalidRequest(
            "source Stripe Connect account is not ready".to_owned(),
        ));
    }
    if source_record.account_type != StripeConnectAccountType::Standard {
        let started = {
            let mut consents = state.stripe_connect_consents.lock().await;
            matches!(
                consents.begin(&challenge.state)?,
                StripeConnectConsentBegin::Started(_)
            )
        };
        if started {
            let relinked = StripeConnectAccountRecord {
                schema_version: 1,
                mode: challenge.mode,
                provider: challenge.provider.clone(),
                account_id: challenge.account_id.clone(),
                account_type: challenge.account_type,
                country: challenge.country.clone(),
                request_nonce: Some(challenge.request_nonce.clone()),
                previous_account_id: None,
                created_at: now,
            };
            let linked = state
                .stripe_connect
                .lock()
                .await
                .insert_relinked(relinked, &challenge.source_provider);
            let mut consents = state.stripe_connect_consents.lock().await;
            if let Err(error) = linked {
                consents.fail(&challenge.state);
                return Err(error);
            }
            consents.complete(&challenge.state, now)?;
        }
        let target_record = state
            .stripe_connect
            .lock()
            .await
            .get(stripe.mode, &challenge.provider)
            .cloned()
            .ok_or_else(|| {
                PaygateError::InvalidConfig(
                    "dual-provider Stripe relink completed without a target binding".to_owned(),
                )
            })?;
        let account =
            retrieve_bound_connect_account(state, stripe, secret_key, &target_record).await?;
        return Ok(StripeConnectRelinkResponse {
            ok: true,
            rail: "fiat",
            processor_rail: "stripe",
            provider: challenge.provider,
            source_provider: challenge.source_provider,
            mode: stripe.mode.as_str(),
            status: "linked",
            account: Some(account),
            onboarding: None,
            copy_paste: None,
        });
    }
    let client_id = stripe.connect_oauth_client_id.as_deref().ok_or_else(|| {
        PaygateError::InvalidConfig(
            "stripe.connect_oauth_client_id is required for cross-provider relink".to_owned(),
        )
    })?;
    let redirect_url = stripe
        .connect_oauth_redirect_url
        .as_deref()
        .ok_or_else(|| {
            PaygateError::InvalidConfig(
                "stripe.connect_oauth_redirect_url is required for cross-provider relink"
                    .to_owned(),
            )
        })?;
    let url = stripe_connect_relink_authorize_url(client_id, redirect_url, &challenge.state)?;
    let onboarding = StripeConnectOnboardingSummary {
        url,
        expires_at: challenge.expires_at,
    };
    let copy_paste = StripeConnectCopyPaste {
        onboarding_url: onboarding.url.clone(),
    };
    Ok(StripeConnectRelinkResponse {
        ok: true,
        rail: "fiat",
        processor_rail: "stripe",
        provider: request.provider,
        source_provider: request.source_provider,
        mode: stripe.mode.as_str(),
        status: "consent_required",
        account: None,
        onboarding: Some(onboarding),
        copy_paste: Some(copy_paste),
    })
}

async fn complete_connect_relink(
    state: &PaygateState,
    query: StripeConnectRelinkReturnQuery,
) -> Result<()> {
    validate_connect_oauth_state(&query.state)?;
    let challenge = {
        let mut store = state.stripe_connect_consents.lock().await;
        match store.begin(&query.state)? {
            StripeConnectConsentBegin::Completed => return Ok(()),
            StripeConnectConsentBegin::Started(challenge) => challenge,
        }
    };
    let result = complete_connect_relink_started(state, &query, &challenge).await;
    let mut store = state.stripe_connect_consents.lock().await;
    match result {
        Ok(()) => store.complete(&query.state, unix_epoch_seconds()?),
        Err(err) => {
            store.fail(&query.state);
            Err(err)
        }
    }
}

async fn complete_connect_relink_started(
    state: &PaygateState,
    query: &StripeConnectRelinkReturnQuery,
    challenge: &StripeConnectConsentRecord,
) -> Result<()> {
    let stripe = require_stripe(state)?;
    if challenge.mode != stripe.mode {
        return Err(PaygateError::InvalidRequest(
            "Stripe consent mode does not match the active paygate mode".to_owned(),
        ));
    }
    let expected_state = stripe_connect_relink_state(
        stripe,
        &challenge.provider,
        &challenge.source_provider,
        &challenge.account_id,
        &challenge.context_revision,
        &challenge.country,
        &challenge.request_nonce,
        challenge.expires_at,
        &challenge.source_consent_signature,
        &challenge.target_service_signature,
    )?;
    if expected_state
        .as_bytes()
        .ct_eq(challenge.state.as_bytes())
        .unwrap_u8()
        != 1
    {
        return Err(PaygateError::InvalidRequest(
            "Stripe relink consent state binding is invalid".to_owned(),
        ));
    }
    if challenge.expires_at < unix_epoch_seconds()? {
        return Err(PaygateError::InvalidRequest(
            "Stripe relink consent challenge has expired".to_owned(),
        ));
    }
    if query.error.is_some() {
        return Err(PaygateError::InvalidRequest(
            "Stripe account holder denied relink consent".to_owned(),
        ));
    }
    if query.scope.as_deref() != Some("read_write") {
        return Err(PaygateError::InvalidRequest(
            "Stripe relink consent did not grant read_write scope".to_owned(),
        ));
    }

    let source_record = state
        .stripe_connect
        .lock()
        .await
        .get_bound(
            stripe.mode,
            &challenge.source_provider,
            &challenge.account_id,
        )
        .cloned()
        .ok_or_else(|| {
            PaygateError::InvalidRequest(
                "source provider Stripe account binding no longer exists".to_owned(),
            )
        })?;
    if source_record.account_id != challenge.account_id
        || source_record.account_type != challenge.account_type
        || source_record.country != challenge.country
    {
        return Err(PaygateError::InvalidRequest(
            "source provider Stripe account binding changed during consent".to_owned(),
        ));
    }
    if let Some(existing) = state
        .stripe_connect
        .lock()
        .await
        .get(stripe.mode, &challenge.provider)
        .cloned()
    {
        if existing.account_id == challenge.account_id {
            return Ok(());
        }
        return Err(PaygateError::InvalidRequest(
            "provider acquired a different Stripe account during consent".to_owned(),
        ));
    }

    let code = query.code.as_deref().ok_or_else(|| {
        PaygateError::InvalidRequest("Stripe relink callback is missing an OAuth code".to_owned())
    })?;
    validate_connect_oauth_code(code)?;
    let secret_key = stripe
        .secret_key
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.secret_key missing".to_owned()))?;
    let oauth = stripe_exchange_connect_oauth_code(&state.http, stripe, secret_key, code).await?;
    let stripe_user_id = json_string_field(&oauth, "stripe_user_id")?;
    validate_stripe_account_id(&stripe_user_id)?;
    if stripe_user_id != challenge.account_id {
        return Err(PaygateError::InvalidRequest(
            "Stripe consent selected a different connected account".to_owned(),
        ));
    }
    let livemode = oauth
        .get("livemode")
        .and_then(Value::as_bool)
        .ok_or_else(|| PaygateError::Stripe("OAuth response missing livemode".to_owned()))?;
    if livemode != (stripe.mode == StripeMode::Live) {
        return Err(PaygateError::InvalidRequest(
            "Stripe OAuth consent mode does not match the active paygate mode".to_owned(),
        ));
    }
    if oauth.get("scope").and_then(Value::as_str) != Some("read_write") {
        return Err(PaygateError::InvalidRequest(
            "Stripe OAuth token did not grant read_write scope".to_owned(),
        ));
    }

    let account = retrieve_bound_connect_account(state, stripe, secret_key, &source_record).await?;
    if !account.ready {
        return Err(PaygateError::InvalidRequest(
            "Stripe account became unready during relink consent".to_owned(),
        ));
    }
    let relinked = StripeConnectAccountRecord {
        schema_version: 1,
        mode: challenge.mode,
        provider: challenge.provider.clone(),
        account_id: challenge.account_id.clone(),
        account_type: challenge.account_type,
        country: challenge.country.clone(),
        request_nonce: Some(challenge.request_nonce.clone()),
        previous_account_id: None,
        created_at: unix_epoch_seconds()?,
    };
    state
        .stripe_connect
        .lock()
        .await
        .insert_relinked(relinked, &challenge.source_provider)
}

fn require_stripe(state: &PaygateState) -> Result<&StripeSettings> {
    let stripe = &state.config.rails.stripe;
    if !stripe.enabled {
        return Err(PaygateError::InvalidRequest(
            "Stripe processor is not enabled".to_owned(),
        ));
    }
    Ok(stripe)
}

async fn stripe_create_connect_account(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    provider: &str,
    country: &str,
    rotation_nonce: Option<&str>,
) -> Result<VerifiedStripeConnectAccount> {
    let form = [
        ("type", stripe.connect_account_type.as_str().to_owned()),
        ("country", country.to_owned()),
        ("capabilities[transfers][requested]", "true".to_owned()),
        ("metadata[mayhem_provider]", provider.to_owned()),
        ("metadata[mayhem_mode]", stripe.mode.as_str().to_owned()),
        (
            "business_profile[product_description]",
            "Mayhem inference provider".to_owned(),
        ),
    ];
    let response = http
        .post(format!(
            "{}/v1/accounts",
            stripe.api_base_url.trim_end_matches('/')
        ))
        .basic_auth(secret_key, Some(""))
        .header(
            "Idempotency-Key",
            stripe_connect_idempotency_key(stripe.mode, provider, rotation_nonce),
        )
        .form(&form)
        .send()
        .await?;
    let value = stripe_json_response(response, "creating connected account").await?;
    stripe_connect_account_summary(value)
}

async fn stripe_retrieve_connect_account(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    account_id: &str,
) -> Result<VerifiedStripeConnectAccount> {
    validate_stripe_account_id(account_id)?;
    let response = http
        .get(format!(
            "{}/v1/accounts/{account_id}",
            stripe.api_base_url.trim_end_matches('/')
        ))
        .basic_auth(secret_key, Some(""))
        .send()
        .await?;
    let value = stripe_json_response(response, "retrieving connected account").await?;
    stripe_connect_account_summary(value)
}

async fn stripe_create_connect_account_link(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    account_id: &str,
    provider: &str,
    request_nonce: &str,
) -> Result<StripeConnectOnboardingSummary> {
    validate_stripe_account_id(account_id)?;
    let form = [
        ("account", account_id.to_owned()),
        ("type", "account_onboarding".to_owned()),
        ("return_url", stripe.connect_return_url.clone()),
        ("refresh_url", stripe.connect_refresh_url.clone()),
        ("collection_options[fields]", "eventually_due".to_owned()),
    ];
    let response = http
        .post(format!(
            "{}/v1/account_links",
            stripe.api_base_url.trim_end_matches('/')
        ))
        .basic_auth(secret_key, Some(""))
        .header(
            "Idempotency-Key",
            stripe_connect_link_idempotency_key(stripe.mode, provider, request_nonce),
        )
        .form(&form)
        .send()
        .await?;
    let value = stripe_json_response(response, "creating connected-account link").await?;
    let url = json_string_field(&value, "url")?;
    validate_hosted_checkout_url("account_link.url", &url, "connect.stripe.com")?;
    Ok(StripeConnectOnboardingSummary {
        url,
        expires_at: json_u64_field(&value, "expires_at")?,
    })
}

async fn stripe_exchange_connect_oauth_code(
    http: &reqwest::Client,
    stripe: &StripeSettings,
    secret_key: &str,
    code: &str,
) -> Result<Value> {
    let form = [
        ("code", code.to_owned()),
        ("grant_type", "authorization_code".to_owned()),
    ];
    let response = http
        .post(&stripe.connect_oauth_token_url)
        .basic_auth(secret_key, Some(""))
        .form(&form)
        .send()
        .await?;
    stripe_json_response(response, "exchanging connected-account OAuth consent").await
}

fn stripe_connect_relink_authorize_url(
    client_id: &str,
    redirect_url: &str,
    state: &str,
) -> Result<String> {
    let mut url =
        reqwest::Url::parse(DEFAULT_STRIPE_CONNECT_OAUTH_AUTHORIZE_URL).map_err(|_| {
            PaygateError::InvalidConfig("Stripe OAuth authorize URL is invalid".to_owned())
        })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("scope", "read_write")
        .append_pair("redirect_uri", redirect_url)
        .append_pair("state", state);
    let url = url.to_string();
    validate_hosted_checkout_url("Stripe OAuth authorize URL", &url, "connect.stripe.com")?;
    Ok(url)
}

fn stripe_connect_relink_state(
    stripe: &StripeSettings,
    provider: &str,
    source_provider: &str,
    account_id: &str,
    context_revision: &str,
    country: &str,
    request_nonce: &str,
    expires_at: u64,
    source_consent_signature: &str,
    target_service_signature: &str,
) -> Result<String> {
    let secret = stripe
        .webhook_secret
        .as_deref()
        .ok_or_else(|| PaygateError::InvalidConfig("stripe.webhook_secret missing".to_owned()))?;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes())
        .map_err(|_| PaygateError::Crypto("invalid Stripe consent HMAC key".to_owned()))?;
    for value in [
        "mayhem-stripe-connect-relink-v1",
        stripe.mode.as_str(),
        provider,
        source_provider,
        account_id,
        context_revision,
        country,
        request_nonce,
        source_consent_signature,
        target_service_signature,
    ] {
        mac.update(value.as_bytes());
        mac.update(b"\0");
    }
    mac.update(&expires_at.to_be_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

async fn stripe_json_response(response: reqwest::Response, action: &str) -> Result<Value> {
    let status = response.status();
    let body = response.text().await?;
    let value: Value = serde_json::from_str(&body).map_err(|_| {
        PaygateError::Stripe(format!(
            "Stripe returned an invalid response while {action}"
        ))
    })?;
    if !status.is_success() {
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Stripe request failed");
        return Err(PaygateError::Stripe(format!(
            "Stripe returned {status} while {action}: {}",
            redact_stripe_credentials(message)
        )));
    }
    Ok(value)
}

fn redact_stripe_credentials(input: &str) -> String {
    const PREFIXES: [&str; 8] = [
        "sk_test_", "sk_live_", "rk_test_", "rk_live_", "pk_test_", "pk_live_", "whsec_", "ac_",
    ];
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    loop {
        let Some((offset, _)) = PREFIXES
            .iter()
            .filter_map(|prefix| remaining.find(prefix).map(|offset| (offset, prefix)))
            .min_by_key(|(offset, _)| *offset)
        else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..offset]);
        let credential = &remaining[offset..];
        let mut end = 0;
        while let Some(byte) = credential.as_bytes().get(end) {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'*' | b'.') {
                end += 1;
            } else {
                break;
            }
        }
        output.push_str("[REDACTED]");
        remaining = &credential[end..];
    }
    output
}

fn stripe_connect_account_summary(value: Value) -> Result<VerifiedStripeConnectAccount> {
    let id = json_string_field(&value, "id")?;
    validate_stripe_account_id(&id)?;
    let account_type = json_string_field(&value, "type")?;
    if !matches!(account_type.as_str(), "express" | "custom" | "standard") {
        return Err(PaygateError::Stripe(
            "connected account has an unsupported type".to_owned(),
        ));
    }
    let country = normalize_stripe_country(&json_string_field(&value, "country")?)?;
    let default_currency =
        normalize_stripe_currency(Some(&json_string_field(&value, "default_currency")?))?;
    let details_submitted = value
        .get("details_submitted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let charges_enabled = value
        .get("charges_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let payouts_enabled = value
        .get("payouts_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let transfers_enabled = value
        .pointer("/capabilities/transfers")
        .and_then(Value::as_str)
        == Some("active");
    let currently_due = stripe_string_array(&value, "/requirements/currently_due")?;
    let eventually_due = stripe_string_array(&value, "/requirements/eventually_due")?;
    let disabled_reason = value
        .pointer("/requirements/disabled_reason")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let owner_provider = value
        .pointer("/metadata/mayhem_provider")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            PaygateError::Stripe(
                "connected account is missing Mayhem provider ownership metadata".to_owned(),
            )
        })?
        .to_ascii_lowercase();
    validate_provider_id(&owner_provider).map_err(|_| {
        PaygateError::Stripe(
            "connected account has invalid Mayhem provider ownership metadata".to_owned(),
        )
    })?;
    let livemode = value
        .get("livemode")
        .and_then(Value::as_bool)
        .ok_or_else(|| PaygateError::Stripe("connected account is missing livemode".to_owned()))?;
    let metadata_mode = match value
        .pointer("/metadata/mayhem_mode")
        .and_then(Value::as_str)
    {
        Some("test") => StripeMode::Test,
        Some("live") => StripeMode::Live,
        _ => {
            return Err(PaygateError::Stripe(
                "connected account has invalid Mayhem mode metadata".to_owned(),
            ));
        }
    };
    Ok(VerifiedStripeConnectAccount {
        summary: StripeConnectAccountSummary {
            id,
            account_type,
            country,
            default_currency,
            details_submitted,
            charges_enabled,
            payouts_enabled,
            transfers_enabled,
            ready: details_submitted && payouts_enabled && transfers_enabled,
            currently_due,
            eventually_due,
            disabled_reason,
        },
        owner_provider,
        livemode,
        metadata_mode,
    })
}

fn verify_new_connect_account(
    account: &VerifiedStripeConnectAccount,
    stripe: &StripeSettings,
    provider: &str,
    account_type: StripeConnectAccountType,
    country: &str,
) -> Result<()> {
    if account.owner_provider != provider {
        return Err(PaygateError::Stripe(
            "created connected account ownership metadata did not match provider".to_owned(),
        ));
    }
    if account.summary.account_type != account_type.as_str() {
        return Err(PaygateError::Stripe(
            "created connected account type did not match request".to_owned(),
        ));
    }
    if account.summary.country != country {
        return Err(PaygateError::Stripe(
            "created connected account country did not match request".to_owned(),
        ));
    }
    verify_connect_account_mode(account, stripe)
}

fn verify_connect_account_record(
    account: &VerifiedStripeConnectAccount,
    stripe: &StripeSettings,
    record: &StripeConnectAccountRecord,
) -> Result<()> {
    if account.summary.id != record.account_id {
        return Err(PaygateError::Stripe(
            "retrieved connected account id did not match binding".to_owned(),
        ));
    }
    if account.summary.account_type != record.account_type.as_str() {
        return Err(PaygateError::Stripe(
            "retrieved connected account type did not match binding".to_owned(),
        ));
    }
    if account.summary.country != record.country {
        return Err(PaygateError::Stripe(
            "retrieved connected account country did not match binding".to_owned(),
        ));
    }
    verify_connect_account_mode(account, stripe)
}

fn verify_connect_account_mode(
    account: &VerifiedStripeConnectAccount,
    stripe: &StripeSettings,
) -> Result<()> {
    if account.livemode != (stripe.mode == StripeMode::Live) {
        return Err(PaygateError::Stripe(
            "connected account mode did not match paygate mode".to_owned(),
        ));
    }
    if account.metadata_mode != stripe.mode {
        return Err(PaygateError::Stripe(
            "connected account Mayhem mode metadata did not match paygate mode".to_owned(),
        ));
    }
    Ok(())
}

fn stripe_string_array(value: &Value, pointer: &str) -> Result<Vec<String>> {
    let Some(array) = value.pointer(pointer) else {
        return Ok(Vec::new());
    };
    array
        .as_array()
        .ok_or_else(|| PaygateError::Stripe(format!("Stripe object {pointer} must be an array")))?
        .iter()
        .map(|entry| {
            entry.as_str().map(str::to_owned).ok_or_else(|| {
                PaygateError::Stripe(format!("Stripe object {pointer} contains a non-string"))
            })
        })
        .collect()
}

fn stripe_connect_idempotency_key(
    mode: StripeMode,
    provider: &str,
    rotation_nonce: Option<&str>,
) -> String {
    let Some(rotation_nonce) = rotation_nonce else {
        return format!("mayhem-connect-account-{}-{provider}", mode.as_str());
    };
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "mayhem-connect-account-rotation-v1:{}:{provider}:{rotation_nonce}",
        mode.as_str()
    ));
    format!(
        "mayhem-connect-account-rotation-{}",
        hex::encode(hasher.finalize())
    )
}

fn stripe_connect_link_idempotency_key(
    mode: StripeMode,
    provider: &str,
    request_nonce: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "mayhem-connect-link-v1:{}:{provider}:{}",
        mode.as_str(),
        request_nonce
    ));
    format!("mayhem-connect-link-{}", hex::encode(hasher.finalize()))
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
            "Stripe returned {status}: {}",
            redact_stripe_credentials(&body)
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
            "Stripe returned {status}: {}",
            redact_stripe_credentials(&body)
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
                    "Stripe events backfill returned {status}: {}",
                    redact_stripe_credentials(&body)
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

impl StripeConnectStore {
    fn memory() -> Self {
        Self {
            accounts: HashMap::new(),
            bindings: HashMap::new(),
            requests: HashMap::new(),
            path: None,
        }
    }

    fn load(path: &Path) -> Result<Self> {
        let mut store = Self {
            accounts: HashMap::new(),
            bindings: HashMap::new(),
            requests: HashMap::new(),
            path: Some(path.to_path_buf()),
        };
        if !path.exists() {
            return Ok(store);
        }
        let text = fs::read_to_string(path)?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let value: Value = serde_json::from_str(line)?;
            let schema_version = value
                .get("schema_version")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    PaygateError::InvalidConfig(
                        "Stripe Connect account record has no schema_version".to_owned(),
                    )
                })?;
            if schema_version != 1 {
                return Err(PaygateError::InvalidConfig(format!(
                    "unsupported Stripe Connect account schema_version {}",
                    schema_version
                )));
            }
            let record: StripeConnectAccountRecord = serde_json::from_value(value)?;
            store.load_record(record)?;
        }
        Ok(store)
    }

    fn key(mode: StripeMode, provider: &str) -> String {
        format!("{}:{provider}", mode.as_str())
    }

    fn binding_key(mode: StripeMode, provider: &str, account_id: &str) -> String {
        format!("{}:{provider}:{account_id}", mode.as_str())
    }

    fn request_key(mode: StripeMode, provider: &str, request_nonce: &str) -> String {
        format!("{}:{provider}:{request_nonce}", mode.as_str())
    }

    fn get(&self, mode: StripeMode, provider: &str) -> Option<&StripeConnectAccountRecord> {
        self.accounts.get(&Self::key(mode, provider))
    }

    fn get_bound(
        &self,
        mode: StripeMode,
        provider: &str,
        account_id: &str,
    ) -> Option<&StripeConnectAccountRecord> {
        self.bindings
            .get(&Self::binding_key(mode, provider, account_id))
    }

    fn replay_rotation(
        &self,
        mode: StripeMode,
        provider: &str,
        request_nonce: &str,
        previous_account_id: &str,
        country: &str,
    ) -> Result<Option<StripeConnectAccountRecord>> {
        let Some(record) = self
            .requests
            .get(&Self::request_key(mode, provider, request_nonce))
        else {
            return Ok(None);
        };
        if record.previous_account_id.as_deref() != Some(previous_account_id)
            || record.country != country
        {
            return Err(PaygateError::InvalidRequest(
                "Stripe Connect request nonce already consumed by different account work"
                    .to_owned(),
            ));
        }
        Ok(Some(record.clone()))
    }

    fn insert_created(&mut self, record: StripeConnectAccountRecord) -> Result<()> {
        if self.bindings.values().any(|existing| {
            existing.mode == record.mode && existing.account_id == record.account_id
        }) {
            return Err(PaygateError::Stripe(
                "Stripe returned an account already bound to another provider".to_owned(),
            ));
        }
        self.insert_record(record)
    }

    fn insert_rotated(
        &mut self,
        record: StripeConnectAccountRecord,
        previous_account_id: &str,
    ) -> Result<()> {
        let key = Self::key(record.mode, &record.provider);
        let existing = self.accounts.get(&key).ok_or_else(|| {
            PaygateError::InvalidRequest(
                "provider has no Stripe Connect account to rotate".to_owned(),
            )
        })?;
        if existing.account_id != previous_account_id {
            return Err(PaygateError::InvalidRequest(
                "Stripe Connect rotation previous account is stale".to_owned(),
            ));
        }
        if record.account_id == previous_account_id {
            return Err(PaygateError::Stripe(
                "Stripe Connect rotation returned the current account".to_owned(),
            ));
        }
        if self.bindings.values().any(|candidate| {
            candidate.mode == record.mode && candidate.account_id == record.account_id
        }) {
            return Err(PaygateError::Stripe(
                "Stripe Connect rotation returned an account that is already bound".to_owned(),
            ));
        }
        self.replace_record(record)
    }

    fn insert_relinked(
        &mut self,
        record: StripeConnectAccountRecord,
        source_provider: &str,
    ) -> Result<()> {
        let source = self
            .get_bound(record.mode, source_provider, &record.account_id)
            .ok_or_else(|| {
                PaygateError::InvalidRequest(
                    "source provider Stripe account binding no longer exists".to_owned(),
                )
            })?;
        if source.account_id != record.account_id
            || source.account_type != record.account_type
            || source.country != record.country
        {
            return Err(PaygateError::InvalidRequest(
                "source provider Stripe account binding changed during consent".to_owned(),
            ));
        }
        self.insert_record(record)
    }

    fn insert_record(&mut self, record: StripeConnectAccountRecord) -> Result<()> {
        let key = Self::key(record.mode, &record.provider);
        if let Some(existing) = self.accounts.get(&key) {
            if existing.account_id == record.account_id {
                return Ok(());
            }
            return Err(PaygateError::InvalidConfig(
                "provider already has a different Stripe Connect account".to_owned(),
            ));
        }
        self.persist_record(&record)?;
        self.index_record(record)?;
        Ok(())
    }

    fn replace_record(&mut self, record: StripeConnectAccountRecord) -> Result<()> {
        self.persist_record(&record)?;
        self.index_record(record)?;
        Ok(())
    }

    fn load_record(&mut self, record: StripeConnectAccountRecord) -> Result<()> {
        if let Some(request_nonce) = record.request_nonce.as_deref() {
            let request_key = Self::request_key(record.mode, &record.provider, request_nonce);
            if let Some(existing) = self.requests.get(&request_key) {
                if existing == &record {
                    return Ok(());
                }
                return Err(PaygateError::InvalidConfig(
                    "Stripe Connect account log reuses a request nonce".to_owned(),
                ));
            }
        } else if record.previous_account_id.is_some() {
            return Err(PaygateError::InvalidConfig(
                "Stripe Connect rotation record has no request nonce".to_owned(),
            ));
        }
        if let Some(previous_account_id) = record.previous_account_id.as_deref() {
            let current = self.get(record.mode, &record.provider).ok_or_else(|| {
                PaygateError::InvalidConfig(
                    "Stripe Connect rotation log has no previous account".to_owned(),
                )
            })?;
            if current.account_id != previous_account_id {
                return Err(PaygateError::InvalidConfig(
                    "Stripe Connect rotation log has a stale previous account".to_owned(),
                ));
            }
        } else if let Some(current) = self.get(record.mode, &record.provider) {
            if current.account_id != record.account_id {
                return Err(PaygateError::InvalidConfig(
                    "Stripe Connect account log replaces an account without rotation CAS"
                        .to_owned(),
                ));
            }
        }
        self.index_record(record)
    }

    fn index_record(&mut self, record: StripeConnectAccountRecord) -> Result<()> {
        let account_key = Self::key(record.mode, &record.provider);
        let binding_key = Self::binding_key(record.mode, &record.provider, &record.account_id);
        if let Some(existing) = self.bindings.get(&binding_key) {
            if existing != &record {
                return Err(PaygateError::InvalidConfig(
                    "Stripe Connect account binding record is inconsistent".to_owned(),
                ));
            }
        }
        if let Some(request_nonce) = record.request_nonce.as_deref() {
            let request_key = Self::request_key(record.mode, &record.provider, request_nonce);
            if let Some(existing) = self.requests.get(&request_key) {
                if existing != &record {
                    return Err(PaygateError::InvalidConfig(
                        "Stripe Connect request nonce record is inconsistent".to_owned(),
                    ));
                }
            }
            self.requests.insert(request_key, record.clone());
        }
        self.accounts.insert(account_key, record.clone());
        self.bindings.insert(binding_key, record.clone());
        Ok(())
    }

    fn persist_record(&self, record: &StripeConnectAccountRecord) -> Result<()> {
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(path)?;
            #[cfg(unix)]
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            writeln!(file, "{}", serde_json::to_string(&record)?)?;
            file.sync_all()?;
        }
        Ok(())
    }
}

impl StripeConnectConsentStore {
    fn memory() -> Self {
        Self {
            challenges: HashMap::new(),
            requests: HashMap::new(),
            processing: HashSet::new(),
            path: None,
        }
    }

    fn load(path: &Path) -> Result<Self> {
        let mut store = Self {
            challenges: HashMap::new(),
            requests: HashMap::new(),
            processing: HashSet::new(),
            path: Some(path.to_path_buf()),
        };
        if !path.exists() {
            return Ok(store);
        }
        let text = fs::read_to_string(path)?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: StripeConnectConsentRecord = serde_json::from_str(line)?;
            if record.schema_version != 1 {
                return Err(PaygateError::InvalidConfig(format!(
                    "unsupported Stripe Connect consent schema_version {}",
                    record.schema_version
                )));
            }
            validate_connect_oauth_state(&record.state).map_err(|_| {
                PaygateError::InvalidConfig(
                    "Stripe Connect consent store contains invalid state".to_owned(),
                )
            })?;
            let request_key =
                Self::request_key(record.mode, &record.provider, &record.request_nonce);
            store.requests.insert(request_key, record.state.clone());
            store.challenges.insert(record.state.clone(), record);
        }
        Ok(store)
    }

    fn request_key(mode: StripeMode, provider: &str, request_nonce: &str) -> String {
        format!("{}:{provider}:{request_nonce}", mode.as_str())
    }

    fn insert_pending(
        &mut self,
        record: StripeConnectConsentRecord,
    ) -> Result<StripeConnectConsentRecord> {
        let request_key = Self::request_key(record.mode, &record.provider, &record.request_nonce);
        if let Some(state) = self.requests.get(&request_key) {
            let existing = self.challenges.get(state).ok_or_else(|| {
                PaygateError::InvalidConfig(
                    "Stripe Connect consent request index is inconsistent".to_owned(),
                )
            })?;
            if existing.source_provider != record.source_provider
                || existing.account_id != record.account_id
                || existing.account_type != record.account_type
                || existing.context_revision != record.context_revision
                || existing.country != record.country
                || existing.expires_at != record.expires_at
                || existing.source_consent_signature != record.source_consent_signature
                || existing.target_service_signature != record.target_service_signature
            {
                return Err(PaygateError::InvalidRequest(
                    "Stripe relink request nonce was already used for different consent state"
                        .to_owned(),
                ));
            }
            return Ok(existing.clone());
        }
        self.append(&record)?;
        self.requests.insert(request_key, record.state.clone());
        self.challenges.insert(record.state.clone(), record.clone());
        Ok(record)
    }

    fn begin(&mut self, state: &str) -> Result<StripeConnectConsentBegin> {
        let challenge = self.challenges.get(state).cloned().ok_or_else(|| {
            PaygateError::InvalidRequest("unknown Stripe relink consent state".to_owned())
        })?;
        if challenge.completed_at.is_some() {
            return Ok(StripeConnectConsentBegin::Completed);
        }
        if !self.processing.insert(state.to_owned()) {
            return Err(PaygateError::InvalidRequest(
                "Stripe relink consent is already being processed".to_owned(),
            ));
        }
        Ok(StripeConnectConsentBegin::Started(challenge))
    }

    fn complete(&mut self, state: &str, completed_at: u64) -> Result<()> {
        let mut challenge = self.challenges.get(state).cloned().ok_or_else(|| {
            PaygateError::InvalidConfig("Stripe relink consent state disappeared".to_owned())
        })?;
        self.processing.remove(state);
        if challenge.completed_at.is_some() {
            return Ok(());
        }
        challenge.completed_at = Some(completed_at);
        self.append(&challenge)?;
        self.challenges.insert(state.to_owned(), challenge);
        Ok(())
    }

    fn fail(&mut self, state: &str) {
        self.processing.remove(state);
    }

    fn append(&self, record: &StripeConnectConsentRecord) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path)?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        writeln!(file, "{}", serde_json::to_string(record)?)?;
        file.sync_all()?;
        Ok(())
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

fn parse_stripe_connect_account_type(field: &str, value: &str) -> Result<StripeConnectAccountType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "express" => Ok(StripeConnectAccountType::Express),
        "custom" => Ok(StripeConnectAccountType::Custom),
        "standard" => Ok(StripeConnectAccountType::Standard),
        _ => Err(PaygateError::InvalidConfig(format!(
            "{field} must be express, custom, or standard"
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

fn default_stripe_connect_accounts_path() -> PathBuf {
    mayhem_home()
        .join("paygate")
        .join("stripe-connect-accounts.jsonl")
}

fn default_stripe_connect_consents_path() -> PathBuf {
    mayhem_home()
        .join("paygate")
        .join("stripe-connect-consents.jsonl")
}

fn default_stripe_internal_auth_secret_path() -> PathBuf {
    mayhem_home().join("paygate").join("internal-auth.secret")
}

fn stripe_internal_auth_nonce_store_path(secret_path: &Path) -> PathBuf {
    secret_path.with_file_name("internal-auth-nonces.json")
}

impl StripeInternalAuthNonceStore {
    fn memory() -> Self {
        Self {
            path: None,
            nonces: BTreeMap::new(),
        }
    }

    fn load(secret_path: &Path, tolerance_seconds: u64) -> Result<Self> {
        let path = stripe_internal_auth_nonce_store_path(secret_path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    path: Some(path),
                    nonces: BTreeMap::new(),
                })
            }
            Err(error) => {
                return Err(PaygateError::InvalidConfig(format!(
                    "could not read Stripe internal auth nonce store metadata: {error}"
                )))
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PaygateError::InvalidConfig(
                "Stripe internal auth nonce store must be a regular non-symlink file".to_owned(),
            ));
        }
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(PaygateError::InvalidConfig(
                "Stripe internal auth nonce store must not be group/world accessible".to_owned(),
            ));
        }
        let mut record: StripeInternalAuthNonceFile = serde_json::from_slice(&fs::read(&path)?)?;
        if record.schema_version != 1
            || record.nonces.iter().any(|(nonce, timestamp)| {
                nonce.len() != 64
                    || nonce != &nonce.to_ascii_lowercase()
                    || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
                    || *timestamp == 0
            })
        {
            return Err(PaygateError::InvalidConfig(
                "Stripe internal auth nonce store is invalid".to_owned(),
            ));
        }
        let now = unix_epoch_seconds()?;
        let oldest = now.saturating_sub(tolerance_seconds);
        let newest = now.saturating_add(tolerance_seconds);
        record
            .nonces
            .retain(|_, timestamp| *timestamp >= oldest && *timestamp <= newest);
        let store = Self {
            path: Some(path),
            nonces: record.nonces,
        };
        store.persist()?;
        Ok(store)
    }

    fn remember(
        &mut self,
        nonce: String,
        timestamp: u64,
        now: u64,
        tolerance_seconds: u64,
    ) -> Result<bool> {
        let oldest = now.saturating_sub(tolerance_seconds);
        let newest = now.saturating_add(tolerance_seconds);
        self.nonces
            .retain(|_, seen_at| *seen_at >= oldest && *seen_at <= newest);
        if self.nonces.contains_key(&nonce) {
            return Ok(false);
        }
        self.nonces.insert(nonce, timestamp);
        self.persist()?;
        Ok(true)
    }

    fn persist(&self) -> Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let parent = path.parent().ok_or_else(|| {
            PaygateError::InvalidConfig(
                "Stripe internal auth nonce store has no parent directory".to_owned(),
            )
        })?;
        fs::create_dir_all(parent)?;
        let record = StripeInternalAuthNonceFile {
            schema_version: 1,
            nonces: self.nonces.clone(),
        };
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        let random = random_seed()?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("internal-auth-nonces.json");
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            hex::encode(&random[..8])
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        if let Err(error) = (|| -> std::io::Result<()> {
            file.write_all(&bytes)?;
            file.flush()?;
            file.sync_all()?;
            #[cfg(windows)]
            if path.exists() {
                fs::remove_file(path)?;
            }
            fs::rename(&temporary, path)?;
            Ok(())
        })() {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(())
    }
}

fn load_stripe_internal_auth_secret(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        PaygateError::InvalidConfig(format!(
            "could not read Stripe internal auth secret metadata: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PaygateError::InvalidConfig(
            "Stripe internal auth secret must be a regular non-symlink file".to_owned(),
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(PaygateError::InvalidConfig(
            "Stripe internal auth secret file must not be group/world accessible".to_owned(),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        PaygateError::InvalidConfig(format!(
            "could not read Stripe internal auth secret: {error}"
        ))
    })?;
    let secret = String::from_utf8(bytes)
        .map_err(|_| {
            PaygateError::InvalidConfig("Stripe internal auth secret must be UTF-8 text".to_owned())
        })?
        .trim()
        .as_bytes()
        .to_vec();
    if !(32..=256).contains(&secret.len()) || secret.iter().any(|byte| byte.is_ascii_control()) {
        return Err(PaygateError::InvalidConfig(
            "Stripe internal auth secret must contain 32-256 printable bytes".to_owned(),
        ));
    }
    Ok(secret)
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

fn validate_connect_redirect_url(field: &str, value: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(value)
        .map_err(|_| PaygateError::InvalidConfig(format!("{field} is invalid")))?;
    if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
        return Err(PaygateError::InvalidConfig(format!(
            "{field} must be an unauthenticated HTTPS URL"
        )));
    }
    Ok(())
}

fn validate_stripe_connect_client_id(client_id: &str) -> Result<()> {
    if client_id.starts_with("ca_") && is_safe_key_part(client_id) {
        Ok(())
    } else {
        Err(PaygateError::InvalidConfig(
            "stripe.connect_oauth_client_id is invalid".to_owned(),
        ))
    }
}

fn validate_stripe_oauth_token_url(value: &str, mode: StripeMode) -> Result<()> {
    let parsed = reqwest::Url::parse(value).map_err(|_| {
        PaygateError::InvalidConfig("stripe.connect_oauth_token_url is invalid".to_owned())
    })?;
    if parsed.username() != "" || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(PaygateError::InvalidConfig(
            "stripe.connect_oauth_token_url must not contain credentials or a fragment".to_owned(),
        ));
    }
    if mode == StripeMode::Live && value != DEFAULT_STRIPE_CONNECT_OAUTH_TOKEN_URL {
        return Err(PaygateError::InvalidConfig(
            "Stripe live mode requires the official Connect OAuth token URL".to_owned(),
        ));
    }
    let test_loopback = mode == StripeMode::Test
        && parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"));
    if parsed.scheme() != "https" && !test_loopback {
        return Err(PaygateError::InvalidConfig(
            "stripe.connect_oauth_token_url must use HTTPS".to_owned(),
        ));
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

fn normalize_stripe_country(value: &str) -> Result<String> {
    let country = value.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(PaygateError::InvalidRequest(
            "Stripe Connect country must be a two-letter ISO country code".to_owned(),
        ));
    }
    Ok(country)
}

fn validate_provider_id(provider: &str) -> Result<()> {
    if provider.len() != 64
        || provider != provider.to_ascii_lowercase()
        || !provider.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PaygateError::InvalidRequest(
            "provider must be a lower-case 32-byte hexadecimal public key".to_owned(),
        ));
    }
    Ok(())
}

fn validate_connect_request_nonce(nonce: &str) -> Result<()> {
    if nonce.len() != 64
        || nonce != nonce.to_ascii_lowercase()
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PaygateError::InvalidRequest(
            "request_nonce must be a lower-case 32-byte hexadecimal value".to_owned(),
        ));
    }
    Ok(())
}

fn validate_connect_context_revision(revision: &str) -> Result<()> {
    if revision.len() != 64
        || revision != revision.to_ascii_lowercase()
        || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PaygateError::InvalidRequest(
            "context_revision must be a lower-case 32-byte hexadecimal value".to_owned(),
        ));
    }
    Ok(())
}

fn validate_source_consent_signature(signature: &str) -> Result<()> {
    if signature.len() != 128
        || signature != signature.to_ascii_lowercase()
        || !signature.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PaygateError::InvalidRequest(
            "source_consent_signature must be a lower-case 64-byte hexadecimal value".to_owned(),
        ));
    }
    Ok(())
}

fn validate_target_service_signature(signature: &str) -> Result<()> {
    if signature.len() != 128
        || signature != signature.to_ascii_lowercase()
        || !signature.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PaygateError::InvalidRequest(
            "target_service_signature must be a lower-case 64-byte hexadecimal value".to_owned(),
        ));
    }
    Ok(())
}

fn validate_connect_oauth_state(state: &str) -> Result<()> {
    if state.len() == 64 && state.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(PaygateError::InvalidRequest(
            "Stripe OAuth state is invalid".to_owned(),
        ))
    }
}

fn validate_connect_oauth_code(code: &str) -> Result<()> {
    if code.starts_with("ac_") && is_safe_key_part(code) {
        Ok(())
    } else {
        Err(PaygateError::InvalidRequest(
            "Stripe OAuth authorization code is invalid".to_owned(),
        ))
    }
}

fn validate_stripe_account_id(account_id: &str) -> Result<()> {
    if account_id.starts_with("acct_") && is_safe_key_part(account_id) {
        Ok(())
    } else {
        Err(PaygateError::Stripe(
            "Stripe connected account id is invalid".to_owned(),
        ))
    }
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
    fn stripe_processor_errors_never_expose_credential_material() {
        let input =
            "Expired API Key provided: sk_test_********LAST123, pk_live_public456, ac_oauth789";
        let redacted = redact_stripe_credentials(input);
        assert_eq!(
            redacted,
            "Expired API Key provided: [REDACTED], [REDACTED], [REDACTED]"
        );
        assert!(!redacted.contains("LAST123"));
        assert!(!redacted.contains("public456"));
        assert!(!redacted.contains("oauth789"));
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
