use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    fmt,
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    audit::{
        aggregate_canary_fingerprints, evaluate_catalog_canary_token_prefix_probe,
        supported_canary_verification_method, token_fingerprint, CanaryProbeSpec,
        CANARY_VERIFICATION_TOKEN_FINGERPRINT, DEFAULT_CANARY_MATCH_MIN_BPS,
        DEFAULT_CANARY_TEMPERATURE,
    },
    failover::{
        midstream_stalled_after, x_mayhem_hedge_requested, FailoverPolicy, SessionFailoverState,
        SessionPriceMu, DEFAULT_MAX_OPEN_ATTEMPTS, DEFAULT_OPEN_TIMEOUT_MILLIS,
        DEFAULT_STALL_TIMEOUT_MILLIS,
    },
    pricing::{normalize_rate_map, text_generation_rate_map, text_usage_mu, RateMapEntry},
    verify_tier1_attestation, AttestationVerificationRequest, EnclaveContractRecord, ProviderKey,
    ReputationEventKind,
};
use axum::{
    body::Body,
    extract::{Multipart, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{
        STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
    },
    Engine as _,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use futures_util::stream;
use mayhem_bridge::{BridgeError, ScBridgeClient, ScBridgeConfig};
use mayhem_proto::{
    default_model_class, migrate_receipt_body, receipt_signing_bytes, session_accept_signing_bytes,
    session_frame_head, spend_voucher_signing_bytes, supported_receipt_signing_bytes,
    AttestationReport, CheckpointPolicy, ReceiptAck, ReceiptBody, ReceiptUsage, SessionReceipt,
    SpendVoucher, SpendVoucherBody, ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION, CONTRACT_VERSION,
    DEFAULT_MODEL_CLASS, SESSION_RECEIPT_SCHEMA_VERSION, USAGE_AUDIO_SECOND, USAGE_IMAGE,
    USAGE_INPUT_CHARACTER, USAGE_INPUT_TOKEN, USAGE_STEP,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;

type SharedState = Arc<GatewayState>;

const EMBEDDED_CATALOG: &str = include_str!("../../../catalog/models.json");
const EMBEDDED_CANARY_DEV_V1: &str = include_str!("../../../catalog/canaries/canary-dev-v1.json");
const EMBEDDED_CANARY_LAUNCH_V1: &str =
    include_str!("../../../catalog/canaries/canary-launch-v1.json");
const DASHBOARD_EXO_LATIN_WOFF2: &[u8] = include_bytes!("dashboard/exo-latin.woff2");
const X_MAYHEM_HEDGE_HEADER: &str = "x-mayhem-hedge";
const X_MAYHEM_MIN_ATT_TIER_HEADER: &str = "x-mayhem-min-att-tier";
const DEFAULT_CANARY_SEED: i64 = 7;
const DASHBOARD_SESSION_TTL_SECONDS: u64 = 15 * 60;
const DASHBOARD_COOKIE_NAME: &str = "mayhem_dashboard";
const DASHBOARD_CSP: &str = "default-src 'self'; connect-src 'self' http://127.0.0.1:*; img-src 'self' data:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'none'";
const DASHBOARD_CSS: &str = r#"
@font-face{font-family:Exo;src:url('/mayhem/dashboard/assets/exo-latin.woff2') format('woff2');font-style:normal;font-weight:400 700;font-display:swap}
:root{color-scheme:dark;--bg:rgb(11,11,12);--surface:rgb(22,22,26);--surface-card:rgb(24,24,27);--surface-raised:rgb(42,42,46);--border:rgb(42,42,46);--border-strong:rgb(41,41,41);--text-primary:rgb(229,231,235);--text-inverse:rgb(255,255,255);--text-muted:rgb(136,138,140);--accent-primary:rgb(197,68,89);--accent-primary-light:rgb(214,120,102);--accent-secondary:rgb(66,187,147);--radius-sm:6px;--radius-md:8px;--radius-pill:999px;--space-1:4px;--space-2:8px;--space-3:12px;--space-4:16px;--space-5:20px;--space-6:24px}
*{box-sizing:border-box;letter-spacing:0}body{margin:0;min-height:100vh;background:var(--bg);color:var(--text-primary);font-family:Exo,system-ui,sans-serif;font-size:15px;line-height:1.5}.nav{position:sticky;top:0;z-index:2;min-height:64px;display:grid;grid-template-columns:auto minmax(180px,500px) auto auto;gap:20px;align-items:center;padding:0 24px;background:rgba(22,22,26,.94);border-bottom:1px solid var(--border);backdrop-filter:blur(12px)}.brand,.wordmark{font-weight:700;color:var(--text-primary)}.brand{font-size:17px;text-decoration:none;white-space:nowrap}.wordmark{margin:0;font-size:64px;line-height:1}.wordmark.compact{font-size:22px}.hem,.wordmark .hem{background:linear-gradient(90deg,var(--accent-primary),var(--accent-primary-light));-webkit-background-clip:text;background-clip:text;color:transparent}.search{height:38px;border:1px solid var(--border);border-radius:var(--radius-pill);background:rgb(16,16,19);display:flex;align-items:center;padding:0 14px;color:var(--text-muted);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px;overflow:hidden;white-space:nowrap}.nav-links{display:flex;gap:18px}.nav-links a{color:var(--text-inverse);text-decoration:none;font-size:15px}.local-pill{justify-self:end;display:inline-flex;align-items:center;gap:7px;border-radius:var(--radius-pill);background:var(--accent-secondary);color:rgb(4,24,19);font-weight:700;font-size:12px;padding:7px 11px}.local-pill::before,.status-dot::before{content:"";width:8px;height:8px;border-radius:999px;background:currentColor}.dashboard{max-width:1280px;margin:0 auto;padding:48px 24px}.hero{text-align:center;margin:0 auto 34px;max-width:760px}.hero p{margin:12px auto 0;color:var(--text-muted);max-width:620px}.component-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:24px}.card{border:1px solid var(--border);border-radius:var(--radius-md);background:var(--surface-card);padding:20px;min-width:0}.card.strong{border:2px solid var(--border-strong)}.card-header{display:flex;align-items:center;justify-content:space-between;gap:14px;margin-bottom:18px}.card h2{margin:0;color:var(--text-inverse);font-size:22px;font-weight:600}.link{color:var(--accent-primary);text-decoration:none;font-weight:600}.detail-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:16px}.label{display:block;color:var(--text-muted);font-size:12px;text-transform:uppercase}.value{margin:4px 0 0;font-size:18px;font-weight:700}.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}.copy-row{display:flex;gap:8px;align-items:center;min-width:0}.copy-row .mono{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.copy-chip,.count-chip,.icon-toggle{border:1px solid var(--border);border-radius:var(--radius-sm);background:transparent;color:var(--text-primary);height:30px;display:inline-flex;align-items:center;justify-content:center}.copy-chip{padding:0 10px;font:inherit;font-size:13px;text-decoration:none}.count-chip{padding:0 10px;background:var(--surface-raised);font-family:ui-monospace,SFMono-Regular,Menlo,monospace}.status-dot{display:inline-flex;align-items:center;gap:8px;color:var(--accent-secondary);font-weight:600}.status-dot.muted{color:var(--text-muted)}.card-footer{display:flex;align-items:center;justify-content:space-between;gap:14px;margin:18px -20px -20px;padding:14px 20px;border-top:1px solid var(--border);color:var(--text-muted);font-size:13px}.chart-shell{height:220px;border-radius:var(--radius-md);background:linear-gradient(180deg,rgba(42,42,46,.35),rgba(24,24,27,.25));border:1px solid rgba(42,42,46,.7);position:relative;overflow:hidden}.chart-grid{position:absolute;inset:0;background:linear-gradient(to right,rgba(136,138,140,.08) 1px,transparent 1px),linear-gradient(to bottom,rgba(136,138,140,.08) 1px,transparent 1px);background-size:25% 25%}.chart-line{position:absolute;left:24px;right:24px;bottom:42px;height:88px;border-bottom:2px solid var(--accent-primary);transform:skewY(-8deg);box-shadow:0 26px 0 rgba(197,68,89,.1)}.chart-point{position:absolute;right:82px;top:70px;background:var(--accent-primary);color:var(--text-inverse);border-radius:var(--radius-sm);padding:5px 8px;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px}.toggle-row{display:flex;gap:8px;align-items:center}.icon-toggle{width:32px;background:var(--surface-raised)}.icon-toggle.active{border-color:var(--accent-primary);color:var(--accent-primary)}.empty-state{min-height:180px;display:grid;place-items:center;text-align:center;color:var(--text-muted)}.empty-icon{width:40px;height:40px;border-radius:var(--radius-md);border:1px solid var(--border);display:grid;place-items:center;margin:0 auto 12px;color:var(--accent-secondary)}.empty-icon::before{content:"";width:16px;height:16px;border-radius:50%;border:2px solid currentColor}.footer{border-top:1px solid var(--border);color:var(--text-muted);display:flex;justify-content:space-between;gap:16px;padding:18px 24px;font-size:13px}@media(max-width:900px){.nav{grid-template-columns:auto 1fr auto}.search{display:none}.nav-links{justify-content:flex-end}.component-grid,.detail-grid{grid-template-columns:1fr}.wordmark{font-size:48px}}@media(max-width:640px){.nav{padding:0 16px;gap:12px}.nav-links{gap:12px}.dashboard{padding:32px 16px}.wordmark{font-size:40px}.card-header,.card-footer,.footer{align-items:flex-start;flex-direction:column}}
"#;
const DASHBOARD_USER_CSS: &str = r#"
.overview-grid{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:16px;margin-bottom:24px}.overview-grid.provider{grid-template-columns:repeat(4,minmax(0,1fr))}.metric-card .value{font-size:24px}.wide-grid{display:grid;grid-template-columns:minmax(0,1.25fr) minmax(360px,.75fr);gap:24px}.wide-grid.provider{grid-template-columns:minmax(0,1fr) minmax(0,1fr)}.table{width:100%;border-collapse:collapse}.table th,.table td{border-bottom:1px solid var(--border);padding:11px 8px;text-align:left;vertical-align:middle}.table th{color:var(--text-muted);font-size:12px;text-transform:uppercase}.table td:last-child,.table th:last-child{text-align:right}.model-list{display:grid;gap:12px}.model-row{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:12px;align-items:center;border:1px solid var(--border);border-radius:var(--radius-md);padding:14px}.model-title{font-weight:700;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.model-meta{margin-top:5px;color:var(--text-muted);font-size:13px}.segmented{display:flex;gap:8px;flex-wrap:wrap}.segment{border:1px solid var(--border);border-radius:var(--radius-sm);height:30px;padding:0 10px;display:inline-flex;align-items:center;color:var(--text-muted)}.segment.active{border-color:var(--accent-primary);color:var(--accent-primary)}.toggle{display:inline-flex;align-items:center;gap:8px;color:var(--text-muted)}.toggle::before{content:"";width:28px;height:16px;border-radius:999px;border:1px solid var(--border);background:var(--surface-raised)}.spend-bars{height:180px;display:flex;align-items:end;gap:10px;padding:18px 12px 8px;border:1px solid var(--border);border-radius:var(--radius-md);background:linear-gradient(180deg,rgba(42,42,46,.24),rgba(24,24,27,.12))}.bar{flex:1;min-width:10px;border-radius:var(--radius-sm) var(--radius-sm) 0 0;background:linear-gradient(180deg,var(--accent-primary-light),var(--accent-primary));height:var(--h)}.mini-bar{height:8px;border-radius:999px;background:var(--surface-raised);overflow:hidden}.mini-bar span{display:block;height:100%;width:var(--w);background:linear-gradient(90deg,var(--accent-secondary),var(--accent-primary-light))}.opencode-card pre{margin:0;white-space:pre-wrap;overflow-wrap:anywhere;color:var(--text-primary);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px}.gateway-row{display:grid;grid-template-columns:1fr auto;gap:10px;align-items:center}.provider-scope{max-width:760px;margin:0 auto 20px;text-align:center}.privacy-note{color:var(--text-muted);font-size:13px}.claim-card pre{margin:0;white-space:pre-wrap;overflow-wrap:anywhere;color:var(--text-primary);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px}@media(max-width:1050px){.overview-grid,.overview-grid.provider,.wide-grid,.wide-grid.provider{grid-template-columns:1fr}.table{font-size:14px}}@media(max-width:640px){.table th:nth-child(3),.table td:nth-child(3){display:none}.overview-grid{gap:12px}}
"#;

#[derive(Clone, Debug)]
pub struct GatewayState {
    models: Arc<Vec<GatewayModel>>,
    receipts: Arc<Mutex<Vec<StoredReceipt>>>,
    probes: Arc<Mutex<Vec<StoredProbeEvent>>>,
    paused_sessions: Arc<Mutex<Vec<PausedSession>>>,
    receipt_config: ReceiptConfig,
    session_backend: Arc<dyn GatewaySessionBackend>,
    hardware_quote_trust: Arc<HardwareQuoteTrust>,
    canaries: Arc<GatewayCanaryRegistry>,
    canary_policy: GatewayCanaryProbePolicy,
    canary_scheduler: Arc<Mutex<GatewayCanaryScheduler>>,
    dashboard_session: Arc<DashboardSession>,
    provider_earnings: Arc<Vec<Value>>,
}

#[derive(Clone)]
struct DashboardSession {
    token: String,
    issued_at: Instant,
    ttl: Duration,
}

impl fmt::Debug for DashboardSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardSession")
            .field("token", &"<redacted>")
            .field("ttl", &self.ttl)
            .field("expires_in", &self.expires_in())
            .finish()
    }
}

impl DashboardSession {
    fn new() -> Self {
        Self {
            token: new_dashboard_token(),
            issued_at: Instant::now(),
            ttl: Duration::from_secs(DASHBOARD_SESSION_TTL_SECONDS),
        }
    }

    fn expires_in(&self) -> Duration {
        self.ttl.saturating_sub(self.issued_at.elapsed())
    }

    fn is_valid(&self, token: &str) -> bool {
        !self.expires_in().is_zero() && token == self.token
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GatewayModel {
    pub id: String,
    pub created: u64,
    pub owned_by: String,
    pub mayhem: MayhemModelInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MayhemModelInfo {
    #[serde(default = "default_model_class")]
    pub model_class: String,
    pub providers_online: u32,
    pub rooms: u32,
    pub price_ref_mu: PriceRefMu,
    pub attestation_tiers: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attestation_tier_labels: BTreeMap<String, String>,
    pub caps: ModelCaps,
    #[serde(default)]
    pub adapter: ShapeAdapterInfo,
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kyb_identities: Vec<ProviderKybInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_candidates: Vec<GatewayRouteCandidate>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProviderKybInfo {
    pub provider: String,
    pub legal_name: String,
    pub jurisdiction: String,
    pub proof_hash: String,
    pub kyb_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GatewayRouteCandidate {
    pub provider: String,
    pub enclave_id: String,
    pub room_id: String,
    pub price_ver: u64,
    pub att_tier: u8,
    pub admin_pubkey: String,
    pub artifact_root: String,
    pub manifest_hash: String,
    pub binary_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kyb: Option<ProviderKybInfo>,
    #[serde(default)]
    pub caps: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceRefMu {
    pub denom: String,
    pub ver: u64,
    pub rate_map: Vec<RateMapEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelCaps {
    pub tools: bool,
    pub json: bool,
    pub ctx: u32,
    pub vision: bool,
    #[serde(default)]
    pub image: bool,
    #[serde(default)]
    pub video: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_modality: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_modalities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ShapeAdapterInfo {
    pub request_shape_family: String,
    pub chat_template_id: String,
    pub tool_call_strategy: String,
    pub reasoning_passthrough: String,
    pub modality_set: Vec<String>,
    pub response_normalization: String,
}

impl Default for ShapeAdapterInfo {
    fn default() -> Self {
        Self {
            request_shape_family: "openai_chat".to_owned(),
            chat_template_id: "generic_chatml".to_owned(),
            tool_call_strategy: "mayhem_json".to_owned(),
            reasoning_passthrough: "strip".to_owned(),
            modality_set: vec!["text".to_owned()],
            response_normalization: "openai_chat".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredReceipt {
    pub voucher: SpendVoucher,
    pub receipt: SessionReceipt,
    pub receipt_ack: ReceiptAck,
}

#[derive(Clone, Debug, Serialize)]
pub struct PausedSession {
    pub session_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredProbeEvent {
    pub probe_id: String,
    pub model_id: String,
    pub provider: String,
    pub enclave_id: String,
    pub binary_hash: String,
    pub canary_set: String,
    pub verification_method: String,
    pub expected_fingerprint: String,
    pub observed_fingerprint: String,
    pub match_bps: u32,
    pub pass: bool,
    pub reputation_event_kind: ReputationEventKind,
    pub session_receipt_hash: String,
    pub evidence_hash: String,
    pub evidence: Value,
    pub probe_command: Value,
}

#[derive(Clone, Debug, Default)]
pub struct GatewayCanaryRegistry {
    pub models: BTreeMap<String, GatewayCanaryModelConfig>,
}

#[derive(Clone, Debug)]
pub struct GatewayCanaryModelConfig {
    pub canary_set: String,
    pub match_min_bps: u32,
    pub verification_method: String,
    pub verification_tolerance_bps: Option<u32>,
    pub prompts: Vec<GatewayCanaryPrompt>,
    pub fingerprints_by_artifact_root: BTreeMap<String, String>,
    pub token_prefixes_by_artifact_root: BTreeMap<String, BTreeMap<String, Vec<i32>>>,
    pub perceptual_hashes_by_artifact_root: BTreeMap<String, BTreeMap<String, String>>,
    pub default_fingerprint: Option<String>,
    pub default_token_prefixes: Option<BTreeMap<String, Vec<i32>>>,
    pub default_perceptual_hashes: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug)]
pub struct GatewayCanaryPrompt {
    pub id: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<Value>>,
    pub max_tokens: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayCanaryProbePolicy {
    pub enabled: bool,
    pub min_interval_sessions: u64,
    pub max_interval_sessions: u64,
    pub seed: i64,
    pub epoch: u64,
}

#[derive(Debug, Default)]
struct GatewayCanaryScheduler {
    counters: BTreeMap<String, u64>,
    next_after: BTreeMap<String, u64>,
    sequence: u64,
}

#[derive(Clone, Debug)]
struct ReceiptConfig {
    cosign_enabled: bool,
    balance_mu: u64,
    rules_ver: u64,
    checkpoint_every: CheckpointPolicy,
    user_seed: [u8; 32],
    provider_seed: [u8; 32],
    enclave_seed: [u8; 32],
}

#[derive(Clone, Debug, Default)]
struct HardwareQuoteTrust {
    apple_app_attest_jwks: Option<Value>,
    nvidia_gb10_device_jwks: Option<Value>,
    nvidia_nras_jwks: Option<Value>,
    nvidia_offline_jwks: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub response_format: Option<Value>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub stop: Option<Value>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

#[derive(Debug, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    #[serde(default)]
    pub prompt: Value,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub stop: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct EmbeddingRequest {
    pub model: String,
    pub input: Value,
    #[serde(default)]
    pub encoding_format: Option<String>,
    #[serde(default)]
    pub dimensions: Option<usize>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImageGenerationRequest {
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub n: Option<u32>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub response_format: Option<String>,
    #[serde(default)]
    pub steps: Option<u64>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AudioSpeechRequest {
    pub model: String,
    pub input: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub response_format: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
}

#[derive(Debug)]
struct AudioTranscriptionRequest {
    model: String,
    audio: Vec<u8>,
    filename: Option<String>,
    response_format: Option<String>,
    language: Option<String>,
    prompt: Option<String>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
    param: Option<&'static str>,
}

pub type GatewaySessionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GatewaySessionResult, GatewaySessionError>> + Send + 'a>>;

pub trait GatewaySessionBackend: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a>;
}

#[derive(Clone, Debug)]
pub struct GatewaySessionResult {
    pub output: ChatOutput,
    pub backend: String,
    pub direct_session: bool,
    pub provider_receipt: Option<ProviderSignedReceipt>,
    pub token_ids: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct ProviderSignedReceipt {
    pub body: ReceiptBody,
    pub enclave_sig: String,
    pub enclave_pubkey: String,
}

#[derive(Clone, Debug)]
pub struct GatewaySessionInvocation {
    pub contract_version: u32,
    pub session_id: String,
    pub user_pubkey: String,
    pub provider_pubkey: Option<String>,
    pub enclave_id: String,
    pub price_ver: u64,
    pub rules_ver: u64,
    pub spend_voucher: SpendVoucher,
    pub attestation: Option<GatewaySessionAttestation>,
    pub hedge: GatewayHedgeInvocation,
    receipt_cosign_enabled: bool,
    receipt_user_seed: [u8; 32],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GatewayHedgeInvocation {
    pub requested: bool,
    pub planned_probe_count: usize,
}

#[derive(Clone, Debug)]
pub struct GatewaySessionAttestation {
    pub contract: EnclaveContractRecord,
    pub trusted_binary_hashes: BTreeSet<String>,
    pub trusted_apple_app_attest_jwks: Option<Value>,
    pub trusted_nvidia_gb10_device_jwks: Option<Value>,
    pub trusted_nvidia_nras_jwks: Option<Value>,
    pub trusted_nvidia_offline_jwks: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct GatewaySessionError {
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug)]
struct LocalOpenAiShapeBackend;

#[derive(Clone, Debug)]
pub struct ScBridgeGatewaySessionConfig {
    pub url: String,
    pub token: String,
    pub open_timeout: Duration,
    pub frame_timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct ScBridgeGatewaySessionBackend {
    config: ScBridgeGatewaySessionConfig,
}

#[derive(Clone, Debug)]
pub struct ChatOutput {
    pub content: Option<String>,
    pub tool_call: Option<ToolCallOutput>,
    pub artifacts: Vec<GatewayArtifactOutput>,
    pub finish_reason: String,
    pub usage: Usage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayArtifactOutput {
    pub id: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub blake3: String,
}

#[derive(Clone, Debug)]
pub struct ToolCallOutput {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl GatewayState {
    pub fn from_embedded_catalog() -> Self {
        Self::from_catalog_json(EMBEDDED_CATALOG).unwrap_or_else(|_| Self::fixture())
    }

    pub fn from_catalog_json(catalog: &str) -> Result<Self, serde_json::Error> {
        let root: Value = serde_json::from_str(catalog)?;
        let created = 1_782_950_400;
        let models = root
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model_from_catalog_value(model, created))
            .collect::<Vec<_>>();
        let canaries = canary_registry_from_catalog_root(&root);
        if models.is_empty() {
            Ok(Self::fixture())
        } else {
            Ok(Self::with_models_and_canaries(models, canaries))
        }
    }

    pub fn fixture() -> Self {
        let mut tiers = BTreeMap::new();
        tiers.insert("T1".to_owned(), 1);
        Self::with_models(vec![GatewayModel {
            id: "mayhem/dev-chat-tools".to_owned(),
            created: 1_782_950_400,
            owned_by: "mayhem".to_owned(),
            mayhem: MayhemModelInfo {
                model_class: DEFAULT_MODEL_CLASS.to_owned(),
                providers_online: 1,
                rooms: 1,
                price_ref_mu: PriceRefMu {
                    denom: "mu_usd".to_owned(),
                    ver: 1,
                    rate_map: text_generation_rate_map(20, 60),
                },
                attestation_tiers: tiers,
                attestation_tier_labels: attestation_tier_labels_for_counts(&BTreeMap::from([(
                    "T1".to_owned(),
                    1,
                )])),
                caps: ModelCaps {
                    tools: true,
                    json: true,
                    ctx: 8192,
                    vision: false,
                    image: false,
                    video: false,
                    audio: false,
                    output_modality: Some("text".to_owned()),
                    output_modalities: vec!["text".to_owned()],
                },
                adapter: ShapeAdapterInfo::default(),
                source: "local-fixture".to_owned(),
                kyb_identities: Vec::new(),
                route_candidates: Vec::new(),
            },
        }])
    }

    pub fn from_models(models: Vec<GatewayModel>) -> Self {
        Self::with_models(models)
    }

    fn with_models(models: Vec<GatewayModel>) -> Self {
        Self::with_models_and_canaries(models, GatewayCanaryRegistry::default())
    }

    fn with_models_and_canaries(
        models: Vec<GatewayModel>,
        canaries: GatewayCanaryRegistry,
    ) -> Self {
        let models = sanitize_gateway_models(models);
        Self {
            models: Arc::new(models),
            receipts: Arc::new(Mutex::new(Vec::new())),
            probes: Arc::new(Mutex::new(Vec::new())),
            paused_sessions: Arc::new(Mutex::new(Vec::new())),
            receipt_config: ReceiptConfig::default(),
            session_backend: Arc::new(LocalOpenAiShapeBackend),
            hardware_quote_trust: Arc::new(HardwareQuoteTrust::default()),
            canaries: Arc::new(canaries),
            canary_policy: GatewayCanaryProbePolicy::default(),
            canary_scheduler: Arc::new(Mutex::new(GatewayCanaryScheduler::default())),
            dashboard_session: Arc::new(DashboardSession::new()),
            provider_earnings: Arc::new(Vec::new()),
        }
    }

    pub fn with_receipt_cosign_enabled(mut self, enabled: bool) -> Self {
        self.receipt_config.cosign_enabled = enabled;
        self
    }

    pub fn with_session_backend(mut self, backend: Arc<dyn GatewaySessionBackend>) -> Self {
        self.session_backend = backend;
        self
    }

    pub fn with_canary_registry(mut self, registry: GatewayCanaryRegistry) -> Self {
        self.canaries = Arc::new(registry);
        self.canary_scheduler = Arc::new(Mutex::new(GatewayCanaryScheduler::default()));
        self
    }

    pub fn with_canary_probe_policy(mut self, policy: GatewayCanaryProbePolicy) -> Self {
        self.canary_policy = policy;
        self.canary_scheduler = Arc::new(Mutex::new(GatewayCanaryScheduler::default()));
        self
    }

    pub fn with_provider_earnings(mut self, earnings: Vec<Value>) -> Self {
        self.provider_earnings = Arc::new(earnings);
        self
    }

    pub fn dashboard_url(&self, gateway_root: &str) -> String {
        format!(
            "{}/mayhem/dashboard?token={}",
            gateway_root.trim_end_matches('/'),
            self.dashboard_session.token
        )
    }

    pub fn dashboard_session_expires_in(&self) -> Duration {
        self.dashboard_session.expires_in()
    }

    pub fn with_apple_app_attest_jwks(mut self, jwks: Value) -> Self {
        let mut trust = (*self.hardware_quote_trust).clone();
        trust.apple_app_attest_jwks = Some(jwks);
        self.hardware_quote_trust = Arc::new(trust);
        self
    }

    pub fn with_nvidia_gb10_device_jwks(mut self, jwks: Value) -> Self {
        let mut trust = (*self.hardware_quote_trust).clone();
        trust.nvidia_gb10_device_jwks = Some(jwks);
        self.hardware_quote_trust = Arc::new(trust);
        self
    }

    pub fn with_nvidia_nras_jwks(mut self, jwks: Value) -> Self {
        let mut trust = (*self.hardware_quote_trust).clone();
        trust.nvidia_nras_jwks = Some(jwks);
        self.hardware_quote_trust = Arc::new(trust);
        self
    }

    pub fn with_nvidia_offline_jwks(mut self, jwks: Value) -> Self {
        let mut trust = (*self.hardware_quote_trust).clone();
        trust.nvidia_offline_jwks = Some(jwks);
        self.hardware_quote_trust = Arc::new(trust);
        self
    }

    pub fn receipts(&self) -> Vec<StoredReceipt> {
        self.receipts
            .lock()
            .expect("receipt store poisoned")
            .clone()
    }

    pub fn probes(&self) -> Vec<StoredProbeEvent> {
        self.probes.lock().expect("probe store poisoned").clone()
    }

    pub fn paused_sessions(&self) -> Vec<PausedSession> {
        self.paused_sessions
            .lock()
            .expect("paused session store poisoned")
            .clone()
    }

    fn receipt_count(&self) -> usize {
        self.receipts.lock().expect("receipt store poisoned").len()
    }

    fn paused_session_count(&self) -> usize {
        self.paused_sessions
            .lock()
            .expect("paused session store poisoned")
            .len()
    }

    fn record_receipt(&self, receipt: StoredReceipt) {
        self.receipts
            .lock()
            .expect("receipt store poisoned")
            .push(receipt);
    }

    fn record_probe(&self, probe: StoredProbeEvent) {
        self.probes
            .lock()
            .expect("probe store poisoned")
            .push(probe);
    }

    fn pause_session(&self, paused: PausedSession) {
        self.paused_sessions
            .lock()
            .expect("paused session store poisoned")
            .push(paused);
    }

    fn model(&self, id: &str) -> Option<GatewayModel> {
        self.models.iter().find(|model| model.id == id).cloned()
    }

    fn first_model(&self) -> Option<GatewayModel> {
        self.models.first().cloned()
    }
}

impl Default for ReceiptConfig {
    fn default() -> Self {
        Self {
            cosign_enabled: true,
            balance_mu: 1_000_000,
            rules_ver: 1,
            checkpoint_every: CheckpointPolicy {
                tokens: 8192,
                ms: 30000,
            },
            user_seed: [41_u8; 32],
            provider_seed: [42_u8; 32],
            enclave_seed: [43_u8; 32],
        }
    }
}

impl Default for GatewayCanaryProbePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            min_interval_sessions: 23,
            max_interval_sessions: 89,
            seed: DEFAULT_CANARY_SEED,
            epoch: 0,
        }
    }
}

impl GatewayCanaryProbePolicy {
    pub fn every_session_for_tests() -> Self {
        Self {
            enabled: true,
            min_interval_sessions: 1,
            max_interval_sessions: 1,
            seed: DEFAULT_CANARY_SEED,
            epoch: 0,
        }
    }
}

pub fn openai_router(state: GatewayState) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(create_chat_completion))
        .route("/v1/completions", post(create_completion))
        .route("/v1/embeddings", post(create_embedding))
        .route("/v1/images/generations", post(create_image_generation))
        .route("/v1/audio/speech", post(create_audio_speech))
        .route("/v1/audio/transcriptions", post(create_audio_transcription))
        .route("/mayhem/status", get(mayhem_status))
        .route("/mayhem/receipts", get(mayhem_receipts))
        .route("/mayhem/probes", get(mayhem_probes))
        .route("/mayhem/balance", get(mayhem_balance))
        .route("/mayhem/dashboard", get(mayhem_dashboard))
        .route("/mayhem/dashboard/provider", get(mayhem_dashboard_provider))
        .route(
            "/mayhem/dashboard/components",
            get(mayhem_dashboard_components),
        )
        .route("/mayhem/dashboard/session", get(mayhem_dashboard_session))
        .route(
            "/mayhem/dashboard/assets/exo-latin.woff2",
            get(mayhem_dashboard_exo_font),
        )
        .with_state(Arc::new(state))
}

pub async fn serve(bind: SocketAddr, state: GatewayState) -> std::io::Result<()> {
    validate_loopback_dashboard_bind(bind)?;
    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, openai_router(state)).await
}

pub fn validate_loopback_dashboard_bind(bind: SocketAddr) -> std::io::Result<()> {
    match bind {
        SocketAddr::V4(addr) if *addr.ip() == Ipv4Addr::LOCALHOST => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Mayhem gateway/dashboard must bind 127.0.0.1 only; got {bind}"),
        )),
    }
}

async fn list_models(State(state): State<SharedState>) -> Response {
    let data = state
        .models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "object": "model",
                "created": model.created,
                "owned_by": model.owned_by,
                "mayhem": model.mayhem,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "object": "list", "data": data })).into_response()
}

async fn create_chat_completion(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Response {
    let options = match GatewayRequestOptions::from_headers(&headers) {
        Ok(options) => options,
        Err(err) => return err.into_response(),
    };
    match build_chat_completion(&state, request, options).await {
        Ok(ChatResponse::Json(value)) => Json(value).into_response(),
        Ok(ChatResponse::Sse(chunks)) => sse_response(chunks),
        Err(err) => err.into_response(),
    }
}

async fn create_completion(
    State(state): State<SharedState>,
    Json(request): Json<CompletionRequest>,
) -> Response {
    match build_completion(&state, request) {
        Ok(ChatResponse::Json(value)) => Json(value).into_response(),
        Ok(ChatResponse::Sse(chunks)) => sse_response(chunks),
        Err(err) => err.into_response(),
    }
}

async fn create_embedding(
    State(state): State<SharedState>,
    Json(request): Json<EmbeddingRequest>,
) -> Response {
    match build_embedding(&state, request) {
        Ok(value) => Json(value).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn create_image_generation(
    State(state): State<SharedState>,
    Json(request): Json<ImageGenerationRequest>,
) -> Response {
    match build_image_generation(&state, request) {
        Ok(value) => Json(value).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn create_audio_speech(
    State(state): State<SharedState>,
    Json(request): Json<AudioSpeechRequest>,
) -> Response {
    match build_audio_speech(&state, request) {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

async fn create_audio_transcription(
    State(state): State<SharedState>,
    multipart: Multipart,
) -> Response {
    match parse_audio_transcription_multipart(multipart)
        .await
        .and_then(|request| build_audio_transcription(&state, request))
    {
        Ok(value) => Json(value).into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct DashboardQuery {
    token: Option<String>,
    provider: Option<String>,
}

async fn mayhem_dashboard(
    State(state): State<SharedState>,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Response {
    if !dashboard_request_authorized(&state, &headers, query.token.as_deref()) {
        return dashboard_html_response(
            StatusCode::UNAUTHORIZED,
            dashboard_locked_html(state.dashboard_session.expires_in().as_secs()),
            None,
        );
    }
    let origin = dashboard_origin_from_headers(&headers);
    dashboard_html_response(
        StatusCode::OK,
        dashboard_user_html(
            &state,
            state.dashboard_session.expires_in().as_secs(),
            &origin,
        ),
        Some(&state.dashboard_session.token),
    )
}

async fn mayhem_dashboard_provider(
    State(state): State<SharedState>,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Response {
    if !dashboard_request_authorized(&state, &headers, query.token.as_deref()) {
        return dashboard_html_response(
            StatusCode::UNAUTHORIZED,
            dashboard_locked_html(state.dashboard_session.expires_in().as_secs()),
            None,
        );
    }
    let origin = dashboard_origin_from_headers(&headers);
    dashboard_html_response(
        StatusCode::OK,
        dashboard_provider_html(
            &state,
            state.dashboard_session.expires_in().as_secs(),
            &origin,
            query.provider.as_deref(),
        ),
        Some(&state.dashboard_session.token),
    )
}

async fn mayhem_dashboard_components(
    State(state): State<SharedState>,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Response {
    if !dashboard_request_authorized(&state, &headers, query.token.as_deref()) {
        return dashboard_html_response(
            StatusCode::UNAUTHORIZED,
            dashboard_locked_html(state.dashboard_session.expires_in().as_secs()),
            None,
        );
    }
    dashboard_html_response(
        StatusCode::OK,
        dashboard_components_html(state.dashboard_session.expires_in().as_secs()),
        Some(&state.dashboard_session.token),
    )
}

async fn mayhem_dashboard_session(
    State(state): State<SharedState>,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Response {
    if !dashboard_request_authorized(&state, &headers, query.token.as_deref()) {
        return dashboard_json_response(
            StatusCode::UNAUTHORIZED,
            json!({
                "ok": false,
                "error": "dashboard_session_required",
            }),
            None,
        );
    }
    dashboard_json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "expires_in_seconds": state.dashboard_session.expires_in().as_secs(),
            "paths": {
                "dashboard": "/mayhem/dashboard",
                "provider_dashboard": "/mayhem/dashboard/provider",
                "status": "/mayhem/status",
                "models": "/v1/models",
                "receipts": "/mayhem/receipts",
                "balance": "/mayhem/balance",
            },
        }),
        Some(&state.dashboard_session.token),
    )
}

async fn mayhem_dashboard_exo_font(
    State(state): State<SharedState>,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Response {
    if !dashboard_request_authorized(&state, &headers, query.token.as_deref()) {
        return with_dashboard_security_headers(StatusCode::UNAUTHORIZED.into_response());
    }
    let mut response = Response::new(Body::from(DASHBOARD_EXO_LATIN_WOFF2.to_vec()));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("font/woff2"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_dashboard_security_headers(response)
}

async fn mayhem_status(State(state): State<SharedState>) -> Response {
    Json(json!({
        "ok": true,
        "version": 1,
        "contract_version": CONTRACT_VERSION,
        "backend": state.session_backend.name(),
        "models": state.models.len(),
        "sessions_active": 0,
        "sessions_paused": state.paused_session_count(),
        "receipts": state.receipt_count(),
        "probes": state.probes.lock().expect("probe store poisoned").len(),
    }))
    .into_response()
}

async fn mayhem_receipts(State(state): State<SharedState>) -> Response {
    Json(json!({
        "object": "list",
        "data": state.receipts(),
        "paused": state.paused_sessions(),
    }))
    .into_response()
}

async fn mayhem_probes(State(state): State<SharedState>) -> Response {
    Json(json!({
        "object": "list",
        "data": state.probes(),
    }))
    .into_response()
}

async fn mayhem_balance(State(state): State<SharedState>) -> Response {
    Json(json!({
        "denom": "mu_usd",
        "balance_mu": state.receipt_config.balance_mu,
        "held_mu": 0
    }))
    .into_response()
}

fn dashboard_request_authorized(
    state: &GatewayState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> bool {
    query_token
        .into_iter()
        .chain(dashboard_header_token(headers))
        .chain(dashboard_bearer_token(headers))
        .chain(dashboard_cookie_token(headers))
        .any(|token| state.dashboard_session.is_valid(token))
}

fn dashboard_header_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-mayhem-dashboard-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn dashboard_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn dashboard_cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie| {
            cookie.split(';').find_map(|part| {
                let (name, value) = part.trim().split_once('=')?;
                (name == DASHBOARD_COOKIE_NAME).then_some(value.trim())
            })
        })
        .filter(|value| !value.is_empty())
}

fn dashboard_html_response(status: StatusCode, body: String, token: Option<&str>) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    if let Some(token) = token {
        if let Ok(value) = HeaderValue::from_str(&dashboard_cookie_value(token)) {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }
    with_dashboard_security_headers(response)
}

fn dashboard_json_response(status: StatusCode, body: Value, token: Option<&str>) -> Response {
    let mut response = (status, Json(body)).into_response();
    if let Some(token) = token {
        if let Ok(value) = HeaderValue::from_str(&dashboard_cookie_value(token)) {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }
    with_dashboard_security_headers(response)
}

fn dashboard_cookie_value(token: &str) -> String {
    format!(
        "{DASHBOARD_COOKIE_NAME}={token}; Path=/mayhem/dashboard; Max-Age={DASHBOARD_SESSION_TTL_SECONDS}; HttpOnly; SameSite=Strict"
    )
}

fn with_dashboard_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(DASHBOARD_CSP),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

fn dashboard_origin_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|host| host.starts_with("127.0.0.1"))
        .map(|host| format!("http://{host}"))
        .unwrap_or_else(|| "http://127.0.0.1".to_owned())
}

fn dashboard_locked_html(expires_in_seconds: u64) -> String {
    dashboard_html_document(
        "Locked",
        &format!(
            r#"<main class="dashboard"><section class="card strong"><span class="label">Local session</span><h1 class="wordmark compact">MAY<span class="hem">HEM</span></h1><p class="muted">Dashboard token required.</p><p class="mono">expires in {expires_in_seconds}s</p></section></main>"#
        ),
    )
}

fn dashboard_user_html(state: &GatewayState, expires_in_seconds: u64, origin: &str) -> String {
    let receipts = state.receipts();
    let latest_receipts = dashboard_latest_receipts(&receipts);
    let active_sessions = latest_receipts
        .iter()
        .filter(|receipt| !receipt.receipt.body.final_receipt)
        .count()
        .saturating_add(state.paused_session_count());
    let lifetime_spend_mu = latest_receipts
        .iter()
        .map(|receipt| receipt.receipt.body.mu_owed_cum)
        .sum::<u64>();
    let gateway_root = origin.trim_end_matches('/');
    let openai_base_url = format!("{gateway_root}/v1");
    let session_rows = dashboard_session_rows(&latest_receipts);
    let model_rows = dashboard_model_rows(&state.models);
    let spend_body = dashboard_spend_body(&latest_receipts);
    let balance_usd = format_mu_usd(state.receipt_config.balance_mu);
    let lifetime_spend = format_mu_usd(lifetime_spend_mu);
    let api_key_masked = "mayhem-local";
    dashboard_html_document(
        "User Dashboard",
        &format!(
            r#"<nav class="nav"><a class="brand" href="/mayhem/dashboard">MAY<span class="hem">HEM</span></a><div class="search">{openai_base_url}</div><div class="nav-links"><a href="/mayhem/dashboard">User</a><a href="/mayhem/dashboard/provider">Provider</a><a href="/mayhem/dashboard/components">Components</a></div><span class="local-pill">LOCAL</span></nav><main class="dashboard"><section class="hero"><h1 class="wordmark">MAY<span class="hem">HEM</span></h1><p>User dashboard</p></section><section class="overview-grid"><article class="card metric-card"><span class="label">Balance</span><p class="value mono">{balance_usd}</p><p class="privacy-note">TAP rate not loaded</p></article><article class="card metric-card"><span class="label">Lifetime spend</span><p class="value mono">{lifetime_spend}</p><p class="privacy-note">from local receipts</p></article><article class="card metric-card"><span class="label">Active sessions</span><p class="value"><span class="count-chip">{active_sessions}</span></p><p class="privacy-note">running plus paused</p></article></section><section class="wide-grid"><article class="card"><div class="card-header"><h2>Sessions</h2><span class="count-chip">{receipt_count}</span></div><table class="table"><thead><tr><th>Model</th><th>Provider</th><th>Tokens</th><th>Cost</th><th>Status</th></tr></thead><tbody>{session_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Gateway</h2><span class="status-dot">Online</span></div><div class="detail-grid"><div><span class="label">Endpoint</span><div class="copy-row"><span class="mono">{openai_base_url}</span><button class="copy-chip" type="button">Copy</button></div></div><div><span class="label">API key</span><div class="copy-row"><span class="mono">mayhem-...</span><button class="copy-chip" type="button">Copy</button></div></div><div><span class="label">Session</span><p class="mono">{expires_in_seconds}s</p></div><div><span class="label">Bind</span><p class="mono">127.0.0.1</p></div></div></article><article class="card"><div class="card-header"><h2>Models</h2><div class="segmented"><span class="segment active">T1+</span><span class="segment">T2+</span><span class="segment">T3+</span><span class="toggle">KYB</span></div></div><div class="model-list">{model_rows}</div></article><article class="card"><div class="card-header"><h2>Spend</h2><span class="count-chip">{lifetime_spend}</span></div>{spend_body}<div class="card-footer"><span>from local receipts</span><span class="mono">{receipt_count} receipts</span></div></article><article class="card opencode-card"><div class="card-header"><h2>opencode</h2><button class="copy-chip" type="button">Copy</button></div><pre>OPENAI_BASE_URL={openai_base_url}
OPENAI_API_KEY={api_key_masked}</pre></article></section></main><footer class="footer"><span>Runs entirely on this machine. No external network calls.</span><span class="mono">127.0.0.1</span></footer>"#,
            receipt_count = receipts.len(),
        ),
    )
}

fn dashboard_provider_html(
    state: &GatewayState,
    expires_in_seconds: u64,
    origin: &str,
    provider_filter: Option<&str>,
) -> String {
    let receipts = state.receipts();
    let latest_receipts = dashboard_latest_receipts(&receipts);
    let probes = state.probes();
    let candidates = dashboard_provider_candidates(&state.models, provider_filter);
    let provider_scope = dashboard_provider_scope(
        provider_filter,
        &candidates,
        &latest_receipts,
        &probes,
        state.provider_earnings.as_ref(),
    );
    let earning_totals =
        dashboard_provider_earning_totals(state.provider_earnings.as_ref(), &provider_scope);
    let local_earned_mu = dashboard_provider_receipt_mu(&latest_receipts, &provider_scope);
    let earned_mu = if earning_totals.loaded {
        earning_totals.total_mu
    } else {
        local_earned_mu
    };
    let claimable_value = if earning_totals.loaded {
        format_mu_usd(earning_totals.claimable_mu)
    } else {
        "not loaded".to_owned()
    };
    let active_sessions = latest_receipts
        .iter()
        .filter(|receipt| {
            dashboard_provider_in_scope(&provider_scope, &receipt.receipt.body.provider)
                && !receipt.receipt.body.final_receipt
        })
        .count();
    let saturation_pct = dashboard_provider_saturation_pct(active_sessions, candidates.len());
    let reputation = dashboard_provider_reputation_bps(&latest_receipts, &probes, &provider_scope)
        .map(format_bps_percent)
        .unwrap_or_else(|| "not loaded".to_owned());
    let gateway_root = origin.trim_end_matches('/');
    let provider_scope_label = dashboard_provider_scope_label(provider_filter, &provider_scope);
    let provider_query = provider_filter
        .map(|provider| format!("?provider={}", html_escape(provider)))
        .unwrap_or_default();
    let enclave_rows = dashboard_provider_enclave_rows(&candidates, &latest_receipts);
    let live_session_rows =
        dashboard_provider_live_session_rows(&latest_receipts, &candidates, &provider_scope);
    let earnings_body =
        dashboard_provider_earnings_body(&latest_receipts, &provider_scope, &earning_totals);
    let holdback_body = dashboard_provider_holdback_body(&earning_totals);
    let hardware_body = dashboard_provider_hwprobe_body(&candidates, &probes, &provider_scope);
    let claim_body =
        dashboard_provider_claim_body(provider_filter, &provider_scope, &earning_totals);
    dashboard_html_document(
        "Provider Dashboard",
        &format!(
            r#"<nav class="nav"><a class="brand" href="/mayhem/dashboard">MAY<span class="hem">HEM</span></a><div class="search">{gateway_root}/mayhem/dashboard/provider{provider_query}</div><div class="nav-links"><a href="/mayhem/dashboard">User</a><a href="/mayhem/dashboard/provider">Provider</a><a href="/mayhem/dashboard/components">Components</a></div><span class="local-pill">LOCAL</span></nav><main class="dashboard"><section class="hero"><h1 class="wordmark">MAY<span class="hem">HEM</span></h1><p>Provider dashboard</p></section><p class="provider-scope mono">{provider_scope_label}</p><section class="overview-grid provider"><article class="card metric-card"><span class="label">Earned this epoch</span><p class="value mono">{earned}</p><p class="privacy-note">{earned_source}</p></article><article class="card metric-card"><span class="label">Pending claim</span><p class="value mono">{claimable_value}</p><p class="privacy-note">from mayhem earnings</p></article><article class="card metric-card"><span class="label">Reputation</span><p class="value mono">{reputation}</p><p class="privacy-note">local receipt/probe evidence</p></article><article class="card metric-card"><span class="label">Saturation</span><p class="value mono">{saturation_pct}%</p><p class="privacy-note">{active_sessions} active sessions</p></article></section><section class="wide-grid provider"><article class="card"><div class="card-header"><h2>Enclaves</h2><span class="count-chip">{candidate_count}</span></div><table class="table"><thead><tr><th>Model</th><th>Backend</th><th>Tier</th><th>Saturation</th><th>Status</th></tr></thead><tbody>{enclave_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Live sessions</h2><span class="count-chip">{receipt_count}</span></div><table class="table"><thead><tr><th>Room</th><th>Model</th><th>Tokens</th><th>Elapsed</th><th>Status</th></tr></thead><tbody>{live_session_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Earnings</h2><div class="segmented"><span class="segment active">Owed {claimable_value}</span><span class="segment">Paid {paid}</span></div></div>{earnings_body}<div class="card-footer"><span>{earnings_source}</span><span class="mono">{epoch_label}</span></div></article><article class="card"><div class="card-header"><h2>Reputation / Holdback</h2><span class="count-chip">{reputation}</span></div>{holdback_body}</article><article class="card"><div class="card-header"><h2>Hardware / Health</h2><span class="{hardware_status_class}">{hardware_status}</span></div>{hardware_body}</article><article class="card claim-card"><div class="card-header"><h2>Claim</h2><button class="copy-chip" type="button">Copy</button></div>{claim_body}</article></section></main><footer class="footer"><span>Local session {expires_in_seconds}s. Runs entirely on this machine. No external network calls.</span><span class="mono">127.0.0.1</span></footer>"#,
            earned = format_mu_usd(earned_mu),
            earned_source = if earning_totals.loaded {
                "ledger earn/* rows"
            } else {
                "local receipts only"
            },
            paid = format_mu_usd(earning_totals.paid_mu),
            earnings_source = if earning_totals.loaded {
                "matches mayhem earnings"
            } else {
                "ledger earnings not loaded"
            },
            epoch_label = earning_totals
                .updated_epoch
                .map(|epoch| format!("epoch {epoch}"))
                .unwrap_or_else(|| "epoch not loaded".to_owned()),
            hardware_status_class = if candidates.is_empty() {
                "status-dot muted"
            } else {
                "status-dot"
            },
            hardware_status = if candidates.is_empty() {
                "No route"
            } else {
                "Healthy"
            },
            candidate_count = candidates.len(),
            receipt_count = latest_receipts
                .iter()
                .filter(|receipt| dashboard_provider_in_scope(
                    &provider_scope,
                    &receipt.receipt.body.provider
                ))
                .count(),
        ),
    )
}

#[derive(Clone, Debug)]
struct DashboardProviderCandidate {
    provider: String,
    model_id: String,
    enclave_id: String,
    room_id: String,
    backend: String,
    att_tier: u8,
    kyb: Option<ProviderKybInfo>,
}

#[derive(Clone, Debug, Default)]
struct DashboardProviderEarningTotals {
    loaded: bool,
    total_mu: u64,
    held_mu: u64,
    paid_mu: u64,
    claimable_mu: u64,
    updated_epoch: Option<u64>,
    holdback_count: usize,
}

fn dashboard_provider_candidates(
    models: &[GatewayModel],
    provider_filter: Option<&str>,
) -> Vec<DashboardProviderCandidate> {
    let mut out = Vec::new();
    for model in models {
        for candidate in &model.mayhem.route_candidates {
            if provider_filter.is_some_and(|provider| provider != candidate.provider) {
                continue;
            }
            out.push(DashboardProviderCandidate {
                provider: candidate.provider.clone(),
                model_id: model.id.clone(),
                enclave_id: candidate.enclave_id.clone(),
                room_id: candidate.room_id.clone(),
                backend: model.mayhem.adapter.request_shape_family.clone(),
                att_tier: candidate.att_tier,
                kyb: candidate.kyb.clone(),
            });
        }
    }
    out
}

fn dashboard_provider_scope(
    provider_filter: Option<&str>,
    candidates: &[DashboardProviderCandidate],
    receipts: &[StoredReceipt],
    probes: &[StoredProbeEvent],
    earnings: &[Value],
) -> BTreeSet<String> {
    let mut scope = BTreeSet::new();
    if let Some(provider) = provider_filter {
        scope.insert(provider.to_owned());
        return scope;
    }
    for candidate in candidates {
        scope.insert(candidate.provider.clone());
    }
    for receipt in receipts {
        scope.insert(receipt.receipt.body.provider.clone());
    }
    for probe in probes {
        scope.insert(probe.provider.clone());
    }
    for entry in earnings {
        if let Some(provider) = entry.get("provider").and_then(Value::as_str) {
            scope.insert(provider.to_owned());
        }
    }
    scope
}

fn dashboard_provider_in_scope(scope: &BTreeSet<String>, provider: &str) -> bool {
    scope.is_empty() || scope.contains(provider)
}

fn dashboard_provider_scope_label(
    provider_filter: Option<&str>,
    scope: &BTreeSet<String>,
) -> String {
    if let Some(provider) = provider_filter {
        return format!(
            "provider {}",
            html_escape(short_text(provider, 34).as_ref())
        );
    }
    if scope.is_empty() {
        "all local providers".to_owned()
    } else {
        format!("all local providers ({})", scope.len())
    }
}

fn dashboard_provider_earning_totals(
    earnings: &[Value],
    scope: &BTreeSet<String>,
) -> DashboardProviderEarningTotals {
    let mut totals = DashboardProviderEarningTotals {
        loaded: !earnings.is_empty(),
        ..DashboardProviderEarningTotals::default()
    };
    for entry in earnings {
        let provider = entry.get("provider").and_then(Value::as_str).unwrap_or("");
        if !provider.is_empty() && !dashboard_provider_in_scope(scope, provider) {
            continue;
        }
        totals.total_mu = totals
            .total_mu
            .saturating_add(entry.get("total_mu").and_then(Value::as_u64).unwrap_or(0));
        totals.held_mu = totals
            .held_mu
            .saturating_add(entry.get("held_mu").and_then(Value::as_u64).unwrap_or(0));
        totals.paid_mu = totals.paid_mu.saturating_add(
            entry
                .get("paid_cum_mu")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        totals.claimable_mu = totals.claimable_mu.saturating_add(
            entry
                .get("claimable_mu")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        totals.updated_epoch = entry
            .get("updated_epoch")
            .and_then(Value::as_u64)
            .or(totals.updated_epoch)
            .max(totals.updated_epoch);
        totals.holdback_count = totals.holdback_count.saturating_add(
            entry
                .get("holdbacks")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
        );
    }
    totals
}

fn dashboard_provider_receipt_mu(receipts: &[StoredReceipt], scope: &BTreeSet<String>) -> u64 {
    receipts
        .iter()
        .filter(|receipt| dashboard_provider_in_scope(scope, &receipt.receipt.body.provider))
        .map(|receipt| receipt.receipt.body.mu_owed_cum)
        .sum()
}

fn dashboard_provider_saturation_pct(active_sessions: usize, candidate_count: usize) -> u64 {
    if candidate_count == 0 {
        return 0;
    }
    ((active_sessions as u64).saturating_mul(100) / candidate_count as u64).min(100)
}

fn dashboard_provider_reputation_bps(
    receipts: &[StoredReceipt],
    probes: &[StoredProbeEvent],
    scope: &BTreeSet<String>,
) -> Option<u32> {
    let mut total = 0_u32;
    let mut score = 0_u32;
    for receipt in receipts {
        if !dashboard_provider_in_scope(scope, &receipt.receipt.body.provider) {
            continue;
        }
        total = total.saturating_add(1);
        score = score.saturating_add(if receipt.receipt.body.final_receipt {
            10_000
        } else {
            5_000
        });
    }
    for probe in probes {
        if !dashboard_provider_in_scope(scope, &probe.provider) {
            continue;
        }
        total = total.saturating_add(1);
        score = score.saturating_add(if probe.pass { 10_000 } else { 0 });
    }
    if total == 0 {
        None
    } else {
        Some(score / total)
    }
}

fn dashboard_provider_enclave_rows(
    candidates: &[DashboardProviderCandidate],
    receipts: &[StoredReceipt],
) -> String {
    if candidates.is_empty() {
        return r#"<tr><td colspan="5"><span class="privacy-note">No provider routes loaded</span></td></tr>"#
            .to_owned();
    }
    candidates
        .iter()
        .take(10)
        .map(|candidate| {
            let active = receipts
                .iter()
                .filter(|receipt| {
                    receipt.receipt.body.provider == candidate.provider
                        && receipt.receipt.body.model_id == candidate.model_id
                        && !receipt.receipt.body.final_receipt
                })
                .count();
            let saturation = dashboard_provider_saturation_pct(active, 1);
            let kyb = candidate
                .kyb
                .as_ref()
                .map(|identity| {
                    format!(
                        " · KYB {} ({})",
                        html_escape(&identity.legal_name),
                        html_escape(&identity.jurisdiction)
                    )
                })
                .unwrap_or_default();
            format!(
                r#"<tr><td><span class="mono">{}</span><p class="privacy-note">{}</p></td><td class="mono">{}</td><td class="mono">T{}{}</td><td><div class="mini-bar"><span style="--w:{}%"></span></div><span class="privacy-note">{}%</span></td><td><span class="status-dot">Serving</span></td></tr>"#,
                html_escape(short_text(&candidate.model_id, 30).as_ref()),
                html_escape(short_text(&candidate.enclave_id, 22).as_ref()),
                html_escape(&candidate.backend),
                candidate.att_tier,
                kyb,
                saturation,
                saturation,
            )
        })
        .collect::<String>()
}

fn dashboard_provider_live_session_rows(
    receipts: &[StoredReceipt],
    candidates: &[DashboardProviderCandidate],
    scope: &BTreeSet<String>,
) -> String {
    let scoped = receipts
        .iter()
        .filter(|receipt| dashboard_provider_in_scope(scope, &receipt.receipt.body.provider))
        .take(8)
        .collect::<Vec<_>>();
    if scoped.is_empty() {
        return r#"<tr><td colspan="5"><span class="privacy-note">No served sessions yet</span></td></tr>"#
            .to_owned();
    }
    scoped
        .into_iter()
        .map(|receipt| {
            let body = &receipt.receipt.body;
            let room = candidates
                .iter()
                .find(|candidate| {
                    candidate.provider == body.provider
                        && candidate.model_id == body.model_id
                        && candidate.enclave_id == body.enclave_id
                })
                .map(|candidate| candidate.room_id.as_str())
                .unwrap_or("room not loaded");
            let status = if body.final_receipt {
                r#"<span class="status-dot muted">Completed</span>"#
            } else {
                r#"<span class="status-dot">Streaming</span>"#
            };
            format!(
                r#"<tr><td><div class="copy-row"><span class="mono">{}</span><button class="copy-chip" type="button">Copy</button></div></td><td class="mono">{}</td><td class="mono">{}/{}</td><td class="mono">{}</td><td>{status}</td></tr>"#,
                html_escape(short_text(room, 18).as_ref()),
                html_escape(short_text(&body.model_id, 24).as_ref()),
                body.usage.input_tokens(),
                body.usage.output_tokens(),
                format_elapsed_since(body.ts),
            )
        })
        .collect::<String>()
}

fn dashboard_provider_earnings_body(
    receipts: &[StoredReceipt],
    scope: &BTreeSet<String>,
    totals: &DashboardProviderEarningTotals,
) -> String {
    if totals.loaded {
        let max = totals
            .total_mu
            .max(totals.held_mu)
            .max(totals.paid_mu)
            .max(totals.claimable_mu)
            .max(1);
        let bars = [
            ("total", totals.total_mu),
            ("held", totals.held_mu),
            ("paid", totals.paid_mu),
            ("claimable", totals.claimable_mu),
        ]
        .into_iter()
        .map(|(label, value)| {
            let height = 8 + value.saturating_mul(92) / max;
            format!(
                r#"<span class="bar" style="--h:{height}%" title="{label} {}"></span>"#,
                format_mu_usd(value)
            )
        })
        .collect::<String>();
        return format!(r#"<div class="spend-bars">{bars}</div>"#);
    }
    let scoped = receipts
        .iter()
        .filter(|receipt| dashboard_provider_in_scope(scope, &receipt.receipt.body.provider))
        .cloned()
        .collect::<Vec<_>>();
    dashboard_spend_body(&scoped)
}

fn dashboard_provider_holdback_body(totals: &DashboardProviderEarningTotals) -> String {
    if !totals.loaded {
        return r#"<div class="empty-state"><div><div class="empty-icon"></div><p>Ledger holdback not loaded</p></div></div>"#
            .to_owned();
    }
    let held_pct = if totals.total_mu == 0 {
        0
    } else {
        totals.held_mu.saturating_mul(100) / totals.total_mu
    };
    format!(
        r#"<div class="detail-grid"><div><span class="label">Held</span><p class="value mono">{}</p></div><div><span class="label">Release buckets</span><p class="value mono">{}</p></div><div><span class="label">Released</span><p class="value mono">{}</p></div><div><span class="label">Claim model</span><p class="value mono">TAP claim</p></div></div><div class="mini-bar"><span style="--w:{}%"></span></div>"#,
        format_mu_usd(totals.held_mu),
        totals.holdback_count,
        format_mu_usd(totals.claimable_mu),
        held_pct.min(100),
    )
}

fn dashboard_provider_hwprobe_body(
    candidates: &[DashboardProviderCandidate],
    probes: &[StoredProbeEvent],
    scope: &BTreeSet<String>,
) -> String {
    let last_probe = probes
        .iter()
        .filter(|probe| dashboard_provider_in_scope(scope, &probe.provider))
        .max_by_key(|probe| {
            probe
                .evidence
                .get("at")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        });
    let max_tier = candidates
        .iter()
        .map(|candidate| candidate.att_tier)
        .max()
        .unwrap_or(0);
    let max_tier_label = if max_tier == 0 {
        "not loaded".to_owned()
    } else {
        format!("T{max_tier}")
    };
    let providers = candidates
        .iter()
        .map(|candidate| candidate.provider.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let last_probe_label = last_probe
        .map(|probe| {
            if probe.pass {
                format!("probe ok {}", format_bps_percent(probe.match_bps))
            } else {
                format!("probe fail {}", format_bps_percent(probe.match_bps))
            }
        })
        .unwrap_or_else(|| "no probe evidence".to_owned());
    format!(
        r#"<div class="detail-grid"><div><span class="label">Providers</span><p class="value mono">{providers}</p></div><div><span class="label">Max tier</span><p class="value mono">{}</p></div><div><span class="label">Last probe</span><p class="value mono">{}</p></div><div><span class="label">hwprobe</span><p class="value mono">not loaded</p></div></div>"#,
        html_escape(&max_tier_label),
        html_escape(&last_probe_label),
    )
}

fn dashboard_provider_claim_body(
    provider_filter: Option<&str>,
    scope: &BTreeSet<String>,
    totals: &DashboardProviderEarningTotals,
) -> String {
    let provider_arg = provider_filter
        .or_else(|| scope.iter().next().map(String::as_str))
        .map(|provider| format!(" --provider {}", shell_single_quote_dashboard(provider)))
        .unwrap_or_default();
    let claimable = if totals.loaded {
        format_mu_usd(totals.claimable_mu)
    } else {
        "not loaded".to_owned()
    };
    format!(
        r#"<div class="detail-grid"><div><span class="label">Claimable</span><p class="value mono">{claimable}</p></div><div><span class="label">Payout target</span><p class="value mono">claim proof required</p></div></div><pre>mayhem earnings{provider_arg} --json
mayhem withdraw --claim-proof &lt;claim-proof.json&gt; --account &lt;tap-account&gt; --json</pre>"#
    )
}

fn format_bps_percent(bps: u32) -> String {
    format!("{}.{:02}%", bps / 100, bps % 100)
}

fn format_elapsed_since(ts: u64) -> String {
    let age = now_secs().saturating_sub(ts);
    if age < 60 {
        format!("{age}s")
    } else if age < 3_600 {
        format!("{}m", age / 60)
    } else {
        format!("{}h", age / 3_600)
    }
}

fn shell_single_quote_dashboard(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn dashboard_components_html(expires_in_seconds: u64) -> String {
    dashboard_html_document(
        "Local Dashboard",
        &format!(
            r#"<nav class="nav"><a class="brand" href="/mayhem/dashboard">MAY<span class="hem">HEM</span></a><div class="search">127.0.0.1 local gateway</div><div class="nav-links"><a href="/mayhem/dashboard">User</a><a href="/mayhem/dashboard/provider">Provider</a></div><span class="local-pill">LOCAL</span></nav><main class="dashboard"><section class="hero"><h1 class="wordmark">MAY<span class="hem">HEM</span></h1><p>Local dashboard surface</p></section><section class="component-grid"><article class="card strong"><div class="card-header"><h2>Account</h2><a class="link" href="/mayhem/dashboard">View all</a></div><div class="detail-grid"><div><span class="label">Balance</span><p class="value mono">1,240.00 TAP</p></div><div><span class="label">Sessions</span><p class="value"><span class="count-chip">12</span></p></div><div><span class="label">Provider</span><div class="copy-row"><span class="mono">testtrac1n57xm5de...</span><button class="copy-chip" type="button">Copy</button></div></div><div><span class="label">Gateway</span><p class="value mono">127.0.0.1</p></div></div><div class="card-footer"><span>Local session {expires_in_seconds}s</span><span class="status-dot">Online</span></div></article><article class="card"><div class="card-header"><h2>Throughput</h2><div class="toggle-row"><span class="count-chip">42 tok/s</span><span class="icon-toggle active">L</span><span class="icon-toggle">B</span></div></div><div class="chart-shell"><div class="chart-grid"></div><div class="chart-line"></div><span class="chart-point">42</span></div><div class="card-footer"><span>Synced locally</span><span class="status-dot muted">Idle</span></div></article><article class="card"><div class="card-header"><h2>Components</h2><span class="count-chip">06</span></div><div class="detail-grid"><div><span class="label">Status</span><p class="status-dot">Online</p></div><div><span class="label">Copy</span><button class="copy-chip" type="button">Copy</button></div><div><span class="label">Count</span><span class="count-chip">128</span></div><div><span class="label">Mono</span><p class="mono">mx/s/session</p></div></div></article><article class="card"><div class="card-header"><h2>Queue</h2><a class="link" href="/mayhem/dashboard">View all</a></div><div class="empty-state"><div><div class="empty-icon"></div><p>No sessions yet</p></div></div></article></section></main><footer class="footer"><span>Runs entirely on this machine. No external network calls.</span><span class="mono">127.0.0.1</span></footer>"#
        ),
    )
}

fn dashboard_latest_receipts(receipts: &[StoredReceipt]) -> Vec<StoredReceipt> {
    let mut latest = BTreeMap::<String, StoredReceipt>::new();
    for receipt in receipts {
        let body = &receipt.receipt.body;
        let replace = latest
            .get(&body.session_id)
            .map(|current| body.seq > current.receipt.body.seq)
            .unwrap_or(true);
        if replace {
            latest.insert(body.session_id.clone(), receipt.clone());
        }
    }
    let mut out = latest.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.receipt
            .body
            .ts
            .cmp(&a.receipt.body.ts)
            .then_with(|| b.receipt.body.seq.cmp(&a.receipt.body.seq))
    });
    out
}

fn dashboard_session_rows(receipts: &[StoredReceipt]) -> String {
    if receipts.is_empty() {
        return r#"<tr><td colspan="5"><span class="privacy-note">No sessions yet</span></td></tr>"#
            .to_owned();
    }
    receipts
        .iter()
        .take(8)
        .map(|receipt| {
            let body = &receipt.receipt.body;
            let status = if body.final_receipt {
                r#"<span class="status-dot muted">Completed</span>"#
            } else {
                r#"<span class="status-dot">Running</span>"#
            };
            format!(
                r#"<tr><td class="mono">{}</td><td><div class="copy-row"><span class="mono">{}</span><button class="copy-chip" type="button">Copy</button></div></td><td class="mono">{}/{}</td><td class="mono">{}</td><td>{status}</td></tr>"#,
                html_escape(short_text(&body.model_id, 28).as_ref()),
                html_escape(short_text(&body.provider, 18).as_ref()),
                body.usage.input_tokens(),
                body.usage.output_tokens(),
                format_mu_usd(body.mu_owed_cum),
            )
        })
        .collect::<String>()
}

fn dashboard_model_rows(models: &[GatewayModel]) -> String {
    if models.is_empty() {
        return r#"<div class="empty-state"><div><div class="empty-icon"></div><p>No models loaded</p></div></div>"#
            .to_owned();
    }
    models
        .iter()
        .take(6)
        .map(|model| {
            let availability = if model.mayhem.providers_online > 0 {
                r#"<span class="status-dot">Online</span>"#
            } else {
                r#"<span class="status-dot muted">Offline</span>"#
            };
            let max_tier = model
                .mayhem
                .attestation_tiers
                .values()
                .copied()
                .max()
                .unwrap_or(1)
                .max(1);
            let kyb = model
                .mayhem
                .kyb_identities
                .first()
                .map(|identity| {
                    format!(
                        " · verified: {} ({})",
                        html_escape(&identity.legal_name),
                        html_escape(&identity.jurisdiction)
                    )
                })
                .unwrap_or_default();
            format!(
                r#"<div class="model-row"><div><div class="model-title mono">{}</div><div class="model-meta">{} · {} · T{}{} · {}</div></div><a class="copy-chip" href="/mayhem/dashboard">Use</a></div>"#,
                html_escape(&model.id),
                html_escape(&model.mayhem.model_class),
                html_escape(&dashboard_model_price(model)),
                max_tier,
                kyb,
                availability,
            )
        })
        .collect::<String>()
}

fn dashboard_model_price(model: &GatewayModel) -> String {
    let entries = model
        .mayhem
        .price_ref_mu
        .rate_map
        .iter()
        .take(3)
        .map(|entry| {
            format!(
                "{}mu/{}/{}",
                entry.per_unit_mu, entry.granularity, entry.unit
            )
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        "unpriced".to_owned()
    } else {
        entries.join(" + ")
    }
}

fn dashboard_spend_body(receipts: &[StoredReceipt]) -> String {
    if receipts.is_empty() {
        return r#"<div class="empty-state"><div><div class="empty-icon"></div><p>No spend yet</p></div></div>"#
            .to_owned();
    }
    let max = receipts
        .iter()
        .map(|receipt| receipt.receipt.body.mu_owed_cum)
        .max()
        .unwrap_or(1)
        .max(1);
    let bars = receipts
        .iter()
        .take(12)
        .map(|receipt| {
            let height = 8 + (receipt.receipt.body.mu_owed_cum.saturating_mul(92) / max);
            format!(r#"<span class="bar" style="--h:{height}%"></span>"#)
        })
        .collect::<String>();
    format!(r#"<div class="spend-bars">{bars}</div>"#)
}

fn format_mu_usd(mu: u64) -> String {
    let cents = mu.saturating_add(5_000) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

fn short_text(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let keep = max.saturating_sub(3).max(1);
    format!("{}...", value.chars().take(keep).collect::<String>())
}

fn html_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn dashboard_html_document(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta http-equiv="Content-Security-Policy" content="{DASHBOARD_CSP}"><title>Mayhem {title}</title><style>{DASHBOARD_CSS}{DASHBOARD_USER_CSS}</style></head><body>{body}</body></html>"#
    )
}

fn new_dashboard_token() -> String {
    let mut bytes = [0_u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"mayhem-dashboard-token-fallback");
        hasher.update(
            &SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
                .to_be_bytes(),
        );
        hasher.update(&(bytes.as_ptr() as usize).to_be_bytes());
        bytes.copy_from_slice(hasher.finalize().as_bytes());
    }
    BASE64_URL_SAFE_NO_PAD.encode(bytes)
}

enum ChatResponse {
    Json(Value),
    Sse(Vec<Value>),
}

#[derive(Clone, Copy)]
struct ResponseMayhemMeta<'a> {
    backend: &'a str,
    direct_session: bool,
    hedge: GatewayHedgeInvocation,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct GatewayRequestOptions {
    hedge_requested: bool,
    min_att_tier: Option<u8>,
}

#[derive(Clone, Debug)]
struct ValidatedDirectSessionAccept {
    enclave_pubkey: String,
}

#[derive(Clone, Debug)]
struct DirectSessionCollected {
    output: ChatOutput,
    provider_receipt: ProviderSignedReceipt,
    token_ids: Vec<i32>,
}

#[derive(Debug, Deserialize)]
struct ProviderReceiptWire {
    #[serde(flatten)]
    body: ReceiptBody,
    enclave_sig: String,
}

#[derive(Debug, Deserialize)]
struct CanarySetDocument {
    set_id: String,
    #[serde(default)]
    prompts: Vec<CanaryPromptDocument>,
}

#[derive(Debug, Deserialize)]
struct CanaryPromptDocument {
    id: String,
    #[serde(default)]
    messages: Vec<ChatMessage>,
    #[serde(default)]
    tools: Option<Vec<Value>>,
    #[serde(default)]
    max_tokens: Option<u32>,
}

struct ExpectedProviderReceipt<'a> {
    provider: &'a str,
    usage: ReceiptUsage,
    mu_owed_cum: u64,
    prompt_hash: String,
}

fn canary_registry_from_catalog_root(root: &Value) -> GatewayCanaryRegistry {
    let canary_sets = embedded_canary_sets();
    let mut models = BTreeMap::new();
    let Some(model_values) = root.get("models").and_then(Value::as_array) else {
        return GatewayCanaryRegistry { models };
    };
    for model in model_values {
        let Some(model_id) = model.get("model_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(canary) = model.get("canary") else {
            continue;
        };
        let Some(canary_set) = canary.get("set_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(prompts) = canary_sets.get(canary_set).cloned() else {
            continue;
        };
        let match_min_bps = canary
            .get("match_min")
            .and_then(Value::as_f64)
            .map(|value| (value * 10_000.0).round().clamp(0.0, 10_000.0) as u32)
            .unwrap_or(DEFAULT_CANARY_MATCH_MIN_BPS);
        let verification_method = canary
            .get("verification_method")
            .and_then(Value::as_str)
            .unwrap_or(CANARY_VERIFICATION_TOKEN_FINGERPRINT)
            .to_owned();
        if !supported_canary_verification_method(&verification_method) {
            continue;
        }
        let verification_tolerance_bps = canary
            .get("verification_tolerance_bps")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let fingerprints = canary
            .get("fingerprints")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|object| object.iter())
            .filter_map(|(artifact_name, value)| {
                value
                    .as_str()
                    .map(|fingerprint| (artifact_name.as_str(), fingerprint.to_owned()))
            })
            .collect::<BTreeMap<_, _>>();
        let token_prefixes = canary_token_prefixes_by_artifact(canary);
        if verification_method == CANARY_VERIFICATION_TOKEN_FINGERPRINT
            && (fingerprints.is_empty() || token_prefixes.is_empty())
        {
            continue;
        }
        let perceptual_hashes = canary_perceptual_hashes_by_artifact(canary);
        if verification_method != CANARY_VERIFICATION_TOKEN_FINGERPRINT
            && perceptual_hashes.is_empty()
        {
            continue;
        }
        let mut fingerprints_by_artifact_root = BTreeMap::new();
        let mut token_prefixes_by_artifact_root = BTreeMap::new();
        let mut perceptual_hashes_by_artifact_root = BTreeMap::new();
        if let Some(artifacts) = model.get("artifacts").and_then(Value::as_object) {
            for (artifact_name, artifact) in artifacts {
                if let Some(artifact_root) = artifact.get("artifact_root").and_then(Value::as_str) {
                    if verification_method == CANARY_VERIFICATION_TOKEN_FINGERPRINT {
                        let Some(fingerprint) = fingerprints.get(artifact_name.as_str()) else {
                            continue;
                        };
                        let Some(prefixes) = token_prefixes.get(artifact_name.as_str()) else {
                            continue;
                        };
                        let expected_fingerprint =
                            aggregate_token_prefixes_for_prompts(&prompts, prefixes);
                        if expected_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                            continue;
                        }
                        fingerprints_by_artifact_root
                            .insert(artifact_root.to_owned(), fingerprint.clone());
                        token_prefixes_by_artifact_root
                            .insert(artifact_root.to_owned(), prefixes.clone());
                    } else if let Some(hashes) = perceptual_hashes.get(artifact_name.as_str()) {
                        perceptual_hashes_by_artifact_root
                            .insert(artifact_root.to_owned(), hashes.clone());
                    }
                }
            }
        }
        let default_fingerprint = fingerprints.values().next().cloned();
        let default_token_prefixes = token_prefixes.values().next().cloned();
        let default_perceptual_hashes = perceptual_hashes.values().next().cloned();
        models.insert(
            model_id.to_owned(),
            GatewayCanaryModelConfig {
                canary_set: canary_set.to_owned(),
                match_min_bps,
                verification_method,
                verification_tolerance_bps,
                prompts,
                fingerprints_by_artifact_root,
                token_prefixes_by_artifact_root,
                perceptual_hashes_by_artifact_root,
                default_fingerprint,
                default_token_prefixes,
                default_perceptual_hashes,
            },
        );
    }
    GatewayCanaryRegistry { models }
}

fn canary_token_prefixes_by_artifact(
    canary: &Value,
) -> BTreeMap<String, BTreeMap<String, Vec<i32>>> {
    canary
        .get("token_prefixes")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|object| object.iter())
        .filter_map(|(artifact_name, value)| {
            let prompts = value.as_object()?;
            let prefixes = prompts
                .iter()
                .filter_map(|(prompt_id, raw_tokens)| {
                    let tokens = raw_tokens
                        .as_array()?
                        .iter()
                        .map(|token| token.as_i64().and_then(|token| i32::try_from(token).ok()))
                        .collect::<Option<Vec<_>>>()?;
                    (!tokens.is_empty()).then_some((prompt_id.clone(), tokens))
                })
                .collect::<BTreeMap<_, _>>();
            (!prefixes.is_empty()).then_some((artifact_name.clone(), prefixes))
        })
        .collect()
}

fn canary_perceptual_hashes_by_artifact(
    canary: &Value,
) -> BTreeMap<String, BTreeMap<String, String>> {
    canary
        .get("perceptual_hashes")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|object| object.iter())
        .filter_map(|(artifact_name, value)| {
            let prompts = value.as_object()?;
            let prompts = prompts
                .iter()
                .filter_map(|(prompt_id, hash)| {
                    hash.as_str()
                        .filter(|hash| !hash.is_empty())
                        .map(|hash| (prompt_id.clone(), hash.to_owned()))
                })
                .collect::<BTreeMap<_, _>>();
            (!prompts.is_empty()).then(|| (artifact_name.clone(), prompts))
        })
        .collect()
}

fn aggregate_token_prefixes_for_prompts(
    prompts: &[GatewayCanaryPrompt],
    prefixes: &BTreeMap<String, Vec<i32>>,
) -> Option<String> {
    let mut prompt_fingerprints = Vec::with_capacity(prompts.len());
    for prompt in prompts {
        let tokens = prefixes.get(&prompt.id)?;
        let fingerprint = token_fingerprint(tokens.iter().copied()).digest;
        prompt_fingerprints.push((prompt.id.as_str(), fingerprint));
    }
    Some(aggregate_canary_fingerprints(
        prompt_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    ))
}

fn embedded_canary_sets() -> BTreeMap<String, Vec<GatewayCanaryPrompt>> {
    [EMBEDDED_CANARY_DEV_V1, EMBEDDED_CANARY_LAUNCH_V1]
        .into_iter()
        .filter_map(|raw| serde_json::from_str::<CanarySetDocument>(raw).ok())
        .map(|doc| {
            let prompts = doc
                .prompts
                .into_iter()
                .map(|prompt| GatewayCanaryPrompt {
                    id: prompt.id,
                    messages: prompt.messages,
                    tools: prompt.tools,
                    max_tokens: prompt.max_tokens.unwrap_or(64).max(1),
                })
                .collect::<Vec<_>>();
            (doc.set_id, prompts)
        })
        .collect()
}

fn sanitize_gateway_models(models: Vec<GatewayModel>) -> Vec<GatewayModel> {
    models
        .into_iter()
        .filter_map(|mut model| {
            model
                .mayhem
                .route_candidates
                .retain(canonical_route_candidate);
            if !model.mayhem.route_candidates.is_empty() {
                let providers = model
                    .mayhem
                    .route_candidates
                    .iter()
                    .map(|candidate| candidate.provider.as_str())
                    .collect::<BTreeSet<_>>();
                let rooms = model
                    .mayhem
                    .route_candidates
                    .iter()
                    .map(|candidate| candidate.room_id.as_str())
                    .collect::<BTreeSet<_>>();
                model.mayhem.providers_online = providers.len().min(u32::MAX as usize) as u32;
                model.mayhem.rooms = rooms.len().min(u32::MAX as usize) as u32;
            }
            if model.mayhem.source == "contract" && model.mayhem.route_candidates.is_empty() {
                return None;
            }
            Some(model)
        })
        .collect()
}

fn canonical_route_candidate(candidate: &GatewayRouteCandidate) -> bool {
    is_hex_len(&candidate.provider, 64)
        && is_hex_len(&candidate.enclave_id, 64)
        && is_hex_len(&candidate.room_id, 32)
        && is_hex_len(&candidate.admin_pubkey, 64)
        && is_hex_len(&candidate.artifact_root, 64)
        && is_hex_len(&candidate.manifest_hash, 64)
        && is_hex_len(&candidate.binary_hash, 64)
}

impl GatewaySessionError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    pub fn into_retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

impl GatewayRequestOptions {
    fn from_headers(headers: &HeaderMap) -> Result<Self, ApiError> {
        Ok(Self {
            hedge_requested: parse_x_mayhem_hedge(headers)?,
            min_att_tier: parse_x_mayhem_min_att_tier(headers)?,
        })
    }
}

fn parse_x_mayhem_min_att_tier(headers: &HeaderMap) -> Result<Option<u8>, ApiError> {
    let Some(value) = headers.get(X_MAYHEM_MIN_ATT_TIER_HEADER) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Min-Att-Tier must be an ASCII integer from 1 through 4",
            Some("X-Mayhem-Min-Att-Tier"),
        )
    })?;
    let value = value.trim();
    let value = value.strip_prefix('T').unwrap_or(value);
    let tier = value.parse::<u8>().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Min-Att-Tier must be an integer from 1 through 4",
            Some("X-Mayhem-Min-Att-Tier"),
        )
    })?;
    if (1..=4).contains(&tier) {
        Ok(Some(tier))
    } else {
        Err(ApiError::bad_request(
            "X-Mayhem-Min-Att-Tier must be between 1 and 4",
            Some("X-Mayhem-Min-Att-Tier"),
        ))
    }
}

fn parse_x_mayhem_hedge(headers: &HeaderMap) -> Result<bool, ApiError> {
    let Some(value) = headers.get(X_MAYHEM_HEDGE_HEADER) else {
        return Ok(false);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Hedge must be an ASCII header value of 0 or 1",
            Some("X-Mayhem-Hedge"),
        )
    })?;
    if x_mayhem_hedge_requested(Some(value)) {
        return Ok(true);
    }
    if value.trim() == "0" {
        return Ok(false);
    }
    Err(ApiError::bad_request(
        "X-Mayhem-Hedge must be 1 to request hedging or 0 to disable it",
        Some("X-Mayhem-Hedge"),
    ))
}

impl From<BridgeError> for GatewaySessionError {
    fn from(error: BridgeError) -> Self {
        Self::new(error.to_string())
    }
}

impl GatewaySessionResult {
    pub fn local_openai_shape(output: ChatOutput) -> Self {
        Self {
            output,
            backend: "local-openai-shape".to_owned(),
            direct_session: false,
            provider_receipt: None,
            token_ids: Vec::new(),
        }
    }
}

impl GatewaySessionBackend for LocalOpenAiShapeBackend {
    fn name(&self) -> &str {
        "local-openai-shape"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            Ok(GatewaySessionResult::local_openai_shape(
                deterministic_chat_output(model, request),
            ))
        })
    }
}

impl ScBridgeGatewaySessionConfig {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
            open_timeout: Duration::from_millis(DEFAULT_OPEN_TIMEOUT_MILLIS),
            frame_timeout: Duration::from_millis(DEFAULT_STALL_TIMEOUT_MILLIS),
        }
    }
}

impl ScBridgeGatewaySessionBackend {
    pub fn new(config: ScBridgeGatewaySessionConfig) -> Self {
        Self { config }
    }
}

impl GatewaySessionBackend for ScBridgeGatewaySessionBackend {
    fn name(&self) -> &str {
        "sc-bridge-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move { self.run_chat_over_bridge(model, request, invocation).await })
    }
}

impl ScBridgeGatewaySessionBackend {
    async fn run_chat_over_bridge(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        invocation: &GatewaySessionInvocation,
    ) -> Result<GatewaySessionResult, GatewaySessionError> {
        let provider = invocation
            .provider_pubkey
            .as_deref()
            .ok_or_else(|| GatewaySessionError::new("model has no canonical provider route"))?;
        let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(
            &self.config.url,
            self.config.token.clone(),
        )?)
        .await?;
        bridge
            .session_subscribe([invocation.session_id.as_str()])
            .await?;
        bridge
            .peer_connect(provider, self.config.open_timeout)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "connecting direct peer {} for session {} failed: {err}",
                    provider, invocation.session_id
                ))
            })?;
        let opened = bridge
            .session_open(provider, &invocation.session_id)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "opening direct session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        if opened.get("direct").and_then(Value::as_bool) != Some(true)
            || opened.get("relayed").and_then(Value::as_bool) == Some(true)
        {
            return Err(GatewaySessionError::retryable(format!(
                "session {} was not opened as a direct non-relayed channel",
                invocation.session_id
            )));
        }

        let now = now_millis_u64();
        let att_nonce = blake3_hex(format!("att:{}:{now}", invocation.session_id).as_bytes());
        let open_frame = json!({
            "t": "s.open",
            "v": 1,
            "contract_version": invocation.contract_version,
            "session_id": invocation.session_id,
            "user": invocation.user_pubkey,
            "enclave_id": invocation.enclave_id,
            "price_ver": invocation.price_ver,
            "rules_ver": invocation.rules_ver,
            "voucher": invocation.spend_voucher,
            "att_nonce": att_nonce,
            "ts": now,
            "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
            "sig": invocation.spend_voucher.user_sig,
        });
        let open_head = session_frame_head(&open_frame)
            .map_err(|err| GatewaySessionError::new(format!("s.open hash failed: {err}")))?;
        bridge
            .session_send(provider, &invocation.session_id, open_frame)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.open for session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;

        let accept = next_session_frame(
            &mut bridge,
            &invocation.session_id,
            self.config.open_timeout,
            &["s.accept", "s.reject"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
            let code = accept
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            let reason = accept
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("no reason provided");
            return Err(GatewaySessionError::retryable(format!(
                "provider rejected session {} with {code}: {reason}",
                invocation.session_id
            )));
        }
        let accept_info = validate_direct_session_accept(
            &accept,
            invocation,
            &open_head,
            &att_nonce,
            now / 1000,
        )?;

        let request_id = blake3_hex(
            format!(
                "rid:{}:{}",
                invocation.session_id,
                serde_json::to_string(&request.messages).unwrap_or_default()
            )
            .as_bytes(),
        )
        .chars()
        .take(32)
        .collect::<String>();
        bridge
            .session_send(
                provider,
                &invocation.session_id,
                json!({
                    "t": "s.req",
                    "rid": request_id,
                    "body": direct_session_request_body(request),
                }),
            )
            .await?;

        let collected = collect_direct_session_output(
            &mut bridge,
            &invocation.session_id,
            &request_id,
            self.config.frame_timeout,
            request,
            &accept_info.enclave_pubkey,
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        let receipt_ack = direct_session_receipt_ack(
            request,
            &collected.output,
            invocation,
            &collected.provider_receipt,
            provider,
            model,
        )?;
        bridge
            .session_send(
                provider,
                &invocation.session_id,
                json!({
                    "t": "s.receipt_ack",
                    "v": 1,
                    "session_id": receipt_ack.session_id,
                    "seq": receipt_ack.seq,
                    "user_sig": receipt_ack.user_sig,
                }),
            )
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.receipt_ack for session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        let _ = bridge
            .session_send(
                provider,
                &invocation.session_id,
                json!({
                    "t": "s.close",
                    "v": 1,
                    "session_id": invocation.session_id,
                    "reason": "done",
                }),
            )
            .await;

        Ok(GatewaySessionResult {
            output: collected.output,
            backend: self.name().to_owned(),
            direct_session: true,
            provider_receipt: Some(collected.provider_receipt),
            token_ids: collected.token_ids,
        })
    }
}

fn validate_direct_session_accept(
    frame: &Value,
    invocation: &GatewaySessionInvocation,
    expected_open_head: &str,
    expected_att_nonce: &str,
    now_ts: u64,
) -> Result<ValidatedDirectSessionAccept, GatewaySessionError> {
    let fail = |message: String| GatewaySessionError::new(message);
    if frame.get("t").and_then(Value::as_str) != Some("s.accept") {
        return Err(fail("provider response was not s.accept".to_owned()));
    }
    if frame.get("session_id").and_then(Value::as_str) != Some(invocation.session_id.as_str()) {
        return Err(fail(format!(
            "provider accept session_id did not match {}",
            invocation.session_id
        )));
    }
    let actual_contract_version = frame_contract_version(frame);
    if actual_contract_version != Some(invocation.contract_version) {
        return Err(fail(contract_upgrade_required_reason(
            invocation.contract_version,
            actual_contract_version,
        )));
    }
    if frame.get("open_head").and_then(Value::as_str) != Some(expected_open_head) {
        return Err(fail(
            "provider accept open_head did not match sent s.open".to_owned(),
        ));
    }
    if frame.get("att_nonce").and_then(Value::as_str) != Some(expected_att_nonce) {
        return Err(fail(
            "provider accept att_nonce did not match sent s.open".to_owned(),
        ));
    }
    let report_value = frame
        .get("att_report")
        .cloned()
        .ok_or_else(|| fail("provider accept missing att_report".to_owned()))?;
    let report: AttestationReport = serde_json::from_value(report_value)
        .map_err(|err| fail(format!("provider accept att_report invalid: {err}")))?;
    if report.schema_version != ATTESTATION_SCHEMA_VERSION {
        return Err(fail(format!(
            "provider accept att_report schema_version {} is not supported",
            report.schema_version
        )));
    }
    if report.alg != ATTESTATION_ALG {
        return Err(fail(format!(
            "provider accept att_report alg {} is not supported",
            report.alg
        )));
    }
    if report.enclave_id != invocation.enclave_id {
        return Err(fail(format!(
            "provider accept att_report enclave_id did not match {}",
            invocation.enclave_id
        )));
    }
    if report.nonce_u != expected_att_nonce {
        return Err(fail(
            "provider accept att_report nonce_u did not match sent s.open".to_owned(),
        ));
    }
    let top_sig = frame
        .get("sig")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("provider accept missing sig".to_owned()))?;
    if let Some(provider) = invocation.provider_pubkey.as_deref() {
        let attestation = invocation.attestation.as_ref().ok_or_else(|| {
            fail("provider accept missing admin enclave attestation metadata".to_owned())
        })?;
        let mut request = AttestationVerificationRequest::new(
            &report,
            &attestation.contract,
            &attestation.trusted_binary_hashes,
            expected_att_nonce,
            provider,
            now_ts,
        );
        request.trusted_apple_app_attest_jwks = attestation.trusted_apple_app_attest_jwks.as_ref();
        request.trusted_nvidia_gb10_device_jwks =
            attestation.trusted_nvidia_gb10_device_jwks.as_ref();
        request.trusted_nvidia_nras_jwks = attestation.trusted_nvidia_nras_jwks.as_ref();
        request.trusted_nvidia_offline_jwks = attestation.trusted_nvidia_offline_jwks.as_ref();
        verify_tier1_attestation(&request).map_err(|err| {
            fail(format!(
                "provider accept attestation verification failed: {err}"
            ))
        })?;
        verify_direct_session_accept_signature(frame, provider, top_sig)?;
    }
    Ok(ValidatedDirectSessionAccept {
        enclave_pubkey: report.enclave_pubkey,
    })
}

fn verify_direct_session_accept_signature(
    frame: &Value,
    provider_pubkey: &str,
    signature_hex: &str,
) -> Result<(), GatewaySessionError> {
    let provider_key = decode_hex_array::<32>(provider_pubkey, "provider pubkey")?;
    let signature = decode_hex_array::<64>(signature_hex, "provider accept sig")?;
    let verifying_key = VerifyingKey::from_bytes(&provider_key)
        .map_err(|err| GatewaySessionError::new(format!("invalid provider pubkey: {err}")))?;
    let signature = Signature::from_bytes(&signature);
    verifying_key
        .verify(
            &session_accept_signing_bytes(frame).map_err(|err| {
                GatewaySessionError::new(format!("provider accept signing payload failed: {err}"))
            })?,
            &signature,
        )
        .map_err(|err| GatewaySessionError::new(format!("provider accept signature failed: {err}")))
}

fn verify_provider_receipt_signature(
    receipt: &ProviderSignedReceipt,
) -> Result<(), GatewaySessionError> {
    let enclave_key = decode_hex_array::<32>(&receipt.enclave_pubkey, "enclave pubkey")?;
    let signature = decode_hex_array::<64>(&receipt.enclave_sig, "provider receipt enclave sig")?;
    let verifying_key = VerifyingKey::from_bytes(&enclave_key)
        .map_err(|err| GatewaySessionError::new(format!("invalid enclave pubkey: {err}")))?;
    let signature = Signature::from_bytes(&signature);
    let payloads = supported_receipt_signing_bytes(&receipt.body).map_err(|err| {
        GatewaySessionError::new(format!("provider receipt signing payload failed: {err}"))
    })?;
    if payloads
        .iter()
        .any(|payload| verifying_key.verify(payload, &signature).is_ok())
    {
        return Ok(());
    }
    Err(GatewaySessionError::new(
        "provider receipt enclave signature failed",
    ))
}

fn decode_hex_array<const N: usize>(
    value: &str,
    label: &'static str,
) -> Result<[u8; N], GatewaySessionError> {
    let bytes = hex::decode(value)
        .map_err(|err| GatewaySessionError::new(format!("{label} is not hex: {err}")))?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        GatewaySessionError::new(format!("{label} must be {N} bytes, got {}", bytes.len()))
    })
}

async fn next_session_frame(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    wait: Duration,
    expected_types: &[&str],
) -> Result<Value, GatewaySessionError> {
    let deadline = Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(GatewaySessionError::new(format!(
                "timed out waiting for {} on session {}",
                expected_types.join("|"),
                session_id
            )));
        }
        match bridge.next_session_frame(remaining).await {
            Ok(event) => {
                if event.get("session_id").and_then(Value::as_str) != Some(session_id) {
                    continue;
                }
                let frame = event.get("frame").cloned().unwrap_or(Value::Null);
                let frame_type = frame.get("t").and_then(Value::as_str).unwrap_or("");
                if expected_types.contains(&frame_type) {
                    return Ok(frame);
                }
            }
            Err(BridgeError::Timeout) => {
                return Err(GatewaySessionError::new(format!(
                    "timed out waiting for {} on session {}",
                    expected_types.join("|"),
                    session_id
                )));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn direct_session_request_body(request: &ChatCompletionRequest) -> Value {
    let mut body = json!({
        "messages": &request.messages,
        "stream": true,
    });
    set_optional_json(
        &mut body,
        "tools",
        request.tools.as_ref().map(|value| json!(value)),
    );
    set_optional_json(
        &mut body,
        "tool_choice",
        request.tool_choice.as_ref().cloned(),
    );
    set_optional_json(
        &mut body,
        "response_format",
        request.response_format.as_ref().cloned(),
    );
    set_optional_json(
        &mut body,
        "temperature",
        request.temperature.map(|value| json!(value)),
    );
    set_optional_json(&mut body, "top_p", request.top_p.map(|value| json!(value)));
    set_optional_json(&mut body, "seed", request.seed.map(|value| json!(value)));
    set_optional_json(&mut body, "stop", request.stop.as_ref().cloned());
    set_optional_json(
        &mut body,
        "max_tokens",
        request.max_tokens.map(|value| json!(value)),
    );
    body
}

fn set_optional_json(body: &mut Value, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        body[key] = value;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectSessionTimeoutKind {
    TimeToFirstToken,
    IdleGap,
    OverallBudget,
}

#[derive(Clone, Debug)]
struct DirectSessionWatchdog {
    started_at_millis: u64,
    first_delta_at_millis: Option<u64>,
    last_delta_at_millis: Option<u64>,
    ttft_timeout_millis: u64,
    idle_timeout_millis: u64,
    overall_timeout_millis: Option<u64>,
}

impl DirectSessionWatchdog {
    fn new(
        started_at_millis: u64,
        ttft_timeout: Duration,
        idle_timeout: Duration,
        overall_timeout: Option<Duration>,
    ) -> Self {
        Self {
            started_at_millis,
            first_delta_at_millis: None,
            last_delta_at_millis: None,
            ttft_timeout_millis: duration_millis_u64(ttft_timeout),
            idle_timeout_millis: duration_millis_u64(idle_timeout),
            overall_timeout_millis: overall_timeout.map(duration_millis_u64),
        }
    }

    fn record_delta(&mut self, now_millis: u64) {
        self.first_delta_at_millis.get_or_insert(now_millis);
        self.last_delta_at_millis = Some(now_millis);
    }

    fn timeout_kind(&self, now_millis: u64) -> Option<DirectSessionTimeoutKind> {
        if self
            .overall_timeout_millis
            .is_some_and(|timeout| now_millis.saturating_sub(self.started_at_millis) > timeout)
        {
            return Some(DirectSessionTimeoutKind::OverallBudget);
        }
        if self.first_delta_at_millis.is_none() {
            if now_millis.saturating_sub(self.started_at_millis) > self.ttft_timeout_millis {
                return Some(DirectSessionTimeoutKind::TimeToFirstToken);
            }
            return None;
        }
        midstream_stalled_after(
            self.last_delta_at_millis,
            now_millis,
            self.idle_timeout_millis,
        )
        .then_some(DirectSessionTimeoutKind::IdleGap)
    }

    fn next_wait_millis(&self, now_millis: u64) -> Result<u64, DirectSessionTimeoutKind> {
        if let Some(kind) = self.timeout_kind(now_millis) {
            return Err(kind);
        }
        let mut deadline = if let Some(last_delta) = self.last_delta_at_millis {
            last_delta
                .saturating_add(self.idle_timeout_millis)
                .saturating_add(1)
        } else {
            self.started_at_millis
                .saturating_add(self.ttft_timeout_millis)
                .saturating_add(1)
        };
        if let Some(overall) = self.overall_timeout_millis {
            deadline = deadline.min(
                self.started_at_millis
                    .saturating_add(overall)
                    .saturating_add(1),
            );
        }
        Ok(deadline.saturating_sub(now_millis).max(1))
    }

    fn timeout_error(&self, session_id: &str, now_millis: u64) -> GatewaySessionError {
        let kind = self
            .timeout_kind(now_millis)
            .unwrap_or_else(|| self.pending_timeout_kind());
        direct_session_timeout_error(kind, session_id)
    }

    fn pending_timeout_kind(&self) -> DirectSessionTimeoutKind {
        if self.first_delta_at_millis.is_none() {
            DirectSessionTimeoutKind::TimeToFirstToken
        } else {
            DirectSessionTimeoutKind::IdleGap
        }
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn direct_session_timeout_error(
    kind: DirectSessionTimeoutKind,
    session_id: &str,
) -> GatewaySessionError {
    GatewaySessionError::new(match kind {
        DirectSessionTimeoutKind::TimeToFirstToken => {
            format!("timed out waiting for first s.delta on session {session_id}")
        }
        DirectSessionTimeoutKind::IdleGap => format!(
            "timed out waiting for next s.delta or s.receipt after idle gap on session {session_id}"
        ),
        DirectSessionTimeoutKind::OverallBudget => {
            format!("timed out waiting for direct session request budget on session {session_id}")
        }
    })
}

async fn collect_direct_session_output(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    request_id: &str,
    wait: Duration,
    request: &ChatCompletionRequest,
    enclave_pubkey: &str,
) -> Result<DirectSessionCollected, GatewaySessionError> {
    let mut content = String::new();
    let mut tool_call = None;
    let mut finish_reason = None;
    let mut usage = None;
    let mut provider_receipt = None;
    let mut token_ids = Vec::new();
    let mut artifact_builders = BTreeMap::new();
    let mut watchdog = DirectSessionWatchdog::new(now_millis_u64(), wait, wait, None);

    while finish_reason.is_none() || provider_receipt.is_none() {
        let remaining_millis = watchdog
            .next_wait_millis(now_millis_u64())
            .map_err(|kind| direct_session_timeout_error(kind, session_id))?;
        let frame = match next_session_frame(
            bridge,
            session_id,
            Duration::from_millis(remaining_millis),
            &["s.delta", "s.receipt", "s.error", "s.close"],
        )
        .await
        {
            Ok(frame) => frame,
            Err(err) if err.message.starts_with("timed out waiting") => {
                return Err(watchdog.timeout_error(session_id, now_millis_u64()));
            }
            Err(err) => return Err(err),
        };
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta") if frame.get("rid").and_then(Value::as_str) == Some(request_id) => {
                watchdog.record_delta(now_millis_u64());
                if let Some(delta) = frame.get("d").and_then(Value::as_str) {
                    content.push_str(delta);
                }
                if let Some(ids) = token_ids_from_session_delta(&frame) {
                    token_ids = ids;
                } else if let Some(token_id) = token_id_from_session_delta(&frame) {
                    token_ids.push(token_id);
                }
                if tool_call.is_none() {
                    tool_call = tool_call_from_session_delta(&frame);
                }
                collect_artifact_from_session_delta(&frame, &mut artifact_builders)?;
                if let Some(fin) = frame.get("fin").and_then(Value::as_str) {
                    finish_reason = Some(fin.to_owned());
                    usage = usage_from_session_delta(&frame);
                }
            }
            Some("s.receipt") => {
                provider_receipt = Some(provider_signed_receipt_from_frame(
                    &frame,
                    session_id,
                    enclave_pubkey,
                )?);
            }
            Some("s.error") => {
                let code = frame
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("provider_error");
                let message = frame
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("provider returned s.error");
                return Err(GatewaySessionError::new(format!(
                    "provider returned {code} on session {session_id}: {message}"
                )));
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if finish_reason.is_none() {
                    return Err(GatewaySessionError::new(format!(
                        "provider closed session {session_id} before final delta: {reason}"
                    )));
                }
                return Err(GatewaySessionError::new(format!(
                    "provider closed session {session_id} before s.receipt: {reason}"
                )));
            }
            _ => {}
        }
    }

    let prompt_text = chat_prompt_text(request);
    let usage = usage.unwrap_or_else(|| usage_for(&prompt_text, &content));
    let artifacts = finish_session_artifacts(artifact_builders)?;
    Ok(DirectSessionCollected {
        output: ChatOutput {
            content: tool_call.is_none().then_some(content),
            tool_call,
            artifacts,
            finish_reason: finish_reason.expect("loop ended with final delta"),
            usage,
        },
        provider_receipt: provider_receipt.expect("loop ended with provider receipt"),
        token_ids,
    })
}

fn token_ids_from_session_delta(frame: &Value) -> Option<Vec<i32>> {
    let ids = frame.get("token_ids")?.as_array()?;
    Some(
        ids.iter()
            .filter_map(|value| value.as_i64().and_then(|id| i32::try_from(id).ok()))
            .collect(),
    )
}

fn token_id_from_session_delta(frame: &Value) -> Option<i32> {
    frame
        .get("token_id")
        .or_else(|| frame.get("chunk").and_then(|chunk| chunk.get("token_id")))
        .and_then(Value::as_i64)
        .and_then(|id| i32::try_from(id).ok())
}

#[derive(Debug)]
struct SessionArtifactBuilder {
    id: String,
    content_type: String,
    bytes: Vec<u8>,
    total_len: Option<usize>,
    blake3: String,
    final_seen: bool,
}

fn collect_artifact_from_session_delta(
    frame: &Value,
    builders: &mut BTreeMap<String, SessionArtifactBuilder>,
) -> Result<(), GatewaySessionError> {
    let Some(artifact) = frame.get("artifact") else {
        return Ok(());
    };
    if artifact.is_null() {
        return Ok(());
    }
    let fail = |message: String| GatewaySessionError::new(message);
    let id = artifact
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| fail("artifact delta missing id".to_owned()))?
        .to_owned();
    let content_type = artifact
        .get("content_type")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| fail(format!("artifact {id} missing content_type")))?
        .to_owned();
    let encoding = artifact
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("hex");
    if encoding != "hex" {
        return Err(fail(format!(
            "artifact {id} used unsupported encoding {encoding}"
        )));
    }
    let data = artifact
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| fail(format!("artifact {id} missing data")))?;
    let bytes = hex::decode(data)
        .map_err(|err| fail(format!("artifact {id} data is not valid hex: {err}")))?;
    let offset = artifact
        .get("offset")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| fail(format!("artifact {id} missing offset")))?;
    let expected_len = artifact
        .get("len")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(bytes.len());
    if expected_len != bytes.len() {
        return Err(fail(format!(
            "artifact {id} len {} did not match decoded bytes {}",
            expected_len,
            bytes.len()
        )));
    }
    let total_len = artifact
        .get("total_len")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let blake3 = artifact
        .get("blake3")
        .and_then(Value::as_str)
        .filter(|value| is_hex_len(value, 64))
        .ok_or_else(|| fail(format!("artifact {id} missing 32-byte blake3 digest")))?
        .to_ascii_lowercase();

    let builder = builders
        .entry(id.clone())
        .or_insert_with(|| SessionArtifactBuilder {
            id: id.clone(),
            content_type: content_type.clone(),
            bytes: Vec::new(),
            total_len,
            blake3: blake3.clone(),
            final_seen: false,
        });
    if builder.content_type != content_type {
        return Err(fail(format!(
            "artifact {id} changed content_type from {} to {}",
            builder.content_type, content_type
        )));
    }
    if builder.blake3 != blake3 {
        return Err(fail(format!("artifact {id} changed blake3 digest")));
    }
    if let (Some(previous), Some(next)) = (builder.total_len, total_len) {
        if previous != next {
            return Err(fail(format!("artifact {id} changed total_len")));
        }
    } else if builder.total_len.is_none() {
        builder.total_len = total_len;
    }
    if offset != builder.bytes.len() {
        return Err(fail(format!(
            "artifact {id} offset gap: expected {}, got {offset}",
            builder.bytes.len()
        )));
    }
    if let Some(total_len) = builder.total_len {
        if offset.saturating_add(bytes.len()) > total_len {
            return Err(fail(format!("artifact {id} exceeds declared total_len")));
        }
    }
    builder.bytes.extend_from_slice(&bytes);
    if artifact
        .get("final")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        builder.final_seen = true;
    }
    Ok(())
}

fn finish_session_artifacts(
    builders: BTreeMap<String, SessionArtifactBuilder>,
) -> Result<Vec<GatewayArtifactOutput>, GatewaySessionError> {
    let mut artifacts = Vec::new();
    for (_id, builder) in builders {
        if !builder.final_seen {
            return Err(GatewaySessionError::new(format!(
                "artifact {} ended without final chunk",
                builder.id
            )));
        }
        if let Some(total_len) = builder.total_len {
            if total_len != builder.bytes.len() {
                return Err(GatewaySessionError::new(format!(
                    "artifact {} total_len {} did not match reconstructed bytes {}",
                    builder.id,
                    total_len,
                    builder.bytes.len()
                )));
            }
        }
        let actual = blake3_hex(&builder.bytes);
        if actual != builder.blake3 {
            return Err(GatewaySessionError::new(format!(
                "artifact {} blake3 mismatch: expected {}, got {}",
                builder.id, builder.blake3, actual
            )));
        }
        artifacts.push(GatewayArtifactOutput {
            id: builder.id,
            content_type: builder.content_type,
            bytes: builder.bytes,
            blake3: builder.blake3,
        });
    }
    Ok(artifacts)
}

fn provider_signed_receipt_from_frame(
    frame: &Value,
    session_id: &str,
    enclave_pubkey: &str,
) -> Result<ProviderSignedReceipt, GatewaySessionError> {
    let fail = |message: String| GatewaySessionError::new(message);
    if frame.get("session_id").and_then(Value::as_str) != Some(session_id) {
        return Err(fail(format!(
            "provider receipt session_id did not match {session_id}"
        )));
    }
    let receipt_value = frame
        .get("receipt")
        .cloned()
        .ok_or_else(|| fail("provider receipt frame missing receipt".to_owned()))?;
    let receipt: ProviderReceiptWire = serde_json::from_value(receipt_value)
        .map_err(|err| fail(format!("provider receipt invalid: {err}")))?;
    if receipt.body.session_id != session_id {
        return Err(fail(format!(
            "provider receipt body session_id did not match {session_id}"
        )));
    }
    if frame.get("seq").and_then(Value::as_u64) != Some(receipt.body.seq) {
        return Err(fail(
            "provider receipt frame seq did not match receipt body".to_owned(),
        ));
    }
    Ok(ProviderSignedReceipt {
        body: receipt.body,
        enclave_sig: receipt.enclave_sig,
        enclave_pubkey: enclave_pubkey.to_owned(),
    })
}

fn direct_session_receipt_ack(
    request: &ChatCompletionRequest,
    output: &ChatOutput,
    invocation: &GatewaySessionInvocation,
    provider_receipt: &ProviderSignedReceipt,
    provider: &str,
    model: &GatewayModel,
) -> Result<ReceiptAck, GatewaySessionError> {
    if !invocation.receipt_cosign_enabled {
        return Err(GatewaySessionError::new(
            "receipt co-signing refused; session paused",
        ));
    }
    let expected = expected_provider_receipt(model, request, output, provider);
    validate_provider_receipt(model, invocation, provider_receipt, expected)?;
    receipt_ack_for_body(&invocation.receipt_user_seed, &provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider receipt ack signing payload failed: {err}"
        ))
    })
}

fn expected_provider_receipt<'a>(
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    output: &ChatOutput,
    provider: &'a str,
) -> ExpectedProviderReceipt<'a> {
    let usage = ReceiptUsage::text(output.usage.prompt_tokens, output.usage.completion_tokens);
    ExpectedProviderReceipt {
        provider,
        mu_owed_cum: calculate_mu_owed(&model.mayhem.price_ref_mu, &usage),
        prompt_hash: blake3_hex(chat_prompt_text(request).as_bytes()),
        usage,
    }
}

fn validate_provider_receipt(
    model: &GatewayModel,
    invocation: &GatewaySessionInvocation,
    provider_receipt: &ProviderSignedReceipt,
    expected: ExpectedProviderReceipt<'_>,
) -> Result<(), GatewaySessionError> {
    let body = migrate_receipt_body(&provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider receipt schema_version is not supported: {err}"
        ))
    })?;
    let checks = [
        (
            body.session_id == invocation.session_id,
            "provider receipt session_id mismatch",
        ),
        (body.seq == 1, "provider receipt seq mismatch"),
        (body.final_receipt, "provider receipt is not final"),
        (
            body.user == invocation.user_pubkey,
            "provider receipt user mismatch",
        ),
        (
            body.provider == expected.provider,
            "provider receipt provider mismatch",
        ),
        (
            body.enclave_id == invocation.enclave_id,
            "provider receipt enclave mismatch",
        ),
        (body.model_id == model.id, "provider receipt model mismatch"),
        (
            body.price_ver == invocation.price_ver,
            "provider receipt price_ver mismatch",
        ),
        (
            body.rules_ver == invocation.rules_ver,
            "provider receipt rules_ver mismatch",
        ),
        (
            body.usage == expected.usage,
            "provider receipt usage mismatch",
        ),
        (
            body.mu_owed_cum == expected.mu_owed_cum,
            "provider receipt amount mismatch",
        ),
        (
            body.prompt_hash == expected.prompt_hash,
            "provider receipt prompt_hash mismatch",
        ),
    ];
    for (ok, message) in checks {
        if !ok {
            return Err(GatewaySessionError::new(message));
        }
    }
    verify_provider_receipt_signature(provider_receipt)
}

fn receipt_ack_for_body(
    user_seed: &[u8; 32],
    body: &ReceiptBody,
) -> Result<ReceiptAck, serde_json::Error> {
    let payload = receipt_signing_bytes(body)?;
    Ok(ReceiptAck {
        session_id: body.session_id.clone(),
        seq: body.seq,
        user_sig: sign_hex(user_seed, &payload),
    })
}

fn tool_call_from_session_delta(frame: &Value) -> Option<ToolCallOutput> {
    let tool = frame.get("tool")?;
    if tool.is_null() {
        return None;
    }
    Some(ToolCallOutput {
        id: tool
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| make_id("call")),
        name: tool
            .get("name")
            .or_else(|| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
            })
            .and_then(Value::as_str)?
            .to_owned(),
        arguments: tool
            .get("arguments")
            .or_else(|| {
                tool.get("function")
                    .and_then(|function| function.get("arguments"))
            })
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| "{}".to_owned()),
    })
}

fn usage_from_session_delta(frame: &Value) -> Option<Usage> {
    let usage = frame.get("usage")?;
    let prompt_tokens = usage
        .get("in")
        .or_else(|| usage.get("input_token"))
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)?;
    let completion_tokens = usage
        .get("out")
        .or_else(|| usage.get("output_token"))
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)?;
    Some(Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    })
}

async fn build_chat_completion(
    state: &GatewayState,
    request: ChatCompletionRequest,
    options: GatewayRequestOptions,
) -> Result<ChatResponse, ApiError> {
    let model = require_model(state, &request.model)?;
    if request.messages.is_empty() {
        return Err(ApiError::bad_request(
            "messages must contain at least one item",
            Some("messages"),
        ));
    }
    validate_chat_modalities(&model, &request)?;
    if request
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
        && !model.mayhem.caps.tools
    {
        return Err(ApiError::bad_request(
            "model does not support tool calling",
            Some("tools"),
        ));
    }
    if request.response_format.is_some() && !model.mayhem.caps.json {
        return Err(ApiError::bad_request(
            "model does not support response_format JSON constraints",
            Some("response_format"),
        ));
    }

    let id = make_id("chatcmpl");
    let created = now_secs();
    let (
        GatewaySessionResult {
            output,
            backend,
            direct_session,
            provider_receipt,
            token_ids: _,
        },
        invocation,
    ) = run_chat_with_route_retry(state, &model, &request, options).await?;
    let mayhem_meta = ResponseMayhemMeta {
        backend: &backend,
        direct_session,
        hedge: invocation.hedge,
    };
    let receipt = state.meter_chat_session(
        &model,
        &request,
        &output,
        &invocation,
        provider_receipt.as_ref(),
    )?;
    state
        .maybe_run_canary_probe_after_session(&model, &invocation)
        .await;
    if request.stream {
        Ok(ChatResponse::Sse(chat_stream_chunks(
            &id,
            created,
            &model.id,
            &output,
            &receipt,
            mayhem_meta,
            request
                .stream_options
                .as_ref()
                .is_some_and(|options| options.include_usage),
        )))
    } else {
        Ok(ChatResponse::Json(chat_response_value(
            &id,
            created,
            &model,
            &output,
            &receipt,
            mayhem_meta,
        )))
    }
}

async fn run_chat_with_route_retry(
    state: &GatewayState,
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    options: GatewayRequestOptions,
) -> Result<(GatewaySessionResult, GatewaySessionInvocation), ApiError> {
    let eligible_routes = eligible_route_candidates(model, options.min_att_tier);
    if !model.mayhem.route_candidates.is_empty() && eligible_routes.is_empty() {
        return Err(ApiError::bad_request(
            "no provider route satisfies X-Mayhem-Min-Att-Tier",
            Some("X-Mayhem-Min-Att-Tier"),
        ));
    }
    let attempt_count = if eligible_routes.is_empty() {
        1
    } else {
        eligible_routes
            .len()
            .min(usize::from(DEFAULT_MAX_OPEN_ATTEMPTS))
    };
    let mut last_retryable_error = None;

    for attempt_index in 0..attempt_count {
        let route = eligible_routes.get(attempt_index).copied();
        let invocation = state.prepare_chat_invocation_for_route(model, request, route, options)?;
        match state
            .session_backend
            .run_chat(model, request, &invocation)
            .await
        {
            Ok(result) => return Ok((result, invocation)),
            Err(err) if err.retryable => {
                last_retryable_error = Some(err.message);
            }
            Err(err) => return Err(ApiError::bad_gateway(err.message, Some("model"))),
        }
    }

    Err(ApiError::bad_gateway(
        format!(
            "all {attempt_count} route attempt(s) failed before spend; last error: {}",
            last_retryable_error.unwrap_or_else(|| "no route attempted".to_owned())
        ),
        Some("model"),
    ))
}

fn eligible_route_candidates(
    model: &GatewayModel,
    min_att_tier: Option<u8>,
) -> Vec<&GatewayRouteCandidate> {
    model
        .mayhem
        .route_candidates
        .iter()
        .filter(|candidate| {
            min_att_tier
                .map(|min_tier| candidate.att_tier >= min_tier)
                .unwrap_or(true)
        })
        .collect()
}

fn build_completion(
    state: &GatewayState,
    request: CompletionRequest,
) -> Result<ChatResponse, ApiError> {
    let model = require_model(state, &request.model)?;
    let id = make_id("cmpl");
    let created = now_secs();
    let prompt = prompt_to_text(&request.prompt);
    let max_tokens = request.max_tokens.unwrap_or(64).max(1);
    let text = format!("Mayhem completion: {}", prompt.trim())
        .chars()
        .take(max_tokens as usize * 8)
        .collect::<String>();
    let usage = usage_for(&prompt, &text);
    let receipt_usage = ReceiptUsage::text(usage.prompt_tokens, usage.completion_tokens);
    let receipt = state.meter_session(&model, &prompt, receipt_usage)?;
    let chunk = json!({
        "id": id,
        "object": "text_completion",
        "created": created,
        "model": model.id,
        "choices": [{
            "text": text,
            "index": 0,
            "logprobs": null,
            "finish_reason": "stop"
        }],
        "usage": usage,
        "mayhem": {
            "receipt": receipt_summary(&receipt),
        },
    });
    if request.stream {
        Ok(ChatResponse::Sse(vec![chunk]))
    } else {
        Ok(ChatResponse::Json(chunk))
    }
}

fn build_embedding(state: &GatewayState, request: EmbeddingRequest) -> Result<Value, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_embeddings(&model) {
        return Err(ApiError::bad_request(
            "model does not support embeddings",
            Some("model"),
        ));
    }
    if request
        .encoding_format
        .as_deref()
        .is_some_and(|format| format != "float")
    {
        return Err(ApiError::bad_request(
            "only encoding_format=float is supported for embeddings",
            Some("encoding_format"),
        ));
    }
    let dimensions = embedding_dimensions(request.dimensions)?;
    let inputs = embedding_inputs(&request.input)?;
    let prompt_tokens = embedding_prompt_tokens(&inputs);
    let prompt_text = inputs.join("\n");
    let receipt = state.meter_session(
        &model,
        &prompt_text,
        ReceiptUsage::from_units([(USAGE_INPUT_TOKEN, prompt_tokens)]),
    )?;
    let data = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| {
            json!({
                "object": "embedding",
                "embedding": deterministic_embedding_vector(&model.id, input, dimensions),
                "index": index,
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "object": "list",
        "data": data,
        "model": model.id,
        "usage": {
            "prompt_tokens": prompt_tokens,
            "total_tokens": prompt_tokens,
        },
        "mayhem": {
            "backend": "local-embedding-shape",
            "model": model.mayhem,
            "receipt": receipt_summary(&receipt),
        },
    }))
}

fn build_image_generation(
    state: &GatewayState,
    request: ImageGenerationRequest,
) -> Result<Value, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_image_generation(&model) {
        return Err(ApiError::bad_request(
            "model does not support image generation",
            Some("model"),
        ));
    }
    if request.prompt.trim().is_empty() {
        return Err(ApiError::bad_request(
            "prompt must not be empty",
            Some("prompt"),
        ));
    }
    let count = image_count(request.n)?;
    let (width, height, size) = image_size(request.size.as_deref())?;
    let steps = image_steps(request.steps)?;
    let response_format = request.response_format.as_deref().unwrap_or("b64_json");
    if !matches!(response_format, "b64_json" | "url") {
        return Err(ApiError::bad_request(
            "response_format must be b64_json or url",
            Some("response_format"),
        ));
    }

    let usage = ReceiptUsage::from_units([
        (USAGE_IMAGE, u64::from(count)),
        (USAGE_STEP, steps.saturating_mul(u64::from(count))),
    ]);
    let receipt = state.meter_session(&model, &image_prompt_text(&request, &size), usage)?;
    let data = (0..count)
        .map(|index| {
            let bytes =
                deterministic_image_bytes(&model.id, &request.prompt, index, width, height, steps);
            let encoded = BASE64_STANDARD.encode(&bytes);
            let artifact = json!({
                "id": format!("image-{index}"),
                "content_type": "image/png",
                "bytes": bytes.len(),
                "blake3": blake3_hex(&bytes),
            });
            if response_format == "url" {
                json!({
                    "url": format!("data:image/png;base64,{encoded}"),
                    "revised_prompt": request.prompt,
                    "mayhem": { "artifact": artifact },
                })
            } else {
                json!({
                    "b64_json": encoded,
                    "revised_prompt": request.prompt,
                    "mayhem": { "artifact": artifact },
                })
            }
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "created": now_secs(),
        "data": data,
        "usage": receipt.receipt.body.usage,
        "mayhem": {
            "backend": "local-image-shape",
            "model": model.mayhem,
            "receipt": receipt_summary(&receipt),
        },
    }))
}

fn build_audio_speech(
    state: &GatewayState,
    request: AudioSpeechRequest,
) -> Result<Response, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_tts(&model) {
        return Err(ApiError::bad_request(
            "model does not support audio speech",
            Some("model"),
        ));
    }
    let input = request.input.trim();
    if input.is_empty() {
        return Err(ApiError::bad_request(
            "input must not be empty",
            Some("input"),
        ));
    }
    let format = speech_response_format(request.response_format.as_deref())?;
    let speed = speech_speed(request.speed)?;
    let input_characters = input.chars().count() as u64;
    let audio_seconds = speech_audio_seconds(input_characters, speed);
    let usage = ReceiptUsage::from_units([
        (USAGE_INPUT_CHARACTER, input_characters),
        (USAGE_AUDIO_SECOND, audio_seconds),
    ]);
    let receipt = state.meter_session(&model, &speech_prompt_text(&request), usage)?;
    let bytes = deterministic_audio_bytes(
        &format,
        &model.id,
        input,
        request.voice.as_deref().unwrap_or("alloy"),
        audio_seconds,
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, speech_content_type(&format))
        .header(
            "x-mayhem-receipt-session-id",
            receipt.receipt.body.session_id.as_str(),
        )
        .header(
            "x-mayhem-mu-owed-cum",
            receipt.receipt.body.mu_owed_cum.to_string(),
        )
        .header(
            "x-mayhem-usage-input-character",
            input_characters.to_string(),
        )
        .header("x-mayhem-usage-audio-second", audio_seconds.to_string())
        .body(Body::from(bytes))
        .map_err(|err| {
            ApiError::internal_message(format!("building speech response failed: {err}"))
        })
}

async fn parse_audio_transcription_multipart(
    mut multipart: Multipart,
) -> Result<AudioTranscriptionRequest, ApiError> {
    let mut model = None;
    let mut audio = None;
    let mut filename = None;
    let mut response_format = None;
    let mut language = None;
    let mut prompt = None;

    while let Some(field) = multipart.next_field().await.map_err(|err| {
        ApiError::bad_request(format!("invalid multipart form: {err}"), Some("file"))
    })? {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "model" => {
                model = Some(field.text().await.map_err(|err| {
                    ApiError::bad_request(format!("invalid model field: {err}"), Some("model"))
                })?);
            }
            "file" => {
                filename = field.file_name().map(str::to_owned);
                let bytes = field.bytes().await.map_err(|err| {
                    ApiError::bad_request(format!("invalid file field: {err}"), Some("file"))
                })?;
                audio = Some(bytes.to_vec());
            }
            "response_format" => {
                response_format = Some(field.text().await.map_err(|err| {
                    ApiError::bad_request(
                        format!("invalid response_format field: {err}"),
                        Some("response_format"),
                    )
                })?);
            }
            "language" => {
                language = Some(field.text().await.map_err(|err| {
                    ApiError::bad_request(
                        format!("invalid language field: {err}"),
                        Some("language"),
                    )
                })?);
            }
            "prompt" => {
                prompt = Some(field.text().await.map_err(|err| {
                    ApiError::bad_request(format!("invalid prompt field: {err}"), Some("prompt"))
                })?);
            }
            _ => {}
        }
    }

    Ok(AudioTranscriptionRequest {
        model: model
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ApiError::bad_request("multipart form missing model", Some("model")))?,
        audio: audio
            .filter(|bytes| !bytes.is_empty())
            .ok_or_else(|| ApiError::bad_request("multipart form missing file", Some("file")))?,
        filename,
        response_format,
        language,
        prompt,
    })
}

fn build_audio_transcription(
    state: &GatewayState,
    request: AudioTranscriptionRequest,
) -> Result<Value, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_stt(&model) {
        return Err(ApiError::bad_request(
            "model does not support audio transcription",
            Some("model"),
        ));
    }
    let response_format = request.response_format.as_deref().unwrap_or("json");
    if !matches!(response_format, "json" | "verbose_json") {
        return Err(ApiError::bad_request(
            "response_format must be json or verbose_json",
            Some("response_format"),
        ));
    }
    let audio_seconds = audio_seconds_for_bytes(request.audio.len());
    let usage = ReceiptUsage::from_units([(USAGE_AUDIO_SECOND, audio_seconds)]);
    let prompt_text = transcription_prompt_text(&request);
    let receipt = state.meter_session(&model, &prompt_text, usage)?;
    let text = deterministic_transcription_text(&request);
    let mut value = json!({
        "text": text,
        "usage": receipt.receipt.body.usage,
        "mayhem": {
            "backend": "local-stt-shape",
            "model": model.mayhem,
            "receipt": receipt_summary(&receipt),
        },
    });
    if response_format == "verbose_json" {
        value["duration"] = json!(audio_seconds);
        value["language"] = json!(request.language.unwrap_or_else(|| "und".to_owned()));
    }
    Ok(value)
}

fn model_supports_embeddings(model: &GatewayModel) -> bool {
    model.mayhem.model_class == "embedding"
        || model.mayhem.caps.output_modality.as_deref() == Some("embedding")
        || model
            .mayhem
            .caps
            .output_modalities
            .iter()
            .any(|modality| modality == "embedding")
        || model
            .mayhem
            .adapter
            .modality_set
            .iter()
            .any(|modality| modality == "embedding")
}

fn model_supports_image_generation(model: &GatewayModel) -> bool {
    model.mayhem.model_class == "image-generation"
        || model.mayhem.caps.image
        || model.mayhem.caps.output_modality.as_deref() == Some("image")
        || model
            .mayhem
            .caps
            .output_modalities
            .iter()
            .any(|modality| modality == "image")
        || model
            .mayhem
            .adapter
            .modality_set
            .iter()
            .any(|modality| modality == "image")
}

fn model_supports_tts(model: &GatewayModel) -> bool {
    model.mayhem.model_class == "tts"
        || model.mayhem.caps.output_modality.as_deref() == Some("audio")
        || model
            .mayhem
            .caps
            .output_modalities
            .iter()
            .any(|modality| modality == "audio")
}

fn model_supports_stt(model: &GatewayModel) -> bool {
    model.mayhem.model_class == "stt"
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ChatInputModalities {
    image: bool,
    audio: bool,
}

fn validate_chat_modalities(
    model: &GatewayModel,
    request: &ChatCompletionRequest,
) -> Result<(), ApiError> {
    let modalities = chat_input_modalities(&request.messages);
    if modalities.image && !model.mayhem.caps.vision {
        return Err(ApiError::bad_request(
            "model does not support image_url chat content",
            Some("messages"),
        ));
    }
    if modalities.audio && !model.mayhem.caps.audio {
        return Err(ApiError::bad_request(
            "model does not support input_audio chat content",
            Some("messages"),
        ));
    }
    Ok(())
}

fn chat_input_modalities(messages: &[ChatMessage]) -> ChatInputModalities {
    let mut modalities = ChatInputModalities::default();
    for message in messages {
        let Value::Array(parts) = &message.content else {
            continue;
        };
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("image_url") => modalities.image = true,
                Some("input_audio") => modalities.audio = true,
                _ => {}
            }
        }
    }
    modalities
}

fn image_count(value: Option<u32>) -> Result<u32, ApiError> {
    let count = value.unwrap_or(1);
    if (1..=10).contains(&count) {
        Ok(count)
    } else {
        Err(ApiError::bad_request("n must be 1..=10", Some("n")))
    }
}

fn image_size(value: Option<&str>) -> Result<(u32, u32, String), ApiError> {
    let size = value.unwrap_or("1024x1024");
    let Some((width, height)) = size.split_once('x') else {
        return Err(ApiError::bad_request(
            "size must be WIDTHxHEIGHT",
            Some("size"),
        ));
    };
    let width = width
        .parse::<u32>()
        .map_err(|_| ApiError::bad_request("size width must be an integer", Some("size")))?;
    let height = height
        .parse::<u32>()
        .map_err(|_| ApiError::bad_request("size height must be an integer", Some("size")))?;
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(ApiError::bad_request(
            "size dimensions must be 1..=4096",
            Some("size"),
        ));
    }
    Ok((width, height, format!("{width}x{height}")))
}

fn image_steps(value: Option<u64>) -> Result<u64, ApiError> {
    let steps = value.unwrap_or(1);
    if (1..=500).contains(&steps) {
        Ok(steps)
    } else {
        Err(ApiError::bad_request(
            "steps must be 1..=500",
            Some("steps"),
        ))
    }
}

fn image_prompt_text(request: &ImageGenerationRequest, size: &str) -> String {
    json!({
        "kind": "image-generation",
        "prompt": request.prompt,
        "n": request.n.unwrap_or(1),
        "size": size,
        "steps": request.steps.unwrap_or(1),
        "user": request.user,
    })
    .to_string()
}

fn deterministic_image_bytes(
    model_id: &str,
    prompt: &str,
    index: u32,
    width: u32,
    height: u32,
    steps: u64,
) -> Vec<u8> {
    let digest = blake3::hash(
        format!("mayhem-image-v1:{model_id}:{prompt}:{index}:{width}:{height}:{steps}").as_bytes(),
    );
    let mut bytes = b"\x89PNG\r\n\x1a\nMAYHEM-IMAGE-V1\n".to_vec();
    bytes.extend_from_slice(format!("{width}x{height};steps={steps};").as_bytes());
    bytes.extend_from_slice(digest.as_bytes());
    bytes
}

fn speech_response_format(value: Option<&str>) -> Result<String, ApiError> {
    let format = value.unwrap_or("mp3");
    if matches!(format, "mp3" | "opus" | "aac" | "flac" | "wav" | "pcm") {
        Ok(format.to_owned())
    } else {
        Err(ApiError::bad_request(
            "response_format must be one of mp3, opus, aac, flac, wav, pcm",
            Some("response_format"),
        ))
    }
}

fn speech_content_type(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        "opus" => "audio/ogg",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "pcm" => "audio/L16",
        _ => "audio/mpeg",
    }
}

fn speech_speed(value: Option<f64>) -> Result<f64, ApiError> {
    let speed = value.unwrap_or(1.0);
    if (0.25..=4.0).contains(&speed) {
        Ok(speed)
    } else {
        Err(ApiError::bad_request(
            "speed must be between 0.25 and 4.0",
            Some("speed"),
        ))
    }
}

fn speech_audio_seconds(input_characters: u64, speed: f64) -> u64 {
    let base_seconds = ceil_div_u64(input_characters.max(1), 32);
    ((base_seconds as f64) / speed).ceil().max(1.0) as u64
}

fn speech_prompt_text(request: &AudioSpeechRequest) -> String {
    json!({
        "kind": "audio-speech",
        "input": request.input,
        "voice": request.voice,
        "response_format": request.response_format,
        "speed": request.speed,
    })
    .to_string()
}

fn deterministic_audio_bytes(
    format: &str,
    model_id: &str,
    input: &str,
    voice: &str,
    audio_seconds: u64,
) -> Vec<u8> {
    let digest = blake3::hash(
        format!("mayhem-audio-v1:{model_id}:{input}:{voice}:{audio_seconds}:{format}").as_bytes(),
    );
    if format == "wav" {
        return deterministic_wav_bytes(digest.as_bytes(), audio_seconds);
    }
    let mut bytes = format!("MAYHEM-AUDIO-{format}\nseconds={audio_seconds}\n").into_bytes();
    bytes.extend_from_slice(digest.as_bytes());
    bytes
}

fn deterministic_wav_bytes(seed: &[u8; 32], audio_seconds: u64) -> Vec<u8> {
    let sample_rate = 8_000_u32;
    let seconds = audio_seconds.clamp(1, 5) as u32;
    let samples = sample_rate.saturating_mul(seconds);
    let data_len = samples.saturating_mul(2);
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32.saturating_add(data_len)).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for index in 0..samples as usize {
        let sample = i16::from(seed[index % seed.len()]) - 128;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn audio_seconds_for_bytes(bytes: usize) -> u64 {
    ceil_div_u64(bytes as u64, 16_000).max(1)
}

fn transcription_prompt_text(request: &AudioTranscriptionRequest) -> String {
    json!({
        "kind": "audio-transcription",
        "filename": request.filename,
        "bytes": request.audio.len(),
        "language": request.language,
        "prompt": request.prompt,
        "audio_hash": blake3_hex(&request.audio),
    })
    .to_string()
}

fn deterministic_transcription_text(request: &AudioTranscriptionRequest) -> String {
    let hint = request
        .prompt
        .as_deref()
        .filter(|prompt| !prompt.trim().is_empty())
        .unwrap_or("audio");
    format!(
        "Mayhem transcription for {hint} ({})",
        blake3_hex(&request.audio)[..12].to_owned()
    )
}

fn ceil_div_u64(value: u64, divisor: u64) -> u64 {
    if value == 0 || divisor == 0 {
        0
    } else {
        value.div_ceil(divisor)
    }
}

fn embedding_dimensions(requested: Option<usize>) -> Result<usize, ApiError> {
    let dimensions = requested.unwrap_or(32);
    if (1..=3072).contains(&dimensions) {
        Ok(dimensions)
    } else {
        Err(ApiError::bad_request(
            "dimensions must be between 1 and 3072",
            Some("dimensions"),
        ))
    }
}

fn embedding_inputs(value: &Value) -> Result<Vec<String>, ApiError> {
    match value {
        Value::String(text) => Ok(vec![text.clone()]),
        Value::Array(items) if items.is_empty() => Err(ApiError::bad_request(
            "input must contain at least one item",
            Some("input"),
        )),
        Value::Array(items) if items.iter().all(Value::is_string) => Ok(items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()),
        Value::Array(items) if items.iter().all(embedding_token_value) => {
            Ok(vec![embedding_token_array_to_text(items)?])
        }
        Value::Array(items) if items.iter().all(Value::is_array) => items
            .iter()
            .map(|item| {
                let tokens = item.as_array().ok_or_else(|| {
                    ApiError::bad_request("input token arrays must be arrays", Some("input"))
                })?;
                embedding_token_array_to_text(tokens)
            })
            .collect(),
        _ => Err(ApiError::bad_request(
            "input must be a string, an array of strings, token IDs, or token ID arrays",
            Some("input"),
        )),
    }
}

fn embedding_token_value(value: &Value) -> bool {
    value.as_i64().is_some() || value.as_u64().is_some()
}

fn embedding_token_array_to_text(tokens: &[Value]) -> Result<String, ApiError> {
    if tokens.is_empty() {
        return Err(ApiError::bad_request(
            "token input arrays must not be empty",
            Some("input"),
        ));
    }
    let mut parts = Vec::with_capacity(tokens.len());
    for token in tokens {
        if let Some(token) = token.as_i64() {
            parts.push(token.to_string());
        } else if let Some(token) = token.as_u64() {
            parts.push(token.to_string());
        } else {
            return Err(ApiError::bad_request(
                "token input arrays must contain only integers",
                Some("input"),
            ));
        }
    }
    Ok(parts.join(" "))
}

fn embedding_prompt_tokens(inputs: &[String]) -> u64 {
    inputs.iter().map(|input| rough_tokens(input).max(1)).sum()
}

fn deterministic_embedding_vector(model_id: &str, input: &str, dimensions: usize) -> Vec<f32> {
    let mut values = (0..dimensions)
        .map(|index| {
            let digest =
                blake3::hash(format!("mayhem-embedding-v1:{model_id}:{input}:{index}").as_bytes());
            let bytes: [u8; 4] = digest.as_bytes()[..4]
                .try_into()
                .expect("blake3 digest has at least four bytes");
            let raw = u32::from_le_bytes(bytes);
            (((f64::from(raw) / f64::from(u32::MAX)) * 2.0) - 1.0) as f32
        })
        .collect::<Vec<_>>();
    let norm = values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        for value in &mut values {
            *value = (f64::from(*value) / norm) as f32;
        }
    }
    values
}

pub fn embedding_cosine_similarity_bps(expected: &[f32], observed: &[f32]) -> Option<u32> {
    if expected.is_empty() || expected.len() != observed.len() {
        return None;
    }
    let (dot, expected_norm, observed_norm) = expected.iter().zip(observed).fold(
        (0.0, 0.0, 0.0),
        |(dot, left_norm, right_norm), (left, right)| {
            let left = f64::from(*left);
            let right = f64::from(*right);
            (
                dot + left * right,
                left_norm + left * left,
                right_norm + right * right,
            )
        },
    );
    if expected_norm == 0.0 || observed_norm == 0.0 {
        return None;
    }
    let cosine = dot / (expected_norm.sqrt() * observed_norm.sqrt());
    Some((cosine.clamp(0.0, 1.0) * 10_000.0).round() as u32)
}

pub fn embedding_canary_matches(expected: &[f32], observed: &[f32], tolerance_bps: u32) -> bool {
    let min_bps = 10_000u32.saturating_sub(tolerance_bps);
    embedding_cosine_similarity_bps(expected, observed).is_some_and(|score| score >= min_bps)
}

impl GatewayCanaryScheduler {
    fn should_probe(&mut self, key: &str, policy: GatewayCanaryProbePolicy) -> bool {
        if !policy.enabled {
            return false;
        }
        let interval = self
            .next_after
            .get(key)
            .copied()
            .unwrap_or_else(|| self.next_interval(key, policy));
        self.next_after.entry(key.to_owned()).or_insert(interval);
        let counter = self.counters.entry(key.to_owned()).or_insert(0);
        *counter = counter.saturating_add(1);
        if *counter < interval {
            return false;
        }
        *counter = 0;
        let next = self.next_interval(key, policy);
        self.next_after.insert(key.to_owned(), next);
        true
    }

    fn next_interval(&mut self, key: &str, policy: GatewayCanaryProbePolicy) -> u64 {
        let min = policy.min_interval_sessions.max(1);
        let max = policy.max_interval_sessions.max(min);
        if min == max {
            return min;
        }
        self.sequence = self.sequence.saturating_add(1);
        let digest = blake3::hash(
            format!(
                "mayhem-canary-schedule:{key}:{}:{}",
                policy.seed, self.sequence
            )
            .as_bytes(),
        );
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&digest.as_bytes()[..8]);
        min + (u64::from_be_bytes(bytes) % (max - min + 1))
    }
}

impl GatewayState {
    async fn maybe_run_canary_probe_after_session(
        &self,
        model: &GatewayModel,
        invocation: &GatewaySessionInvocation,
    ) {
        let Some(config) = self.canaries.models.get(&model.id).cloned() else {
            return;
        };
        if config.prompts.is_empty() {
            return;
        }
        if config.verification_method != CANARY_VERIFICATION_TOKEN_FINGERPRINT {
            return;
        }
        let route_key = canary_route_key(model, invocation);
        let should_probe = self
            .canary_scheduler
            .lock()
            .expect("canary scheduler poisoned")
            .should_probe(&route_key, self.canary_policy);
        if !should_probe {
            return;
        }
        match self
            .run_canary_probe_for_route(model, invocation, &config)
            .await
        {
            Ok(probe) => self.record_probe(probe),
            Err(err) => {
                self.record_probe(failed_canary_runtime_probe(
                    model,
                    invocation,
                    &config,
                    err.message,
                    self.canary_policy.epoch,
                    &self.receipt_config.user_seed,
                ));
            }
        }
    }

    async fn run_canary_probe_for_route(
        &self,
        model: &GatewayModel,
        served_invocation: &GatewaySessionInvocation,
        config: &GatewayCanaryModelConfig,
    ) -> Result<StoredProbeEvent, ApiError> {
        let expected_fingerprint = canary_expected_fingerprint(config, served_invocation)
            .ok_or_else(|| {
                ApiError::bad_gateway(
                    "no catalog canary fingerprint for served artifact",
                    Some("model"),
                )
            })?;
        let expected_token_prefixes = canary_expected_token_prefixes(config, served_invocation)
            .ok_or_else(|| {
                ApiError::bad_gateway(
                    "no catalog canary token prefixes for served artifact",
                    Some("model"),
                )
            })?;
        let route = canary_served_route(model, served_invocation);
        let mut prompt_reports = Vec::with_capacity(config.prompts.len());
        let mut receipt_hashes = Vec::with_capacity(config.prompts.len());
        let mut stored_receipts = Vec::with_capacity(config.prompts.len());
        let mut observed_tokens = BTreeMap::new();

        for prompt in &config.prompts {
            let request = canary_chat_request(&model.id, config, prompt, self.canary_policy.seed);
            let invocation = self.prepare_chat_invocation_for_route(
                model,
                &request,
                route.as_ref(),
                GatewayRequestOptions::default(),
            )?;
            let result = self
                .session_backend
                .run_chat(model, &request, &invocation)
                .await
                .map_err(|err| ApiError::bad_gateway(err.message, Some("model")))?;
            let token_fingerprint = token_fingerprint(result.token_ids.iter().copied()).digest;
            observed_tokens.insert(prompt.id.clone(), result.token_ids.clone());
            let receipt = self.meter_chat_session(
                model,
                &request,
                &result.output,
                &invocation,
                result.provider_receipt.as_ref(),
            )?;
            let receipt_hash = stable_value_hash(&json!(receipt));
            receipt_hashes.push(receipt_hash.clone());
            stored_receipts.push(receipt);
            prompt_reports.push(json!({
                "prompt_id": prompt.id,
                "request": request,
                "token_count": result.token_ids.len(),
                "token_ids": result.token_ids,
                "token_fingerprint": token_fingerprint,
                "session_id": invocation.session_id,
                "receipt_hash": receipt_hash,
            }));
        }

        let spec = CanaryProbeSpec {
            model: model.id.clone(),
            canary_set: config.canary_set.clone(),
            prompt_id: format!("aggregate:{}", config.prompts.len()),
            prompt: config
                .prompts
                .iter()
                .map(|prompt| prompt.id.as_str())
                .collect::<Vec<_>>()
                .join(","),
            seed: self.canary_policy.seed,
            max_tokens: config
                .prompts
                .iter()
                .map(|prompt| prompt.max_tokens)
                .max()
                .unwrap_or(1),
        };
        let evaluation = evaluate_catalog_canary_token_prefix_probe(
            &spec,
            &expected_token_prefixes,
            &observed_tokens,
        );
        let provider = served_invocation
            .provider_pubkey
            .clone()
            .unwrap_or_else(|| verifying_key_hex(&self.receipt_config.provider_seed));
        let binary_hash = served_invocation
            .attestation
            .as_ref()
            .map(|attestation| attestation.contract.binary_hash.clone())
            .unwrap_or_default();
        let evidence = json!({
            "schema_version": 1,
            "kind": "mayhem-automatic-canary-probe-evidence",
            "model": model.id,
            "provider": provider,
            "enclave_id": served_invocation.enclave_id,
            "binary_hash": binary_hash,
            "canary_set": config.canary_set,
            "verification_method": config.verification_method,
            "catalog_expected_fingerprint": expected_fingerprint,
            "catalog_expected_token_prefixes": expected_token_prefixes,
            "observed_fingerprint": evaluation.observed_fingerprint,
            "evaluation": evaluation,
            "prompts": prompt_reports,
            "receipt_hashes": receipt_hashes,
        });
        let evidence_hash = stable_value_hash(&evidence);
        let session_receipt_hash = stable_value_hash(&json!({
            "domain": "mayhem-canary-receipt-bundle-v1",
            "receipt_hashes": receipt_hashes,
        }));
        let at = now_secs();
        let probe_id = stable_value_hash(&json!({
            "provider": provider,
            "enclave_id": served_invocation.enclave_id,
            "canary_set": config.canary_set,
            "epoch": self.canary_policy.epoch,
            "evidence_hash": evidence_hash,
        }));
        let mut probe_command = json!({
            "op": "probe_result",
            "probe_id": probe_id,
            "probe_kind": "canary",
            "provider": provider,
            "enclave_id": served_invocation.enclave_id,
            "binary_hash": binary_hash,
            "epoch": self.canary_policy.epoch,
            "at": at,
            "canary_set": config.canary_set,
            "verification_method": config.verification_method,
            "match_bps": evaluation.match_bps,
            "pass": evaluation.pass,
            "session_receipt_hash": session_receipt_hash,
            "evidence_hash": evidence_hash,
        });
        probe_command["auditor_sig"] = json!(probe_result_signature(
            &self.receipt_config.user_seed,
            &probe_command,
            &verifying_key_hex(&self.receipt_config.user_seed),
        ));
        let event = StoredProbeEvent {
            probe_id: probe_command["probe_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            model_id: model.id.clone(),
            provider,
            enclave_id: served_invocation.enclave_id.clone(),
            binary_hash,
            canary_set: config.canary_set.clone(),
            verification_method: evaluation.verification_method.clone(),
            expected_fingerprint: evaluation.expected_fingerprint.clone(),
            observed_fingerprint: evaluation.observed_fingerprint.clone(),
            match_bps: evaluation.match_bps,
            pass: evaluation.pass,
            reputation_event_kind: evaluation.reputation_event_kind(),
            session_receipt_hash,
            evidence_hash,
            evidence: json!({
                "evidence": evidence,
                "receipts": stored_receipts,
            }),
            probe_command,
        };
        Ok(event)
    }

    fn prepare_chat_invocation_for_route(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        route: Option<&GatewayRouteCandidate>,
        options: GatewayRequestOptions,
    ) -> Result<GatewaySessionInvocation, ApiError> {
        let prompt_text = chat_prompt_text(request);
        let session_id = session_id_for(&model.id, &prompt_text);
        let enclave_id = route
            .map(|candidate| candidate.enclave_id.clone())
            .unwrap_or_else(|| enclave_id_for_model(&model.id));
        let price_ver = route
            .map(|candidate| candidate.price_ver)
            .unwrap_or(model.mayhem.price_ref_mu.ver);
        let attestation = route.map(|candidate| GatewaySessionAttestation {
            contract: EnclaveContractRecord {
                enclave_id: candidate.enclave_id.clone(),
                admin_pubkey: candidate.admin_pubkey.clone(),
                model_id: model.id.clone(),
                model_class: model.mayhem.model_class.clone(),
                artifact_root: candidate.artifact_root.clone(),
                manifest_hash: candidate.manifest_hash.clone(),
                binary_hash: candidate.binary_hash.clone(),
                att_tier: candidate.att_tier,
                caps: candidate.caps.clone(),
            },
            trusted_binary_hashes: BTreeSet::from([candidate.binary_hash.clone()]),
            trusted_apple_app_attest_jwks: self.hardware_quote_trust.apple_app_attest_jwks.clone(),
            trusted_nvidia_gb10_device_jwks: self
                .hardware_quote_trust
                .nvidia_gb10_device_jwks
                .clone(),
            trusted_nvidia_nras_jwks: self.hardware_quote_trust.nvidia_nras_jwks.clone(),
            trusted_nvidia_offline_jwks: self.hardware_quote_trust.nvidia_offline_jwks.clone(),
        });
        let max_spend_mu = estimate_max_spend_mu(model, request, &prompt_text);
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            enclave_id: enclave_id.clone(),
            price_ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        Ok(GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            user_pubkey: verifying_key_hex(&self.receipt_config.user_seed),
            provider_pubkey: route.map(|candidate| candidate.provider.clone()),
            enclave_id,
            price_ver,
            rules_ver: self.receipt_config.rules_ver,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
            },
            attestation,
            hedge: hedge_invocation_for_model(model, options),
            receipt_cosign_enabled: self.receipt_config.cosign_enabled,
            receipt_user_seed: self.receipt_config.user_seed,
        })
    }

    fn meter_chat_session(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        output: &ChatOutput,
        invocation: &GatewaySessionInvocation,
        provider_receipt: Option<&ProviderSignedReceipt>,
    ) -> Result<StoredReceipt, ApiError> {
        let prompt_text = chat_prompt_text(request);
        let usage = ReceiptUsage::text(output.usage.prompt_tokens, output.usage.completion_tokens);
        let mu_owed_cum = calculate_mu_owed(&model.mayhem.price_ref_mu, &usage);
        if mu_owed_cum > invocation.spend_voucher.body.max_spend_mu {
            return Err(ApiError::payment_required(
                "session usage exceeded signed spend voucher",
                Some("model"),
            ));
        }

        if !self.receipt_config.cosign_enabled {
            self.pause_session(PausedSession {
                session_id: invocation.session_id.clone(),
                reason: "receipt co-signing refused; session paused".to_owned(),
            });
            return Err(ApiError::conflict(
                "receipt co-signing refused; session paused",
                None,
            ));
        }

        let provider = invocation
            .provider_pubkey
            .clone()
            .unwrap_or_else(|| verifying_key_hex(&self.receipt_config.provider_seed));
        let receipt = if let Some(provider_receipt) = provider_receipt {
            self.cosign_provider_receipt(
                model,
                invocation,
                provider_receipt,
                ExpectedProviderReceipt {
                    provider: &provider,
                    usage: usage.clone(),
                    mu_owed_cum,
                    prompt_hash: blake3_hex(prompt_text.as_bytes()),
                },
            )?
        } else {
            let body = ReceiptBody {
                schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
                session_id: invocation.session_id.clone(),
                seq: 1,
                final_receipt: true,
                user: invocation.user_pubkey.clone(),
                provider,
                enclave_id: invocation.enclave_id.clone(),
                model_id: model.id.clone(),
                price_ver: invocation.price_ver,
                rules_ver: invocation.rules_ver,
                usage,
                mu_owed_cum,
                prompt_hash: blake3_hex(prompt_text.as_bytes()),
                ts: now_millis_u64(),
            };
            let receipt_payload = receipt_signing_bytes(&body).map_err(ApiError::internal)?;
            let user_sig = sign_hex(&self.receipt_config.user_seed, &receipt_payload);
            SessionReceipt {
                body,
                enclave_sig: sign_hex(&self.receipt_config.enclave_seed, &receipt_payload),
                user_sig,
            }
        };
        let user_sig = receipt.user_sig.clone();
        let receipt_ack = ReceiptAck {
            session_id: receipt.body.session_id.clone(),
            seq: receipt.body.seq,
            user_sig,
        };
        let stored = StoredReceipt {
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
        };
        self.record_receipt(stored.clone());
        Ok(stored)
    }

    fn cosign_provider_receipt(
        &self,
        model: &GatewayModel,
        invocation: &GatewaySessionInvocation,
        provider_receipt: &ProviderSignedReceipt,
        expected: ExpectedProviderReceipt<'_>,
    ) -> Result<SessionReceipt, ApiError> {
        let body = &provider_receipt.body;
        validate_provider_receipt(model, invocation, provider_receipt, expected)
            .map_err(|err| ApiError::bad_gateway(err.message, Some("model")))?;
        let receipt_ack = receipt_ack_for_body(&self.receipt_config.user_seed, body)
            .map_err(ApiError::internal)?;
        Ok(SessionReceipt {
            body: body.clone(),
            enclave_sig: provider_receipt.enclave_sig.clone(),
            user_sig: receipt_ack.user_sig,
        })
    }

    fn meter_session(
        &self,
        model: &GatewayModel,
        prompt_text: &str,
        usage: ReceiptUsage,
    ) -> Result<StoredReceipt, ApiError> {
        let mu_owed_cum = calculate_mu_owed(&model.mayhem.price_ref_mu, &usage);
        let max_spend_mu = mu_owed_cum.max(1_000);
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }

        let session_id = session_id_for(&model.id, prompt_text);
        let enclave_id = enclave_id_for_model(&model.id);
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            enclave_id: enclave_id.clone(),
            price_ver: model.mayhem.price_ref_mu.ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        let voucher = SpendVoucher {
            body: voucher_body,
            user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
        };

        if !self.receipt_config.cosign_enabled {
            self.pause_session(PausedSession {
                session_id,
                reason: "receipt co-signing refused; session paused".to_owned(),
            });
            return Err(ApiError::conflict(
                "receipt co-signing refused; session paused",
                None,
            ));
        }

        let body = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: session_id.clone(),
            seq: 1,
            final_receipt: true,
            user: verifying_key_hex(&self.receipt_config.user_seed),
            provider: verifying_key_hex(&self.receipt_config.provider_seed),
            enclave_id,
            model_id: model.id.clone(),
            price_ver: model.mayhem.price_ref_mu.ver,
            rules_ver: self.receipt_config.rules_ver,
            usage,
            mu_owed_cum,
            prompt_hash: blake3_hex(prompt_text.as_bytes()),
            ts: now_millis_u64(),
        };
        let receipt_payload = receipt_signing_bytes(&body).map_err(ApiError::internal)?;
        let user_sig = sign_hex(&self.receipt_config.user_seed, &receipt_payload);
        let receipt = SessionReceipt {
            body,
            enclave_sig: sign_hex(&self.receipt_config.enclave_seed, &receipt_payload),
            user_sig: user_sig.clone(),
        };
        let receipt_ack = ReceiptAck {
            session_id: receipt.body.session_id.clone(),
            seq: receipt.body.seq,
            user_sig,
        };
        let stored = StoredReceipt {
            voucher,
            receipt,
            receipt_ack,
        };
        self.record_receipt(stored.clone());
        Ok(stored)
    }
}

fn frame_contract_version(frame: &Value) -> Option<u32> {
    frame
        .get("contract_version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
}

fn contract_upgrade_required_reason(expected: u32, actual: Option<u32>) -> String {
    let actual = actual
        .map(|version| version.to_string())
        .unwrap_or_else(|| "missing/legacy".to_owned());
    format!(
        "contract upgrade required: expected CONTRACT_VERSION {expected}, got {actual}; update Mayhem on the out-of-sync node before opening sessions"
    )
}

fn canary_route_key(model: &GatewayModel, invocation: &GatewaySessionInvocation) -> String {
    format!(
        "{}:{}:{}",
        model.id,
        invocation
            .provider_pubkey
            .as_deref()
            .unwrap_or("local-provider"),
        invocation.enclave_id
    )
}

fn canary_served_route(
    model: &GatewayModel,
    invocation: &GatewaySessionInvocation,
) -> Option<GatewayRouteCandidate> {
    let provider = invocation.provider_pubkey.as_deref()?;
    model
        .mayhem
        .route_candidates
        .iter()
        .find(|candidate| {
            candidate.provider == provider && candidate.enclave_id == invocation.enclave_id
        })
        .cloned()
}

fn canary_expected_fingerprint(
    config: &GatewayCanaryModelConfig,
    invocation: &GatewaySessionInvocation,
) -> Option<String> {
    invocation
        .attestation
        .as_ref()
        .and_then(|attestation| {
            config
                .fingerprints_by_artifact_root
                .get(&attestation.contract.artifact_root)
                .cloned()
        })
        .or_else(|| config.default_fingerprint.clone())
}

fn canary_expected_token_prefixes(
    config: &GatewayCanaryModelConfig,
    invocation: &GatewaySessionInvocation,
) -> Option<BTreeMap<String, Vec<i32>>> {
    invocation
        .attestation
        .as_ref()
        .and_then(|attestation| {
            config
                .token_prefixes_by_artifact_root
                .get(&attestation.contract.artifact_root)
                .cloned()
        })
        .or_else(|| config.default_token_prefixes.clone())
}

fn canary_chat_request(
    model_id: &str,
    _config: &GatewayCanaryModelConfig,
    prompt: &GatewayCanaryPrompt,
    seed: i64,
) -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: model_id.to_owned(),
        messages: prompt.messages.clone(),
        stream: true,
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
        tools: prompt.tools.clone(),
        tool_choice: None,
        response_format: None,
        temperature: Some(DEFAULT_CANARY_TEMPERATURE),
        top_p: None,
        seed: Some(seed),
        stop: None,
        max_tokens: Some(prompt.max_tokens.max(1)),
    }
}

fn failed_canary_runtime_probe(
    model: &GatewayModel,
    invocation: &GatewaySessionInvocation,
    config: &GatewayCanaryModelConfig,
    reason: String,
    epoch: u64,
    auditor_seed: &[u8; 32],
) -> StoredProbeEvent {
    let provider = invocation
        .provider_pubkey
        .clone()
        .unwrap_or_else(|| "local-provider".to_owned());
    let binary_hash = invocation
        .attestation
        .as_ref()
        .map(|attestation| attestation.contract.binary_hash.clone())
        .unwrap_or_default();
    let evidence = json!({
        "schema_version": 1,
        "kind": "mayhem-automatic-canary-probe-runtime-failure",
        "model": model.id,
        "provider": provider,
        "enclave_id": invocation.enclave_id,
        "binary_hash": binary_hash,
        "canary_set": config.canary_set,
        "verification_method": config.verification_method,
        "reason": reason,
    });
    let evidence_hash = stable_value_hash(&evidence);
    let at = now_secs();
    let probe_id = stable_value_hash(&json!({
        "provider": provider,
        "enclave_id": invocation.enclave_id,
        "canary_set": config.canary_set,
        "epoch": epoch,
        "runtime_failure": evidence_hash,
    }));
    let mut probe_command = json!({
        "op": "probe_result",
        "probe_id": probe_id,
        "probe_kind": "canary",
        "provider": provider,
        "enclave_id": invocation.enclave_id,
        "binary_hash": binary_hash,
        "epoch": epoch,
        "at": at,
        "canary_set": config.canary_set,
        "verification_method": config.verification_method,
        "match_bps": 0,
        "pass": false,
        "session_receipt_hash": stable_value_hash(&json!({
            "domain": "mayhem-canary-runtime-failure-v1",
            "evidence_hash": evidence_hash,
        })),
        "evidence_hash": evidence_hash,
    });
    probe_command["auditor_sig"] = json!(probe_result_signature(
        auditor_seed,
        &probe_command,
        &verifying_key_hex(auditor_seed),
    ));
    StoredProbeEvent {
        probe_id,
        model_id: model.id.clone(),
        provider,
        enclave_id: invocation.enclave_id.clone(),
        binary_hash,
        canary_set: config.canary_set.clone(),
        verification_method: config.verification_method.clone(),
        expected_fingerprint: canary_expected_fingerprint(config, invocation).unwrap_or_default(),
        observed_fingerprint: String::new(),
        match_bps: 0,
        pass: false,
        reputation_event_kind: ReputationEventKind::ProbeFail,
        session_receipt_hash: probe_command["session_receipt_hash"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        evidence_hash,
        evidence,
        probe_command,
    }
}

fn hedge_invocation_for_model(
    model: &GatewayModel,
    options: GatewayRequestOptions,
) -> GatewayHedgeInvocation {
    let candidates = model
        .mayhem
        .route_candidates
        .iter()
        .map(|candidate| {
            ProviderKey::new(
                candidate.provider.clone(),
                candidate.enclave_id.clone(),
                candidate.room_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    let failover_state = SessionFailoverState::new(
        FailoverPolicy::default(),
        SessionPriceMu {
            rate_map: model.mayhem.price_ref_mu.rate_map.clone(),
        },
        0,
        0,
    );
    let planned_probe_count = failover_state
        .hedge_plan(&candidates, options.hedge_requested.then_some("1"), 0)
        .map(|plan| plan.probes.len())
        .unwrap_or(0);
    GatewayHedgeInvocation {
        requested: options.hedge_requested,
        planned_probe_count,
    }
}

fn deterministic_chat_output(model: &GatewayModel, request: &ChatCompletionRequest) -> ChatOutput {
    let prompt_text = request
        .messages
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n");
    let output = if let Some(tool_result) = last_tool_result(&request.messages) {
        ChatOutput {
            content: Some(format!("Tool result received: {tool_result}")),
            tool_call: None,
            artifacts: Vec::new(),
            finish_reason: "stop".to_owned(),
            usage: usage_for(&prompt_text, &tool_result),
        }
    } else if let Some(name) = requested_tool_name(request) {
        let tool_call = ToolCallOutput {
            id: make_id("call"),
            arguments: deterministic_tool_arguments(&name),
            name,
        };
        ChatOutput {
            content: None,
            tool_call: Some(tool_call),
            artifacts: Vec::new(),
            finish_reason: "tool_calls".to_owned(),
            usage: usage_for(&prompt_text, "{}"),
        }
    } else {
        let modalities = chat_input_modalities(&request.messages);
        let content = if wants_json(&request.response_format) {
            json!({
                "ok": true,
                "model": model.id,
                "mayhem": { "response_format": request.response_format },
            })
            .to_string()
        } else if modalities.image {
            format!(
                "Mayhem response from {}: received image input with {}",
                model.id,
                last_user_text(&request.messages)
            )
        } else if modalities.audio {
            format!(
                "Mayhem response from {}: received audio input with {}",
                model.id,
                last_user_text(&request.messages)
            )
        } else {
            format!(
                "Mayhem response from {}: {}",
                model.id,
                last_user_text(&request.messages)
            )
        };
        ChatOutput {
            usage: usage_for(&prompt_text, &content),
            content: Some(content),
            tool_call: None,
            artifacts: Vec::new(),
            finish_reason: "stop".to_owned(),
        }
    };
    output
}

fn chat_response_value(
    id: &str,
    created: u64,
    model: &GatewayModel,
    output: &ChatOutput,
    receipt: &StoredReceipt,
    mayhem_meta: ResponseMayhemMeta<'_>,
) -> Value {
    let message = if let Some(tool_call) = &output.tool_call {
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [tool_call_value(tool_call)],
        })
    } else {
        json!({
            "role": "assistant",
            "content": output.content.clone().unwrap_or_default(),
        })
    };
    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model.id,
        "choices": [{
            "index": 0,
            "message": message,
            "logprobs": null,
            "finish_reason": output.finish_reason,
        }],
        "usage": output.usage,
        "mayhem": {
            "backend": mayhem_meta.backend,
            "direct_session": mayhem_meta.direct_session,
            "artifacts": artifact_summaries(&output.artifacts),
            "hedge": {
                "requested": mayhem_meta.hedge.requested,
                "planned_probe_count": mayhem_meta.hedge.planned_probe_count,
            },
            "model": model.mayhem,
            "receipt": receipt_summary(receipt),
        },
    })
}

fn chat_stream_chunks(
    id: &str,
    created: u64,
    model: &str,
    output: &ChatOutput,
    receipt: &StoredReceipt,
    mayhem_meta: ResponseMayhemMeta<'_>,
    include_usage: bool,
) -> Vec<Value> {
    let mut chunks = vec![chat_chunk(
        id,
        created,
        model,
        json!({ "role": "assistant" }),
        None,
        None,
    )];
    if let Some(tool_call) = &output.tool_call {
        chunks.push(chat_chunk(
            id,
            created,
            model,
            json!({ "tool_calls": [tool_call_value(tool_call)] }),
            None,
            None,
        ));
    } else if let Some(content) = &output.content {
        for part in stream_parts(content) {
            chunks.push(chat_chunk(
                id,
                created,
                model,
                json!({ "content": part }),
                None,
                None,
            ));
        }
    }
    let mut finish_chunk = chat_chunk(
        id,
        created,
        model,
        json!({}),
        Some(output.finish_reason.as_str()),
        None,
    );
    if !output.artifacts.is_empty() {
        finish_chunk["mayhem"] = json!({
            "artifacts": artifact_summaries(&output.artifacts),
        });
    }
    chunks.push(finish_chunk);
    if include_usage {
        chunks.push(json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [],
            "usage": output.usage,
            "mayhem": {
                "backend": mayhem_meta.backend,
                "direct_session": mayhem_meta.direct_session,
                "artifacts": artifact_summaries(&output.artifacts),
                "hedge": {
                    "requested": mayhem_meta.hedge.requested,
                    "planned_probe_count": mayhem_meta.hedge.planned_probe_count,
                },
                "receipt": receipt_summary(receipt),
            },
        }));
    }
    chunks
}

fn chat_chunk(
    id: &str,
    created: u64,
    model: &str,
    delta: Value,
    finish_reason: Option<&str>,
    usage: Option<Usage>,
) -> Value {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "logprobs": null,
            "finish_reason": finish_reason,
        }],
        "usage": usage,
    })
}

fn artifact_summaries(artifacts: &[GatewayArtifactOutput]) -> Vec<Value> {
    artifacts
        .iter()
        .map(|artifact| {
            json!({
                "id": artifact.id,
                "content_type": artifact.content_type,
                "bytes": artifact.bytes.len(),
                "blake3": artifact.blake3,
            })
        })
        .collect()
}

fn tool_call_value(tool_call: &ToolCallOutput) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.arguments,
        }
    })
}

fn receipt_summary(receipt: &StoredReceipt) -> Value {
    json!({
        "session_id": receipt.receipt.body.session_id,
        "seq": receipt.receipt.body.seq,
        "final": receipt.receipt.body.final_receipt,
        "mu_owed_cum": receipt.receipt.body.mu_owed_cum,
        "prompt_hash": receipt.receipt.body.prompt_hash,
        "receipt_ack": receipt.receipt_ack,
    })
}

fn sse_response(chunks: Vec<Value>) -> Response {
    let events = chunks
        .into_iter()
        .map(|chunk| {
            Ok::<Event, Infallible>(
                Event::default()
                    .json_data(chunk)
                    .unwrap_or_else(|_| Event::default().data("{}")),
            )
        })
        .chain(std::iter::once(Ok(Event::default().data("[DONE]"))));
    let mut response = Sse::new(stream::iter(events)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response
}

fn model_from_catalog_value(model: &Value, created: u64) -> Option<GatewayModel> {
    let id = model.get("model_id")?.as_str()?.to_owned();
    let caps = model.get("caps").unwrap_or(&Value::Null);
    let price = model.get("price_ref_mu").unwrap_or(&Value::Null);
    let rate_map = price_rate_map_from_catalog_value(price);
    let tiers = attestation_tiers_from_catalog_value(model);
    Some(GatewayModel {
        id,
        created,
        owned_by: "mayhem".to_owned(),
        mayhem: MayhemModelInfo {
            model_class: model
                .get("model_class")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_MODEL_CLASS)
                .to_owned(),
            providers_online: 0,
            rooms: 0,
            price_ref_mu: PriceRefMu {
                denom: price
                    .get("denom")
                    .and_then(Value::as_str)
                    .unwrap_or("mu_usd")
                    .to_owned(),
                ver: price
                    .get("ver")
                    .or_else(|| price.get("price_ver"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1),
                rate_map,
            },
            attestation_tier_labels: attestation_tier_labels_from_catalog_value(model)
                .unwrap_or_else(|| attestation_tier_labels_for_counts(&tiers)),
            attestation_tiers: tiers,
            caps: ModelCaps {
                tools: caps.get("tools").and_then(Value::as_bool).unwrap_or(false),
                json: caps.get("json").and_then(Value::as_bool).unwrap_or(false),
                ctx: caps
                    .get("ctx_max")
                    .and_then(Value::as_u64)
                    .and_then(|ctx| u32::try_from(ctx).ok())
                    .unwrap_or(0),
                vision: caps.get("vision").and_then(Value::as_bool).unwrap_or(false),
                image: caps.get("image").and_then(Value::as_bool).unwrap_or(false),
                video: caps.get("video").and_then(Value::as_bool).unwrap_or(false),
                audio: caps.get("audio").and_then(Value::as_bool).unwrap_or(false),
                output_modality: caps
                    .get("output_modality")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                output_modalities: caps_output_modalities(caps),
            },
            adapter: shape_adapter_from_catalog_value(model),
            source: "catalog".to_owned(),
            kyb_identities: Vec::new(),
            route_candidates: Vec::new(),
        },
    })
}

fn price_rate_map_from_catalog_value(price: &Value) -> Vec<RateMapEntry> {
    if let Some(entries) = price.get("rate_map").and_then(Value::as_array) {
        let rate_map = entries
            .iter()
            .filter_map(|entry| {
                Some(RateMapEntry {
                    unit: entry.get("unit")?.as_str()?.to_owned(),
                    per_unit_mu: entry.get("per_unit_mu")?.as_u64()?,
                    granularity: entry.get("granularity")?.as_u64()?,
                })
            })
            .collect::<Vec<_>>();
        if !rate_map.is_empty() {
            return normalize_rate_map(rate_map);
        }
    }

    text_generation_rate_map(
        price.get("in_per_1k").and_then(Value::as_u64).unwrap_or(0),
        price.get("out_per_1k").and_then(Value::as_u64).unwrap_or(0),
    )
}

fn caps_output_modalities(caps: &Value) -> Vec<String> {
    if let Some(entries) = caps.get("output_modalities").and_then(Value::as_array) {
        return entries
            .iter()
            .filter_map(Value::as_str)
            .filter(|modality| !modality.is_empty())
            .map(str::to_owned)
            .collect();
    }
    caps.get("output_modality")
        .and_then(Value::as_str)
        .filter(|modality| !modality.is_empty())
        .map(|modality| vec![modality.to_owned()])
        .unwrap_or_default()
}

fn shape_adapter_from_catalog_value(model: &Value) -> ShapeAdapterInfo {
    let adapter = model.get("adapter").unwrap_or(&Value::Null);
    let defaults = ShapeAdapterInfo::default();
    ShapeAdapterInfo {
        request_shape_family: adapter
            .get("request_shape_family")
            .and_then(Value::as_str)
            .unwrap_or(defaults.request_shape_family.as_str())
            .to_owned(),
        chat_template_id: adapter
            .get("chat_template_id")
            .and_then(Value::as_str)
            .unwrap_or(defaults.chat_template_id.as_str())
            .to_owned(),
        tool_call_strategy: adapter
            .get("tool_call_strategy")
            .and_then(Value::as_str)
            .unwrap_or(defaults.tool_call_strategy.as_str())
            .to_owned(),
        reasoning_passthrough: adapter
            .get("reasoning_passthrough")
            .and_then(Value::as_str)
            .unwrap_or(defaults.reasoning_passthrough.as_str())
            .to_owned(),
        modality_set: adapter
            .get("modality_set")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|entries| !entries.is_empty())
            .unwrap_or(defaults.modality_set),
        response_normalization: adapter
            .get("response_normalization")
            .and_then(Value::as_str)
            .unwrap_or(defaults.response_normalization.as_str())
            .to_owned(),
    }
}

fn attestation_tiers_from_catalog_value(model: &Value) -> BTreeMap<String, u32> {
    let mut tiers = BTreeMap::new();
    if let Some(object) = model.get("attestation_tiers").and_then(Value::as_object) {
        for (tier, count) in object {
            if let Some(count) = count.as_u64().and_then(|count| u32::try_from(count).ok()) {
                tiers.insert(tier.clone(), count);
            }
        }
    }
    if tiers.is_empty() {
        tiers.insert("T1".to_owned(), 0);
    }
    tiers
}

fn attestation_tier_labels_from_catalog_value(model: &Value) -> Option<BTreeMap<String, String>> {
    let object = model
        .get("attestation_tier_labels")
        .and_then(Value::as_object)?;
    let labels = object
        .iter()
        .filter_map(|(tier, label)| {
            label
                .as_str()
                .filter(|label| !label.trim().is_empty())
                .map(|label| (tier.clone(), label.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    if labels.is_empty() {
        None
    } else {
        Some(labels)
    }
}

fn attestation_tier_labels_for_counts(tiers: &BTreeMap<String, u32>) -> BTreeMap<String, String> {
    tiers
        .keys()
        .map(|tier| {
            let label = match tier.as_str() {
                "T1" => "Tier 1 - software self-attestation; economic/trust only",
                "T2" => {
                    "Tier 2 - hardware device identity; Apple App Attest strong / NVIDIA GB10 device medium; not prompt-confidential"
                }
                "T3" => {
                    "Tier 3 - hardware confidential compute; prompt-confidential when supported"
                }
                "T4" => "Tier 4 - admin KYB verified identity; not prompt-confidential",
                _ => "Unknown attestation tier",
            };
            (tier.clone(), label.to_owned())
        })
        .collect()
}

fn require_model(state: &GatewayState, model: &str) -> Result<GatewayModel, ApiError> {
    if let Some(model) = state.model(model) {
        return Ok(model);
    }
    if model == "mayhem/default" {
        if let Some(model) = state.first_model() {
            return Ok(model);
        }
    }
    Err(ApiError {
        status: StatusCode::NOT_FOUND,
        message: format!("model '{model}' is not available"),
        param: Some("model"),
    })
}

fn calculate_mu_owed(price: &PriceRefMu, usage: &ReceiptUsage) -> u64 {
    text_usage_mu(&price.rate_map, usage)
}

fn session_id_for(model_id: &str, prompt_text: &str) -> String {
    blake3_hex(format!("{model_id}:{prompt_text}:{}", now_millis()).as_bytes())
}

fn enclave_id_for_model(model_id: &str) -> String {
    blake3_hex(format!("mayhem-local-enclave:{model_id}").as_bytes())
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn stable_value_hash(value: &Value) -> String {
    blake3::hash(stable_json_value(value).to_string().as_bytes())
        .to_hex()
        .to_string()
}

fn stable_json_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(stable_json_value).collect()),
        Value::Object(map) => {
            let mut stable = serde_json::Map::new();
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in entries {
                stable.insert(key.clone(), stable_json_value(value));
            }
            Value::Object(stable)
        }
        other => other.clone(),
    }
}

fn probe_result_signature(auditor_seed: &[u8; 32], value: &Value, auditor: &str) -> String {
    let evidence = json!({
        "auditor": auditor,
        "probe_id": value.get("probe_id").cloned().unwrap_or(Value::Null),
        "probe_kind": value.get("probe_kind").cloned().unwrap_or(Value::Null),
        "provider": value.get("provider").cloned().unwrap_or(Value::Null),
        "enclave_id": value.get("enclave_id").cloned().unwrap_or(Value::Null),
        "binary_hash": value.get("binary_hash").cloned().unwrap_or(Value::Null),
        "canary_set": value.get("canary_set").cloned().unwrap_or(Value::Null),
        "verification_method": value.get("verification_method").cloned().unwrap_or(Value::Null),
        "session_receipt_hash": value.get("session_receipt_hash").cloned().unwrap_or(Value::Null),
        "evidence_hash": value.get("evidence_hash").cloned().unwrap_or(Value::Null),
        "match_bps": value.get("match_bps").cloned().unwrap_or(Value::Null),
        "pass": value.get("pass").cloned().unwrap_or(Value::Null),
        "epoch": value.get("epoch").cloned().unwrap_or(Value::Null),
        "at": value.get("at").cloned().unwrap_or(Value::Null),
    });
    let message = format!("mayhem-probe-result-v1{}", stable_json_value(&evidence));
    sign_hex(auditor_seed, message.as_bytes())
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn sign_hex(seed: &[u8; 32], payload: &[u8]) -> String {
    let key = SigningKey::from_bytes(seed);
    hex::encode(key.sign(payload).to_bytes())
}

fn verifying_key_hex(seed: &[u8; 32]) -> String {
    hex::encode(SigningKey::from_bytes(seed).verifying_key().to_bytes())
}

fn requested_tool_name(request: &ChatCompletionRequest) -> Option<String> {
    if matches_tool_choice_none(&request.tool_choice) {
        return None;
    }
    if let Some(name) = tool_choice_function_name(&request.tool_choice) {
        return Some(name);
    }
    request.tools.as_ref()?.iter().find_map(|tool| {
        tool.get("function")?
            .get("name")?
            .as_str()
            .map(str::to_owned)
    })
}

fn deterministic_tool_arguments(name: &str) -> String {
    match name {
        "bash" => json!({ "command": "printf mayhem-opencode-tool-ok" }).to_string(),
        "write" => {
            json!({ "filePath": "mayhem-opencode-tool-ok.txt", "content": "mayhem-opencode-tool-ok" })
                .to_string()
        }
        _ => "{}".to_owned(),
    }
}

fn matches_tool_choice_none(value: &Option<Value>) -> bool {
    matches!(value, Some(Value::String(choice)) if choice == "none")
}

fn tool_choice_function_name(value: &Option<Value>) -> Option<String> {
    value
        .as_ref()?
        .get("function")?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

fn wants_json(value: &Option<Value>) -> bool {
    matches!(
        value
            .as_ref()
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str),
        Some("json_object" | "json_schema")
    )
}

fn last_tool_result(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "tool")
        .map(message_to_text)
}

fn last_user_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(message_to_text)
        .unwrap_or_default()
}

fn message_to_text(message: &ChatMessage) -> String {
    content_to_text(&message.content)
}

fn chat_prompt_text(request: &ChatCompletionRequest) -> String {
    request
        .messages
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(content_part_to_text)
            .collect::<Vec<_>>()
            .join(""),
        other => other.to_string(),
    }
}

fn content_part_to_text(part: &Value) -> String {
    match part.get("type").and_then(Value::as_str) {
        Some("text") => part
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        Some("image_url") => {
            let url = part
                .get("image_url")
                .and_then(|image| image.get("url"))
                .or_else(|| part.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("[image:{}]", blake3_hex(url.as_bytes()))
        }
        Some("input_audio") => {
            let audio = part.get("input_audio").unwrap_or(&Value::Null);
            let format = audio
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let data = audio
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("[audio:{format}:{}]", blake3_hex(data.as_bytes()))
        }
        _ => part
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| content_to_text(part)),
    }
}

fn prompt_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(content_to_text)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn stream_parts(content: &str) -> Vec<String> {
    let mut parts = content
        .split_inclusive(' ')
        .map(str::to_owned)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() && !content.is_empty() {
        parts.push(content.to_owned());
    }
    parts
}

fn usage_for(input: &str, output: &str) -> Usage {
    let prompt_tokens = rough_tokens(input);
    let completion_tokens = rough_tokens(output);
    Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

fn estimate_max_spend_mu(
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    prompt_text: &str,
) -> u64 {
    let usage = ReceiptUsage::text(
        rough_tokens(prompt_text),
        u64::from(request.max_tokens.unwrap_or(1024).max(1)),
    );
    calculate_mu_owed(&model.mayhem.price_ref_mu, &usage).max(1_000)
}

fn rough_tokens(text: &str) -> u64 {
    if text.trim().is_empty() {
        0
    } else {
        text.split_whitespace().count() as u64
    }
}

fn make_id(prefix: &str) -> String {
    format!("{prefix}-mayhem-{}", now_millis())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_secs()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis()
}

fn now_millis_u64() -> u64 {
    match u64::try_from(now_millis()) {
        Ok(millis) => millis,
        Err(_) => u64::MAX,
    }
}

impl ApiError {
    fn bad_request(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            param,
        }
    }

    fn payment_required(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::PAYMENT_REQUIRED,
            message: message.into(),
            param,
        }
    }

    fn conflict(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
            param,
        }
    }

    fn bad_gateway(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
            param,
        }
    }

    fn internal(err: serde_json::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("receipt signing payload failed: {err}"),
            param: None,
        }
    }

    fn internal_message(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            param: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": {
                    "message": self.message,
                    "type": "invalid_request_error",
                    "param": self.param,
                    "code": null,
                }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mayhem_proto::{
        attestation_signing_bytes, receipt_signing_bytes_for_version, AttestationSigner,
    };

    #[test]
    fn sc_bridge_direct_session_defaults_match_p4_3_failover_timeouts() {
        let config = ScBridgeGatewaySessionConfig::new("ws://127.0.0.1:49222", "token");
        assert_eq!(
            config.open_timeout,
            Duration::from_millis(DEFAULT_OPEN_TIMEOUT_MILLIS)
        );
        assert_eq!(
            config.frame_timeout,
            Duration::from_millis(DEFAULT_STALL_TIMEOUT_MILLIS)
        );
        assert!(config.open_timeout <= Duration::from_secs(3));
        assert!(config.frame_timeout < Duration::from_secs(20));
    }

    #[test]
    fn direct_session_watchdog_resets_idle_gap_per_delta() {
        let mut watchdog = DirectSessionWatchdog::new(
            0,
            Duration::from_millis(15),
            Duration::from_millis(15),
            None,
        );
        assert_eq!(watchdog.next_wait_millis(15), Ok(1));
        watchdog.record_delta(10);
        assert_eq!(watchdog.next_wait_millis(25), Ok(1));
        watchdog.record_delta(25);
        assert_eq!(watchdog.next_wait_millis(40), Ok(1));
        watchdog.record_delta(40);
        assert_eq!(watchdog.next_wait_millis(55), Ok(1));
        assert_eq!(
            watchdog.next_wait_millis(56),
            Err(DirectSessionTimeoutKind::IdleGap)
        );
    }

    #[test]
    fn direct_session_watchdog_trips_ttft_before_first_delta() {
        let watchdog = DirectSessionWatchdog::new(
            100,
            Duration::from_millis(10),
            Duration::from_millis(15),
            None,
        );
        assert_eq!(watchdog.next_wait_millis(110), Ok(1));
        assert_eq!(
            watchdog.next_wait_millis(111),
            Err(DirectSessionTimeoutKind::TimeToFirstToken)
        );
    }

    #[test]
    fn direct_session_watchdog_has_distinct_overall_budget() {
        let mut watchdog = DirectSessionWatchdog::new(
            0,
            Duration::from_millis(10),
            Duration::from_millis(10),
            Some(Duration::from_millis(30)),
        );
        watchdog.record_delta(25);
        assert_eq!(watchdog.next_wait_millis(30), Ok(1));
        assert_eq!(
            watchdog.next_wait_millis(31),
            Err(DirectSessionTimeoutKind::OverallBudget)
        );
    }

    #[test]
    fn embedding_canary_matches_exact_vector_and_cosine_tolerance() {
        let expected = deterministic_embedding_vector("admin/embed-fixture", "fixed canary", 16);
        let same = deterministic_embedding_vector("admin/embed-fixture", "fixed canary", 16);
        let different =
            deterministic_embedding_vector("admin/embed-fixture", "different canary", 16);

        assert_eq!(expected, same);
        assert_eq!(
            embedding_cosine_similarity_bps(&expected, &same),
            Some(10_000)
        );
        assert!(embedding_canary_matches(&expected, &same, 0));
        assert!(!embedding_canary_matches(&expected, &different, 1));
        assert_eq!(embedding_cosine_similarity_bps(&expected, &[]), None);
    }

    #[test]
    fn direct_session_accept_pins_session_enclave_provider_and_signature() {
        let invocation = test_invocation();
        let frame = test_accept_frame(&invocation);
        validate_direct_session_accept(
            &frame,
            &invocation,
            test_open_head().as_str(),
            test_att_nonce().as_str(),
            test_now_ts(),
        )
        .expect("matching accept is valid");

        let mut wrong_session = frame.clone();
        wrong_session["session_id"] = json!("bb".repeat(32));
        assert_accept_err(&wrong_session, &invocation, "session_id");

        let mut legacy_contract = frame.clone();
        legacy_contract
            .as_object_mut()
            .expect("s.accept object")
            .remove("contract_version");
        legacy_contract["sig"] = json!("ee".repeat(64));
        assert_accept_err(&legacy_contract, &invocation, "contract upgrade required");

        let mut future_contract = frame.clone();
        future_contract["contract_version"] = json!(invocation.contract_version + 1);
        assert_accept_err(&future_contract, &invocation, "contract upgrade required");

        let mut wrong_open_head = frame.clone();
        wrong_open_head["open_head"] = json!("99".repeat(32));
        assert_accept_err(&wrong_open_head, &invocation, "open_head");

        let mut wrong_att_nonce = frame.clone();
        wrong_att_nonce["att_nonce"] = json!("98".repeat(32));
        assert_accept_err(&wrong_att_nonce, &invocation, "att_nonce");

        let mut wrong_enclave = frame.clone();
        wrong_enclave["att_report"]["enclave_id"] = json!("cc".repeat(32));
        assert_accept_err(&wrong_enclave, &invocation, "enclave_id");

        let mut wrong_report_nonce = frame.clone();
        wrong_report_nonce["att_report"] =
            test_attestation_report_with_nonce(&invocation, "97".repeat(32));
        sign_accept_frame(&mut wrong_report_nonce);
        assert_accept_err(&wrong_report_nonce, &invocation, "nonce_u");

        let mut wrong_manifest = frame.clone();
        wrong_manifest["att_report"] =
            test_attestation_report_with_mutation(&invocation, test_att_nonce(), |report| {
                report.manifest_hash = "ab".repeat(32);
            });
        sign_accept_frame(&mut wrong_manifest);
        assert_accept_err(&wrong_manifest, &invocation, "manifest_hash");

        let mut wrong_provider = frame.clone();
        wrong_provider["att_report"]["provider_pubkey"] = json!("dd".repeat(32));
        assert_accept_err(&wrong_provider, &invocation, "provider_pubkey");

        let mut wrong_report_sig = frame.clone();
        wrong_report_sig["att_report"]["sig_provider"] = json!("12".repeat(64));
        sign_accept_frame(&mut wrong_report_sig);
        assert_accept_err(&wrong_report_sig, &invocation, "provider signature");

        let mut wrong_sig = frame;
        wrong_sig["sig"] = json!("ee".repeat(64));
        assert_accept_err(&wrong_sig, &invocation, "signature");
    }

    #[test]
    fn provider_signed_receipt_must_match_admin_terms_usage_and_enclave_key() {
        let state = GatewayState::fixture();
        let model = test_model();
        let request = test_chat_request(&model.id);
        let output = test_chat_output();
        let invocation = test_invocation();
        let provider_receipt = test_provider_receipt(&model, &request, &output, &invocation);

        let stored = state
            .meter_chat_session(
                &model,
                &request,
                &output,
                &invocation,
                Some(&provider_receipt),
            )
            .expect("matching provider receipt is co-signed");
        assert_eq!(stored.receipt.enclave_sig, provider_receipt.enclave_sig);
        assert_eq!(stored.receipt.body, provider_receipt.body);
        assert_eq!(stored.receipt_ack.user_sig, stored.receipt.user_sig);

        let mut legacy_signed = provider_receipt.clone();
        legacy_signed.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes_for_version(&legacy_signed.body, 1).unwrap(),
        );
        verify_provider_receipt_signature(&legacy_signed)
            .expect("legacy v1 provider receipt signature remains accepted");

        let live_ack = direct_session_receipt_ack(
            &request,
            &output,
            &invocation,
            &provider_receipt,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect("direct session ack signs the accepted receipt");
        assert_eq!(live_ack, stored.receipt_ack);

        let mut wrong_amount = provider_receipt.clone();
        wrong_amount.body.mu_owed_cum = wrong_amount.body.mu_owed_cum.saturating_add(1);
        wrong_amount.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&wrong_amount.body).unwrap(),
        );
        let err = state
            .meter_chat_session(&model, &request, &output, &invocation, Some(&wrong_amount))
            .expect_err("wrong amount must be rejected");
        assert!(err.message.contains("amount"));

        let mut wrong_sig = provider_receipt;
        wrong_sig.enclave_sig = "11".repeat(64);
        let err = state
            .meter_chat_session(&model, &request, &output, &invocation, Some(&wrong_sig))
            .expect_err("wrong enclave signature must be rejected");
        assert!(err.message.contains("signature"));
    }

    fn assert_accept_err(frame: &Value, invocation: &GatewaySessionInvocation, needle: &str) {
        let err = validate_direct_session_accept(
            frame,
            invocation,
            test_open_head().as_str(),
            test_att_nonce().as_str(),
            test_now_ts(),
        )
        .expect_err("mutated accept must be rejected");
        assert!(
            err.message.contains(needle),
            "expected error to contain {needle}, got {}",
            err.message
        );
    }

    fn test_invocation() -> GatewaySessionInvocation {
        let identity = test_identity();
        let session_id = "aa".repeat(32);
        let enclave_id = mayhem_proto::catalog_enclave_id(&identity);
        let contract = EnclaveContractRecord {
            enclave_id: enclave_id.clone(),
            admin_pubkey: identity.admin_pubkey.clone(),
            model_id: identity.model_id.clone(),
            model_class: DEFAULT_MODEL_CLASS.to_owned(),
            artifact_root: identity.artifact_root.clone(),
            manifest_hash: identity.manifest_hash.clone(),
            binary_hash: identity.binary_hash.clone(),
            att_tier: 1,
            caps: json!({}),
        };
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            enclave_id: enclave_id.clone(),
            price_ver: 7,
            max_spend_mu: 1000,
            checkpoint_every: CheckpointPolicy {
                tokens: 128,
                ms: 30_000,
            },
        };
        GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            user_pubkey: verifying_key_hex(&test_user_seed()),
            provider_pubkey: Some(verifying_key_hex(&test_provider_seed())),
            enclave_id,
            price_ver: 7,
            rules_ver: 3,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: "44".repeat(64),
            },
            attestation: Some(GatewaySessionAttestation {
                contract,
                trusted_binary_hashes: BTreeSet::from([identity.binary_hash]),
                trusted_apple_app_attest_jwks: None,
                trusted_nvidia_gb10_device_jwks: None,
                trusted_nvidia_nras_jwks: None,
                trusted_nvidia_offline_jwks: None,
            }),
            hedge: GatewayHedgeInvocation::default(),
            receipt_cosign_enabled: true,
            receipt_user_seed: test_user_seed(),
        }
    }

    fn test_model() -> GatewayModel {
        GatewayModel {
            id: test_identity().model_id,
            created: 1,
            owned_by: "mayhem".to_owned(),
            mayhem: MayhemModelInfo {
                model_class: DEFAULT_MODEL_CLASS.to_owned(),
                providers_online: 1,
                rooms: 1,
                price_ref_mu: PriceRefMu {
                    denom: "mu_usd".to_owned(),
                    ver: 7,
                    rate_map: text_generation_rate_map(20, 60),
                },
                attestation_tiers: BTreeMap::from([("T1".to_owned(), 1)]),
                attestation_tier_labels: attestation_tier_labels_for_counts(&BTreeMap::from([(
                    "T1".to_owned(),
                    1,
                )])),
                caps: ModelCaps {
                    tools: true,
                    json: true,
                    ctx: 8192,
                    vision: false,
                    image: false,
                    video: false,
                    audio: false,
                    output_modality: Some("text".to_owned()),
                    output_modalities: vec!["text".to_owned()],
                },
                adapter: ShapeAdapterInfo::default(),
                source: "test".to_owned(),
                kyb_identities: Vec::new(),
                route_candidates: Vec::new(),
            },
        }
    }

    #[test]
    fn canary_registry_preserves_seed_perceptual_hash_descriptor() {
        let root = json!({
            "models": [{
                "model_id": "admin/image-fixture",
                "canary": {
                    "set_id": "canary-dev-v1",
                    "match_min": 0.95,
                    "verification_method": "seed_perceptual_hash",
                    "verification_tolerance_bps": 500,
                    "perceptual_hashes": {
                        "diffusers-fp16": {
                            "fixed-image": "ffffffffffffffff"
                        }
                    }
                },
                "artifacts": {
                    "diffusers-fp16": {
                        "artifact_root": "ab".repeat(32)
                    }
                }
            }]
        });

        let registry = canary_registry_from_catalog_root(&root);
        let config = registry
            .models
            .get("admin/image-fixture")
            .expect("image canary config");
        assert_eq!(config.verification_method, "seed_perceptual_hash");
        assert_eq!(config.verification_tolerance_bps, Some(500));
        assert_eq!(config.match_min_bps, 9_500);
        assert!(config.fingerprints_by_artifact_root.is_empty());
        assert!(config.token_prefixes_by_artifact_root.is_empty());
        assert_eq!(
            config.perceptual_hashes_by_artifact_root
                ["abababababababababababababababababababababababababababababababab"]["fixed-image"],
            "ffffffffffffffff"
        );
    }

    fn test_chat_request(model_id: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: model_id.to_owned(),
            messages: vec![ChatMessage {
                role: "user".to_owned(),
                content: json!("hello mayhem"),
                name: None,
                extra: BTreeMap::new(),
            }],
            stream: false,
            stream_options: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            temperature: None,
            top_p: None,
            seed: None,
            stop: None,
            max_tokens: None,
        }
    }

    #[test]
    fn direct_session_request_body_preserves_multimodal_content_parts() {
        let mut request = test_chat_request("mayhem/vision");
        request.messages[0].content = json!([
            { "type": "text", "text": "what is shown?" },
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,aW1hZ2U=" } },
            { "type": "input_audio", "input_audio": { "data": "UklGRg==", "format": "wav" } }
        ]);

        let body = direct_session_request_body(&request);

        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,aW1hZ2U="
        );
        assert_eq!(
            body["messages"][0]["content"][2]["input_audio"]["data"],
            "UklGRg=="
        );
        assert!(chat_prompt_text(&request).contains("[image:"));
        assert!(chat_prompt_text(&request).contains("[audio:wav:"));
    }

    fn test_chat_output() -> ChatOutput {
        ChatOutput {
            content: Some("receipt ok".to_owned()),
            tool_call: None,
            artifacts: Vec::new(),
            finish_reason: "stop".to_owned(),
            usage: Usage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            },
        }
    }

    #[test]
    fn session_artifact_delta_collects_chunks_and_verifies_digest() {
        let bytes = b"\x89PNG mayhem image".to_vec();
        let digest = blake3_hex(&bytes);
        let mut builders = BTreeMap::new();

        collect_artifact_from_session_delta(
            &json!({
                "t": "s.delta",
                "rid": "rid-1",
                "artifact": {
                    "id": "image-1",
                    "content_type": "image/png",
                    "encoding": "hex",
                    "offset": 0,
                    "len": 4,
                    "total_len": bytes.len(),
                    "blake3": digest,
                    "data": hex::encode(&bytes[..4]),
                    "final": false,
                }
            }),
            &mut builders,
        )
        .unwrap();
        collect_artifact_from_session_delta(
            &json!({
                "t": "s.delta",
                "rid": "rid-1",
                "artifact": {
                    "id": "image-1",
                    "content_type": "image/png",
                    "encoding": "hex",
                    "offset": 4,
                    "len": bytes.len() - 4,
                    "total_len": bytes.len(),
                    "blake3": digest,
                    "data": hex::encode(&bytes[4..]),
                    "final": true,
                }
            }),
            &mut builders,
        )
        .unwrap();

        let artifacts = finish_session_artifacts(builders).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id, "image-1");
        assert_eq!(artifacts[0].content_type, "image/png");
        assert_eq!(artifacts[0].bytes, bytes);
        assert_eq!(artifacts[0].blake3, blake3_hex(&artifacts[0].bytes));
    }

    #[test]
    fn session_artifact_delta_rejects_corrupt_digest() {
        let bytes = b"not the claimed image".to_vec();
        let mut builders = BTreeMap::new();
        collect_artifact_from_session_delta(
            &json!({
                "t": "s.delta",
                "rid": "rid-1",
                "artifact": {
                    "id": "image-1",
                    "content_type": "image/png",
                    "encoding": "hex",
                    "offset": 0,
                    "len": bytes.len(),
                    "total_len": bytes.len(),
                    "blake3": "00".repeat(32),
                    "data": hex::encode(&bytes),
                    "final": true,
                }
            }),
            &mut builders,
        )
        .unwrap();

        let err = finish_session_artifacts(builders).expect_err("digest mismatch must reject");
        assert!(err.message.contains("blake3 mismatch"));
    }

    fn test_provider_receipt(
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        output: &ChatOutput,
        invocation: &GatewaySessionInvocation,
    ) -> ProviderSignedReceipt {
        let usage = ReceiptUsage::text(output.usage.prompt_tokens, output.usage.completion_tokens);
        let body = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: invocation.session_id.clone(),
            seq: 1,
            final_receipt: true,
            user: invocation.user_pubkey.clone(),
            provider: invocation.provider_pubkey.clone().unwrap(),
            enclave_id: invocation.enclave_id.clone(),
            model_id: model.id.clone(),
            price_ver: invocation.price_ver,
            rules_ver: invocation.rules_ver,
            usage: usage.clone(),
            mu_owed_cum: calculate_mu_owed(&model.mayhem.price_ref_mu, &usage),
            prompt_hash: blake3_hex(chat_prompt_text(request).as_bytes()),
            ts: 123,
        };
        ProviderSignedReceipt {
            enclave_sig: sign_hex(&test_enclave_seed(), &receipt_signing_bytes(&body).unwrap()),
            body,
            enclave_pubkey: verifying_key_hex(&test_enclave_seed()),
        }
    }

    fn test_identity() -> mayhem_proto::CatalogEnclaveIdentity {
        mayhem_proto::CatalogEnclaveIdentity {
            admin_pubkey: "33".repeat(32),
            model_id: "mayhem/test-model@q4".to_owned(),
            artifact_root: "aa".repeat(32),
            manifest_hash: "bb".repeat(32),
            binary_hash: "cc".repeat(32),
        }
    }

    fn test_accept_frame(invocation: &GatewaySessionInvocation) -> Value {
        let mut frame = json!({
            "t": "s.accept",
            "v": 1,
            "contract_version": invocation.contract_version,
            "session_id": invocation.session_id,
            "open_head": test_open_head(),
            "att_nonce": test_att_nonce(),
            "att_report": test_attestation_report(invocation),
            "engine": { "ctx": 8192 },
            "ts": 123,
            "nonce": "66".repeat(32),
        });
        sign_accept_frame(&mut frame);
        frame
    }

    fn test_attestation_report(invocation: &GatewaySessionInvocation) -> Value {
        test_attestation_report_with_nonce(invocation, test_att_nonce())
    }

    fn test_attestation_report_with_nonce(
        invocation: &GatewaySessionInvocation,
        nonce_u: String,
    ) -> Value {
        test_attestation_report_with_mutation(invocation, nonce_u, |_| {})
    }

    fn test_attestation_report_with_mutation<F>(
        invocation: &GatewaySessionInvocation,
        nonce_u: String,
        mutate: F,
    ) -> Value
    where
        F: FnOnce(&mut AttestationReport),
    {
        let contract = &invocation
            .attestation
            .as_ref()
            .expect("test invocation has attestation")
            .contract;
        let mut report = AttestationReport {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            alg: ATTESTATION_ALG.to_owned(),
            enclave_id: invocation.enclave_id.clone(),
            enclave_pubkey: verifying_key_hex(&test_enclave_seed()),
            provider_pubkey: invocation.provider_pubkey.clone().unwrap(),
            manifest_hash: contract.manifest_hash.clone(),
            binary_hash: contract.binary_hash.clone(),
            att_tier: contract.att_tier,
            hw_quote: None,
            boot_epoch: 1,
            report_ts: 2,
            nonce_u,
            runtime_config: mayhem_proto::AttestationRuntimeConfig::default(),
            sig_enclave: String::new(),
            sig_provider: String::new(),
        };
        mutate(&mut report);
        let body = report.body();
        report.sig_enclave = sign_hex(
            &test_enclave_seed(),
            &attestation_signing_bytes(&body, AttestationSigner::Enclave).unwrap(),
        );
        report.sig_provider = sign_hex(
            &test_provider_seed(),
            &attestation_signing_bytes(&body, AttestationSigner::Provider).unwrap(),
        );
        serde_json::to_value(report).unwrap()
    }

    fn sign_accept_frame(frame: &mut Value) {
        let sig = sign_hex(
            &test_provider_seed(),
            &session_accept_signing_bytes(frame).unwrap(),
        );
        frame["sig"] = json!(sig);
    }

    fn test_provider_seed() -> [u8; 32] {
        [7; 32]
    }

    fn test_user_seed() -> [u8; 32] {
        [41; 32]
    }

    fn test_enclave_seed() -> [u8; 32] {
        [8; 32]
    }

    fn test_open_head() -> String {
        "77".repeat(32)
    }

    fn test_att_nonce() -> String {
        "88".repeat(32)
    }

    fn test_now_ts() -> u64 {
        210
    }
}
