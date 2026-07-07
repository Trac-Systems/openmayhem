use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    fmt, fs,
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    audit::{
        aggregate_canary_fingerprints, evaluate_catalog_canary_token_prefix_probe,
        supported_canary_verification_method, token_fingerprint, CanaryProbeSpec,
        CANARY_VERIFICATION_CONTEXT_NEEDLE, CANARY_VERIFICATION_TOKEN_FINGERPRINT,
        DEFAULT_CANARY_MATCH_MIN_BPS, DEFAULT_CANARY_TEMPERATURE,
    },
    failover::{
        default_ttft_timeout_millis, midstream_stalled_after, x_mayhem_hedge_requested,
        FailoverPolicy, RedispatchMode, SessionFailoverState, SessionPriceMu,
        DEFAULT_MAX_OPEN_ATTEMPTS, DEFAULT_OPEN_TIMEOUT_MILLIS, DEFAULT_PROVIDER_COOLOFF_MILLIS,
        DEFAULT_STALL_TIMEOUT_MILLIS, DEFAULT_TTFT_BASE_TIMEOUT_MILLIS,
    },
    pricing::{normalize_rate_map, priced_usage_mu, text_generation_rate_map, RateMapEntry},
    provider_table::{
        ContractProviderSnapshot, LcgBalancerRng, ProviderCapacityMismatchEvent,
        ProviderObservationSample, ProviderTable, ProviderTableEntry, ProviderUnderdeliveryEvent,
        RequestRequirements, SelectionWeights, DEFAULT_AUDIO_REALTIME_FACTOR_FLOOR,
        DEFAULT_EMBEDDING_INPUT_TOKENS_FLOOR_PER_S, DEFAULT_IMAGE_FLOOR_IMAGES_PER_S,
        DEFAULT_LLM_GENERATION_FLOOR_TOK_S, DEFAULT_SATURATION_CUTOFF,
    },
    verify_tier1_attestation, AttestationVerificationRequest, EnclaveContractRecord,
    HeartbeatAttestation, HeartbeatCaps, HeartbeatPerf, HeartbeatQueue, HeartbeatSlots,
    ProviderHeartbeat, ProviderKey, ProviderProbation, ReputationEventKind,
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
use futures_util::{stream, Stream};
use mayhem_bridge::{BridgeError, ScBridgeClient, ScBridgeConfig};
use mayhem_proto::{
    chunk_json_payload, ctx_bracket_for_tokens_in_schedule, default_ctx_bracket_schedule,
    default_model_class, migrate_receipt_body, reassemble_json_payload, receipt_signing_bytes,
    session_accept_signing_bytes, session_frame_head, spend_voucher_signing_bytes,
    supported_receipt_signing_bytes, AttestationReport, CheckpointPolicy, CtxBracketSchedule,
    PayloadChunk, PayloadChunkManifest, ReceiptAck, ReceiptBody, ReceiptUsage, SessionReceipt,
    SpendVoucher, SpendVoucherBody, ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION, CONTRACT_VERSION,
    DEFAULT_MODEL_CLASS, DEFAULT_SESSION_MAX_FRAME_BYTES, DEFAULT_SESSION_PAYLOAD_CHUNK_BYTES,
    SESSION_RECEIPT_SCHEMA_VERSION, USAGE_AUDIO_SECOND, USAGE_CACHED_INPUT_TOKEN, USAGE_IMAGE,
    USAGE_INPUT_CHARACTER, USAGE_STEP,
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
const X_MAYHEM_MAX_PRICE_MU_HEADER: &str = "x-mayhem-max-price-mu";
const X_MAYHEM_MAX_WAIT_MS_HEADER: &str = "x-mayhem-max-wait-ms";
const X_MAYHEM_MIN_CTX_HEADER: &str = "x-mayhem-min-ctx";
const X_MAYHEM_QUANT_HEADER: &str = "x-mayhem-quant";
const X_MAYHEM_OPEN_TIMEOUT_MS_HEADER: &str = "x-mayhem-open-timeout-ms";
const X_MAYHEM_TTFT_TIMEOUT_MS_HEADER: &str = "x-mayhem-ttft-timeout-ms";
const X_MAYHEM_STALL_TIMEOUT_MS_HEADER: &str = "x-mayhem-stall-timeout-ms";
const X_MAYHEM_MIN_TOK_S_HEADER: &str = "x-mayhem-min-tok-s";
pub const DEFAULT_ROUTE_MAX_WAIT_MS: u64 = 10_000;
pub const MAX_ROUTE_MAX_WAIT_MS: u64 = 60_000;
const ROUTE_WAIT_POLL_MS: u64 = 1_000;
const ROUTE_REPUTATION_PRIORITY_BPS_DELTA: u32 = 500;
const DEFAULT_QUANT_BUCKET: &str = "unknown";
const DEFAULT_CANARY_SEED: i64 = 7;
const CONTEXT_NEEDLE_MIN_CTX: u32 = 32_768;
const CONTEXT_NEEDLE_MAX_TOKENS: u32 = 16;
const CONTEXT_NEEDLE_FILLER_WORDS_PER_LINE: usize = 32;
const DEFAULT_THROUGHPUT_FLOOR_SAMPLE_MILLIS: u64 = 1_000;
const DEFAULT_EPOCH_SECONDS: u64 = 3_600;
const DASHBOARD_SESSION_TTL_SECONDS: u64 = 15 * 60;
const DASHBOARD_COOKIE_NAME: &str = "mayhem_dashboard";
const DASHBOARD_CSP: &str = "default-src 'self'; connect-src 'self' http://127.0.0.1:*; img-src 'self' data:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'none'";
const DASHBOARD_CSS: &str = r#"
@font-face{font-family:Exo;src:url('/mayhem/dashboard/assets/exo-latin.woff2') format('woff2');font-style:normal;font-weight:400 700;font-display:swap}
:root{color-scheme:dark;--bg:rgb(11,11,12);--surface:rgb(22,22,26);--surface-card:rgb(24,24,27);--surface-raised:rgb(42,42,46);--border:rgb(42,42,46);--border-strong:rgb(41,41,41);--text-primary:rgb(229,231,235);--text-inverse:rgb(255,255,255);--text-muted:rgb(136,138,140);--accent-primary:rgb(197,68,89);--accent-primary-light:rgb(214,120,102);--accent-secondary:rgb(66,187,147);--radius-sm:6px;--radius-md:8px;--radius-pill:999px;--space-1:4px;--space-2:8px;--space-3:12px;--space-4:16px;--space-5:20px;--space-6:24px}
*{box-sizing:border-box;letter-spacing:0}body{margin:0;min-height:100vh;background:var(--bg);color:var(--text-primary);font-family:Exo,system-ui,sans-serif;font-size:15px;line-height:1.5}.nav{position:sticky;top:0;z-index:2;min-height:64px;display:grid;grid-template-columns:auto minmax(180px,500px) auto auto;gap:20px;align-items:center;padding:0 24px;background:rgba(22,22,26,.94);border-bottom:1px solid var(--border);backdrop-filter:blur(12px)}.brand,.wordmark{font-weight:700;color:var(--text-primary)}.brand{font-size:17px;text-decoration:none;white-space:nowrap}.wordmark{margin:0;font-size:64px;line-height:1}.wordmark.compact{font-size:22px}.hem,.wordmark .hem{background:linear-gradient(90deg,var(--accent-primary),var(--accent-primary-light));-webkit-background-clip:text;background-clip:text;color:transparent}.search{height:38px;border:1px solid var(--border);border-radius:var(--radius-pill);background:rgb(16,16,19);display:flex;align-items:center;padding:0 14px;color:var(--text-muted);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px;overflow:hidden;white-space:nowrap}.nav-links{display:flex;gap:18px}.nav-links a{color:var(--text-inverse);text-decoration:none;font-size:15px}.local-pill{justify-self:end;display:inline-flex;align-items:center;gap:7px;border-radius:var(--radius-pill);background:var(--accent-secondary);color:rgb(4,24,19);font-weight:700;font-size:12px;padding:7px 11px}.local-pill::before,.status-dot::before{content:"";width:8px;height:8px;border-radius:999px;background:currentColor}.dashboard{max-width:1280px;margin:0 auto;padding:48px 24px}.hero{text-align:center;margin:0 auto 34px;max-width:760px}.hero p{margin:12px auto 0;color:var(--text-muted);max-width:620px}.component-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:24px}.card{border:1px solid var(--border);border-radius:var(--radius-md);background:var(--surface-card);padding:20px;min-width:0}.card.strong{border:2px solid var(--border-strong)}.card-header{display:flex;align-items:center;justify-content:space-between;gap:14px;margin-bottom:18px}.card h2{margin:0;color:var(--text-inverse);font-size:22px;font-weight:600}.link{color:var(--accent-primary);text-decoration:none;font-weight:600}.detail-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:16px}.label{display:block;color:var(--text-muted);font-size:12px;text-transform:uppercase}.value{margin:4px 0 0;font-size:18px;font-weight:700}.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}.copy-row{display:flex;gap:8px;align-items:center;min-width:0}.copy-row .mono{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.copy-chip,.count-chip,.icon-toggle{border:1px solid var(--border);border-radius:var(--radius-sm);background:transparent;color:var(--text-primary);height:30px;display:inline-flex;align-items:center;justify-content:center}.copy-chip{padding:0 10px;font:inherit;font-size:13px;text-decoration:none}.count-chip{padding:0 10px;background:var(--surface-raised);font-family:ui-monospace,SFMono-Regular,Menlo,monospace}.status-dot{display:inline-flex;align-items:center;gap:8px;color:var(--accent-secondary);font-weight:600}.status-dot.muted{color:var(--text-muted)}.card-footer{display:flex;align-items:center;justify-content:space-between;gap:14px;margin:18px -20px -20px;padding:14px 20px;border-top:1px solid var(--border);color:var(--text-muted);font-size:13px}.chart-shell{height:220px;border-radius:var(--radius-md);background:linear-gradient(180deg,rgba(42,42,46,.35),rgba(24,24,27,.25));border:1px solid rgba(42,42,46,.7);position:relative;overflow:hidden}.chart-grid{position:absolute;inset:0;background:linear-gradient(to right,rgba(136,138,140,.08) 1px,transparent 1px),linear-gradient(to bottom,rgba(136,138,140,.08) 1px,transparent 1px);background-size:25% 25%}.chart-line{position:absolute;left:24px;right:24px;bottom:42px;height:88px;border-bottom:2px solid var(--accent-primary);transform:skewY(-8deg);box-shadow:0 26px 0 rgba(197,68,89,.1)}.chart-point{position:absolute;right:82px;top:70px;background:var(--accent-primary);color:var(--text-inverse);border-radius:var(--radius-sm);padding:5px 8px;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px}.toggle-row{display:flex;gap:8px;align-items:center}.icon-toggle{width:32px;background:var(--surface-raised)}.icon-toggle.active{border-color:var(--accent-primary);color:var(--accent-primary)}.empty-state{min-height:180px;display:grid;place-items:center;text-align:center;color:var(--text-muted)}.empty-icon{width:40px;height:40px;border-radius:var(--radius-md);border:1px solid var(--border);display:grid;place-items:center;margin:0 auto 12px;color:var(--accent-secondary)}.empty-icon::before{content:"";width:16px;height:16px;border-radius:50%;border:2px solid currentColor}.footer{border-top:1px solid var(--border);color:var(--text-muted);display:flex;justify-content:space-between;gap:16px;padding:18px 24px;font-size:13px}@media(max-width:900px){.nav{grid-template-columns:auto 1fr auto}.search{display:none}.nav-links{justify-content:flex-end}.component-grid,.detail-grid{grid-template-columns:1fr}.wordmark{font-size:48px}}@media(max-width:640px){.nav{padding:0 16px;gap:12px}.nav-links{gap:12px}.dashboard{padding:32px 16px}.wordmark{font-size:40px}.card-header,.card-footer,.footer{align-items:flex-start;flex-direction:column}}
"#;
const DASHBOARD_USER_CSS: &str = r#"
.overview-grid{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:16px;margin-bottom:24px}.overview-grid.provider{grid-template-columns:repeat(4,minmax(0,1fr))}.metric-card .value{font-size:24px}.wide-grid{display:grid;grid-template-columns:minmax(0,1.25fr) minmax(360px,.75fr);gap:24px}.wide-grid.provider,.wide-grid.network{grid-template-columns:minmax(0,1fr) minmax(0,1fr)}.wide-grid.network .card{overflow:hidden}.table{width:100%;border-collapse:collapse}.table th,.table td{border-bottom:1px solid var(--border);padding:11px 8px;text-align:left;vertical-align:middle}.table th{color:var(--text-muted);font-size:12px;text-transform:uppercase}.table td:last-child,.table th:last-child{text-align:right}.model-list{display:grid;gap:12px}.model-row{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:12px;align-items:center;border:1px solid var(--border);border-radius:var(--radius-md);padding:14px}.model-title{font-weight:700;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.model-meta{margin-top:5px;color:var(--text-muted);font-size:13px}.segmented{display:flex;gap:8px;flex-wrap:wrap}.segment,.badge{border:1px solid var(--border);border-radius:var(--radius-sm);height:30px;padding:0 10px;display:inline-flex;align-items:center;color:var(--text-muted);white-space:nowrap}.segment.active,.badge.good{border-color:var(--accent-primary);color:var(--accent-primary)}.badge.live{border-color:var(--accent-secondary);color:var(--accent-secondary)}.badge-row{display:flex;gap:8px;flex-wrap:wrap}.toggle{display:inline-flex;align-items:center;gap:8px;color:var(--text-muted)}.toggle::before{content:"";width:28px;height:16px;border-radius:999px;border:1px solid var(--border);background:var(--surface-raised)}.spend-bars{height:180px;display:flex;align-items:end;gap:10px;padding:18px 12px 8px;border:1px solid var(--border);border-radius:var(--radius-md);background:linear-gradient(180deg,rgba(42,42,46,.24),rgba(24,24,27,.12))}.bar{flex:1;min-width:10px;border-radius:var(--radius-sm) var(--radius-sm) 0 0;background:linear-gradient(180deg,var(--accent-primary-light),var(--accent-primary));height:var(--h)}.mini-bar{height:8px;border-radius:999px;background:var(--surface-raised);overflow:hidden}.mini-bar span{display:block;height:100%;width:var(--w);background:linear-gradient(90deg,var(--accent-secondary),var(--accent-primary-light))}.load-cell{min-width:150px}.load-cell .mini-bar{margin-top:6px}.load-cell .privacy-note{display:block;margin-top:4px}.opencode-card pre{margin:0;white-space:pre-wrap;overflow-wrap:anywhere;color:var(--text-primary);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px}.gateway-row{display:grid;grid-template-columns:1fr auto;gap:10px;align-items:center}.provider-scope{max-width:760px;margin:0 auto 20px;text-align:center}.privacy-note{color:var(--text-muted);font-size:13px}.claim-card pre{margin:0;white-space:pre-wrap;overflow-wrap:anywhere;color:var(--text-primary);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px}@media(max-width:1050px){.overview-grid,.overview-grid.provider,.wide-grid,.wide-grid.provider,.wide-grid.network{grid-template-columns:1fr}.table{font-size:14px}}@media(max-width:640px){.table th:nth-child(3),.table td:nth-child(3){display:none}.overview-grid{gap:12px}}
"#;

#[derive(Clone, Debug)]
pub struct GatewayState {
    models: Arc<Vec<GatewayModel>>,
    receipts: Arc<Mutex<Vec<StoredReceipt>>>,
    probes: Arc<Mutex<Vec<StoredProbeEvent>>>,
    reputation_events: Arc<Mutex<Vec<StoredReputationEvent>>>,
    paused_sessions: Arc<Mutex<Vec<PausedSession>>>,
    receipt_config: ReceiptConfig,
    session_backend: Arc<dyn GatewaySessionBackend>,
    hardware_quote_trust: Arc<HardwareQuoteTrust>,
    canaries: Arc<GatewayCanaryRegistry>,
    canary_policy: GatewayCanaryProbePolicy,
    canary_scheduler: Arc<Mutex<GatewayCanaryScheduler>>,
    dashboard_session: Arc<DashboardSession>,
    provider_earnings: Arc<Vec<Value>>,
    provider_load_progress_dir: Arc<Option<PathBuf>>,
    provider_table: Arc<Mutex<ProviderTable>>,
    provider_cooloffs: Arc<Mutex<BTreeMap<ProviderKey, u64>>>,
    chat_affinity: Arc<Mutex<BTreeMap<ChatAffinityKey, ProviderKey>>>,
    access_control: Arc<GatewayAccessControl>,
    epoch_seconds: u64,
    ctx_bracket_schedule: Arc<CtxBracketSchedule>,
    failover_policy: GatewayFailoverPolicyConfig,
    default_max_price_mu: Option<u64>,
    default_max_wait_ms: u64,
    default_min_ctx: Option<u32>,
    dev_session_shim: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ChatAffinityKey {
    model_id: String,
    conversation_id: String,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quant_buckets: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_app_version: Option<String>,
    pub caps: ModelCaps,
    #[serde(default)]
    pub adapter: ShapeAdapterInfo,
    #[serde(default, skip_serializing_if = "GatewayFailoverPolicyConfig::is_empty")]
    pub failover: GatewayFailoverPolicyConfig,
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
    #[serde(default)]
    pub accepted_rails: Vec<String>,
    pub enclave_id: String,
    pub room_id: String,
    pub price_ver: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_ref_mu: Option<PriceRefMu>,
    #[serde(default)]
    pub min_ask_mu: u64,
    pub att_tier: u8,
    #[serde(default = "default_quant_bucket")]
    pub quant: String,
    pub admin_pubkey: String,
    pub artifact_root: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifact_sidecar_roots: BTreeMap<String, String>,
    pub manifest_hash: String,
    pub binary_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kyb: Option<ProviderKybInfo>,
    #[serde(default = "default_reputation_bps")]
    pub reputation_bps: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probation: Option<ProviderProbation>,
    #[serde(default)]
    pub caps: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_run: Option<GatewayLocalRunBadge>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GatewayLocalRunBadge {
    pub marker: String,
    pub status: String,
    pub label: String,
    pub reason: String,
    pub requested_ctx: u64,
    pub served_ctx: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_tok_s: Option<String>,
    pub memory_required_human: String,
    pub memory_budget_human: String,
    pub download_human: String,
    pub eta: String,
}

fn default_reputation_bps() -> u32 {
    10_000
}

fn default_quant_bucket() -> String {
    DEFAULT_QUANT_BUCKET.to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PriceRefMu {
    pub denom: String,
    pub ver: u64,
    pub rate_map: Vec<RateMapEntry>,
    pub per_req_mu: u64,
    pub min_session_mu: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derivation: Option<Value>,
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
    pub max_image_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_image_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_image_steps: Option<u64>,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GatewayFailoverPolicyConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_tok_s: Option<f64>,
}

impl GatewayFailoverPolicyConfig {
    pub fn is_empty(&self) -> bool {
        self.open_timeout_ms.is_none()
            && self.ttft_timeout_ms.is_none()
            && self.stall_timeout_ms.is_none()
            && self.min_tok_s.is_none()
    }

    fn sanitized(self) -> Self {
        Self {
            open_timeout_ms: self.open_timeout_ms.filter(|value| *value > 0),
            ttft_timeout_ms: self.ttft_timeout_ms.filter(|value| *value > 0),
            stall_timeout_ms: self.stall_timeout_ms.filter(|value| *value > 0),
            min_tok_s: self
                .min_tok_s
                .filter(|value| value.is_finite() && *value > 0.0),
        }
    }

    fn merged_with(self, override_config: Self) -> Self {
        let base = self.sanitized();
        let override_config = override_config.sanitized();
        Self {
            open_timeout_ms: override_config.open_timeout_ms.or(base.open_timeout_ms),
            ttft_timeout_ms: override_config.ttft_timeout_ms.or(base.ttft_timeout_ms),
            stall_timeout_ms: override_config.stall_timeout_ms.or(base.stall_timeout_ms),
            min_tok_s: override_config.min_tok_s.or(base.min_tok_s),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GatewayFailoverInvocation {
    pub open_timeout_ms: u64,
    pub ttft_timeout_ms: u64,
    pub stall_timeout_ms: u64,
    pub min_tok_s: Option<f64>,
}

impl Default for GatewayFailoverInvocation {
    fn default() -> Self {
        Self {
            open_timeout_ms: DEFAULT_OPEN_TIMEOUT_MILLIS,
            ttft_timeout_ms: DEFAULT_TTFT_BASE_TIMEOUT_MILLIS,
            stall_timeout_ms: DEFAULT_STALL_TIMEOUT_MILLIS,
            min_tok_s: None,
        }
    }
}

impl GatewayFailoverInvocation {
    fn from_config_for_prompt(config: GatewayFailoverPolicyConfig, prompt_tokens: u64) -> Self {
        let config = config.sanitized();
        Self {
            open_timeout_ms: config
                .open_timeout_ms
                .unwrap_or(DEFAULT_OPEN_TIMEOUT_MILLIS),
            ttft_timeout_ms: config
                .ttft_timeout_ms
                .unwrap_or_else(|| default_ttft_timeout_millis(prompt_tokens)),
            stall_timeout_ms: config
                .stall_timeout_ms
                .unwrap_or(DEFAULT_STALL_TIMEOUT_MILLIS),
            min_tok_s: config.min_tok_s,
        }
    }

    fn open_timeout(self) -> Duration {
        Duration::from_millis(self.open_timeout_ms)
    }

    fn ttft_timeout(self) -> Duration {
        Duration::from_millis(self.ttft_timeout_ms)
    }

    fn stall_timeout(self) -> Duration {
        Duration::from_millis(self.stall_timeout_ms)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredReceipt {
    pub rail: String,
    pub voucher: SpendVoucher,
    pub receipt: SessionReceipt,
    pub receipt_ack: ReceiptAck,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<GatewayTokenAttribution>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayTokenBudgetPeriod {
    Total,
    Day,
    Month,
}

impl GatewayTokenBudgetPeriod {
    fn window_seconds(self) -> Option<u64> {
        match self {
            Self::Total => None,
            Self::Day => Some(24 * 60 * 60),
            Self::Month => Some(30 * 24 * 60 * 60),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GatewayTokenRecord {
    pub name: String,
    pub token_hash: String,
    pub token_id: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_mu: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_period: Option<GatewayTokenBudgetPeriod>,
    #[serde(default)]
    pub spent_total_mu: u64,
    #[serde(default)]
    pub spent_period_mu: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rate_per_minute: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
}

impl GatewayTokenRecord {
    pub fn is_active(&self, now: u64) -> bool {
        self.revoked_at.is_none()
            && self
                .expires_at
                .map(|expires_at| expires_at > now)
                .unwrap_or(true)
    }

    fn reset_budget_window_if_needed(&mut self, now: u64) {
        let Some(period) = self.budget_period else {
            return;
        };
        let Some(window_seconds) = period.window_seconds() else {
            self.period_started_at.get_or_insert(self.created_at);
            return;
        };
        let started_at = self.period_started_at.unwrap_or(self.created_at);
        if now.saturating_sub(started_at) >= window_seconds {
            self.spent_period_mu = 0;
            self.period_started_at = Some(now);
        } else {
            self.period_started_at = Some(started_at);
        }
    }

    fn effective_spent_mu(&self) -> u64 {
        match self
            .budget_period
            .unwrap_or(GatewayTokenBudgetPeriod::Total)
        {
            GatewayTokenBudgetPeriod::Total => self.spent_total_mu,
            GatewayTokenBudgetPeriod::Day | GatewayTokenBudgetPeriod::Month => self.spent_period_mu,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GatewayTokenStore {
    #[serde(default = "default_gateway_token_store_version")]
    pub version: u32,
    #[serde(default)]
    pub tokens: Vec<GatewayTokenRecord>,
}

impl Default for GatewayTokenStore {
    fn default() -> Self {
        Self::empty()
    }
}

impl GatewayTokenStore {
    pub fn empty() -> Self {
        Self {
            version: default_gateway_token_store_version(),
            tokens: Vec::new(),
        }
    }

    pub fn normalized(mut self) -> Self {
        if self.version == 0 {
            self.version = default_gateway_token_store_version();
        }
        self
    }

    pub fn active_token_count(&self, now: u64) -> usize {
        self.tokens
            .iter()
            .filter(|token| token.is_active(now))
            .count()
    }
}

fn default_gateway_token_store_version() -> u32 {
    1
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GatewayTokenAttribution {
    pub name: String,
    pub token_id: String,
}

#[derive(Clone, Debug, Default)]
struct GatewayTokenRateWindow {
    started_at: u64,
    count: u32,
}

#[derive(Clone, Debug)]
pub struct GatewayAccessControl {
    require_auth: bool,
    store_path: Option<PathBuf>,
    store: Arc<Mutex<GatewayTokenStore>>,
    rate_windows: Arc<Mutex<BTreeMap<String, GatewayTokenRateWindow>>>,
}

impl GatewayAccessControl {
    pub fn disabled() -> Self {
        Self::new(false, GatewayTokenStore::empty(), None)
    }

    pub fn new(require_auth: bool, store: GatewayTokenStore, store_path: Option<PathBuf>) -> Self {
        Self {
            require_auth,
            store_path,
            store: Arc::new(Mutex::new(store.normalized())),
            rate_windows: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn requires_auth(&self) -> bool {
        self.require_auth
    }

    pub fn has_active_tokens(&self, now: u64) -> bool {
        self.store
            .lock()
            .expect("gateway token store poisoned")
            .active_token_count(now)
            > 0
    }

    pub fn token_count(&self) -> usize {
        self.store
            .lock()
            .expect("gateway token store poisoned")
            .tokens
            .len()
    }

    fn authorize(
        &self,
        headers: &HeaderMap,
        model: Option<&str>,
    ) -> Result<Option<GatewayTokenAttribution>, ApiError> {
        let Some(raw_token) = gateway_bearer_token(headers)? else {
            if self.require_auth {
                return Err(ApiError::unauthorized(
                    "missing bearer token",
                    Some("Authorization"),
                ));
            }
            return Ok(None);
        };
        let token_hash = gateway_token_hash(&raw_token);
        let now = now_secs();
        let mut store = self.store.lock().expect("gateway token store poisoned");
        let token = store
            .tokens
            .iter_mut()
            .find(|token| token.token_hash == token_hash)
            .ok_or_else(|| ApiError::unauthorized("invalid bearer token", Some("Authorization")))?;
        if token.revoked_at.is_some() {
            return Err(ApiError::unauthorized(
                "revoked bearer token",
                Some("Authorization"),
            ));
        }
        if token.expires_at.is_some_and(|expires_at| expires_at <= now) {
            return Err(ApiError::unauthorized(
                "expired bearer token",
                Some("Authorization"),
            ));
        }
        if let Some(model) = model {
            if !token.models.is_empty() && !token.models.iter().any(|allowed| allowed == model) {
                return Err(ApiError::forbidden(
                    "bearer token is not allowed to use this model",
                    Some("model"),
                ));
            }
        }
        token.reset_budget_window_if_needed(now);
        if token
            .budget_mu
            .is_some_and(|budget_mu| token.effective_spent_mu() >= budget_mu)
        {
            return Err(ApiError::payment_required(
                "bearer token budget cap reached",
                Some("Authorization"),
            ));
        }
        self.check_rate_limit(token, now)?;
        token.last_used_at = Some(now);
        let attribution = GatewayTokenAttribution {
            name: token.name.clone(),
            token_id: token.token_id.clone(),
        };
        self.persist_store(&store)?;
        Ok(Some(attribution))
    }

    fn ensure_budget_allows(
        &self,
        attribution: &Option<GatewayTokenAttribution>,
        max_spend_mu: u64,
    ) -> Result<(), ApiError> {
        let Some(attribution) = attribution else {
            return Ok(());
        };
        let now = now_secs();
        let mut store = self.store.lock().expect("gateway token store poisoned");
        let token = store
            .tokens
            .iter_mut()
            .find(|token| token.name == attribution.name && token.token_id == attribution.token_id)
            .ok_or_else(|| ApiError::unauthorized("invalid bearer token", Some("Authorization")))?;
        token.reset_budget_window_if_needed(now);
        if token.budget_mu.is_some_and(|budget_mu| {
            token.effective_spent_mu().saturating_add(max_spend_mu) > budget_mu
        }) {
            return Err(ApiError::payment_required(
                "bearer token budget cap reached",
                Some("Authorization"),
            ));
        }
        self.persist_store(&store)
    }

    fn record_spend(
        &self,
        attribution: &GatewayTokenAttribution,
        spend_mu_delta: u64,
    ) -> Result<(), ApiError> {
        if spend_mu_delta == 0 {
            return Ok(());
        }
        let now = now_secs();
        let mut store = self.store.lock().expect("gateway token store poisoned");
        let Some(token) = store
            .tokens
            .iter_mut()
            .find(|token| token.name == attribution.name && token.token_id == attribution.token_id)
        else {
            return Ok(());
        };
        token.reset_budget_window_if_needed(now);
        token.spent_total_mu = token.spent_total_mu.saturating_add(spend_mu_delta);
        token.spent_period_mu = token.spent_period_mu.saturating_add(spend_mu_delta);
        self.persist_store(&store)
    }

    fn summary(&self) -> Value {
        let now = now_secs();
        let store = self.store.lock().expect("gateway token store poisoned");
        let tokens = store
            .tokens
            .iter()
            .map(|token| {
                let active = token.is_active(now);
                json!({
                    "name": token.name,
                    "token_id": token.token_id,
                    "active": active,
                    "expires_at": token.expires_at,
                    "budget_mu": token.budget_mu,
                    "budget_period": token.budget_period,
                    "spent_total_mu": token.spent_total_mu,
                    "spent_period_mu": token.spent_period_mu,
                    "spent_total_usd": format_mu_usd(token.spent_total_mu),
                    "last_used_at": token.last_used_at,
                    "revoked_at": token.revoked_at,
                    "max_rate_per_minute": token.max_rate_per_minute,
                    "models": token.models,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "require_auth": self.require_auth,
            "token_count": store.tokens.len(),
            "active_token_count": store.active_token_count(now),
            "tokens": tokens,
        })
    }

    fn check_rate_limit(&self, token: &GatewayTokenRecord, now: u64) -> Result<(), ApiError> {
        let Some(limit) = token.max_rate_per_minute else {
            return Ok(());
        };
        let mut windows = self
            .rate_windows
            .lock()
            .expect("gateway token rate windows poisoned");
        let window =
            windows
                .entry(token.token_id.clone())
                .or_insert_with(|| GatewayTokenRateWindow {
                    started_at: now,
                    count: 0,
                });
        if now.saturating_sub(window.started_at) >= 60 {
            window.started_at = now;
            window.count = 0;
        }
        if window.count >= limit {
            return Err(ApiError::too_many_requests(
                "bearer token rate limit reached",
                Some("Authorization"),
            ));
        }
        window.count = window.count.saturating_add(1);
        Ok(())
    }

    fn persist_store(&self, store: &GatewayTokenStore) -> Result<(), ApiError> {
        let Some(path) = self.store_path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ApiError::internal_message(format!(
                    "creating gateway token store directory failed: {err}"
                ))
            })?;
        }
        let bytes = serde_json::to_vec_pretty(store).map_err(ApiError::internal)?;
        fs::write(path, bytes).map_err(|err| {
            ApiError::internal_message(format!("writing gateway token store failed: {err}"))
        })
    }
}

pub fn gateway_token_hash(token: &str) -> String {
    blake3_hex(format!("mayhem-gateway-token-v1\0{}", token.trim()).as_bytes())
}

fn gateway_bearer_token(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::unauthorized("invalid Authorization header", Some("Authorization"))
    })?;
    let mut parts = value.split_whitespace();
    let Some(scheme) = parts.next() else {
        return Ok(None);
    };
    let Some(token) = parts.next() else {
        return Err(ApiError::unauthorized(
            "invalid Authorization header",
            Some("Authorization"),
        ));
    };
    if !scheme.eq_ignore_ascii_case("Bearer") || parts.next().is_some() {
        return Err(ApiError::unauthorized(
            "invalid Authorization header",
            Some("Authorization"),
        ));
    }
    Ok(Some(token.to_owned()))
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

#[derive(Clone, Debug, Serialize)]
pub struct StoredReputationEvent {
    pub provider: String,
    pub event_id: String,
    pub kind: String,
    pub epoch: u64,
    pub at: u64,
    pub evidence_hash: String,
    pub evidence: Value,
    pub command: Value,
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
    rail: String,
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
    pub user: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
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

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
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
    pub cfg_scale: Option<f32>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug)]
pub struct AudioTranscriptionRequest {
    pub model: String,
    pub audio: Vec<u8>,
    pub content_type: Option<String>,
    pub filename: Option<String>,
    pub response_format: Option<String>,
    pub language: Option<String>,
    pub prompt: Option<String>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
    param: Option<&'static str>,
}

pub type GatewaySessionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GatewaySessionResult, GatewaySessionError>> + Send + 'a>>;

pub type GatewayEmbeddingFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GatewayEmbeddingResult, GatewaySessionError>> + Send + 'a>>;

pub type GatewayImageGenerationFuture<'a> = Pin<
    Box<dyn Future<Output = Result<GatewayImageGenerationResult, GatewaySessionError>> + Send + 'a>,
>;

pub type GatewayAudioSpeechFuture<'a> = Pin<
    Box<dyn Future<Output = Result<GatewayAudioSpeechResult, GatewaySessionError>> + Send + 'a>,
>;

pub type GatewayAudioTranscriptionFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<GatewayAudioTranscriptionResult, GatewaySessionError>>
            + Send
            + 'a,
    >,
>;

pub type GatewayHedgeProbeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GatewayHedgeProbeResult, GatewaySessionError>> + Send + 'a>>;

pub trait GatewaySessionBackend: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    fn bridge_stream_config(&self) -> Option<ScBridgeGatewaySessionConfig> {
        None
    }
    fn hedge_probe<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayHedgeProbeFuture<'a> {
        Box::pin(async move {
            Ok(GatewayHedgeProbeResult {
                provider: invocation.provider_pubkey.clone().unwrap_or_default(),
                ttft_ms: 0,
            })
        })
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a>;

    fn run_embedding<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a EmbeddingRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayEmbeddingFuture<'a> {
        Box::pin(async move {
            Err(GatewaySessionError::new(format!(
                "{} backend does not support embeddings",
                self.name()
            )))
        })
    }

    fn run_image_generation<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ImageGenerationRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayImageGenerationFuture<'a> {
        Box::pin(async move {
            Err(GatewaySessionError::new(format!(
                "{} backend does not support image generation",
                self.name()
            )))
        })
    }

    fn run_audio_speech<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a AudioSpeechRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayAudioSpeechFuture<'a> {
        Box::pin(async move {
            Err(GatewaySessionError::new(format!(
                "{} backend does not support audio speech",
                self.name()
            )))
        })
    }

    fn run_audio_transcription<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a AudioTranscriptionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayAudioTranscriptionFuture<'a> {
        Box::pin(async move {
            Err(GatewaySessionError::new(format!(
                "{} backend does not support audio transcription",
                self.name()
            )))
        })
    }
}

#[derive(Clone, Debug)]
pub struct GatewaySessionResult {
    pub output: ChatOutput,
    pub backend: String,
    pub direct_session: bool,
    pub provider_receipt: Option<ProviderSignedReceipt>,
    pub token_ids: Vec<i32>,
    pub quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
pub struct GatewayEmbeddingResult {
    pub output: EmbeddingOutput,
    pub backend: String,
    pub direct_session: bool,
    pub provider_receipt: Option<ProviderSignedReceipt>,
    pub quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
pub struct GatewayImageGenerationResult {
    pub output: ImageGenerationOutput,
    pub backend: String,
    pub direct_session: bool,
    pub provider_receipt: Option<ProviderSignedReceipt>,
    pub quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
pub struct GatewayAudioSpeechResult {
    pub output: AudioSpeechOutput,
    pub backend: String,
    pub direct_session: bool,
    pub provider_receipt: Option<ProviderSignedReceipt>,
    pub quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
pub struct GatewayAudioTranscriptionResult {
    pub output: AudioTranscriptionOutput,
    pub backend: String,
    pub direct_session: bool,
    pub provider_receipt: Option<ProviderSignedReceipt>,
    pub quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GatewaySessionQuality {
    pub ttft_ms: u64,
    pub tok_s: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct GatewaySessionPartial {
    pub output: ChatOutput,
    pub provider_receipt: ProviderSignedReceipt,
    pub token_ids: Vec<i32>,
    pub quality: Option<GatewaySessionQuality>,
    pub reason: String,
    pub redispatch_mode: RedispatchMode,
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
    pub rail: String,
    pub user_pubkey: String,
    pub provider_pubkey: Option<String>,
    pub enclave_id: String,
    pub price_ver: u64,
    pub opened_at: u64,
    pub served_ctx: u32,
    pub ctx_bracket: String,
    pub ctx_bracket_table_ver: u32,
    pub rules_ver: u64,
    pub spend_voucher: SpendVoucher,
    pub attestation: Option<GatewaySessionAttestation>,
    pub hedge: GatewayHedgeInvocation,
    pub failover: GatewayFailoverInvocation,
    pub access_token: Option<GatewayTokenAttribution>,
    receipt_cosign_enabled: bool,
    receipt_user_seed: [u8; 32],
}

impl GatewaySessionInvocation {
    fn with_hedge_probe_outcome(mut self, outcome: &GatewayHedgeProbeOutcome) -> Self {
        self.hedge.actual_probe_count = outcome.actual_probe_count;
        if let Some(winner) = outcome.winner.as_ref() {
            self.hedge.winner_provider = Some(winner.provider.clone());
            self.hedge.winner_ttft_ms = Some(winner.ttft_ms);
        }
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GatewayHedgeInvocation {
    pub requested: bool,
    pub planned_probe_count: usize,
    pub actual_probe_count: usize,
    pub winner_provider: Option<String>,
    pub winner_ttft_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayHedgeProbeResult {
    pub provider: String,
    pub ttft_ms: u64,
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
    pub clean_refusal: bool,
    pub clean_refusal_code: Option<String>,
    pub partial: Option<Box<GatewaySessionPartial>>,
}

#[derive(Debug)]
struct NoProviderSessionBackend;

#[derive(Debug)]
struct LocalOpenAiShapeBackend;

#[derive(Clone, Debug)]
pub struct ScBridgeGatewaySessionConfig {
    pub url: String,
    pub token: String,
    pub open_timeout: Duration,
    pub ttft_timeout: Duration,
    pub frame_timeout: Duration,
    pub min_tok_s: Option<f64>,
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

#[derive(Clone, Debug)]
pub struct EmbeddingOutput {
    pub embeddings: Vec<Vec<f32>>,
    pub usage: Usage,
}

#[derive(Clone, Debug)]
pub struct ImageGenerationOutput {
    pub artifacts: Vec<GatewayArtifactOutput>,
    pub usage: ReceiptUsage,
}

#[derive(Clone, Debug)]
pub struct AudioSpeechOutput {
    pub artifacts: Vec<GatewayArtifactOutput>,
    pub usage: ReceiptUsage,
}

#[derive(Clone, Debug)]
pub struct AudioTranscriptionOutput {
    pub text: String,
    pub usage: ReceiptUsage,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

    pub fn canary_registry_from_catalog_json(
        catalog: &str,
    ) -> Result<GatewayCanaryRegistry, serde_json::Error> {
        let root: Value = serde_json::from_str(catalog)?;
        Ok(canary_registry_from_catalog_root(&root))
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
                    per_req_mu: 0,
                    min_session_mu: 0,
                    derivation: None,
                },
                attestation_tiers: tiers,
                attestation_tier_labels: attestation_tier_labels_for_counts(&BTreeMap::from([(
                    "T1".to_owned(),
                    1,
                )])),
                quant_buckets: BTreeMap::from([(DEFAULT_QUANT_BUCKET.to_owned(), 1)]),
                min_app_version: None,
                caps: ModelCaps {
                    tools: true,
                    json: true,
                    ctx: 8192,
                    vision: false,
                    image: false,
                    video: false,
                    audio: false,
                    max_image_width: None,
                    max_image_height: None,
                    max_image_steps: None,
                    output_modality: Some("text".to_owned()),
                    output_modalities: vec!["text".to_owned()],
                },
                adapter: ShapeAdapterInfo::default(),
                failover: GatewayFailoverPolicyConfig::default(),
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
        let receipt_config = ReceiptConfig::default();
        let provider_table = provider_table_from_models(&models, receipt_config.rules_ver);
        Self {
            models: Arc::new(models),
            receipts: Arc::new(Mutex::new(Vec::new())),
            probes: Arc::new(Mutex::new(Vec::new())),
            reputation_events: Arc::new(Mutex::new(Vec::new())),
            paused_sessions: Arc::new(Mutex::new(Vec::new())),
            receipt_config,
            session_backend: Arc::new(NoProviderSessionBackend),
            hardware_quote_trust: Arc::new(HardwareQuoteTrust::default()),
            canaries: Arc::new(canaries),
            canary_policy: GatewayCanaryProbePolicy::default(),
            canary_scheduler: Arc::new(Mutex::new(GatewayCanaryScheduler::default())),
            dashboard_session: Arc::new(DashboardSession::new()),
            provider_earnings: Arc::new(Vec::new()),
            provider_load_progress_dir: Arc::new(None),
            provider_table: Arc::new(Mutex::new(provider_table)),
            provider_cooloffs: Arc::new(Mutex::new(BTreeMap::new())),
            chat_affinity: Arc::new(Mutex::new(BTreeMap::new())),
            access_control: Arc::new(GatewayAccessControl::disabled()),
            epoch_seconds: DEFAULT_EPOCH_SECONDS,
            ctx_bracket_schedule: Arc::new(default_ctx_bracket_schedule()),
            failover_policy: GatewayFailoverPolicyConfig::default(),
            default_max_price_mu: None,
            default_max_wait_ms: DEFAULT_ROUTE_MAX_WAIT_MS,
            default_min_ctx: None,
            dev_session_shim: false,
        }
    }

    pub fn with_receipt_cosign_enabled(mut self, enabled: bool) -> Self {
        self.receipt_config.cosign_enabled = enabled;
        self
    }

    pub fn with_receipt_rail(mut self, rail: impl Into<String>) -> Self {
        self.receipt_config.rail = rail.into();
        self
    }

    pub fn with_receipt_user_seed(mut self, seed: [u8; 32]) -> Self {
        self.receipt_config.user_seed = seed;
        self
    }

    pub fn with_receipt_balance_mu(mut self, balance_mu: u64) -> Self {
        self.receipt_config.balance_mu = balance_mu;
        self
    }

    pub fn with_receipt_checkpoint_every(mut self, checkpoint_every: CheckpointPolicy) -> Self {
        self.receipt_config.checkpoint_every = CheckpointPolicy {
            tokens: checkpoint_every.tokens.max(1),
            ms: checkpoint_every.ms,
        };
        self
    }

    pub fn with_session_backend(mut self, backend: Arc<dyn GatewaySessionBackend>) -> Self {
        self.session_backend = backend;
        self
    }

    pub fn with_dev_session_shim(mut self) -> Self {
        self.session_backend = Arc::new(LocalOpenAiShapeBackend);
        self.dev_session_shim = true;
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

    pub fn with_provider_load_progress_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.provider_load_progress_dir = Arc::new(Some(dir.into()));
        self
    }

    pub fn with_provider_heartbeats(self, heartbeats: Vec<ProviderHeartbeat>) -> Self {
        let now = now_millis_u64();
        {
            let mut table = self.provider_table.lock().expect("provider table poisoned");
            for heartbeat in heartbeats {
                table.upsert_heartbeat(heartbeat, now);
            }
        }
        self
    }

    pub fn with_access_control(mut self, access_control: GatewayAccessControl) -> Self {
        self.access_control = Arc::new(access_control);
        self
    }

    pub fn with_failover_policy(mut self, policy: GatewayFailoverPolicyConfig) -> Self {
        self.failover_policy = policy.sanitized();
        self
    }

    pub fn with_epoch_seconds(mut self, epoch_seconds: u64) -> Self {
        self.epoch_seconds = epoch_seconds.max(1);
        self
    }

    pub fn epoch_seconds(&self) -> u64 {
        self.epoch_seconds
    }

    pub fn with_ctx_bracket_schedule(mut self, schedule: CtxBracketSchedule) -> Self {
        self.ctx_bracket_schedule = Arc::new(schedule);
        self
    }

    pub fn ctx_bracket_table_version(&self) -> u32 {
        self.ctx_bracket_schedule.current.ver
    }

    pub fn with_default_max_price_mu(mut self, max_price_mu: Option<u64>) -> Self {
        self.default_max_price_mu = max_price_mu.filter(|value| *value > 0);
        self
    }

    pub fn with_default_max_wait_ms(mut self, max_wait_ms: Option<u64>) -> Self {
        self.default_max_wait_ms = max_wait_ms
            .unwrap_or(DEFAULT_ROUTE_MAX_WAIT_MS)
            .min(MAX_ROUTE_MAX_WAIT_MS);
        self
    }

    pub fn with_default_min_ctx(mut self, min_ctx: Option<u32>) -> Self {
        self.default_min_ctx = min_ctx.filter(|value| *value > 0);
        self
    }

    fn request_options_from_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<GatewayRequestOptions, ApiError> {
        let mut options = GatewayRequestOptions::from_headers(headers)?;
        if options.max_price_mu.is_none() {
            options.max_price_mu = self.default_max_price_mu;
        }
        if !headers.contains_key(X_MAYHEM_MAX_WAIT_MS_HEADER) {
            options.max_wait_ms = self.default_max_wait_ms;
        }
        if options.min_ctx.is_none() {
            options.min_ctx = self.default_min_ctx;
        }
        Ok(options)
    }

    fn authorize_gateway_request(
        &self,
        headers: &HeaderMap,
        model: Option<&str>,
    ) -> Result<Option<GatewayTokenAttribution>, ApiError> {
        self.access_control.authorize(headers, model)
    }

    fn access_summary(&self) -> Value {
        self.access_control.summary()
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

    pub fn reputation_events(&self) -> Vec<StoredReputationEvent> {
        self.reputation_events
            .lock()
            .expect("reputation event store poisoned")
            .clone()
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

    fn record_receipt(&self, receipt: StoredReceipt) -> Result<(), ApiError> {
        let spend_delta = receipt.access_token.as_ref().and_then(|access_token| {
            let session_id = receipt.receipt.body.session_id.as_str();
            let cumulative = receipt.receipt.body.mu_owed_cum;
            let receipts = self.receipts.lock().expect("receipt store poisoned");
            let previous = receipts
                .iter()
                .filter(|existing| {
                    existing.access_token.as_ref() == Some(access_token)
                        && existing.receipt.body.session_id == session_id
                })
                .map(|existing| existing.receipt.body.mu_owed_cum)
                .max()
                .unwrap_or(0);
            cumulative
                .checked_sub(previous)
                .filter(|delta| *delta > 0)
                .map(|delta| (access_token.clone(), delta))
        });
        self.receipts
            .lock()
            .expect("receipt store poisoned")
            .push(receipt);
        if let Some((access_token, delta)) = spend_delta {
            self.access_control.record_spend(&access_token, delta)?;
        }
        Ok(())
    }

    fn record_probe(&self, probe: StoredProbeEvent) {
        self.probes
            .lock()
            .expect("probe store poisoned")
            .push(probe);
    }

    fn record_reputation_event(&self, event: StoredReputationEvent) {
        let mut events = self
            .reputation_events
            .lock()
            .expect("reputation event store poisoned");
        if events.iter().any(|existing| {
            existing.provider == event.provider && existing.event_id == event.event_id
        }) {
            return;
        }
        events.push(event);
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
            rail: "fiat".to_owned(),
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
        .route("/mayhem/reputation-events", get(mayhem_reputation_events))
        .route("/mayhem/balance", get(mayhem_balance))
        .route("/mayhem/dashboard", get(mayhem_dashboard))
        .route("/mayhem/dashboard/network", get(mayhem_dashboard_network))
        .route("/mayhem/dashboard/provider", get(mayhem_dashboard_provider))
        .route("/mayhem/dashboard/session", get(mayhem_dashboard_session))
        .route(
            "/mayhem/dashboard/assets/exo-latin.woff2",
            get(mayhem_dashboard_exo_font),
        )
        .with_state(Arc::new(state))
}

pub async fn serve(bind: SocketAddr, state: GatewayState) -> std::io::Result<()> {
    validate_gateway_bind_access(
        bind,
        state.access_control.requires_auth(),
        state.access_control.has_active_tokens(now_secs()),
    )?;
    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, openai_router(state)).await
}

pub fn gateway_bind_is_loopback(bind: SocketAddr) -> bool {
    bind.ip().is_loopback()
}

pub fn validate_gateway_bind_access(
    bind: SocketAddr,
    auth_required: bool,
    has_active_tokens: bool,
) -> std::io::Result<()> {
    if auth_required && !has_active_tokens {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Refusing to open the gateway to the network without access tokens. Run `mayhem tokens create` first.",
        ));
    }
    if gateway_bind_is_loopback(bind) {
        return Ok(());
    }
    if !has_active_tokens {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Refusing to open the gateway to the network without access tokens. Run `mayhem tokens create` first.",
        ));
    }
    if !auth_required {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Refusing to open the gateway to the network without enforced bearer-token auth.",
        ));
    }
    Ok(())
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

async fn list_models(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(err) = state.authorize_gateway_request(&headers, None) {
        return err.into_response();
    }
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
    let access_token = match state.authorize_gateway_request(&headers, Some(&request.model)) {
        Ok(access_token) => access_token,
        Err(err) => return err.into_response(),
    };
    let mut options = match state.request_options_from_headers(&headers) {
        Ok(options) => options,
        Err(err) => return err.into_response(),
    };
    options.access_token = access_token;
    match build_chat_completion(state.clone(), request, options).await {
        Ok(ChatResponse::Json(value)) => Json(value).into_response(),
        Ok(ChatResponse::Sse(chunks)) => sse_response(chunks),
        Ok(ChatResponse::SseStream(events)) => sse_stream_response(events),
        Err(err) => err.into_response(),
    }
}

async fn create_completion(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<CompletionRequest>,
) -> Response {
    if let Err(err) = state.authorize_gateway_request(&headers, Some(&request.model)) {
        return err.into_response();
    }
    match build_completion(&state, request) {
        Ok(ChatResponse::Json(value)) => Json(value).into_response(),
        Ok(ChatResponse::Sse(chunks)) => sse_response(chunks),
        Ok(ChatResponse::SseStream(events)) => sse_stream_response(events),
        Err(err) => err.into_response(),
    }
}

async fn create_embedding(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<EmbeddingRequest>,
) -> Response {
    let access_token = match state.authorize_gateway_request(&headers, Some(&request.model)) {
        Ok(access_token) => access_token,
        Err(err) => return err.into_response(),
    };
    let mut options = match state.request_options_from_headers(&headers) {
        Ok(options) => options,
        Err(err) => return err.into_response(),
    };
    options.access_token = access_token;
    match build_embedding(&state, request, options).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn create_image_generation(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<ImageGenerationRequest>,
) -> Response {
    let access_token = match state.authorize_gateway_request(&headers, Some(&request.model)) {
        Ok(access_token) => access_token,
        Err(err) => return err.into_response(),
    };
    let mut options = match state.request_options_from_headers(&headers) {
        Ok(options) => options,
        Err(err) => return err.into_response(),
    };
    options.access_token = access_token;
    match build_image_generation(&state, request, options).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn create_audio_speech(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<AudioSpeechRequest>,
) -> Response {
    let access_token = match state.authorize_gateway_request(&headers, Some(&request.model)) {
        Ok(access_token) => access_token,
        Err(err) => return err.into_response(),
    };
    let mut options = match state.request_options_from_headers(&headers) {
        Ok(options) => options,
        Err(err) => return err.into_response(),
    };
    options.access_token = access_token;
    match build_audio_speech(&state, request, options).await {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

async fn create_audio_transcription(
    State(state): State<SharedState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Response {
    let options = match state.request_options_from_headers(&headers) {
        Ok(options) => options,
        Err(err) => return err.into_response(),
    };
    match parse_audio_transcription_multipart(multipart)
        .await
        .and_then(|request| Ok((request, options)))
    {
        Ok((request, mut options)) => {
            let access_token = match state.authorize_gateway_request(&headers, Some(&request.model))
            {
                Ok(access_token) => access_token,
                Err(err) => return err.into_response(),
            };
            options.access_token = access_token;
            match build_audio_transcription(&state, request, options).await {
                Ok(value) => Json(value).into_response(),
                Err(err) => err.into_response(),
            }
        }
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

async fn mayhem_dashboard_network(
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
        dashboard_network_html(
            &state,
            state.dashboard_session.expires_in().as_secs(),
            &origin,
        ),
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
                "network_dashboard": "/mayhem/dashboard/network",
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

async fn mayhem_status(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(err) = state.authorize_gateway_request(&headers, None) {
        return err.into_response();
    }
    Json(json!({
        "ok": true,
        "version": 1,
        "contract_version": CONTRACT_VERSION,
        "backend": state.session_backend.name(),
        "dev_session_shim": state.dev_session_shim,
        "models": state.models.len(),
        "sessions_active": 0,
        "sessions_paused": state.paused_session_count(),
        "receipts": state.receipt_count(),
        "probes": state.probes.lock().expect("probe store poisoned").len(),
        "access": state.access_summary(),
    }))
    .into_response()
}

async fn mayhem_receipts(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(err) = state.authorize_gateway_request(&headers, None) {
        return err.into_response();
    }
    Json(json!({
        "object": "list",
        "data": state.receipts(),
        "paused": state.paused_sessions(),
    }))
    .into_response()
}

async fn mayhem_probes(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(err) = state.authorize_gateway_request(&headers, None) {
        return err.into_response();
    }
    Json(json!({
        "object": "list",
        "data": state.probes(),
    }))
    .into_response()
}

async fn mayhem_reputation_events(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = state.authorize_gateway_request(&headers, None) {
        return err.into_response();
    }
    Json(json!({
        "object": "list",
        "data": state.reputation_events(),
    }))
    .into_response()
}

async fn mayhem_balance(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(err) = state.authorize_gateway_request(&headers, None) {
        return err.into_response();
    }
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

fn dashboard_tier_tooltip() -> &'static str {
    "Tier 1: runs Mayhem software; trust is economic. Tier 2: proven Apple or NVIDIA hardware running the real app. Tier 3: Only Tier 3 keeps prompts private from the provider's own machine. Tier 4: admin-verified business identity, not prompt privacy; Tier 4 can still read prompts. Higher numbers are not a privacy ladder."
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
    let access_summary = state.access_summary();
    let token_rows = dashboard_access_token_rows(&access_summary);
    let token_count = access_summary
        .get("token_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let auth_mode = if access_summary
        .get("require_auth")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "Required"
    } else {
        "Optional local"
    };
    let balance_usd = format_mu_usd(state.receipt_config.balance_mu);
    let lifetime_spend = format_mu_usd(lifetime_spend_mu);
    let api_key_masked = "mayhem-local";
    let tier_tooltip = html_escape(dashboard_tier_tooltip());
    dashboard_html_document(
        "User Dashboard",
        &format!(
            r#"<nav class="nav"><a class="brand" href="/mayhem/dashboard">MAY<span class="hem">HEM</span></a><div class="search">{openai_base_url}</div><div class="nav-links"><a href="/mayhem/dashboard">User</a><a href="/mayhem/dashboard/network">Network</a><a href="/mayhem/dashboard/provider">Provider</a></div><span class="local-pill">LOCAL</span></nav><main class="dashboard"><section class="hero"><h1 class="wordmark">MAY<span class="hem">HEM</span></h1><p>User dashboard</p></section><section class="overview-grid"><article class="card metric-card"><span class="label">Balance</span><p class="value mono">{balance_usd}</p><p class="privacy-note">TAP rate not loaded</p></article><article class="card metric-card"><span class="label">Lifetime spend</span><p class="value mono">{lifetime_spend}</p><p class="privacy-note">from local receipts</p></article><article class="card metric-card"><span class="label">Active sessions</span><p class="value"><span class="count-chip">{active_sessions}</span></p><p class="privacy-note">running plus paused</p></article></section><section class="wide-grid"><article class="card"><div class="card-header"><h2>Sessions</h2><span class="count-chip">{receipt_count}</span></div><table class="table"><thead><tr><th>Model</th><th>Provider</th><th>Tokens</th><th>Cost</th><th>Status</th></tr></thead><tbody>{session_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Gateway</h2><span class="status-dot">Online</span></div><div class="detail-grid"><div><span class="label">Endpoint</span><div class="copy-row"><span class="mono">{openai_base_url}</span><button class="copy-chip" type="button">Copy</button></div></div><div><span class="label">Access</span><p class="mono">{auth_mode}</p></div><div><span class="label">Session</span><p class="mono">{expires_in_seconds}s</p></div><div><span class="label">Bind</span><p class="mono">127.0.0.1</p></div></div></article><article class="card"><div class="card-header"><h2>Access Tokens</h2><span class="count-chip">{token_count}</span></div><table class="table"><thead><tr><th>Name</th><th>Spend</th><th>Last Used</th><th>Status</th></tr></thead><tbody>{token_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Models</h2><div class="segmented" title="{tier_tooltip}" aria-label="{tier_tooltip}"><span class="segment active" title="{tier_tooltip}">T1+</span><span class="segment" title="{tier_tooltip}">T2+</span><span class="segment" title="{tier_tooltip}">T3+</span><span class="toggle" title="{tier_tooltip}">KYB</span></div></div><div class="model-list">{model_rows}</div></article><article class="card"><div class="card-header"><h2>Spend</h2><span class="count-chip">{lifetime_spend}</span></div>{spend_body}<div class="card-footer"><span>from local receipts</span><span class="mono">{receipt_count} receipts</span></div></article><article class="card opencode-card"><div class="card-header"><h2>opencode</h2><button class="copy-chip" type="button">Copy</button></div><pre>OPENAI_BASE_URL={openai_base_url}
OPENAI_API_KEY={api_key_masked}</pre></article></section></main><footer class="footer"><span>Runs entirely on this machine. No external network calls.</span><span class="mono">127.0.0.1</span></footer>"#,
            receipt_count = receipts.len(),
        ),
    )
}

fn dashboard_access_token_rows(access_summary: &Value) -> String {
    let Some(tokens) = access_summary.get("tokens").and_then(Value::as_array) else {
        return r#"<tr><td colspan="4"><span class="privacy-note">No gateway access tokens</span></td></tr>"#
            .to_owned();
    };
    if tokens.is_empty() {
        return r#"<tr><td colspan="4"><span class="privacy-note">No gateway access tokens</span></td></tr>"#
            .to_owned();
    }
    tokens
        .iter()
        .map(|token| {
            let name = token.get("name").and_then(Value::as_str).unwrap_or("token");
            let token_id = token
                .get("token_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let spend = token
                .get("spent_total_mu")
                .and_then(Value::as_u64)
                .map(format_mu_usd)
                .unwrap_or_else(|| "$0.000000".to_owned());
            let last_used = token
                .get("last_used_at")
                .and_then(Value::as_u64)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "never".to_owned());
            let active = token
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let status = if active {
                r#"<span class="status-dot">Active</span>"#
            } else {
                r#"<span class="status-dot muted">Inactive</span>"#
            };
            format!(
                r#"<tr><td><span class="mono">{}</span><p class="privacy-note">{}</p></td><td><span class="mono">{}</span></td><td><span class="mono">{}</span></td><td>{}</td></tr>"#,
                html_escape(short_text(name, 24).as_ref()),
                html_escape(short_text(token_id, 18).as_ref()),
                html_escape(&spend),
                html_escape(&last_used),
                status,
            )
        })
        .collect::<String>()
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
    let load_progress = dashboard_provider_load_progress(state);
    let loading_rows =
        dashboard_provider_loading_row_count(&candidates, &load_progress, provider_filter);
    let enclave_rows = dashboard_provider_enclave_rows(
        &candidates,
        &latest_receipts,
        &load_progress,
        provider_filter,
    );
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
            r#"<nav class="nav"><a class="brand" href="/mayhem/dashboard">MAY<span class="hem">HEM</span></a><div class="search">{gateway_root}/mayhem/dashboard/provider{provider_query}</div><div class="nav-links"><a href="/mayhem/dashboard">User</a><a href="/mayhem/dashboard/network">Network</a><a href="/mayhem/dashboard/provider">Provider</a></div><span class="local-pill">LOCAL</span></nav><main class="dashboard"><section class="hero"><h1 class="wordmark">MAY<span class="hem">HEM</span></h1><p>Provider dashboard</p></section><p class="provider-scope mono">{provider_scope_label}</p><section class="overview-grid provider"><article class="card metric-card"><span class="label">Earned this epoch</span><p class="value mono">{earned}</p><p class="privacy-note">{earned_source}</p></article><article class="card metric-card"><span class="label">Pending claim</span><p class="value mono">{claimable_value}</p><p class="privacy-note">from mayhem earnings</p></article><article class="card metric-card"><span class="label">Reputation</span><p class="value mono">{reputation}</p><p class="privacy-note">local receipt/probe evidence</p></article><article class="card metric-card"><span class="label">Saturation</span><p class="value mono">{saturation_pct}%</p><p class="privacy-note">{active_sessions} active sessions</p></article></section><section class="wide-grid provider"><article class="card"><div class="card-header"><h2>Enclaves</h2><span class="count-chip">{candidate_count}</span></div><table class="table"><thead><tr><th>Model</th><th>Backend</th><th>Tier</th><th>Saturation</th><th>Status</th></tr></thead><tbody>{enclave_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Live sessions</h2><span class="count-chip">{receipt_count}</span></div><table class="table"><thead><tr><th>Room</th><th>Model</th><th>Tokens</th><th>Elapsed</th><th>Status</th></tr></thead><tbody>{live_session_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Earnings</h2><div class="segmented"><span class="segment active">Owed {claimable_value}</span><span class="segment">Paid {paid}</span></div></div>{earnings_body}<div class="card-footer"><span>{earnings_source}</span><span class="mono">{epoch_label}</span></div></article><article class="card"><div class="card-header"><h2>Reputation / Holdback</h2><span class="count-chip">{reputation}</span></div>{holdback_body}</article><article class="card"><div class="card-header"><h2>Hardware / Health</h2><span class="{hardware_status_class}">{hardware_status}</span></div>{hardware_body}</article><article class="card claim-card"><div class="card-header"><h2>Claim</h2><button class="copy-chip" type="button">Copy</button></div>{claim_body}</article></section></main><footer class="footer"><span>Local session {expires_in_seconds}s. Runs entirely on this machine. No external network calls.</span><span class="mono">127.0.0.1</span></footer>"#,
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
                if loading_rows > 0 {
                    "status-dot"
                } else {
                    "status-dot muted"
                }
            } else {
                "status-dot"
            },
            hardware_status = if candidates.is_empty() {
                if loading_rows > 0 {
                    "Loading"
                } else {
                    "No route"
                }
            } else {
                "Healthy"
            },
            candidate_count = candidates.len() + loading_rows,
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

fn dashboard_network_html(state: &GatewayState, expires_in_seconds: u64, origin: &str) -> String {
    let entries = {
        let table = state
            .provider_table
            .lock()
            .expect("provider table poisoned");
        table.entries(now_millis_u64())
    };
    let provider_count = state
        .models
        .iter()
        .flat_map(|model| model.mayhem.route_candidates.iter())
        .map(|candidate| candidate.provider.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let route_count = state
        .models
        .iter()
        .map(|model| model.mayhem.route_candidates.len())
        .sum::<usize>();
    let live_count = entries
        .iter()
        .filter(|entry| dashboard_entry_has_live_heartbeat(entry))
        .count();
    let unavailable_models = state
        .models
        .iter()
        .filter(|model| !dashboard_model_has_live_provider(model, &entries))
        .count();
    let gateway_root = origin.trim_end_matches('/');
    let model_rows = dashboard_network_model_rows(&state.models, &entries);
    let provider_rows = dashboard_network_provider_rows(&state.models, &entries);
    dashboard_html_document(
        "Network Explorer",
        &format!(
            r#"<nav class="nav"><a class="brand" href="/mayhem/dashboard">MAY<span class="hem">HEM</span></a><div class="search">{gateway_root}/mayhem/dashboard/network</div><div class="nav-links"><a href="/mayhem/dashboard">User</a><a href="/mayhem/dashboard/network">Network</a><a href="/mayhem/dashboard/provider">Provider</a></div><span class="local-pill">LOCAL</span></nav><main class="dashboard"><section class="hero"><h1 class="wordmark">MAY<span class="hem">HEM</span></h1><p>Network explorer</p></section><section class="overview-grid provider"><article class="card metric-card"><span class="label">Catalog models</span><p class="value mono">{model_count}</p><p class="privacy-note">from gateway catalog state</p></article><article class="card metric-card"><span class="label">Canonical providers</span><p class="value mono">{provider_count}</p><p class="privacy-note">from route candidates</p></article><article class="card metric-card"><span class="label">Live heartbeats</span><p class="value mono">{live_count}</p><p class="privacy-note">signed provider reports</p></article><article class="card metric-card"><span class="label">Unavailable models</span><p class="value mono">{unavailable_models}</p><p class="privacy-note">no live provider heartbeat</p></article></section><section class="wide-grid network"><article class="card"><div class="card-header"><h2>Models</h2><span class="count-chip">{model_count}</span></div><table class="table"><thead><tr><th>Model</th><th>Availability</th><th>Abilities</th><th>Terms</th><th>Constraints</th></tr></thead><tbody>{model_rows}</tbody></table></article><article class="card"><div class="card-header"><h2>Providers</h2><span class="count-chip">{route_count}</span></div><table class="table"><thead><tr><th>Provider</th><th>Route</th><th>Backend</th><th>Rails / price</th><th>Status</th></tr></thead><tbody>{provider_rows}</tbody></table></article></section></main><footer class="footer"><span>Local session {expires_in_seconds}s. Explorer data is local gateway state from catalog, contract route candidates, and provider heartbeats.</span><span class="mono">127.0.0.1</span></footer>"#,
            model_count = state.models.len(),
        ),
    )
}

fn dashboard_network_model_rows(models: &[GatewayModel], entries: &[ProviderTableEntry]) -> String {
    if models.is_empty() {
        return r#"<tr><td colspan="5"><span class="privacy-note">No catalog models loaded</span></td></tr>"#
            .to_owned();
    }
    models
        .iter()
        .map(|model| {
            let route_count = model.mayhem.route_candidates.len();
            let room_count = model
                .mayhem
                .route_candidates
                .iter()
                .map(|candidate| candidate.room_id.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            let live_count = model
                .mayhem
                .route_candidates
                .iter()
                .filter(|candidate| {
                    dashboard_entry_for_route(entries, candidate)
                        .is_some_and(dashboard_entry_has_live_heartbeat)
                })
                .count();
            let availability = if live_count > 0 {
                format!(r#"<span class="status-dot">Online</span><p class="privacy-note">{live_count}/{route_count} providers live</p>"#)
            } else if route_count > 0 {
                format!(r#"<span class="status-dot muted">Unavailable</span><p class="privacy-note">{route_count} joined; heartbeat pending</p>"#)
            } else {
                r#"<span class="status-dot muted">Unavailable</span><p class="privacy-note">no canonical provider route</p>"#
                    .to_owned()
            };
            let constraints = dashboard_model_constraints(model, route_count, room_count);
            format!(
                r#"<tr><td><span class="mono">{}</span><p class="privacy-note">{}</p></td><td>{availability}</td><td>{}</td><td><span class="mono">{}</span><p class="privacy-note">price v{} · {}</p></td><td>{constraints}</td></tr>"#,
                html_escape(short_text(&model.id, 34).as_ref()),
                html_escape(&model.mayhem.source),
                dashboard_badges(&dashboard_model_abilities(model), "badge"),
                html_escape(&dashboard_model_price(model)),
                model.mayhem.price_ref_mu.ver,
                html_escape(&dashboard_model_price_derivation(model)),
            )
        })
        .collect::<String>()
}

fn dashboard_network_provider_rows(
    models: &[GatewayModel],
    entries: &[ProviderTableEntry],
) -> String {
    let mut rows = String::new();
    for model in models {
        for candidate in &model.mayhem.route_candidates {
            let entry = dashboard_entry_for_route(entries, candidate);
            let backend = dashboard_route_engine(model, candidate);
            let rails = dashboard_route_rails(candidate);
            let quality = dashboard_route_quality(entry);
            let status = dashboard_route_status(model, candidate, entry);
            let price = route_price_ref_mu(model, Some(candidate));
            rows.push_str(&format!(
                r#"<tr><td><span class="mono">{}</span><p class="privacy-note">{}</p></td><td><span class="mono">{}</span><p class="privacy-note">room {}</p></td><td><span class="mono">{}</span><p class="privacy-note">{}</p></td><td><span class="mono">{}</span><p class="privacy-note">{} · price v{}</p></td><td>{}<p class="privacy-note">{}</p></td></tr>"#,
                html_escape(short_text(&candidate.provider, 18).as_ref()),
                html_escape(short_text(&candidate.enclave_id, 18).as_ref()),
                html_escape(short_text(&model.id, 28).as_ref()),
                html_escape(short_text(&candidate.room_id, 14).as_ref()),
                html_escape(&backend),
                dashboard_badges(&dashboard_route_abilities(model, candidate, entry), "badge"),
                html_escape(&rails),
                html_escape(&dashboard_price(price)),
                price.ver,
                status,
                html_escape(&quality),
            ));
        }
    }
    if rows.is_empty() {
        r#"<tr><td colspan="5"><span class="privacy-note">No canonical provider routes loaded</span></td></tr>"#
            .to_owned()
    } else {
        rows
    }
}

fn dashboard_entry_for_route<'a>(
    entries: &'a [ProviderTableEntry],
    candidate: &GatewayRouteCandidate,
) -> Option<&'a ProviderTableEntry> {
    entries.iter().find(|entry| {
        entry.key.provider == candidate.provider
            && entry.key.enclave_id == candidate.enclave_id
            && entry.key.room_id == candidate.room_id
    })
}

fn dashboard_entry_has_live_heartbeat(entry: &ProviderTableEntry) -> bool {
    entry
        .heartbeat
        .as_ref()
        .is_some_and(|heartbeat| !heartbeat.sig.trim().is_empty())
}

fn dashboard_model_has_live_provider(model: &GatewayModel, entries: &[ProviderTableEntry]) -> bool {
    model.mayhem.route_candidates.iter().any(|candidate| {
        dashboard_entry_for_route(entries, candidate)
            .is_some_and(dashboard_entry_has_live_heartbeat)
    })
}

fn dashboard_model_abilities(model: &GatewayModel) -> Vec<String> {
    let mut abilities = BTreeSet::new();
    for modality in model
        .mayhem
        .caps
        .output_modalities
        .iter()
        .chain(model.mayhem.caps.output_modality.iter())
        .chain(model.mayhem.adapter.modality_set.iter())
    {
        if !modality.trim().is_empty() {
            abilities.insert(modality.trim().to_owned());
        }
    }
    if model.mayhem.model_class == DEFAULT_MODEL_CLASS {
        abilities.insert("text".to_owned());
    }
    if model.mayhem.caps.tools {
        abilities.insert("tools".to_owned());
    }
    if model.mayhem.caps.json {
        abilities.insert("json".to_owned());
    }
    if model.mayhem.caps.vision {
        abilities.insert("vision".to_owned());
    }
    if model.mayhem.caps.image {
        abilities.insert("image".to_owned());
    }
    if model.mayhem.caps.video {
        abilities.insert("video".to_owned());
    }
    if model.mayhem.caps.audio {
        abilities.insert("audio".to_owned());
    }
    abilities.insert(format!("ctx {}", model.mayhem.caps.ctx));
    abilities.into_iter().collect()
}

fn dashboard_route_abilities(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    entry: Option<&ProviderTableEntry>,
) -> Vec<String> {
    let caps = entry
        .and_then(|entry| {
            entry
                .heartbeat
                .as_ref()
                .map(|heartbeat| heartbeat.caps.clone())
        })
        .unwrap_or_else(|| heartbeat_caps_for_route(model, candidate));
    let mut abilities = BTreeSet::new();
    if caps.tools {
        abilities.insert("tools".to_owned());
    }
    if caps.json {
        abilities.insert("json".to_owned());
    }
    if caps.vision {
        abilities.insert("vision".to_owned());
    }
    abilities.insert(format!("ctx {}", caps.ctx));
    abilities.into_iter().collect()
}

fn dashboard_model_constraints(
    model: &GatewayModel,
    route_count: usize,
    room_count: usize,
) -> String {
    let mut constraints = Vec::new();
    constraints.push(format!("{} routes", route_count));
    constraints.push(format!("{} rooms", room_count));
    constraints.push(dashboard_attestation_summary(model));
    if !model.mayhem.kyb_identities.is_empty() {
        constraints.push(format!("{} KYB", model.mayhem.kyb_identities.len()));
    }
    if let Some(min_app) = &model.mayhem.min_app_version {
        constraints.push(format!("min app {min_app}"));
    }
    dashboard_badges(&constraints, "badge")
}

fn dashboard_attestation_summary(model: &GatewayModel) -> String {
    let labels = model
        .mayhem
        .attestation_tier_labels
        .iter()
        .map(|(tier, label)| format!("{tier}:{label}"))
        .collect::<Vec<_>>();
    if !labels.is_empty() {
        return labels.join(", ");
    }
    let tiers = model
        .mayhem
        .attestation_tiers
        .iter()
        .map(|(tier, count)| format!("{tier}:{count}"))
        .collect::<Vec<_>>();
    if tiers.is_empty() {
        "T1".to_owned()
    } else {
        tiers.join(", ")
    }
}

fn dashboard_route_engine(model: &GatewayModel, candidate: &GatewayRouteCandidate) -> String {
    candidate
        .caps
        .get("engine")
        .or_else(|| candidate.caps.get("backend"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(model.mayhem.adapter.request_shape_family.as_str())
        .to_owned()
}

fn dashboard_route_rails(candidate: &GatewayRouteCandidate) -> String {
    if candidate.accepted_rails.is_empty() {
        "not advertised".to_owned()
    } else {
        candidate.accepted_rails.join(", ")
    }
}

fn dashboard_route_quality(entry: Option<&ProviderTableEntry>) -> String {
    let Some(entry) = entry else {
        return "no provider-table entry".to_owned();
    };
    if !dashboard_entry_has_live_heartbeat(entry) {
        return "live heartbeat pending".to_owned();
    }
    let Some(heartbeat) = entry.heartbeat.as_ref() else {
        return "live heartbeat pending".to_owned();
    };
    let mut parts = vec![
        format!("sat {:.0}%", heartbeat.sat.clamp(0.0, 1.0) * 100.0),
        format!("slots {}/{}", heartbeat.slots.active, heartbeat.slots.max),
        format!("ttft {}ms", heartbeat.perf.ttft_ms),
    ];
    if let Some(tok_s) = heartbeat.perf.tok_s.filter(|value| value.is_finite()) {
        parts.push(format!("{tok_s:.1} tok/s"));
    }
    if let Some(age) = entry.heartbeat_age_millis {
        parts.push(format!("age {}ms", age));
    }
    parts.join(", ")
}

fn dashboard_route_status(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    entry: Option<&ProviderTableEntry>,
) -> String {
    let mut details = vec![
        format!("T{}", candidate.att_tier),
        candidate.quant.clone(),
        format!(
            "rep {}",
            format_bps_percent(candidate.reputation_bps.min(10_000))
        ),
    ];
    if candidate.kyb.is_some() {
        details.push("KYB".to_owned());
    }
    if candidate
        .probation
        .as_ref()
        .is_some_and(|probation| probation.active)
    {
        details.push("probation".to_owned());
    }
    let att = entry
        .and_then(|entry| entry.attestation_head.as_ref())
        .map(|_| "attested")
        .unwrap_or("attestation pending");
    details.push(att.to_owned());
    let label = if entry
        .and_then(|entry| entry.heartbeat.as_ref())
        .is_some_and(|heartbeat| !heartbeat.accepting_new)
    {
        r#"<span class="status-dot muted">Draining</span>"#
    } else if entry.is_some_and(dashboard_entry_has_live_heartbeat) {
        r#"<span class="status-dot">Online</span>"#
    } else if model.mayhem.route_candidates.is_empty() {
        r#"<span class="status-dot muted">Unavailable</span>"#
    } else {
        r#"<span class="status-dot muted">Joined</span>"#
    };
    let local_run = dashboard_local_run_status(candidate.local_run.as_ref());
    format!(
        "{label}<span class=\"privacy-note\">{}</span>{local_run}",
        html_escape(&details.join(", "))
    )
}

fn dashboard_local_run_status(local_run: Option<&GatewayLocalRunBadge>) -> String {
    let Some(local_run) = local_run else {
        return String::new();
    };
    let tok_s = local_run
        .estimated_tok_s
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!(" · {value} tok/s est"))
        .unwrap_or_default();
    format!(
        r#"<span class="privacy-note">local {} {} · ctx {}/{}{} · mem {}/{} · download {} · ETA {}</span>"#,
        html_escape(&local_run.marker),
        html_escape(&local_run.label),
        local_run.served_ctx,
        local_run.requested_ctx,
        html_escape(&tok_s),
        html_escape(&local_run.memory_required_human),
        html_escape(&local_run.memory_budget_human),
        html_escape(&local_run.download_human),
        html_escape(&local_run.eta),
    )
}

fn dashboard_badges(values: &[String], class_name: &str) -> String {
    if values.is_empty() {
        return r#"<span class="privacy-note">none advertised</span>"#.to_owned();
    }
    let badges = values
        .iter()
        .map(|value| {
            format!(
                r#"<span class="{class_name}">{}</span>"#,
                html_escape(value)
            )
        })
        .collect::<String>();
    format!(r#"<div class="badge-row">{badges}</div>"#)
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

#[derive(Clone, Debug, Default, Deserialize)]
struct DashboardProviderLoadProgress {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    enclave_id: String,
    #[serde(default)]
    artifact: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    phase: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    position: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    percent: Option<u64>,
    #[serde(default)]
    updated_at_ms: u64,
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

fn dashboard_provider_load_progress(
    state: &GatewayState,
) -> BTreeMap<(String, String), DashboardProviderLoadProgress> {
    let mut out = BTreeMap::new();
    let Some(dir) = state.provider_load_progress_dir.as_ref().as_ref() else {
        return out;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(progress) = serde_json::from_str::<DashboardProviderLoadProgress>(&raw) else {
            continue;
        };
        if progress.provider.is_empty() || progress.enclave_id.is_empty() {
            continue;
        }
        let key = (progress.provider.clone(), progress.enclave_id.clone());
        let replace = out
            .get(&key)
            .map(|existing: &DashboardProviderLoadProgress| {
                existing.updated_at_ms <= progress.updated_at_ms
            })
            .unwrap_or(true);
        if replace {
            out.insert(key, progress);
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
    load_progress: &BTreeMap<(String, String), DashboardProviderLoadProgress>,
    provider_filter: Option<&str>,
) -> String {
    let mut seen = BTreeSet::new();
    let mut rows = candidates
        .iter()
        .take(10)
        .map(|candidate| {
            seen.insert((candidate.provider.clone(), candidate.enclave_id.clone()));
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
            let status = dashboard_provider_enclave_status(load_progress.get(&(
                candidate.provider.clone(),
                candidate.enclave_id.clone(),
            )));
            format!(
                r#"<tr><td><span class="mono">{}</span><p class="privacy-note">{}</p></td><td class="mono">{}</td><td class="mono">T{}{}</td><td><div class="mini-bar"><span style="--w:{}%"></span></div><span class="privacy-note">{}%</span></td><td>{}</td></tr>"#,
                html_escape(short_text(&candidate.model_id, 30).as_ref()),
                html_escape(short_text(&candidate.enclave_id, 22).as_ref()),
                html_escape(&candidate.backend),
                candidate.att_tier,
                kyb,
                saturation,
                saturation,
                status,
            )
        })
        .collect::<String>();
    let remaining = 10_usize.saturating_sub(seen.len());
    for ((_, _), progress) in load_progress
        .iter()
        .filter(|((provider, enclave_id), _)| {
            provider_filter.map_or(true, |filter| filter == provider)
                && !seen.contains(&(provider.clone(), enclave_id.clone()))
        })
        .take(remaining)
    {
        rows.push_str(&dashboard_provider_progress_only_row(progress));
    }
    if rows.is_empty() {
        return r#"<tr><td colspan="5"><span class="privacy-note">No provider routes loaded</span></td></tr>"#
            .to_owned();
    }
    rows
}

fn dashboard_provider_loading_row_count(
    candidates: &[DashboardProviderCandidate],
    load_progress: &BTreeMap<(String, String), DashboardProviderLoadProgress>,
    provider_filter: Option<&str>,
) -> usize {
    let seen = candidates
        .iter()
        .map(|candidate| (candidate.provider.clone(), candidate.enclave_id.clone()))
        .collect::<BTreeSet<_>>();
    load_progress
        .iter()
        .filter(|((provider, enclave_id), _)| {
            provider_filter.map_or(true, |filter| filter == provider)
                && !seen.contains(&(provider.clone(), enclave_id.clone()))
        })
        .count()
}

fn dashboard_provider_progress_only_row(progress: &DashboardProviderLoadProgress) -> String {
    let model = if progress.model_id.trim().is_empty() {
        "local provider load"
    } else {
        progress.model_id.trim()
    };
    let backend = if progress.artifact.trim().is_empty() {
        "loading"
    } else {
        progress.artifact.trim()
    };
    format!(
        r#"<tr><td><span class="mono">{}</span><p class="privacy-note">{}</p></td><td class="mono">{}</td><td class="mono">pending</td><td><div class="mini-bar"><span style="--w:0%"></span></div><span class="privacy-note">0%</span></td><td>{}</td></tr>"#,
        html_escape(short_text(model, 30).as_ref()),
        html_escape(short_text(&progress.enclave_id, 22).as_ref()),
        html_escape(backend),
        dashboard_provider_enclave_status(Some(progress)),
    )
}

fn dashboard_provider_enclave_status(progress: Option<&DashboardProviderLoadProgress>) -> String {
    let Some(progress) = progress else {
        return r#"<span class="status-dot">Serving</span>"#.to_owned();
    };
    let phase = progress.phase.trim();
    let status = progress.status.trim();
    if status == "complete" && (phase == "serving" || phase == "joined") {
        let label = if phase == "joined" {
            "Joined"
        } else {
            "Serving"
        };
        return format!(r#"<span class="status-dot">{label}</span>"#);
    }
    if status == "error" {
        let detail = non_empty_load_label(progress);
        return format!(
            r#"<div class="load-cell"><span class="status-dot muted">Load failed</span><span class="privacy-note">{}</span></div>"#,
            html_escape(&detail),
        );
    }
    let percent = progress
        .percent
        .or_else(|| dashboard_provider_progress_percent(progress.position, progress.total))
        .unwrap_or(0)
        .min(100);
    let phase_label = if phase.is_empty() { "load" } else { phase };
    let headline = if status == "complete" {
        "Ready"
    } else {
        "Loading"
    };
    format!(
        r#"<div class="load-cell"><span class="status-dot">{headline}</span><div class="mini-bar"><span style="--w:{percent}%"></span></div><span class="privacy-note">{} {}%</span></div>"#,
        html_escape(phase_label),
        percent,
    )
}

fn dashboard_provider_progress_percent(position: Option<u64>, total: Option<u64>) -> Option<u64> {
    match (position, total) {
        (Some(position), Some(total)) if total > 0 => {
            Some(((u128::from(position) * 100) / u128::from(total)).min(100) as u64)
        }
        _ => None,
    }
}

fn non_empty_load_label(progress: &DashboardProviderLoadProgress) -> String {
    if progress.phase.trim().is_empty() {
        progress.label.clone()
    } else {
        progress.phase.clone()
    }
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
                body.usage.prompt_tokens(),
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
                body.usage.prompt_tokens(),
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
    let tier_tooltip = html_escape(dashboard_tier_tooltip());
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
                r#"<div class="model-row"><div><div class="model-title mono">{}</div><div class="model-meta" title="{}" aria-label="{}">{} · {} · T{}{} · {}</div></div><a class="copy-chip" href="/mayhem/dashboard">Use</a></div>"#,
                html_escape(&model.id),
                tier_tooltip,
                tier_tooltip,
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
    dashboard_price(&model.mayhem.price_ref_mu)
}

fn dashboard_price(price: &PriceRefMu) -> String {
    let entries = price
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

fn dashboard_model_price_derivation(model: &GatewayModel) -> String {
    let Some(derivation) = model.mayhem.price_ref_mu.derivation.as_ref() else {
        return "derivation pending".to_owned();
    };
    let epoch = derivation_u64(derivation, &["epoch"])
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_owned());
    let seed_ver = derivation_u64(derivation, &["seed_price", "ver"])
        .map(|value| format!("v{value}"))
        .unwrap_or_else(|| "?".to_owned());
    let result_ver = derivation_u64(derivation, &["result_price", "ver"])
        .or_else(|| derivation_u64(derivation, &["price_ver"]))
        .map(|value| format!("v{value}"))
        .unwrap_or_else(|| "price".to_owned());
    let utilization = derivation_u64(derivation, &["controller", "utilization_bps"])
        .map(format_bps)
        .unwrap_or_else(|| "?".to_owned());
    let demand_mu = derivation_u64(derivation, &["usage", "active_demand_mu"])
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_owned());
    let sessions = derivation_u64(derivation, &["usage", "session_count"])
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_owned());
    let supply = derivation_u64(derivation, &["controller", "active_supply"])
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_owned());
    let source = derivation_str(derivation, &["controller", "source"])
        .or_else(|| derivation_str(derivation, &["price_source"]))
        .unwrap_or("market");
    let frozen = if derivation_bool(derivation, &["controller", "frozen"]).unwrap_or(false) {
        " · frozen"
    } else {
        ""
    };
    let root = derivation_str(derivation, &["price_root"])
        .map(|value| format!(" · root {}", short_text(value, 12)))
        .unwrap_or_default();
    let leaf = derivation_str(derivation, &["derivation_hash"])
        .map(|value| format!(" · leaf {}", short_text(value, 12)))
        .unwrap_or_default();
    format!(
        "price = f(seed {seed_ver}, U {utilization}, demand {demand_mu}mu, {sessions} sessions, supply {supply}) -> {result_ver} · epoch {epoch} · {source}{frozen}{root}{leaf}"
    )
}

fn derivation_u64<'a>(value: &'a Value, path: &[&str]) -> Option<u64> {
    derivation_value(value, path)?.as_u64()
}

fn derivation_bool<'a>(value: &'a Value, path: &[&str]) -> Option<bool> {
    derivation_value(value, path)?.as_bool()
}

fn derivation_str<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    derivation_value(value, path)?.as_str()
}

fn derivation_value<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn format_bps(value: u64) -> String {
    format!("{}.{:02}%", value / 100, value % 100)
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

type SseEventStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;
const LIVE_SSE_MIN_EVENT_BUFFER: usize = 512;
const LIVE_SSE_MAX_EVENT_BUFFER: usize = 16_384;
const LIVE_SSE_DEFAULT_MAX_TOKENS: usize = 1024;
const DIRECT_SESSION_CHECKPOINT_ACK_RESEND_MS: u64 = 5_000;

enum ChatResponse {
    Json(Value),
    Sse(Vec<Value>),
    SseStream(SseEventStream),
}

#[derive(Clone)]
struct ResponseMayhemMeta<'a> {
    backend: &'a str,
    direct_session: bool,
    billable: bool,
    dev_session: bool,
    hedge: GatewayHedgeInvocation,
}

#[derive(Clone, Debug, PartialEq)]
struct GatewayRequestOptions {
    hedge_requested: bool,
    min_att_tier: Option<u8>,
    max_price_mu: Option<u64>,
    max_wait_ms: u64,
    min_ctx: Option<u32>,
    quant: Option<String>,
    failover_overrides: GatewayFailoverPolicyConfig,
    access_token: Option<GatewayTokenAttribution>,
}

impl Default for GatewayRequestOptions {
    fn default() -> Self {
        Self {
            hedge_requested: false,
            min_att_tier: None,
            max_price_mu: None,
            max_wait_ms: DEFAULT_ROUTE_MAX_WAIT_MS,
            min_ctx: None,
            quant: None,
            failover_overrides: GatewayFailoverPolicyConfig::default(),
            access_token: None,
        }
    }
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
    quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
struct DirectEmbeddingSessionCollected {
    output: EmbeddingOutput,
    provider_receipt: ProviderSignedReceipt,
    quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
struct DirectImageGenerationSessionCollected {
    output: ImageGenerationOutput,
    provider_receipt: ProviderSignedReceipt,
    quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
struct DirectAudioSpeechSessionCollected {
    output: AudioSpeechOutput,
    provider_receipt: ProviderSignedReceipt,
    quality: Option<GatewaySessionQuality>,
}

#[derive(Clone, Debug)]
struct DirectAudioTranscriptionSessionCollected {
    output: AudioTranscriptionOutput,
    provider_receipt: ProviderSignedReceipt,
    quality: Option<GatewaySessionQuality>,
}

struct GatewaySessionRun {
    result: GatewaySessionResult,
    invocation: GatewaySessionInvocation,
    metering_request: ChatCompletionRequest,
    metering_output: ChatOutput,
}

struct GatewayEmbeddingRun {
    result: GatewayEmbeddingResult,
    invocation: GatewaySessionInvocation,
    metering_inputs: Vec<String>,
    metering_output: EmbeddingOutput,
}

struct GatewayImageGenerationRun {
    result: GatewayImageGenerationResult,
    invocation: GatewaySessionInvocation,
    metering_request: ImageGenerationRequest,
    metering_output: ImageGenerationOutput,
}

struct GatewayAudioSpeechRun {
    result: GatewayAudioSpeechResult,
    invocation: GatewaySessionInvocation,
    metering_request: AudioSpeechRequest,
    metering_output: AudioSpeechOutput,
}

struct GatewayAudioTranscriptionRun {
    result: GatewayAudioTranscriptionResult,
    invocation: GatewaySessionInvocation,
    metering_request: AudioTranscriptionRequest,
    metering_output: AudioTranscriptionOutput,
}

#[derive(Clone, Debug, Default)]
struct GatewayHedgeProbeOutcome {
    actual_probe_count: usize,
    winner: Option<GatewayHedgeProbeResult>,
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
    seq: u64,
    final_receipt: bool,
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

fn provider_table_from_models(models: &[GatewayModel], rules_ver: u64) -> ProviderTable {
    let mut table = ProviderTable::new();
    let now = now_millis_u64();
    for model in models {
        for candidate in &model.mayhem.route_candidates {
            table.upsert_contract(contract_snapshot_for_route(model, candidate, rules_ver));
            table.upsert_fallback_heartbeat(heartbeat_for_route(model, candidate, now), now);
        }
    }
    table
}

fn contract_snapshot_for_route(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    rules_ver: u64,
) -> ContractProviderSnapshot {
    let price = route_price_ref_mu(model, Some(candidate));
    ContractProviderSnapshot {
        provider: candidate.provider.clone(),
        provider_status: Some("active".to_owned()),
        enclave_id: candidate.enclave_id.clone(),
        model_id: model.id.clone(),
        room_id: candidate.room_id.clone(),
        consent_ver: rules_ver,
        reputation: f64::from(candidate.reputation_bps.min(10_000)) / 10_000.0,
        price_ver: price.ver,
        rate_map: price.rate_map.clone(),
        per_req_mu: price.per_req_mu,
        min_session_mu: price.min_session_mu,
        ref_rate_map: price.rate_map.clone(),
        probation: candidate.probation.clone(),
        caps: heartbeat_caps_for_route(model, candidate),
        attestation_head: Some(candidate.binary_hash.clone()),
    }
}

fn heartbeat_for_route(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    now_millis: u64,
) -> ProviderHeartbeat {
    let price = route_price_ref_mu(model, Some(candidate));
    ProviderHeartbeat {
        t: "hb".to_owned(),
        v: crate::HEARTBEAT_SCHEMA_VERSION,
        contract_version: CONTRACT_VERSION,
        provider: candidate.provider.clone(),
        enclave_id: candidate.enclave_id.clone(),
        model_id: model.id.clone(),
        room_id: candidate.room_id.clone(),
        sat: 0.0,
        slots: HeartbeatSlots {
            active: 0,
            active_requests: 0,
            max: 1,
        },
        q: HeartbeatQueue {
            free_slots: 0,
            engine_backlog: 0,
            est_wait_ms: 0,
        },
        perf: HeartbeatPerf {
            tok_s: Some(50.0),
            ttft_ms: 150,
        },
        price_ver: price.ver,
        min_ask_mu: candidate.min_ask_mu,
        accepting_new: true,
        caps: heartbeat_caps_for_route(model, candidate),
        att: HeartbeatAttestation {
            epoch: 0,
            head: candidate.binary_hash.clone(),
        },
        ts: now_millis / 1000,
        nonce: blake3_hex(
            format!(
                "route:{}:{}:{}:{now_millis}",
                candidate.provider, candidate.enclave_id, candidate.room_id
            )
            .as_bytes(),
        ),
        sig: String::new(),
    }
}

fn heartbeat_caps_from_model(caps: &ModelCaps) -> HeartbeatCaps {
    HeartbeatCaps {
        tools: caps.tools,
        json: caps.json,
        ctx: caps.ctx,
        vision: caps.vision,
    }
}

fn heartbeat_caps_for_route(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
) -> HeartbeatCaps {
    let fallback = heartbeat_caps_from_model(&model.mayhem.caps);
    HeartbeatCaps {
        tools: candidate
            .caps
            .get("tools")
            .and_then(Value::as_bool)
            .unwrap_or(fallback.tools),
        json: candidate
            .caps
            .get("json")
            .and_then(Value::as_bool)
            .unwrap_or(fallback.json),
        ctx: candidate
            .caps
            .get("ctx")
            .or_else(|| candidate.caps.get("ctx_max"))
            .and_then(Value::as_u64)
            .and_then(|ctx| u32::try_from(ctx).ok())
            .unwrap_or(fallback.ctx),
        vision: candidate
            .caps
            .get("vision")
            .and_then(Value::as_bool)
            .unwrap_or(fallback.vision),
    }
}

fn route_caps_ctx(model: &GatewayModel, candidate: &GatewayRouteCandidate) -> u32 {
    heartbeat_caps_for_route(model, candidate).ctx
}

fn model_served_ctx(model: &GatewayModel) -> u32 {
    model.mayhem.caps.ctx
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
        && candidate
            .artifact_sidecar_roots
            .values()
            .all(|root| is_hex_len(root, 64))
        && is_hex_len(&candidate.manifest_hash, 64)
        && is_hex_len(&candidate.binary_hash, 64)
}

impl GatewaySessionError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            clean_refusal: false,
            clean_refusal_code: None,
            partial: None,
        }
    }

    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            clean_refusal: false,
            clean_refusal_code: None,
            partial: None,
        }
    }

    pub fn clean_refusal(message: impl Into<String>) -> Self {
        Self::clean_refusal_with_code(message, None)
    }

    pub fn clean_refusal_with_code(message: impl Into<String>, code: Option<&str>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            clean_refusal: true,
            clean_refusal_code: code.map(str::to_owned),
            partial: None,
        }
    }

    pub fn retryable_partial(message: impl Into<String>, partial: GatewaySessionPartial) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            clean_refusal: false,
            clean_refusal_code: None,
            partial: Some(Box::new(partial)),
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
            max_price_mu: parse_x_mayhem_max_price_mu(headers)?,
            max_wait_ms: parse_x_mayhem_max_wait_ms(headers)?,
            min_ctx: parse_x_mayhem_min_ctx(headers)?,
            quant: parse_x_mayhem_quant(headers)?,
            failover_overrides: parse_x_mayhem_failover_overrides(headers)?,
            access_token: None,
        })
    }
}

fn parse_x_mayhem_max_wait_ms(headers: &HeaderMap) -> Result<u64, ApiError> {
    let Some(value) = headers.get(X_MAYHEM_MAX_WAIT_MS_HEADER) else {
        return Ok(DEFAULT_ROUTE_MAX_WAIT_MS);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Max-Wait-Ms must be an ASCII integer millisecond value",
            Some("X-Mayhem-Max-Wait-Ms"),
        )
    })?;
    let parsed = value.trim().parse::<u64>().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Max-Wait-Ms must be an integer millisecond value",
            Some("X-Mayhem-Max-Wait-Ms"),
        )
    })?;
    if parsed > MAX_ROUTE_MAX_WAIT_MS {
        return Err(ApiError::bad_request(
            "X-Mayhem-Max-Wait-Ms must be <= 60000",
            Some("X-Mayhem-Max-Wait-Ms"),
        ));
    }
    Ok(parsed)
}

fn parse_x_mayhem_max_price_mu(headers: &HeaderMap) -> Result<Option<u64>, ApiError> {
    let Some(value) = headers.get(X_MAYHEM_MAX_PRICE_MU_HEADER) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Max-Price-Mu must be an ASCII positive integer µUSD value",
            Some("X-Mayhem-Max-Price-Mu"),
        )
    })?;
    let parsed = value.trim().parse::<u64>().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Max-Price-Mu must be a positive integer µUSD value",
            Some("X-Mayhem-Max-Price-Mu"),
        )
    })?;
    if parsed == 0 {
        return Err(ApiError::bad_request(
            "X-Mayhem-Max-Price-Mu must be greater than 0",
            Some("X-Mayhem-Max-Price-Mu"),
        ));
    }
    Ok(Some(parsed))
}

fn parse_x_mayhem_min_ctx(headers: &HeaderMap) -> Result<Option<u32>, ApiError> {
    let Some(value) = headers.get(X_MAYHEM_MIN_CTX_HEADER) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Min-Ctx must be an ASCII positive integer token count",
            Some("X-Mayhem-Min-Ctx"),
        )
    })?;
    let parsed = value.trim().parse::<u64>().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Min-Ctx must be a positive integer token count",
            Some("X-Mayhem-Min-Ctx"),
        )
    })?;
    let parsed = u32::try_from(parsed).map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Min-Ctx must fit in a u32 token count",
            Some("X-Mayhem-Min-Ctx"),
        )
    })?;
    if parsed == 0 {
        return Err(ApiError::bad_request(
            "X-Mayhem-Min-Ctx must be greater than 0",
            Some("X-Mayhem-Min-Ctx"),
        ));
    }
    Ok(Some(parsed))
}

fn parse_x_mayhem_failover_overrides(
    headers: &HeaderMap,
) -> Result<GatewayFailoverPolicyConfig, ApiError> {
    reject_admin_controlled_timeout_header(
        headers,
        X_MAYHEM_OPEN_TIMEOUT_MS_HEADER,
        "X-Mayhem-Open-Timeout-Ms",
    )?;
    reject_admin_controlled_timeout_header(
        headers,
        X_MAYHEM_TTFT_TIMEOUT_MS_HEADER,
        "X-Mayhem-TTFT-Timeout-Ms",
    )?;
    reject_admin_controlled_timeout_header(
        headers,
        X_MAYHEM_STALL_TIMEOUT_MS_HEADER,
        "X-Mayhem-Stall-Timeout-Ms",
    )?;
    Ok(GatewayFailoverPolicyConfig {
        open_timeout_ms: None,
        ttft_timeout_ms: None,
        stall_timeout_ms: None,
        min_tok_s: parse_nonnegative_float_header(
            headers,
            X_MAYHEM_MIN_TOK_S_HEADER,
            "X-Mayhem-Min-Tok-S",
        )?,
    })
}

fn reject_admin_controlled_timeout_header(
    headers: &HeaderMap,
    key: &'static str,
    display: &'static str,
) -> Result<(), ApiError> {
    if headers.contains_key(key) {
        return Err(ApiError::bad_request(
            format!("{display} is admin catalog controlled; set model failover policy instead"),
            Some(display),
        ));
    }
    Ok(())
}

fn parse_nonnegative_float_header(
    headers: &HeaderMap,
    key: &'static str,
    display: &'static str,
) -> Result<Option<f64>, ApiError> {
    let Some(value) = headers.get(key) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            format!("{display} must be an ASCII non-negative number"),
            Some(display),
        )
    })?;
    let parsed = value.trim().parse::<f64>().map_err(|_| {
        ApiError::bad_request(
            format!("{display} must be a non-negative number"),
            Some(display),
        )
    })?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(ApiError::bad_request(
            format!("{display} must be a finite non-negative number"),
            Some(display),
        ));
    }
    Ok(Some(parsed))
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

fn parse_x_mayhem_quant(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get(X_MAYHEM_QUANT_HEADER) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            "X-Mayhem-Quant must be an ASCII quant bucket",
            Some("X-Mayhem-Quant"),
        )
    })?;
    normalize_quant_bucket(value)
        .map(Some)
        .map_err(|message| ApiError::bad_request(message, Some("X-Mayhem-Quant")))
}

fn normalize_quant_bucket(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
    if normalized.is_empty() {
        return Err("X-Mayhem-Quant must not be empty".to_owned());
    }
    if matches!(
        normalized.as_str(),
        "unknown" | "fp32" | "fp16" | "bf16" | "fp8" | "nvfp4" | "int8" | "int4"
    ) {
        return Ok(normalized);
    }
    let inferred = quant_bucket_from_descriptor(&normalized);
    if inferred != DEFAULT_QUANT_BUCKET {
        return Ok(inferred);
    }
    Err(
        "X-Mayhem-Quant must be one of fp32, fp16, bf16, fp8, nvfp4, int8, int4, unknown"
            .to_owned(),
    )
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
            quality: None,
        }
    }
}

impl GatewaySessionBackend for NoProviderSessionBackend {
    fn name(&self) -> &str {
        "no-live-provider"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async {
            Err(GatewaySessionError::new(
                "no provider available: production gateway requires an active provider joined to an admin-created room",
            ))
        })
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
            Ok(GatewaySessionResult::local_openai_shape(dev_chat_output(
                model, request,
            )))
        })
    }
}

impl ScBridgeGatewaySessionConfig {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
            open_timeout: Duration::from_millis(DEFAULT_OPEN_TIMEOUT_MILLIS),
            ttft_timeout: Duration::from_millis(DEFAULT_TTFT_BASE_TIMEOUT_MILLIS),
            frame_timeout: Duration::from_millis(DEFAULT_STALL_TIMEOUT_MILLIS),
            min_tok_s: None,
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

    fn bridge_stream_config(&self) -> Option<ScBridgeGatewaySessionConfig> {
        Some(self.config.clone())
    }

    fn hedge_probe<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayHedgeProbeFuture<'a> {
        Box::pin(async move { self.hedge_probe_over_bridge(invocation).await })
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move { self.run_chat_over_bridge(model, request, invocation).await })
    }

    fn run_embedding<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a EmbeddingRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayEmbeddingFuture<'a> {
        Box::pin(async move {
            self.run_embedding_over_bridge(model, request, invocation)
                .await
        })
    }

    fn run_image_generation<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ImageGenerationRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayImageGenerationFuture<'a> {
        Box::pin(async move {
            self.run_image_generation_over_bridge(model, request, invocation)
                .await
        })
    }

    fn run_audio_speech<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a AudioSpeechRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayAudioSpeechFuture<'a> {
        Box::pin(async move {
            self.run_audio_speech_over_bridge(model, request, invocation)
                .await
        })
    }

    fn run_audio_transcription<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a AudioTranscriptionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayAudioTranscriptionFuture<'a> {
        Box::pin(async move {
            self.run_audio_transcription_over_bridge(model, request, invocation)
                .await
        })
    }
}

impl ScBridgeGatewaySessionBackend {
    async fn hedge_probe_over_bridge(
        &self,
        invocation: &GatewaySessionInvocation,
    ) -> Result<GatewayHedgeProbeResult, GatewaySessionError> {
        let provider = invocation
            .provider_pubkey
            .as_deref()
            .ok_or_else(|| GatewaySessionError::new("hedge probe has no canonical provider"))?;
        let started = Instant::now();
        let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(
            &self.config.url,
            self.config.token.clone(),
        )?)
        .await?;
        bridge
            .session_subscribe([invocation.session_id.as_str()])
            .await?;
        bridge
            .peer_connect(provider, invocation.failover.open_timeout())
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "hedge peer connect to provider {} for session {} failed: {err}",
                    provider, invocation.session_id
                ))
            })?;
        let opened = bridge
            .session_open(provider, &invocation.session_id)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "hedge session open {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        if opened.get("direct").and_then(Value::as_bool) != Some(true)
            || opened.get("relayed").and_then(Value::as_bool) == Some(true)
        {
            return Err(GatewaySessionError::retryable(format!(
                "hedge session {} was not direct/non-relayed",
                invocation.session_id
            )));
        }
        let _ = bridge
            .session_send(
                provider,
                &invocation.session_id,
                json!({
                    "t": "s.close",
                    "v": 1,
                    "session_id": invocation.session_id,
                    "reason": "hedge_probe_pre_spend",
                }),
            )
            .await;
        Ok(GatewayHedgeProbeResult {
            provider: provider.to_owned(),
            ttft_ms: duration_millis_u64(started.elapsed()).max(1),
        })
    }

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
            .peer_connect(provider, invocation.failover.open_timeout())
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
            "session_id": invocation.session_id.clone(),
            "rail": invocation.rail.clone(),
            "user": invocation.user_pubkey.clone(),
            "enclave_id": invocation.enclave_id.clone(),
            "price_ver": invocation.price_ver,
            "at": invocation.opened_at,
            "served_ctx": invocation.served_ctx,
            "ctx_bracket": invocation.ctx_bracket.clone(),
            "ctx_bracket_table_ver": invocation.ctx_bracket_table_ver,
            "rules_ver": invocation.rules_ver,
            "voucher": invocation.spend_voucher.clone(),
            "att_nonce": att_nonce,
            "ts": now,
            "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
            "sig": invocation.spend_voucher.user_sig.clone(),
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
            invocation.failover.open_timeout(),
            &["s.accept", "s.reject"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
            return Err(provider_reject_session_error(
                &accept,
                &invocation.session_id,
            ));
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
        let request_body = direct_session_request_body(request);
        send_direct_session_request_frames(
            &mut bridge,
            provider,
            &invocation.session_id,
            &request_id,
            &request_body,
        )
        .await?;

        let collected = match collect_direct_session_output(
            &mut bridge,
            &invocation.session_id,
            &request_id,
            invocation,
            request,
            provider,
            model,
            &accept_info.enclave_pubkey,
        )
        .await
        {
            Ok(collected) => collected,
            Err(err) => {
                if let Some(partial) = err.partial.as_ref() {
                    let receipt_ack = direct_session_partial_receipt_ack(
                        request, invocation, partial, provider, model,
                    )?;
                    let _ = send_direct_session_frame_with_peer_reconnect(
                        &mut bridge,
                        provider,
                        &invocation.session_id,
                        json!({
                            "t": "s.receipt_ack",
                            "v": 1,
                            "session_id": receipt_ack.session_id,
                            "seq": receipt_ack.seq,
                            "user_sig": receipt_ack.user_sig,
                            "reason": partial.reason.as_str(),
                        }),
                        invocation.failover.open_timeout(),
                        "sending partial s.receipt_ack",
                    )
                    .await;
                    let _ = bridge
                        .session_send(
                            provider,
                            &invocation.session_id,
                            json!({
                                "t": "s.close",
                                "v": 1,
                                "session_id": invocation.session_id,
                                "reason": "redispatch",
                            }),
                        )
                        .await;
                }
                return Err(err.into_retryable());
            }
        };
        let receipt_ack = direct_session_receipt_ack(
            request,
            &collected.output,
            invocation,
            &collected.provider_receipt,
            provider,
            model,
        )?;
        send_direct_session_frame_with_peer_reconnect(
            &mut bridge,
            provider,
            &invocation.session_id,
            json!({
                "t": "s.receipt_ack",
                "v": 1,
                "session_id": receipt_ack.session_id,
                "seq": receipt_ack.seq,
                "user_sig": receipt_ack.user_sig,
            }),
            invocation.failover.open_timeout(),
            "sending s.receipt_ack",
        )
        .await?;
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
            quality: collected.quality,
        })
    }

    async fn run_embedding_over_bridge(
        &self,
        model: &GatewayModel,
        request: &EmbeddingRequest,
        invocation: &GatewaySessionInvocation,
    ) -> Result<GatewayEmbeddingResult, GatewaySessionError> {
        let provider = invocation
            .provider_pubkey
            .as_deref()
            .ok_or_else(|| GatewaySessionError::new("model has no canonical provider route"))?;
        let inputs =
            embedding_input_texts_from_value(&request.input).map_err(GatewaySessionError::new)?;
        let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(
            &self.config.url,
            self.config.token.clone(),
        )?)
        .await?;
        bridge
            .session_subscribe([invocation.session_id.as_str()])
            .await?;
        bridge
            .peer_connect(provider, invocation.failover.open_timeout())
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "connecting direct peer {} for embedding session {} failed: {err}",
                    provider, invocation.session_id
                ))
            })?;
        let opened = bridge
            .session_open(provider, &invocation.session_id)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "opening direct embedding session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        if opened.get("direct").and_then(Value::as_bool) != Some(true)
            || opened.get("relayed").and_then(Value::as_bool) == Some(true)
        {
            return Err(GatewaySessionError::retryable(format!(
                "embedding session {} was not opened as a direct non-relayed channel",
                invocation.session_id
            )));
        }

        let now = now_millis_u64();
        let att_nonce = blake3_hex(format!("att:{}:{now}", invocation.session_id).as_bytes());
        let open_frame = json!({
            "t": "s.open",
            "v": 1,
            "contract_version": invocation.contract_version,
            "session_id": invocation.session_id.clone(),
            "rail": invocation.rail.clone(),
            "user": invocation.user_pubkey.clone(),
            "enclave_id": invocation.enclave_id.clone(),
            "price_ver": invocation.price_ver,
            "at": invocation.opened_at,
            "served_ctx": invocation.served_ctx,
            "ctx_bracket": invocation.ctx_bracket.clone(),
            "ctx_bracket_table_ver": invocation.ctx_bracket_table_ver,
            "rules_ver": invocation.rules_ver,
            "voucher": invocation.spend_voucher.clone(),
            "att_nonce": att_nonce,
            "ts": now,
            "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
            "sig": invocation.spend_voucher.user_sig.clone(),
        });
        let open_head = session_frame_head(&open_frame)
            .map_err(|err| GatewaySessionError::new(format!("s.open hash failed: {err}")))?;
        bridge
            .session_send(provider, &invocation.session_id, open_frame)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.open for embedding session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;

        let accept = next_session_frame(
            &mut bridge,
            &invocation.session_id,
            invocation.failover.open_timeout(),
            &["s.accept", "s.reject"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
            return Err(provider_reject_session_error(
                &accept,
                &invocation.session_id,
            ));
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
                embedding_prompt_text(&inputs)
            )
            .as_bytes(),
        )
        .chars()
        .take(32)
        .collect::<String>();
        let request_body = direct_session_embedding_request_body(request);
        send_direct_session_request_frames(
            &mut bridge,
            provider,
            &invocation.session_id,
            &request_id,
            &request_body,
        )
        .await?;

        let collected = collect_direct_session_embedding_output(
            &mut bridge,
            &invocation.session_id,
            &request_id,
            invocation.failover,
            &inputs,
            &accept_info.enclave_pubkey,
        )
        .await?;
        let receipt_ack = direct_session_embedding_receipt_ack(
            &inputs,
            &collected.output,
            invocation,
            &collected.provider_receipt,
            provider,
            model,
        )?;
        send_direct_session_frame_with_peer_reconnect(
            &mut bridge,
            provider,
            &invocation.session_id,
            json!({
                "t": "s.receipt_ack",
                "v": 1,
                "session_id": receipt_ack.session_id,
                "seq": receipt_ack.seq,
                "user_sig": receipt_ack.user_sig,
            }),
            invocation.failover.open_timeout(),
            "sending embedding s.receipt_ack",
        )
        .await?;
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

        Ok(GatewayEmbeddingResult {
            output: collected.output,
            backend: self.name().to_owned(),
            direct_session: true,
            provider_receipt: Some(collected.provider_receipt),
            quality: collected.quality,
        })
    }

    async fn run_image_generation_over_bridge(
        &self,
        model: &GatewayModel,
        request: &ImageGenerationRequest,
        invocation: &GatewaySessionInvocation,
    ) -> Result<GatewayImageGenerationResult, GatewaySessionError> {
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
            .peer_connect(provider, invocation.failover.open_timeout())
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "connecting direct peer {} for image session {} failed: {err}",
                    provider, invocation.session_id
                ))
            })?;
        let opened = bridge
            .session_open(provider, &invocation.session_id)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "opening direct image session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        if opened.get("direct").and_then(Value::as_bool) != Some(true)
            || opened.get("relayed").and_then(Value::as_bool) == Some(true)
        {
            return Err(GatewaySessionError::retryable(format!(
                "image session {} was not opened as a direct non-relayed channel",
                invocation.session_id
            )));
        }

        let now = now_millis_u64();
        let att_nonce = blake3_hex(format!("att:{}:{now}", invocation.session_id).as_bytes());
        let open_frame = json!({
            "t": "s.open",
            "v": 1,
            "contract_version": invocation.contract_version,
            "session_id": invocation.session_id.clone(),
            "rail": invocation.rail.clone(),
            "user": invocation.user_pubkey.clone(),
            "enclave_id": invocation.enclave_id.clone(),
            "price_ver": invocation.price_ver,
            "at": invocation.opened_at,
            "served_ctx": invocation.served_ctx,
            "ctx_bracket": invocation.ctx_bracket.clone(),
            "ctx_bracket_table_ver": invocation.ctx_bracket_table_ver,
            "rules_ver": invocation.rules_ver,
            "voucher": invocation.spend_voucher.clone(),
            "att_nonce": att_nonce,
            "ts": now,
            "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
            "sig": invocation.spend_voucher.user_sig.clone(),
        });
        let open_head = session_frame_head(&open_frame)
            .map_err(|err| GatewaySessionError::new(format!("s.open hash failed: {err}")))?;
        bridge
            .session_send(provider, &invocation.session_id, open_frame)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.open for image session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;

        let accept = next_session_frame(
            &mut bridge,
            &invocation.session_id,
            invocation.failover.open_timeout(),
            &["s.accept", "s.reject"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
            return Err(provider_reject_session_error(
                &accept,
                &invocation.session_id,
            ));
        }
        let accept_info = validate_direct_session_accept(
            &accept,
            invocation,
            &open_head,
            &att_nonce,
            now / 1000,
        )?;

        let request_body = direct_session_image_generation_request_body(request);
        let request_id = blake3_hex(
            format!(
                "rid:{}:{}",
                invocation.session_id,
                stable_json_value(&request_body)
            )
            .as_bytes(),
        )
        .chars()
        .take(32)
        .collect::<String>();
        send_direct_session_request_frames(
            &mut bridge,
            provider,
            &invocation.session_id,
            &request_id,
            &request_body,
        )
        .await?;

        let collected = collect_direct_session_image_generation_output(
            &mut bridge,
            &invocation.session_id,
            &request_id,
            invocation.failover,
            request,
            &accept_info.enclave_pubkey,
        )
        .await?;
        let receipt_ack = direct_session_image_generation_receipt_ack(
            request,
            &collected.output,
            invocation,
            &collected.provider_receipt,
            provider,
            model,
        )?;
        send_direct_session_frame_with_peer_reconnect(
            &mut bridge,
            provider,
            &invocation.session_id,
            json!({
                "t": "s.receipt_ack",
                "v": 1,
                "session_id": receipt_ack.session_id,
                "seq": receipt_ack.seq,
                "user_sig": receipt_ack.user_sig,
            }),
            invocation.failover.open_timeout(),
            "sending image s.receipt_ack",
        )
        .await?;
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

        Ok(GatewayImageGenerationResult {
            output: collected.output,
            backend: self.name().to_owned(),
            direct_session: true,
            provider_receipt: Some(collected.provider_receipt),
            quality: collected.quality,
        })
    }

    async fn run_audio_speech_over_bridge(
        &self,
        model: &GatewayModel,
        request: &AudioSpeechRequest,
        invocation: &GatewaySessionInvocation,
    ) -> Result<GatewayAudioSpeechResult, GatewaySessionError> {
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
            .peer_connect(provider, invocation.failover.open_timeout())
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "connecting direct peer {} for audio speech session {} failed: {err}",
                    provider, invocation.session_id
                ))
            })?;
        let opened = bridge
            .session_open(provider, &invocation.session_id)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "opening direct audio speech session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        if opened.get("direct").and_then(Value::as_bool) != Some(true)
            || opened.get("relayed").and_then(Value::as_bool) == Some(true)
        {
            return Err(GatewaySessionError::retryable(format!(
                "audio speech session {} was not opened as a direct non-relayed channel",
                invocation.session_id
            )));
        }

        let now = now_millis_u64();
        let att_nonce = blake3_hex(format!("att:{}:{now}", invocation.session_id).as_bytes());
        let open_frame = json!({
            "t": "s.open",
            "v": 1,
            "contract_version": invocation.contract_version,
            "session_id": invocation.session_id.clone(),
            "rail": invocation.rail.clone(),
            "user": invocation.user_pubkey.clone(),
            "enclave_id": invocation.enclave_id.clone(),
            "price_ver": invocation.price_ver,
            "at": invocation.opened_at,
            "served_ctx": invocation.served_ctx,
            "ctx_bracket": invocation.ctx_bracket.clone(),
            "ctx_bracket_table_ver": invocation.ctx_bracket_table_ver,
            "rules_ver": invocation.rules_ver,
            "voucher": invocation.spend_voucher.clone(),
            "att_nonce": att_nonce,
            "ts": now,
            "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
            "sig": invocation.spend_voucher.user_sig.clone(),
        });
        let open_head = session_frame_head(&open_frame)
            .map_err(|err| GatewaySessionError::new(format!("s.open hash failed: {err}")))?;
        bridge
            .session_send(provider, &invocation.session_id, open_frame)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.open for audio speech session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;

        let accept = next_session_frame(
            &mut bridge,
            &invocation.session_id,
            invocation.failover.open_timeout(),
            &["s.accept", "s.reject"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
            return Err(provider_reject_session_error(
                &accept,
                &invocation.session_id,
            ));
        }
        let accept_info = validate_direct_session_accept(
            &accept,
            invocation,
            &open_head,
            &att_nonce,
            now / 1000,
        )?;

        let request_body = direct_session_audio_speech_request_body(request);
        let request_id = request_id_for_body(&invocation.session_id, &request_body);
        send_direct_session_request_frames(
            &mut bridge,
            provider,
            &invocation.session_id,
            &request_id,
            &request_body,
        )
        .await?;

        let collected = collect_direct_session_audio_speech_output(
            &mut bridge,
            &invocation.session_id,
            &request_id,
            invocation.failover,
            request,
            &accept_info.enclave_pubkey,
        )
        .await?;
        let receipt_ack = direct_session_audio_speech_receipt_ack(
            request,
            &collected.output,
            invocation,
            &collected.provider_receipt,
            provider,
            model,
        )?;
        send_direct_session_frame_with_peer_reconnect(
            &mut bridge,
            provider,
            &invocation.session_id,
            json!({
                "t": "s.receipt_ack",
                "v": 1,
                "session_id": receipt_ack.session_id,
                "seq": receipt_ack.seq,
                "user_sig": receipt_ack.user_sig,
            }),
            invocation.failover.open_timeout(),
            "sending audio speech s.receipt_ack",
        )
        .await?;
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

        Ok(GatewayAudioSpeechResult {
            output: collected.output,
            backend: self.name().to_owned(),
            direct_session: true,
            provider_receipt: Some(collected.provider_receipt),
            quality: collected.quality,
        })
    }

    async fn run_audio_transcription_over_bridge(
        &self,
        model: &GatewayModel,
        request: &AudioTranscriptionRequest,
        invocation: &GatewaySessionInvocation,
    ) -> Result<GatewayAudioTranscriptionResult, GatewaySessionError> {
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
            .peer_connect(provider, invocation.failover.open_timeout())
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "connecting direct peer {} for audio transcription session {} failed: {err}",
                    provider, invocation.session_id
                ))
            })?;
        let opened = bridge
            .session_open(provider, &invocation.session_id)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "opening direct audio transcription session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        if opened.get("direct").and_then(Value::as_bool) != Some(true)
            || opened.get("relayed").and_then(Value::as_bool) == Some(true)
        {
            return Err(GatewaySessionError::retryable(format!(
                "audio transcription session {} was not opened as a direct non-relayed channel",
                invocation.session_id
            )));
        }

        let now = now_millis_u64();
        let att_nonce = blake3_hex(format!("att:{}:{now}", invocation.session_id).as_bytes());
        let open_frame = json!({
            "t": "s.open",
            "v": 1,
            "contract_version": invocation.contract_version,
            "session_id": invocation.session_id.clone(),
            "rail": invocation.rail.clone(),
            "user": invocation.user_pubkey.clone(),
            "enclave_id": invocation.enclave_id.clone(),
            "price_ver": invocation.price_ver,
            "at": invocation.opened_at,
            "served_ctx": invocation.served_ctx,
            "ctx_bracket": invocation.ctx_bracket.clone(),
            "ctx_bracket_table_ver": invocation.ctx_bracket_table_ver,
            "rules_ver": invocation.rules_ver,
            "voucher": invocation.spend_voucher.clone(),
            "att_nonce": att_nonce,
            "ts": now,
            "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
            "sig": invocation.spend_voucher.user_sig.clone(),
        });
        let open_head = session_frame_head(&open_frame)
            .map_err(|err| GatewaySessionError::new(format!("s.open hash failed: {err}")))?;
        bridge
            .session_send(provider, &invocation.session_id, open_frame)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.open for audio transcription session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;

        let accept = next_session_frame(
            &mut bridge,
            &invocation.session_id,
            invocation.failover.open_timeout(),
            &["s.accept", "s.reject"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
            return Err(provider_reject_session_error(
                &accept,
                &invocation.session_id,
            ));
        }
        let accept_info = validate_direct_session_accept(
            &accept,
            invocation,
            &open_head,
            &att_nonce,
            now / 1000,
        )?;

        let request_body = direct_session_audio_transcription_request_body(request);
        let request_id = request_id_for_body(&invocation.session_id, &request_body);
        send_direct_session_request_frames(
            &mut bridge,
            provider,
            &invocation.session_id,
            &request_id,
            &request_body,
        )
        .await?;

        let collected = collect_direct_session_audio_transcription_output(
            &mut bridge,
            &invocation.session_id,
            &request_id,
            invocation.failover,
            request,
            &accept_info.enclave_pubkey,
        )
        .await?;
        let receipt_ack = direct_session_audio_transcription_receipt_ack(
            request,
            &collected.output,
            invocation,
            &collected.provider_receipt,
            provider,
            model,
        )?;
        send_direct_session_frame_with_peer_reconnect(
            &mut bridge,
            provider,
            &invocation.session_id,
            json!({
                "t": "s.receipt_ack",
                "v": 1,
                "session_id": receipt_ack.session_id,
                "seq": receipt_ack.seq,
                "user_sig": receipt_ack.user_sig,
            }),
            invocation.failover.open_timeout(),
            "sending audio transcription s.receipt_ack",
        )
        .await?;
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

        Ok(GatewayAudioTranscriptionResult {
            output: collected.output,
            backend: self.name().to_owned(),
            direct_session: true,
            provider_receipt: Some(collected.provider_receipt),
            quality: collected.quality,
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

fn provider_reject_session_error(frame: &Value, session_id: &str) -> GatewaySessionError {
    let code = frame
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    let reason = frame
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("no reason provided");
    let message = format!("provider rejected session {session_id} with {code}: {reason}");
    if clean_provider_reject_code(code) {
        GatewaySessionError::clean_refusal_with_code(message, Some(code))
    } else {
        GatewaySessionError::retryable(message)
    }
}

fn clean_provider_reject_code(code: &str) -> bool {
    matches!(
        code,
        "CAPACITY" | "BUSY" | "RATE" | "QUOTA" | "PRICE_FLOOR" | "DRAINING"
    )
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
            Err(BridgeError::Closed) => {
                return Err(GatewaySessionError::retryable(format!(
                    "SC-Bridge closed while waiting for {} on session {}",
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
    set_optional_json(
        &mut body,
        "user",
        request.user.as_ref().map(|value| json!(value)),
    );
    if !request.metadata.is_empty() {
        body["metadata"] = json!(&request.metadata);
    }
    body
}

fn direct_session_embedding_request_body(request: &EmbeddingRequest) -> Value {
    let mut body = json!({
        "kind": "embedding",
        "input": &request.input,
        "encoding_format": request.encoding_format.as_deref().unwrap_or("float"),
    });
    set_optional_json(
        &mut body,
        "dimensions",
        request.dimensions.map(|value| json!(value)),
    );
    body
}

fn direct_session_image_generation_request_body(request: &ImageGenerationRequest) -> Value {
    let mut body = json!({
        "kind": "image_generation",
        "prompt": &request.prompt,
        "n": image_generation_count(request),
        "size": request.size.as_deref().unwrap_or("512x512"),
        "steps": image_generation_steps(request),
        "cfg_scale": image_generation_cfg_scale(request),
        "response_format": request.response_format.as_deref().unwrap_or("b64_json"),
    });
    set_optional_json(&mut body, "seed", request.seed.map(|value| json!(value)));
    body
}

fn direct_session_audio_speech_request_body(request: &AudioSpeechRequest) -> Value {
    let mut body = json!({
        "kind": "audio_speech",
        "input": &request.input,
        "response_format": audio_speech_response_format(request),
    });
    set_optional_json(
        &mut body,
        "voice",
        request.voice.as_ref().map(|value| json!(value)),
    );
    set_optional_json(&mut body, "speed", request.speed.map(|value| json!(value)));
    body
}

fn direct_session_audio_transcription_request_body(request: &AudioTranscriptionRequest) -> Value {
    let mut body = json!({
        "kind": "audio_transcription",
        "audio": {
            "encoding": "hex",
            "content_type": request.content_type.as_deref().unwrap_or("audio/wav"),
            "filename": request.filename.as_deref().unwrap_or("audio.wav"),
            "data": hex_encode(&request.audio),
        },
        "audio_seconds": audio_transcription_seconds(request),
        "response_format": request.response_format.as_deref().unwrap_or("json"),
    });
    set_optional_json(
        &mut body,
        "language",
        request.language.as_ref().map(|value| json!(value)),
    );
    set_optional_json(
        &mut body,
        "prompt",
        request.prompt.as_ref().map(|value| json!(value)),
    );
    body
}

fn request_id_for_body(session_id: &str, body: &Value) -> String {
    blake3_hex(format!("rid:{session_id}:{}", stable_json_value(body)).as_bytes())
        .chars()
        .take(32)
        .collect()
}

async fn send_direct_session_request_frames(
    bridge: &mut ScBridgeClient,
    provider: &str,
    session_id: &str,
    request_id: &str,
    body: &Value,
) -> Result<(), GatewaySessionError> {
    let max_frame_bytes = direct_session_max_frame_bytes();
    for frame in direct_session_request_frames(request_id, body, max_frame_bytes)? {
        bridge
            .session_send(provider, session_id, frame)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.req for session {session_id} to provider {provider} failed: {err}"
                ))
            })?;
    }
    Ok(())
}

fn direct_session_request_frames(
    request_id: &str,
    body: &Value,
    max_frame_bytes: usize,
) -> Result<Vec<Value>, GatewaySessionError> {
    let direct = json!({
        "t": "s.req",
        "rid": request_id,
        "body": body,
    });
    if session_frame_json_len(&direct)? <= max_frame_bytes {
        return Ok(vec![direct]);
    }

    let chunk_size = direct_session_payload_chunk_bytes(max_frame_bytes);
    let (manifest, chunks) = chunk_json_payload(body, chunk_size)
        .map_err(|err| GatewaySessionError::new(format!("chunking s.req payload failed: {err}")))?;
    let payload_id = manifest.blake3.clone();
    let mut frames = Vec::with_capacity(chunks.len().saturating_add(1));
    for chunk in chunks {
        frames.push(json!({
            "t": "s.req_chunk",
            "v": 1,
            "rid": request_id,
            "payload_id": payload_id,
            "chunk": chunk,
        }));
    }
    frames.push(json!({
        "t": "s.req",
        "rid": request_id,
        "body_ref": manifest,
    }));

    for frame in &frames {
        let len = session_frame_json_len(frame)?;
        if len > max_frame_bytes {
            return Err(GatewaySessionError::new(format!(
                "chunked s.req frame {} bytes exceeds session max {max_frame_bytes} bytes",
                len
            )));
        }
    }
    Ok(frames)
}

fn direct_session_max_frame_bytes() -> usize {
    std::env::var("MAYHEM_SESSION_MAX_FRAME_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 8 * 1024)
        .unwrap_or(DEFAULT_SESSION_MAX_FRAME_BYTES)
}

fn direct_session_payload_chunk_bytes(max_frame_bytes: usize) -> usize {
    let safe_raw = max_frame_bytes.saturating_sub(4096) / 3;
    DEFAULT_SESSION_PAYLOAD_CHUNK_BYTES.min(safe_raw.max(1024))
}

fn session_frame_json_len(frame: &Value) -> Result<usize, GatewaySessionError> {
    serde_json::to_vec(frame)
        .map(|bytes| bytes.len())
        .map_err(|err| GatewaySessionError::new(format!("serializing session frame failed: {err}")))
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
    min_tok_s: Option<f64>,
}

impl DirectSessionWatchdog {
    fn new(
        started_at_millis: u64,
        ttft_timeout: Duration,
        idle_timeout: Duration,
        overall_timeout: Option<Duration>,
        min_tok_s: Option<f64>,
    ) -> Self {
        Self {
            started_at_millis,
            first_delta_at_millis: None,
            last_delta_at_millis: None,
            ttft_timeout_millis: duration_millis_u64(ttft_timeout),
            idle_timeout_millis: duration_millis_u64(idle_timeout),
            overall_timeout_millis: overall_timeout.map(duration_millis_u64),
            min_tok_s: min_tok_s.filter(|value| value.is_finite() && *value > 0.0),
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

    fn throughput_floor_violation(&self, output_tokens: u64, now_millis: u64) -> Option<f64> {
        let min_tok_s = self.min_tok_s?;
        self.first_delta_at_millis?;
        if output_tokens == 0 {
            return None;
        }
        let first_delta_at_millis = self.first_delta_at_millis?;
        let elapsed_millis = now_millis.saturating_sub(first_delta_at_millis).max(1);
        if elapsed_millis < DEFAULT_THROUGHPUT_FLOOR_SAMPLE_MILLIS {
            return None;
        }
        let tok_s = output_tokens as f64 * 1000.0 / elapsed_millis as f64;
        (tok_s < min_tok_s).then_some(tok_s)
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

fn direct_session_throughput_floor_error(session_id: &str, tok_s: f64, min_tok_s: f64) -> String {
    format!(
        "provider throughput {tok_s:.2} tok/s stayed below floor {min_tok_s:.2} tok/s on session {session_id}"
    )
}

fn streamed_output_token_count(content: &str, token_ids: &[i32]) -> u64 {
    if token_ids.is_empty() {
        rough_tokens(content)
    } else {
        u64::try_from(token_ids.len()).unwrap_or(u64::MAX)
    }
}

fn generated_tokens_per_second(
    output_tokens: u64,
    first_delta_at_millis: u64,
    completed_at_millis: u64,
) -> Option<f64> {
    if output_tokens == 0 {
        return None;
    }
    let elapsed_millis = completed_at_millis
        .saturating_sub(first_delta_at_millis)
        .max(1);
    let tok_s = output_tokens as f64 * 1000.0 / elapsed_millis as f64;
    tok_s.is_finite().then_some(tok_s)
}

fn units_per_second(units: u64, started_at_millis: u64, completed_at_millis: u64) -> Option<f64> {
    if units == 0 {
        return None;
    }
    let elapsed_millis = completed_at_millis.saturating_sub(started_at_millis).max(1);
    let per_second = units as f64 * 1000.0 / elapsed_millis as f64;
    per_second.is_finite().then_some(per_second)
}

fn quality_from_session_delta(frame: &Value) -> Option<GatewaySessionQuality> {
    let quality = frame.get("quality")?;
    let ttft_ms = quality
        .get("ttft_ms")
        .and_then(Value::as_u64)
        .or_else(|| quality.get("compute_ms").and_then(Value::as_u64))?
        .max(1);
    let tok_s = quality
        .get("tok_s")
        .or_else(|| quality.get("throughput"))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);
    Some(GatewaySessionQuality { ttft_ms, tok_s })
}

#[derive(Debug, Default)]
struct SessionDeltaPayloadChunks {
    chunks: BTreeMap<String, Vec<PayloadChunk>>,
}

fn session_delta_payload_key(request_id: &str, field: &str, payload_id: &str) -> String {
    format!("{request_id}:{field}:{payload_id}")
}

fn valid_session_delta_ref_field(field: &str) -> bool {
    matches!(field, "tool" | "embeddings" | "token_ids")
}

fn collect_session_delta_chunk(
    frame: &Value,
    pending: &mut SessionDeltaPayloadChunks,
) -> Result<(), GatewaySessionError> {
    let request_id = frame
        .get("rid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewaySessionError::new("s.delta_chunk missing rid"))?;
    let field = frame
        .get("field")
        .and_then(Value::as_str)
        .filter(|field| valid_session_delta_ref_field(field))
        .ok_or_else(|| GatewaySessionError::new("s.delta_chunk has unsupported field"))?;
    let payload_id = frame
        .get("payload_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewaySessionError::new("s.delta_chunk missing payload_id"))?;
    let chunk: PayloadChunk = serde_json::from_value(
        frame
            .get("chunk")
            .cloned()
            .ok_or_else(|| GatewaySessionError::new("s.delta_chunk missing chunk"))?,
    )
    .map_err(|err| GatewaySessionError::new(format!("invalid s.delta_chunk chunk: {err}")))?;
    pending
        .chunks
        .entry(session_delta_payload_key(request_id, field, payload_id))
        .or_default()
        .push(chunk);
    Ok(())
}

fn resolve_session_delta_ref_field(
    frame: &Value,
    field: &'static str,
    pending: &mut SessionDeltaPayloadChunks,
) -> Result<Option<Value>, GatewaySessionError> {
    let ref_key = format!("{field}_ref");
    let Some(manifest_value) = frame.get(&ref_key) else {
        return Ok(None);
    };
    let request_id = frame
        .get("rid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewaySessionError::new(format!("s.delta {ref_key} missing rid")))?;
    let manifest: PayloadChunkManifest = serde_json::from_value(manifest_value.clone())
        .map_err(|err| GatewaySessionError::new(format!("invalid s.delta {ref_key}: {err}")))?;
    let key = session_delta_payload_key(request_id, field, &manifest.blake3);
    let chunks = pending.chunks.remove(&key).ok_or_else(|| {
        GatewaySessionError::new(format!(
            "s.delta {ref_key} {} has no received chunks",
            manifest.blake3
        ))
    })?;
    reassemble_json_payload(&manifest, &chunks)
        .map(Some)
        .map_err(|err| {
            GatewaySessionError::new(format!("reassembling s.delta {ref_key} failed: {err}"))
        })
}

async fn collect_direct_session_output(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    request_id: &str,
    invocation: &GatewaySessionInvocation,
    request: &ChatCompletionRequest,
    provider: &str,
    model: &GatewayModel,
    enclave_pubkey: &str,
) -> Result<DirectSessionCollected, GatewaySessionError> {
    let failover = invocation.failover;
    let mut content = String::new();
    let mut tool_call = None;
    let mut finish_reason = None;
    let mut claimed_usage = None;
    let mut final_provider_receipt = None;
    let mut latest_checkpoint_receipt = None;
    let mut pending_checkpoint_receipt: Option<ProviderSignedReceipt> = None;
    let mut token_ids = Vec::new();
    let mut artifact_builders = BTreeMap::new();
    let mut delta_payload_chunks = SessionDeltaPayloadChunks::default();
    let mut provider_quality = None;
    let started_at_millis = now_millis_u64();
    let mut watchdog = DirectSessionWatchdog::new(
        started_at_millis,
        failover.ttft_timeout(),
        failover.stall_timeout(),
        None,
        failover.min_tok_s,
    );

    while finish_reason.is_none() || final_provider_receipt.is_none() {
        let remaining_millis = watchdog
            .next_wait_millis(now_millis_u64())
            .map_err(|kind| direct_session_timeout_error(kind, session_id))?;
        let frame = match next_session_frame(
            bridge,
            session_id,
            Duration::from_millis(remaining_millis),
            &[
                "s.delta",
                "s.delta_chunk",
                "s.receipt",
                "s.error",
                "s.close",
            ],
        )
        .await
        {
            Ok(frame) => frame,
            Err(err) if err.message.starts_with("timed out waiting") => {
                let now = now_millis_u64();
                let timeout = watchdog.timeout_error(session_id, now);
                if let Some(partial) = interrupted_direct_session_partial(
                    &content,
                    tool_call.clone(),
                    latest_checkpoint_receipt.as_ref(),
                    &token_ids,
                    &watchdog,
                    now,
                    "mid_stream_timeout",
                ) {
                    return Err(GatewaySessionError::retryable_partial(
                        timeout.message,
                        partial,
                    ));
                }
                return Err(timeout);
            }
            Err(err) if err.retryable => {
                let now = now_millis_u64();
                return Err(retryable_interrupted_direct_session_error(
                    err,
                    &content,
                    tool_call.clone(),
                    latest_checkpoint_receipt.as_ref(),
                    &token_ids,
                    &watchdog,
                    now,
                    "bridge_closed",
                ));
            }
            Err(err) => return Err(err),
        };
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta_chunk")
                if frame.get("rid").and_then(Value::as_str) == Some(request_id) =>
            {
                watchdog.record_delta(now_millis_u64());
                collect_session_delta_chunk(&frame, &mut delta_payload_chunks)?;
            }
            Some("s.delta") if frame.get("rid").and_then(Value::as_str) == Some(request_id) => {
                let now = now_millis_u64();
                watchdog.record_delta(now);
                if let Some(delta) = frame.get("d").and_then(Value::as_str) {
                    content.push_str(delta);
                }
                if let Some(ids) = token_ids_from_session_delta(&frame) {
                    token_ids = ids;
                } else if let Some(ids) =
                    token_ids_ref_from_session_delta(&frame, &mut delta_payload_chunks)?
                {
                    token_ids = ids;
                } else if let Some(ids) = token_ids_delta_from_session_delta(&frame) {
                    token_ids.extend(ids);
                } else if let Some(token_id) = token_id_from_session_delta(&frame) {
                    token_ids.push(token_id);
                }
                if let Some(receipt) = pending_checkpoint_receipt.take() {
                    if maybe_ack_direct_session_checkpoint_receipt(
                        bridge,
                        provider,
                        session_id,
                        request,
                        invocation,
                        &content,
                        tool_call.clone(),
                        &receipt,
                        &token_ids,
                        &watchdog,
                        now,
                        model,
                    )
                    .await?
                    .is_some()
                    {
                        latest_checkpoint_receipt = Some(receipt);
                    } else {
                        pending_checkpoint_receipt = Some(receipt);
                    }
                }
                if tool_call.is_none() {
                    tool_call =
                        tool_call_from_session_delta_resolving(&frame, &mut delta_payload_chunks)?;
                }
                collect_artifact_from_session_delta(&frame, &mut artifact_builders)?;
                if let Some(fin) = frame.get("fin").and_then(Value::as_str) {
                    finish_reason = Some(fin.to_owned());
                    claimed_usage = usage_from_session_delta(&frame);
                    provider_quality =
                        provider_quality.or_else(|| quality_from_session_delta(&frame));
                }
                if finish_reason.is_none() {
                    let output_tokens = streamed_output_token_count(&content, &token_ids);
                    if let Some(tok_s) = watchdog.throughput_floor_violation(output_tokens, now) {
                        let err = direct_session_throughput_floor_error(
                            session_id,
                            tok_s,
                            failover.min_tok_s.expect("floor checked"),
                        );
                        if let Some(partial) = interrupted_direct_session_partial(
                            &content,
                            tool_call.clone(),
                            latest_checkpoint_receipt.as_ref(),
                            &token_ids,
                            &watchdog,
                            now,
                            "throughput_floor",
                        ) {
                            return Err(GatewaySessionError::retryable_partial(err, partial));
                        }
                        return Err(GatewaySessionError::retryable(err));
                    }
                }
            }
            Some("s.receipt") => {
                let receipt =
                    provider_signed_receipt_from_frame(&frame, session_id, enclave_pubkey)?;
                if receipt.body.final_receipt {
                    if pending_checkpoint_receipt.is_some() {
                        return Err(GatewaySessionError::new(
                            "provider sent final receipt before pending checkpoint was acknowledged",
                        ));
                    }
                    final_provider_receipt = Some(receipt);
                } else {
                    if pending_checkpoint_receipt.is_some() {
                        return Err(GatewaySessionError::new(
                            "provider sent checkpoint receipt before previous checkpoint was acknowledged",
                        ));
                    }
                    let now = now_millis_u64();
                    if maybe_ack_direct_session_checkpoint_receipt(
                        bridge,
                        provider,
                        session_id,
                        request,
                        invocation,
                        &content,
                        tool_call.clone(),
                        &receipt,
                        &token_ids,
                        &watchdog,
                        now,
                        model,
                    )
                    .await?
                    .is_some()
                    {
                        latest_checkpoint_receipt = Some(receipt);
                    } else {
                        pending_checkpoint_receipt = Some(receipt);
                    }
                }
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
                let err = format!("provider returned {code} on session {session_id}: {message}");
                if let Some(partial) = interrupted_direct_session_partial(
                    &content,
                    tool_call.clone(),
                    latest_checkpoint_receipt.as_ref(),
                    &token_ids,
                    &watchdog,
                    now_millis_u64(),
                    "mid_stream_error",
                ) {
                    return Err(GatewaySessionError::retryable_partial(err, partial));
                }
                return Err(GatewaySessionError::new(err));
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if finish_reason.is_none() {
                    let err = format!(
                        "provider closed session {session_id} before final delta: {reason}"
                    );
                    if let Some(partial) = interrupted_direct_session_partial(
                        &content,
                        tool_call.clone(),
                        latest_checkpoint_receipt.as_ref(),
                        &token_ids,
                        &watchdog,
                        now_millis_u64(),
                        "mid_stream_close",
                    ) {
                        return Err(GatewaySessionError::retryable_partial(err, partial));
                    }
                    return Err(GatewaySessionError::new(err));
                }
                return Err(GatewaySessionError::new(format!(
                    "provider closed session {session_id} before s.receipt: {reason}"
                )));
            }
            _ => {}
        }
    }

    let usage = observed_chat_usage(request, &content, &token_ids);
    if let Some(claimed) = claimed_usage {
        if claimed != usage {
            return Err(GatewaySessionError::new(format!(
                "provider reported usage {:?} did not match gateway-observed usage {:?}",
                claimed, usage
            )));
        }
    }
    let completed_at_millis = now_millis_u64();
    let quality = provider_quality.or_else(|| {
        watchdog
            .first_delta_at_millis
            .map(|first_delta_at_millis| GatewaySessionQuality {
                ttft_ms: first_delta_at_millis.saturating_sub(started_at_millis),
                tok_s: generated_tokens_per_second(
                    usage.completion_tokens,
                    first_delta_at_millis,
                    completed_at_millis,
                ),
            })
    });
    let artifacts = finish_session_artifacts(artifact_builders)?;
    Ok(DirectSessionCollected {
        output: ChatOutput {
            content: tool_call.is_none().then_some(content),
            tool_call,
            artifacts,
            finish_reason: finish_reason.expect("loop ended with final delta"),
            usage,
        },
        provider_receipt: final_provider_receipt.expect("loop ended with provider receipt"),
        token_ids,
        quality,
    })
}

fn observed_chat_usage(request: &ChatCompletionRequest, content: &str, token_ids: &[i32]) -> Usage {
    let prompt_tokens = rough_tokens(&chat_prompt_text(request));
    let completion_tokens = streamed_output_token_count(content, token_ids);
    Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

async fn collect_direct_session_embedding_output(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    request_id: &str,
    failover: GatewayFailoverInvocation,
    inputs: &[String],
    enclave_pubkey: &str,
) -> Result<DirectEmbeddingSessionCollected, GatewaySessionError> {
    let mut embeddings = None;
    let mut usage = None;
    let mut provider_receipt = None;
    let mut delta_payload_chunks = SessionDeltaPayloadChunks::default();
    let mut provider_quality = None;
    let started_at_millis = now_millis_u64();
    let mut watchdog = DirectSessionWatchdog::new(
        started_at_millis,
        failover.ttft_timeout(),
        failover.stall_timeout(),
        None,
        None,
    );

    while embeddings.is_none() || provider_receipt.is_none() {
        let remaining_millis = watchdog
            .next_wait_millis(now_millis_u64())
            .map_err(|kind| direct_session_timeout_error(kind, session_id))?;
        let frame = next_session_frame(
            bridge,
            session_id,
            Duration::from_millis(remaining_millis),
            &[
                "s.delta",
                "s.delta_chunk",
                "s.receipt",
                "s.error",
                "s.close",
            ],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta_chunk")
                if frame.get("rid").and_then(Value::as_str) == Some(request_id) =>
            {
                watchdog.record_delta(now_millis_u64());
                collect_session_delta_chunk(&frame, &mut delta_payload_chunks)?;
            }
            Some("s.delta") if frame.get("rid").and_then(Value::as_str) == Some(request_id) => {
                let now = now_millis_u64();
                watchdog.record_delta(now);
                if embeddings.is_none() {
                    embeddings = embeddings_from_session_delta(&frame, &mut delta_payload_chunks)?;
                }
                if usage.is_none() {
                    usage = usage_from_session_delta(&frame);
                }
                provider_quality = provider_quality.or_else(|| quality_from_session_delta(&frame));
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
                    "provider returned {code} on embedding session {session_id}: {message}"
                )));
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(GatewaySessionError::new(format!(
                    "provider closed embedding session {session_id} before completion: {reason}"
                )));
            }
            _ => {}
        }
    }

    let completed_at_millis = now_millis_u64();
    let output = EmbeddingOutput {
        embeddings: embeddings.expect("loop ended with embeddings"),
        usage: usage.unwrap_or_else(|| embedding_usage_for_inputs(inputs)),
    };
    let quality = provider_quality.or_else(|| {
        watchdog
            .first_delta_at_millis
            .map(|first_delta_at_millis| GatewaySessionQuality {
                ttft_ms: first_delta_at_millis.saturating_sub(started_at_millis),
                tok_s: units_per_second(
                    output.usage.prompt_tokens,
                    started_at_millis,
                    completed_at_millis,
                ),
            })
    });
    Ok(DirectEmbeddingSessionCollected {
        output,
        provider_receipt: provider_receipt.expect("loop ended with provider receipt"),
        quality,
    })
}

async fn collect_direct_session_image_generation_output(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    request_id: &str,
    failover: GatewayFailoverInvocation,
    request: &ImageGenerationRequest,
    enclave_pubkey: &str,
) -> Result<DirectImageGenerationSessionCollected, GatewaySessionError> {
    let mut finish_seen = false;
    let mut usage = None;
    let mut provider_receipt = None;
    let mut artifact_builders = BTreeMap::new();
    let mut provider_quality = None;
    let started_at_millis = now_millis_u64();
    let mut watchdog = DirectSessionWatchdog::new(
        started_at_millis,
        failover.ttft_timeout(),
        failover.stall_timeout(),
        None,
        None,
    );

    while !finish_seen || provider_receipt.is_none() {
        let remaining_millis = watchdog
            .next_wait_millis(now_millis_u64())
            .map_err(|kind| direct_session_timeout_error(kind, session_id))?;
        let frame = next_session_frame(
            bridge,
            session_id,
            Duration::from_millis(remaining_millis),
            &["s.delta", "s.receipt", "s.error", "s.close"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta") if frame.get("rid").and_then(Value::as_str) == Some(request_id) => {
                watchdog.record_delta(now_millis_u64());
                collect_artifact_from_session_delta(&frame, &mut artifact_builders)?;
                if frame.get("fin").and_then(Value::as_str).is_some() {
                    finish_seen = true;
                    usage = receipt_usage_from_session_delta(&frame);
                    provider_quality =
                        provider_quality.or_else(|| quality_from_session_delta(&frame));
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
                    "provider returned {code} on image session {session_id}: {message}"
                )));
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(GatewaySessionError::new(format!(
                    "provider closed image session {session_id} before completion: {reason}"
                )));
            }
            _ => {}
        }
    }

    let completed_at_millis = now_millis_u64();
    let artifacts = finish_session_artifacts(artifact_builders)?;
    if artifacts.is_empty() {
        return Err(GatewaySessionError::new(format!(
            "provider image session {session_id} finished without image artifacts"
        )));
    }
    let usage =
        usage.unwrap_or_else(|| image_generation_usage_for_observed(request, artifacts.len()));
    let quality = provider_quality.or_else(|| {
        watchdog
            .first_delta_at_millis
            .map(|first_delta_at_millis| GatewaySessionQuality {
                ttft_ms: first_delta_at_millis.saturating_sub(started_at_millis),
                tok_s: units_per_second(
                    usage.get(USAGE_IMAGE),
                    started_at_millis,
                    completed_at_millis,
                ),
            })
    });
    Ok(DirectImageGenerationSessionCollected {
        output: ImageGenerationOutput { artifacts, usage },
        provider_receipt: provider_receipt.expect("loop ended with provider receipt"),
        quality,
    })
}

async fn collect_direct_session_audio_speech_output(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    request_id: &str,
    failover: GatewayFailoverInvocation,
    request: &AudioSpeechRequest,
    enclave_pubkey: &str,
) -> Result<DirectAudioSpeechSessionCollected, GatewaySessionError> {
    let mut finish_seen = false;
    let mut usage = None;
    let mut provider_receipt = None;
    let mut artifact_builders = BTreeMap::new();
    let mut provider_quality = None;
    let started_at_millis = now_millis_u64();
    let mut watchdog = DirectSessionWatchdog::new(
        started_at_millis,
        failover.ttft_timeout(),
        failover.stall_timeout(),
        None,
        None,
    );

    while !finish_seen || provider_receipt.is_none() {
        let remaining_millis = watchdog
            .next_wait_millis(now_millis_u64())
            .map_err(|kind| direct_session_timeout_error(kind, session_id))?;
        let frame = next_session_frame(
            bridge,
            session_id,
            Duration::from_millis(remaining_millis),
            &["s.delta", "s.receipt", "s.error", "s.close"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta") if frame.get("rid").and_then(Value::as_str) == Some(request_id) => {
                watchdog.record_delta(now_millis_u64());
                collect_artifact_from_session_delta(&frame, &mut artifact_builders)?;
                if frame.get("fin").and_then(Value::as_str).is_some() {
                    finish_seen = true;
                    usage = receipt_usage_from_session_delta(&frame);
                    provider_quality =
                        provider_quality.or_else(|| quality_from_session_delta(&frame));
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
                    "provider returned {code} on audio speech session {session_id}: {message}"
                )));
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(GatewaySessionError::new(format!(
                    "provider closed audio speech session {session_id} before completion: {reason}"
                )));
            }
            _ => {}
        }
    }

    let completed_at_millis = now_millis_u64();
    let artifacts = finish_session_artifacts(artifact_builders)?;
    if artifacts.is_empty() {
        return Err(GatewaySessionError::new(format!(
            "provider audio speech session {session_id} finished without audio artifacts"
        )));
    }
    let usage = usage.unwrap_or_else(|| audio_speech_usage_for_observed(request, &artifacts));
    let quality = provider_quality.or_else(|| {
        watchdog
            .first_delta_at_millis
            .map(|first_delta_at_millis| GatewaySessionQuality {
                ttft_ms: first_delta_at_millis.saturating_sub(started_at_millis),
                tok_s: units_per_second(
                    usage.get(USAGE_AUDIO_SECOND),
                    started_at_millis,
                    completed_at_millis,
                ),
            })
    });
    Ok(DirectAudioSpeechSessionCollected {
        output: AudioSpeechOutput { artifacts, usage },
        provider_receipt: provider_receipt.expect("loop ended with provider receipt"),
        quality,
    })
}

async fn collect_direct_session_audio_transcription_output(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    request_id: &str,
    failover: GatewayFailoverInvocation,
    request: &AudioTranscriptionRequest,
    enclave_pubkey: &str,
) -> Result<DirectAudioTranscriptionSessionCollected, GatewaySessionError> {
    let mut content = String::new();
    let mut finish_seen = false;
    let mut usage = None;
    let mut provider_receipt = None;
    let mut provider_quality = None;
    let started_at_millis = now_millis_u64();
    let mut watchdog = DirectSessionWatchdog::new(
        started_at_millis,
        failover.ttft_timeout(),
        failover.stall_timeout(),
        None,
        None,
    );

    while !finish_seen || provider_receipt.is_none() {
        let remaining_millis = watchdog
            .next_wait_millis(now_millis_u64())
            .map_err(|kind| direct_session_timeout_error(kind, session_id))?;
        let frame = next_session_frame(
            bridge,
            session_id,
            Duration::from_millis(remaining_millis),
            &["s.delta", "s.receipt", "s.error", "s.close"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta") if frame.get("rid").and_then(Value::as_str) == Some(request_id) => {
                watchdog.record_delta(now_millis_u64());
                if let Some(delta) = frame.get("d").and_then(Value::as_str) {
                    content.push_str(delta);
                }
                if frame.get("fin").and_then(Value::as_str).is_some() {
                    finish_seen = true;
                    usage = receipt_usage_from_session_delta(&frame);
                    provider_quality =
                        provider_quality.or_else(|| quality_from_session_delta(&frame));
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
                    "provider returned {code} on audio transcription session {session_id}: {message}"
                )));
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(GatewaySessionError::new(format!(
                    "provider closed audio transcription session {session_id} before completion: {reason}"
                )));
            }
            _ => {}
        }
    }

    let text = content.trim().to_owned();
    if text.is_empty() {
        return Err(GatewaySessionError::new(format!(
            "provider audio transcription session {session_id} finished with empty transcript"
        )));
    }
    let completed_at_millis = now_millis_u64();
    let usage = usage.unwrap_or_else(|| audio_transcription_usage_for_request(request));
    let quality = provider_quality.or_else(|| {
        watchdog
            .first_delta_at_millis
            .map(|first_delta_at_millis| GatewaySessionQuality {
                ttft_ms: first_delta_at_millis.saturating_sub(started_at_millis),
                tok_s: units_per_second(
                    usage.get(USAGE_AUDIO_SECOND),
                    started_at_millis,
                    completed_at_millis,
                ),
            })
    });
    Ok(DirectAudioTranscriptionSessionCollected {
        output: AudioTranscriptionOutput { text, usage },
        provider_receipt: provider_receipt.expect("loop ended with provider receipt"),
        quality,
    })
}

fn embeddings_from_session_delta(
    frame: &Value,
    pending: &mut SessionDeltaPayloadChunks,
) -> Result<Option<Vec<Vec<f32>>>, GatewaySessionError> {
    if let Some(value) = frame.get("embeddings").filter(|value| !value.is_null()) {
        return serde_json::from_value::<Vec<Vec<f32>>>(value.clone())
            .map(Some)
            .map_err(|err| GatewaySessionError::new(format!("invalid embeddings delta: {err}")));
    }
    if let Some(value) = resolve_session_delta_ref_field(frame, "embeddings", pending)? {
        return serde_json::from_value::<Vec<Vec<f32>>>(value)
            .map(Some)
            .map_err(|err| {
                GatewaySessionError::new(format!("invalid embeddings_ref delta: {err}"))
            });
    }
    if let Some(value) = frame.get("embedding").filter(|value| !value.is_null()) {
        return serde_json::from_value::<Vec<f32>>(value.clone())
            .map(|embedding| Some(vec![embedding]))
            .map_err(|err| GatewaySessionError::new(format!("invalid embedding delta: {err}")));
    }
    Ok(None)
}

fn interrupted_direct_session_partial(
    content: &str,
    tool_call: Option<ToolCallOutput>,
    provider_receipt: Option<&ProviderSignedReceipt>,
    token_ids: &[i32],
    watchdog: &DirectSessionWatchdog,
    now_millis: u64,
    reason: &str,
) -> Option<GatewaySessionPartial> {
    let first_delta_at_millis = watchdog.first_delta_at_millis?;
    let provider_receipt = provider_receipt?;
    if provider_receipt.body.final_receipt {
        return None;
    }
    let usage = usage_from_receipt_usage(&provider_receipt.body.usage);
    let quality = Some(GatewaySessionQuality {
        ttft_ms: first_delta_at_millis.saturating_sub(watchdog.started_at_millis),
        tok_s: generated_tokens_per_second(
            usage.completion_tokens,
            first_delta_at_millis,
            now_millis,
        ),
    });
    Some(GatewaySessionPartial {
        output: ChatOutput {
            content: tool_call.is_none().then_some(content.to_owned()),
            tool_call,
            artifacts: Vec::new(),
            finish_reason: "interrupted".to_owned(),
            usage,
        },
        provider_receipt: provider_receipt.clone(),
        token_ids: token_ids.to_vec(),
        quality,
        reason: reason.to_owned(),
        redispatch_mode: RedispatchMode::FullMessageHistoryClientSide,
    })
}

fn retryable_interrupted_direct_session_error(
    err: GatewaySessionError,
    content: &str,
    tool_call: Option<ToolCallOutput>,
    provider_receipt: Option<&ProviderSignedReceipt>,
    token_ids: &[i32],
    watchdog: &DirectSessionWatchdog,
    now_millis: u64,
    reason: &str,
) -> GatewaySessionError {
    if let Some(partial) = interrupted_direct_session_partial(
        content,
        tool_call,
        provider_receipt,
        token_ids,
        watchdog,
        now_millis,
        reason,
    ) {
        return GatewaySessionError::retryable_partial(err.message, partial);
    }
    err
}

fn client_disconnect_direct_session_error(
    content: &str,
    tool_call: Option<ToolCallOutput>,
    provider_receipt: Option<&ProviderSignedReceipt>,
    token_ids: &[i32],
    watchdog: &DirectSessionWatchdog,
    now_millis: u64,
) -> GatewaySessionError {
    retryable_interrupted_direct_session_error(
        GatewaySessionError::retryable("end-user disconnected before stream completed"),
        content,
        tool_call,
        provider_receipt,
        token_ids,
        watchdog,
        now_millis,
        "client_disconnect",
    )
}

fn direct_session_checkpoint_partial(
    request: &ChatCompletionRequest,
    content: &str,
    tool_call: Option<ToolCallOutput>,
    provider_receipt: ProviderSignedReceipt,
    token_ids: &[i32],
    watchdog: &DirectSessionWatchdog,
    now_millis: u64,
) -> GatewaySessionPartial {
    let usage = observed_chat_usage(request, content, token_ids);
    let quality =
        watchdog
            .first_delta_at_millis
            .map(|first_delta_at_millis| GatewaySessionQuality {
                ttft_ms: first_delta_at_millis.saturating_sub(watchdog.started_at_millis),
                tok_s: generated_tokens_per_second(
                    usage.completion_tokens,
                    first_delta_at_millis,
                    now_millis,
                ),
            });
    GatewaySessionPartial {
        output: ChatOutput {
            content: tool_call.is_none().then_some(content.to_owned()),
            tool_call,
            artifacts: Vec::new(),
            finish_reason: "checkpoint".to_owned(),
            usage,
        },
        provider_receipt,
        token_ids: token_ids.to_vec(),
        quality,
        reason: "checkpoint".to_owned(),
        redispatch_mode: RedispatchMode::FullMessageHistoryClientSide,
    }
}

async fn send_direct_session_frame_with_peer_reconnect(
    bridge: &mut ScBridgeClient,
    provider: &str,
    session_id: &str,
    frame: Value,
    open_timeout: Duration,
    action: &str,
) -> Result<(), GatewaySessionError> {
    match bridge
        .session_send(provider, session_id, frame.clone())
        .await
    {
        Ok(_) => Ok(()),
        Err(err) if bridge_error_missing_direct_connection(&err) => {
            bridge.peer_connect(provider, open_timeout).await.map_err(|connect_err| {
                GatewaySessionError::retryable(format!(
                    "{action} for session {session_id} to provider {provider} failed after direct connection was missing; reconnect failed: {connect_err}"
                ))
            })?;
            bridge
                .session_send(provider, session_id, frame)
                .await
                .map_err(|send_err| {
                    GatewaySessionError::retryable(format!(
                        "{action} for session {session_id} to provider {provider} failed after peer reconnect: {send_err}"
                    ))
                })?;
            Ok(())
        }
        Err(err) => Err(GatewaySessionError::retryable(format!(
            "{action} for session {session_id} to provider {provider} failed: {err}"
        ))),
    }
}

fn bridge_error_missing_direct_connection(error: &BridgeError) -> bool {
    error.to_string().contains("No direct connection")
}

async fn maybe_ack_direct_session_checkpoint_receipt(
    bridge: &mut ScBridgeClient,
    provider: &str,
    session_id: &str,
    request: &ChatCompletionRequest,
    invocation: &GatewaySessionInvocation,
    content: &str,
    tool_call: Option<ToolCallOutput>,
    receipt: &ProviderSignedReceipt,
    token_ids: &[i32],
    watchdog: &DirectSessionWatchdog,
    now_millis: u64,
    model: &GatewayModel,
) -> Result<Option<Value>, GatewaySessionError> {
    let observed_usage = observed_chat_usage(request, content, token_ids);
    let claimed_prompt_tokens = receipt.body.usage.prompt_tokens();
    let claimed_output_tokens = receipt.body.usage.output_tokens();
    if claimed_prompt_tokens != observed_usage.prompt_tokens {
        return Err(GatewaySessionError::new(
            "provider partial receipt prompt usage mismatch",
        ));
    }
    if claimed_output_tokens > observed_usage.completion_tokens {
        return Ok(None);
    }
    if claimed_output_tokens < observed_usage.completion_tokens {
        return Err(GatewaySessionError::new(
            "provider partial receipt trails gateway-observed output",
        ));
    }

    let partial = direct_session_checkpoint_partial(
        request,
        content,
        tool_call,
        receipt.clone(),
        token_ids,
        watchdog,
        now_millis,
    );
    let receipt_ack =
        direct_session_partial_receipt_ack(request, invocation, &partial, provider, model)?;
    let ack_frame = json!({
        "t": "s.receipt_ack",
        "v": 1,
        "session_id": receipt_ack.session_id,
        "seq": receipt_ack.seq,
        "user_sig": receipt_ack.user_sig,
        "reason": "checkpoint",
    });
    send_direct_session_frame_with_peer_reconnect(
        bridge,
        provider,
        session_id,
        ack_frame.clone(),
        invocation.failover.open_timeout(),
        "sending checkpoint s.receipt_ack",
    )
    .await?;
    Ok(Some(ack_frame))
}

fn usage_from_receipt_usage(usage: &ReceiptUsage) -> Usage {
    let prompt_tokens = usage.prompt_tokens();
    let completion_tokens = usage.output_tokens();
    Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

fn token_ids_from_session_delta(frame: &Value) -> Option<Vec<i32>> {
    let ids = frame.get("token_ids")?.as_array()?;
    Some(
        ids.iter()
            .filter_map(|value| value.as_i64().and_then(|id| i32::try_from(id).ok()))
            .collect(),
    )
}

fn token_ids_ref_from_session_delta(
    frame: &Value,
    pending: &mut SessionDeltaPayloadChunks,
) -> Result<Option<Vec<i32>>, GatewaySessionError> {
    let Some(value) = resolve_session_delta_ref_field(frame, "token_ids", pending)? else {
        return Ok(None);
    };
    serde_json::from_value::<Vec<i32>>(value)
        .map(Some)
        .map_err(|err| GatewaySessionError::new(format!("invalid token_ids_ref delta: {err}")))
}

fn token_ids_delta_from_session_delta(frame: &Value) -> Option<Vec<i32>> {
    let ids = frame.get("token_ids_delta")?.as_array()?;
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
    let usage = expected_text_usage_for_provider(
        Some(&provider_receipt.body.usage),
        output.usage.prompt_tokens,
        output.usage.completion_tokens,
        &invocation.spend_voucher.body.locked_rate_map,
    )?;
    let expected = ExpectedProviderReceipt {
        provider,
        seq: provider_receipt.body.seq,
        final_receipt: true,
        mu_owed_cum: calculate_locked_mu_owed(invocation, &usage),
        prompt_hash: blake3_hex(chat_prompt_text(request).as_bytes()),
        usage,
    };
    if expected.mu_owed_cum > invocation.spend_voucher.body.max_spend_mu {
        return Err(GatewaySessionError::new(
            "provider receipt exceeds signed spend voucher",
        ));
    }
    validate_provider_receipt(model, invocation, provider_receipt, expected)?;
    receipt_ack_for_body(&invocation.receipt_user_seed, &provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider receipt ack signing payload failed: {err}"
        ))
    })
}

fn direct_session_embedding_receipt_ack(
    inputs: &[String],
    output: &EmbeddingOutput,
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
    let expected = expected_embedding_provider_receipt(model, inputs, output, provider, invocation);
    validate_provider_receipt(model, invocation, provider_receipt, expected)?;
    receipt_ack_for_body(&invocation.receipt_user_seed, &provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider embedding receipt ack signing payload failed: {err}"
        ))
    })
}

fn direct_session_image_generation_receipt_ack(
    request: &ImageGenerationRequest,
    output: &ImageGenerationOutput,
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
    let expected =
        expected_image_generation_provider_receipt(model, request, output, provider, invocation);
    validate_provider_receipt(model, invocation, provider_receipt, expected)?;
    receipt_ack_for_body(&invocation.receipt_user_seed, &provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider image receipt ack signing payload failed: {err}"
        ))
    })
}

fn direct_session_audio_speech_receipt_ack(
    request: &AudioSpeechRequest,
    output: &AudioSpeechOutput,
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
    let expected =
        expected_audio_speech_provider_receipt(model, request, output, provider, invocation);
    validate_provider_receipt(model, invocation, provider_receipt, expected)?;
    receipt_ack_for_body(&invocation.receipt_user_seed, &provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider audio speech receipt ack signing payload failed: {err}"
        ))
    })
}

fn direct_session_audio_transcription_receipt_ack(
    request: &AudioTranscriptionRequest,
    output: &AudioTranscriptionOutput,
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
    let expected =
        expected_audio_transcription_provider_receipt(model, request, output, provider, invocation);
    validate_provider_receipt(model, invocation, provider_receipt, expected)?;
    receipt_ack_for_body(&invocation.receipt_user_seed, &provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider audio transcription receipt ack signing payload failed: {err}"
        ))
    })
}

fn direct_session_partial_receipt_ack(
    request: &ChatCompletionRequest,
    invocation: &GatewaySessionInvocation,
    partial: &GatewaySessionPartial,
    provider: &str,
    model: &GatewayModel,
) -> Result<ReceiptAck, GatewaySessionError> {
    if !invocation.receipt_cosign_enabled {
        return Err(GatewaySessionError::new(
            "receipt co-signing refused; session paused",
        ));
    }
    let body = &partial.provider_receipt.body;
    let usage = expected_text_usage_for_provider(
        Some(&partial.provider_receipt.body.usage),
        partial.output.usage.prompt_tokens,
        partial.output.usage.completion_tokens,
        &invocation.spend_voucher.body.locked_rate_map,
    )?;
    let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
    if mu_owed_cum > invocation.spend_voucher.body.max_spend_mu {
        return Err(GatewaySessionError::new(
            "provider partial receipt exceeds signed spend voucher",
        ));
    }
    validate_provider_receipt(
        model,
        invocation,
        &partial.provider_receipt,
        ExpectedProviderReceipt {
            provider,
            seq: body.seq,
            final_receipt: false,
            mu_owed_cum,
            usage,
            prompt_hash: blake3_hex(chat_prompt_text(request).as_bytes()),
        },
    )?;
    receipt_ack_for_body(&invocation.receipt_user_seed, body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider partial receipt ack signing payload failed: {err}"
        ))
    })
}

fn expected_embedding_provider_receipt<'a>(
    _model: &GatewayModel,
    inputs: &[String],
    output: &EmbeddingOutput,
    provider: &'a str,
    invocation: &GatewaySessionInvocation,
) -> ExpectedProviderReceipt<'a> {
    let usage = ReceiptUsage::text(output.usage.prompt_tokens, 0);
    ExpectedProviderReceipt {
        provider,
        seq: 1,
        final_receipt: true,
        mu_owed_cum: calculate_locked_mu_owed(invocation, &usage),
        prompt_hash: blake3_hex(embedding_prompt_text(inputs).as_bytes()),
        usage,
    }
}

fn expected_image_generation_provider_receipt<'a>(
    _model: &GatewayModel,
    request: &ImageGenerationRequest,
    output: &ImageGenerationOutput,
    provider: &'a str,
    invocation: &GatewaySessionInvocation,
) -> ExpectedProviderReceipt<'a> {
    let usage = output.usage.clone();
    ExpectedProviderReceipt {
        provider,
        seq: 1,
        final_receipt: true,
        mu_owed_cum: calculate_locked_mu_owed(invocation, &usage),
        prompt_hash: image_generation_prompt_hash(request),
        usage,
    }
}

fn expected_audio_speech_provider_receipt<'a>(
    _model: &GatewayModel,
    request: &AudioSpeechRequest,
    output: &AudioSpeechOutput,
    provider: &'a str,
    invocation: &GatewaySessionInvocation,
) -> ExpectedProviderReceipt<'a> {
    let usage = output.usage.clone();
    ExpectedProviderReceipt {
        provider,
        seq: 1,
        final_receipt: true,
        mu_owed_cum: calculate_locked_mu_owed(invocation, &usage),
        prompt_hash: audio_speech_prompt_hash(request),
        usage,
    }
}

fn expected_audio_transcription_provider_receipt<'a>(
    _model: &GatewayModel,
    request: &AudioTranscriptionRequest,
    output: &AudioTranscriptionOutput,
    provider: &'a str,
    invocation: &GatewaySessionInvocation,
) -> ExpectedProviderReceipt<'a> {
    let usage = output.usage.clone();
    ExpectedProviderReceipt {
        provider,
        seq: 1,
        final_receipt: true,
        mu_owed_cum: calculate_locked_mu_owed(invocation, &usage),
        prompt_hash: audio_transcription_prompt_hash(request),
        usage,
    }
}

fn expected_text_usage_for_provider(
    provider_usage: Option<&ReceiptUsage>,
    observed_prompt_tokens: u64,
    observed_completion_tokens: u64,
    locked_rate_map: &[RateMapEntry],
) -> Result<ReceiptUsage, GatewaySessionError> {
    let Some(provider_usage) = provider_usage else {
        return Ok(ReceiptUsage::text(
            observed_prompt_tokens,
            observed_completion_tokens,
        ));
    };
    if provider_usage.cached_input_tokens() == 0 {
        return Ok(ReceiptUsage::text(
            observed_prompt_tokens,
            observed_completion_tokens,
        ));
    }
    if !locked_rate_map
        .iter()
        .any(|entry| entry.unit == USAGE_CACHED_INPUT_TOKEN)
    {
        return Err(GatewaySessionError::new(
            "provider receipt claimed cached input but locked rate_map lacks cached_input_token",
        ));
    }
    if provider_usage.prompt_tokens() != observed_prompt_tokens {
        return Err(GatewaySessionError::new(
            "provider receipt cached prompt usage mismatch",
        ));
    }
    if provider_usage.output_tokens() != observed_completion_tokens {
        return Err(GatewaySessionError::new(
            "provider receipt cached output usage mismatch",
        ));
    }
    Ok(provider_usage.clone())
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
        (body.seq == expected.seq, "provider receipt seq mismatch"),
        (
            body.final_receipt == expected.final_receipt,
            "provider receipt finality mismatch",
        ),
        (
            body.rail == invocation.rail,
            "provider receipt rail mismatch",
        ),
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
            body.locked_rate_map == invocation.spend_voucher.body.locked_rate_map,
            "provider receipt locked_rate_map mismatch",
        ),
        (
            body.locked_per_req_mu == invocation.spend_voucher.body.locked_per_req_mu,
            "provider receipt locked_per_req_mu mismatch",
        ),
        (
            body.locked_min_session_mu == invocation.spend_voucher.body.locked_min_session_mu,
            "provider receipt locked_min_session_mu mismatch",
        ),
        (
            body.served_ctx == invocation.served_ctx
                && body.served_ctx == invocation.spend_voucher.body.served_ctx,
            "provider receipt served_ctx mismatch",
        ),
        (
            body.ctx_bracket == invocation.ctx_bracket
                && body.ctx_bracket == invocation.spend_voucher.body.ctx_bracket,
            "provider receipt ctx_bracket mismatch",
        ),
        (
            body.ctx_bracket_table_ver == invocation.ctx_bracket_table_ver
                && body.ctx_bracket_table_ver
                    == invocation.spend_voucher.body.ctx_bracket_table_ver,
            "provider receipt ctx_bracket_table_ver mismatch",
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

fn tool_call_from_session_delta_resolving(
    frame: &Value,
    pending: &mut SessionDeltaPayloadChunks,
) -> Result<Option<ToolCallOutput>, GatewaySessionError> {
    if let Some(tool) = frame.get("tool").filter(|tool| !tool.is_null()) {
        return Ok(tool_call_from_session_value(tool));
    }
    if let Some(tool) = resolve_session_delta_ref_field(frame, "tool", pending)? {
        return Ok(tool_call_from_session_value(&tool));
    }
    Ok(None)
}

fn tool_call_from_session_value(tool: &Value) -> Option<ToolCallOutput> {
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

fn receipt_usage_from_session_delta(frame: &Value) -> Option<ReceiptUsage> {
    serde_json::from_value(frame.get("usage")?.clone()).ok()
}

async fn run_embedding_with_route_retry(
    state: &GatewayState,
    model: &GatewayModel,
    request: &EmbeddingRequest,
    inputs: &[String],
    options: GatewayRequestOptions,
) -> Result<GatewayEmbeddingRun, ApiError> {
    let eligible_routes =
        ordered_route_candidates_for_embedding_with_options(state, model, inputs, &options);
    let RouteWaitOutcome {
        routes: eligible_routes,
        waited,
    } = wait_for_eligible_routes(state, model, &options, eligible_routes, || {
        ordered_route_candidates_for_embedding_with_options(state, model, inputs, &options)
    })
    .await;
    if !model.mayhem.route_candidates.is_empty() && eligible_routes.is_empty() {
        if waited {
            return Err(route_wait_expired_error(&options));
        }
        return Err(no_eligible_route_error(state, model, &options));
    }
    if eligible_routes.is_empty() && !state.dev_session_shim {
        return Err(ApiError::service_unavailable(
            "no provider available: production gateway requires an active provider joined to an admin-created room",
            Some("model"),
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
        let invocation =
            state.prepare_embedding_invocation_for_route(model, inputs, route, &options)?;
        let attempt_started = Instant::now();
        match state
            .session_backend
            .run_embedding(model, request, &invocation)
            .await
        {
            Ok(result) => {
                record_route_observation(
                    state,
                    route,
                    observation_sample_from_embedding_success(&result, attempt_started.elapsed()),
                );
                let metering_output = result.output.clone();
                return Ok(GatewayEmbeddingRun {
                    result,
                    invocation,
                    metering_inputs: inputs.to_vec(),
                    metering_output,
                });
            }
            Err(err) if err.retryable => {
                record_retryable_route_attempt(state, route, attempt_started.elapsed(), &err);
                last_retryable_error = Some(err.message);
            }
            Err(err) => {
                record_route_failure_attempt(state, route, attempt_started.elapsed());
                return Err(ApiError::bad_gateway(err.message, Some("model")));
            }
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

async fn run_image_generation_with_route_retry(
    state: &GatewayState,
    model: &GatewayModel,
    request: &ImageGenerationRequest,
    options: GatewayRequestOptions,
) -> Result<GatewayImageGenerationRun, ApiError> {
    let eligible_routes =
        ordered_route_candidates_for_image_generation_with_options(state, model, request, &options);
    let RouteWaitOutcome {
        routes: eligible_routes,
        waited,
    } = wait_for_eligible_routes(state, model, &options, eligible_routes, || {
        ordered_route_candidates_for_image_generation_with_options(state, model, request, &options)
    })
    .await;
    if !model.mayhem.route_candidates.is_empty() && eligible_routes.is_empty() {
        if waited {
            return Err(route_wait_expired_error(&options));
        }
        return Err(no_eligible_route_error(state, model, &options));
    }
    if eligible_routes.is_empty() && !state.dev_session_shim {
        return Err(ApiError::service_unavailable(
            "no provider available: production gateway requires an active provider joined to an admin-created room",
            Some("model"),
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
        let invocation =
            state.prepare_image_generation_invocation_for_route(model, request, route, &options)?;
        let attempt_started = Instant::now();
        match state
            .session_backend
            .run_image_generation(model, request, &invocation)
            .await
        {
            Ok(result) => {
                record_route_observation(
                    state,
                    route,
                    observation_sample_from_image_generation_success(
                        &result,
                        attempt_started.elapsed(),
                    ),
                );
                let metering_request = request.clone();
                let metering_output = result.output.clone();
                return Ok(GatewayImageGenerationRun {
                    result,
                    invocation,
                    metering_request,
                    metering_output,
                });
            }
            Err(err) if err.retryable => {
                record_retryable_route_attempt(state, route, attempt_started.elapsed(), &err);
                last_retryable_error = Some(err.message);
            }
            Err(err) => {
                record_route_failure_attempt(state, route, attempt_started.elapsed());
                return Err(ApiError::bad_gateway(err.message, Some("model")));
            }
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

async fn run_audio_speech_with_route_retry(
    state: &GatewayState,
    model: &GatewayModel,
    request: &AudioSpeechRequest,
    options: GatewayRequestOptions,
) -> Result<GatewayAudioSpeechRun, ApiError> {
    let eligible_routes =
        ordered_route_candidates_for_audio_speech_with_options(state, model, request, &options);
    let RouteWaitOutcome {
        routes: eligible_routes,
        waited,
    } = wait_for_eligible_routes(state, model, &options, eligible_routes, || {
        ordered_route_candidates_for_audio_speech_with_options(state, model, request, &options)
    })
    .await;
    if !model.mayhem.route_candidates.is_empty() && eligible_routes.is_empty() {
        if waited {
            return Err(route_wait_expired_error(&options));
        }
        return Err(no_eligible_route_error(state, model, &options));
    }
    if eligible_routes.is_empty() && !state.dev_session_shim {
        return Err(ApiError::service_unavailable(
            "no provider available: production gateway requires an active provider joined to an admin-created room",
            Some("model"),
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
        let invocation =
            state.prepare_audio_speech_invocation_for_route(model, request, route, &options)?;
        let attempt_started = Instant::now();
        match state
            .session_backend
            .run_audio_speech(model, request, &invocation)
            .await
        {
            Ok(result) => {
                record_route_observation(
                    state,
                    route,
                    observation_sample_from_audio_speech_success(
                        &result,
                        attempt_started.elapsed(),
                    ),
                );
                let metering_request = request.clone();
                let metering_output = result.output.clone();
                return Ok(GatewayAudioSpeechRun {
                    result,
                    invocation,
                    metering_request,
                    metering_output,
                });
            }
            Err(err) if err.retryable => {
                record_retryable_route_attempt(state, route, attempt_started.elapsed(), &err);
                last_retryable_error = Some(err.message);
            }
            Err(err) => {
                record_route_failure_attempt(state, route, attempt_started.elapsed());
                return Err(ApiError::bad_gateway(err.message, Some("model")));
            }
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

async fn run_audio_transcription_with_route_retry(
    state: &GatewayState,
    model: &GatewayModel,
    request: &AudioTranscriptionRequest,
    options: GatewayRequestOptions,
) -> Result<GatewayAudioTranscriptionRun, ApiError> {
    let eligible_routes = ordered_route_candidates_for_audio_transcription_with_options(
        state, model, request, &options,
    );
    let RouteWaitOutcome {
        routes: eligible_routes,
        waited,
    } = wait_for_eligible_routes(state, model, &options, eligible_routes, || {
        ordered_route_candidates_for_audio_transcription_with_options(
            state, model, request, &options,
        )
    })
    .await;
    if !model.mayhem.route_candidates.is_empty() && eligible_routes.is_empty() {
        if waited {
            return Err(route_wait_expired_error(&options));
        }
        return Err(no_eligible_route_error(state, model, &options));
    }
    if eligible_routes.is_empty() && !state.dev_session_shim {
        return Err(ApiError::service_unavailable(
            "no provider available: production gateway requires an active provider joined to an admin-created room",
            Some("model"),
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
        let invocation = state
            .prepare_audio_transcription_invocation_for_route(model, request, route, &options)?;
        let attempt_started = Instant::now();
        match state
            .session_backend
            .run_audio_transcription(model, request, &invocation)
            .await
        {
            Ok(result) => {
                record_route_observation(
                    state,
                    route,
                    observation_sample_from_audio_transcription_success(
                        &result,
                        attempt_started.elapsed(),
                    ),
                );
                let metering_request = request.clone();
                let metering_output = result.output.clone();
                return Ok(GatewayAudioTranscriptionRun {
                    result,
                    invocation,
                    metering_request,
                    metering_output,
                });
            }
            Err(err) if err.retryable => {
                record_retryable_route_attempt(state, route, attempt_started.elapsed(), &err);
                last_retryable_error = Some(err.message);
            }
            Err(err) => {
                record_route_failure_attempt(state, route, attempt_started.elapsed());
                return Err(ApiError::bad_gateway(err.message, Some("model")));
            }
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

fn no_eligible_route_error(
    state: &GatewayState,
    model: &GatewayModel,
    options: &GatewayRequestOptions,
) -> ApiError {
    let rail = state.receipt_config.rail.clone();
    let rail_candidates = model
        .mayhem
        .route_candidates
        .iter()
        .filter(|candidate| {
            candidate
                .accepted_rails
                .iter()
                .any(|candidate_rail| candidate_rail == &rail)
        })
        .collect::<Vec<_>>();
    if rail_candidates.is_empty() {
        return ApiError::payment_required(
            format!("no provider accepts the {rail} payment rail"),
            Some("model"),
        );
    }
    let att_candidates = rail_candidates
        .iter()
        .copied()
        .filter(|candidate| {
            options
                .min_att_tier
                .map(|min_tier| candidate.att_tier >= min_tier)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if att_candidates.is_empty() {
        return ApiError::bad_request(
            "no provider route satisfies X-Mayhem-Min-Att-Tier",
            Some("X-Mayhem-Min-Att-Tier"),
        );
    }
    let quant_candidates = att_candidates
        .iter()
        .copied()
        .filter(|candidate| {
            options
                .quant
                .as_deref()
                .map(|quant| candidate.quant.eq_ignore_ascii_case(quant))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if quant_candidates.is_empty() {
        return ApiError::bad_request(
            "no provider route satisfies X-Mayhem-Quant",
            Some("X-Mayhem-Quant"),
        );
    }
    let now_millis = now_millis_u64();
    if quant_candidates
        .iter()
        .all(|candidate| state.route_provider_in_cooloff(candidate, now_millis))
    {
        return ApiError::service_unavailable(
            "all otherwise eligible provider routes are cooling off after a retryable failure",
            Some("model"),
        );
    }
    if options.max_price_mu.is_some() {
        return no_price_band_route_error();
    }
    ApiError::bad_request("no provider route is currently eligible", Some("model"))
}

struct RouteWaitOutcome<'a> {
    routes: Vec<&'a GatewayRouteCandidate>,
    waited: bool,
}

async fn wait_for_eligible_routes<'a, F>(
    state: &GatewayState,
    model: &'a GatewayModel,
    options: &GatewayRequestOptions,
    routes: Vec<&'a GatewayRouteCandidate>,
    refresh: F,
) -> RouteWaitOutcome<'a>
where
    F: FnMut() -> Vec<&'a GatewayRouteCandidate>,
{
    wait_for_eligible_routes_with_poll(
        state,
        model,
        options,
        routes,
        refresh,
        Duration::from_millis(ROUTE_WAIT_POLL_MS),
    )
    .await
}

async fn wait_for_eligible_routes_with_poll<'a, F>(
    state: &GatewayState,
    model: &'a GatewayModel,
    options: &GatewayRequestOptions,
    mut routes: Vec<&'a GatewayRouteCandidate>,
    mut refresh: F,
    poll_interval: Duration,
) -> RouteWaitOutcome<'a>
where
    F: FnMut() -> Vec<&'a GatewayRouteCandidate>,
{
    if model.mayhem.route_candidates.is_empty()
        || !routes.is_empty()
        || options.max_wait_ms == 0
        || !route_static_filters_have_candidates(state, model, options)
    {
        return RouteWaitOutcome {
            routes,
            waited: false,
        };
    }
    let max_wait = Duration::from_millis(options.max_wait_ms.min(MAX_ROUTE_MAX_WAIT_MS));
    let started = Instant::now();
    let mut waited = false;
    while started.elapsed() < max_wait {
        waited = true;
        let remaining = max_wait.saturating_sub(started.elapsed());
        let nap = remaining.min(poll_interval.max(Duration::from_millis(1)));
        tokio::time::sleep(nap).await;
        routes = refresh();
        if !routes.is_empty() {
            break;
        }
    }
    RouteWaitOutcome { routes, waited }
}

fn route_static_filters_have_candidates(
    state: &GatewayState,
    model: &GatewayModel,
    options: &GatewayRequestOptions,
) -> bool {
    model
        .mayhem
        .route_candidates
        .iter()
        .filter(|candidate| {
            candidate
                .accepted_rails
                .iter()
                .any(|rail| rail == &state.receipt_config.rail)
        })
        .filter(|candidate| {
            options
                .min_att_tier
                .map(|min_tier| candidate.att_tier >= min_tier)
                .unwrap_or(true)
        })
        .any(|candidate| {
            options
                .quant
                .as_deref()
                .map(|quant| candidate.quant.eq_ignore_ascii_case(quant))
                .unwrap_or(true)
        })
}

fn route_wait_expired_error(options: &GatewayRequestOptions) -> ApiError {
    ApiError::service_unavailable(
        format!(
            "network busy: no provider capacity became available before X-Mayhem-Max-Wait-Ms ({})",
            options.max_wait_ms
        ),
        Some("model"),
    )
}

fn no_price_band_route_error() -> ApiError {
    ApiError::bad_request(
        "no provider route is at or below X-Mayhem-Max-Price-Mu",
        Some("X-Mayhem-Max-Price-Mu"),
    )
}

fn ensure_max_price_allows(quote_mu: u64, max_price_mu: Option<u64>) -> Result<(), ApiError> {
    if max_price_mu.is_some_and(|max_price_mu| quote_mu > max_price_mu) {
        return Err(no_price_band_route_error());
    }
    Ok(())
}

async fn build_chat_completion(
    state: SharedState,
    request: ChatCompletionRequest,
    options: GatewayRequestOptions,
) -> Result<ChatResponse, ApiError> {
    let model = require_model(&state, &request.model)?;
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
    if request.stream && !state.dev_session_shim {
        if let Some(config) = state.session_backend.bridge_stream_config() {
            return build_live_chat_completion(
                state.clone(),
                model,
                request,
                options,
                id,
                created,
                config,
            )
            .await;
        }
    }
    let GatewaySessionRun {
        result:
            GatewaySessionResult {
                output,
                backend,
                direct_session,
                provider_receipt,
                token_ids: _,
                quality: _,
            },
        invocation,
        metering_request,
        metering_output,
    } = run_chat_with_route_retry(&state, &model, &request, options).await?;
    let mayhem_meta = ResponseMayhemMeta {
        backend: &backend,
        direct_session,
        billable: !state.dev_session_shim,
        dev_session: state.dev_session_shim,
        hedge: invocation.hedge.clone(),
    };
    let receipt = if state.dev_session_shim {
        None
    } else {
        let receipt = state.meter_chat_session(
            &model,
            &metering_request,
            &metering_output,
            &invocation,
            provider_receipt.as_ref(),
        )?;
        state
            .maybe_run_canary_probe_after_session(&model, &invocation)
            .await;
        Some(receipt)
    };
    if request.stream {
        Ok(ChatResponse::Sse(chat_stream_chunks(
            &id,
            created,
            &model.id,
            &output,
            receipt.as_ref(),
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
            receipt.as_ref(),
            mayhem_meta,
        )))
    }
}

struct LiveDirectChatSession {
    state: SharedState,
    model: GatewayModel,
    request: ChatCompletionRequest,
    options: GatewayRequestOptions,
    invocation: GatewaySessionInvocation,
    route: Option<GatewayRouteCandidate>,
    attempt_started: Instant,
    bridge: ScBridgeClient,
    provider: String,
    request_id: String,
    enclave_pubkey: String,
    id: String,
    created: u64,
    include_usage: bool,
    backend: String,
}

async fn build_live_chat_completion(
    state: SharedState,
    model: GatewayModel,
    request: ChatCompletionRequest,
    options: GatewayRequestOptions,
    id: String,
    created: u64,
    config: ScBridgeGatewaySessionConfig,
) -> Result<ChatResponse, ApiError> {
    let session =
        prepare_live_direct_chat_session(state, model, request, options, id, created, config)
            .await?;
    Ok(ChatResponse::SseStream(live_direct_chat_sse_stream(
        session,
    )))
}

async fn prepare_live_direct_chat_session(
    state: SharedState,
    model: GatewayModel,
    request: ChatCompletionRequest,
    options: GatewayRequestOptions,
    id: String,
    created: u64,
    config: ScBridgeGatewaySessionConfig,
) -> Result<LiveDirectChatSession, ApiError> {
    let eligible_route_refs =
        ordered_route_candidates_for_request_with_options(&state, &model, &request, &options);
    let RouteWaitOutcome {
        routes: mut eligible_route_refs,
        waited,
    } = wait_for_eligible_routes(&state, &model, &options, eligible_route_refs, || {
        ordered_route_candidates_for_request_with_options(&state, &model, &request, &options)
    })
    .await;
    if !model.mayhem.route_candidates.is_empty() && eligible_route_refs.is_empty() {
        if waited {
            return Err(route_wait_expired_error(&options));
        }
        return Err(no_eligible_route_error(&state, &model, &options));
    }
    if eligible_route_refs.is_empty() {
        return Err(ApiError::service_unavailable(
            "no provider available: production gateway requires an active provider joined to an admin-created room",
            Some("model"),
        ));
    }
    let hedge_probe =
        run_hedge_probes_if_requested(&state, &model, &request, &eligible_route_refs, &options)
            .await?;
    if let Some(winner) = hedge_probe.winner.as_ref() {
        if let Some(winner_index) = eligible_route_refs
            .iter()
            .position(|route| route.provider == winner.provider)
        {
            let winner_route = eligible_route_refs.remove(winner_index);
            eligible_route_refs.insert(0, winner_route);
        }
    }
    let eligible_routes = eligible_route_refs.into_iter().cloned().collect::<Vec<_>>();

    let attempt_count = eligible_routes
        .len()
        .min(usize::from(DEFAULT_MAX_OPEN_ATTEMPTS));
    let mut last_retryable_error = None;
    for attempt_index in 0..attempt_count {
        let route = eligible_routes.get(attempt_index);
        let invocation =
            state.prepare_chat_invocation_for_route(&model, &request, route, &options)?;
        let invocation = invocation.with_hedge_probe_outcome(&hedge_probe);
        let attempt_started = Instant::now();
        match open_live_direct_chat_session(&config, &request, &invocation).await {
            Ok((bridge, provider, request_id, enclave_pubkey)) => {
                let include_usage = request
                    .stream_options
                    .as_ref()
                    .is_some_and(|options| options.include_usage);
                return Ok(LiveDirectChatSession {
                    state,
                    model,
                    request,
                    options,
                    invocation,
                    route: route.cloned(),
                    attempt_started,
                    bridge,
                    provider,
                    request_id,
                    enclave_pubkey,
                    id,
                    created,
                    include_usage,
                    backend: "sc-bridge-direct-session".to_owned(),
                });
            }
            Err(err) if err.retryable => {
                record_retryable_route_attempt(&state, route, attempt_started.elapsed(), &err);
                last_retryable_error = Some(err.message);
            }
            Err(err) => {
                record_route_failure_attempt(&state, route, attempt_started.elapsed());
                return Err(ApiError::bad_gateway(err.message, Some("model")));
            }
        }
    }

    Err(ApiError::bad_gateway(
        format!(
            "all {attempt_count} route attempt(s) failed before streaming; last error: {}",
            last_retryable_error.unwrap_or_else(|| "no route attempted".to_owned())
        ),
        Some("model"),
    ))
}

async fn open_live_direct_chat_session(
    config: &ScBridgeGatewaySessionConfig,
    request: &ChatCompletionRequest,
    invocation: &GatewaySessionInvocation,
) -> Result<(ScBridgeClient, String, String, String), GatewaySessionError> {
    let provider = invocation
        .provider_pubkey
        .as_deref()
        .ok_or_else(|| GatewaySessionError::new("model has no canonical provider route"))?;
    let mut bridge =
        ScBridgeClient::connect(ScBridgeConfig::new(&config.url, config.token.clone())?).await?;
    bridge
        .session_subscribe([invocation.session_id.as_str()])
        .await?;
    bridge
        .peer_connect(provider, invocation.failover.open_timeout())
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
        "session_id": invocation.session_id.clone(),
        "rail": invocation.rail.clone(),
        "user": invocation.user_pubkey.clone(),
        "enclave_id": invocation.enclave_id.clone(),
        "price_ver": invocation.price_ver,
        "at": invocation.opened_at,
        "served_ctx": invocation.served_ctx,
        "ctx_bracket": invocation.ctx_bracket.clone(),
        "ctx_bracket_table_ver": invocation.ctx_bracket_table_ver,
        "rules_ver": invocation.rules_ver,
        "voucher": invocation.spend_voucher.clone(),
        "att_nonce": att_nonce,
        "ts": now,
        "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
        "sig": invocation.spend_voucher.user_sig.clone(),
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
        invocation.failover.open_timeout(),
        &["s.accept", "s.reject"],
    )
    .await
    .map_err(GatewaySessionError::into_retryable)?;
    if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
        return Err(provider_reject_session_error(
            &accept,
            &invocation.session_id,
        ));
    }
    let accept_info =
        validate_direct_session_accept(&accept, invocation, &open_head, &att_nonce, now / 1000)?;
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
    let request_body = direct_session_request_body(request);
    send_direct_session_request_frames(
        &mut bridge,
        provider,
        &invocation.session_id,
        &request_id,
        &request_body,
    )
    .await?;
    Ok((
        bridge,
        provider.to_owned(),
        request_id,
        accept_info.enclave_pubkey,
    ))
}

fn live_direct_chat_sse_stream(session: LiveDirectChatSession) -> SseEventStream {
    let capacity = live_sse_event_buffer_capacity(&session.request);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(capacity);
    tokio::spawn(async move {
        run_live_direct_chat_sse(session, tx).await;
    });
    Box::pin(stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|event| (event, rx))
    }))
}

fn live_sse_event_buffer_capacity(request: &ChatCompletionRequest) -> usize {
    let max_tokens = request
        .max_tokens
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(LIVE_SSE_DEFAULT_MAX_TOKENS);
    max_tokens
        .saturating_mul(2)
        .saturating_add(64)
        .clamp(LIVE_SSE_MIN_EVENT_BUFFER, LIVE_SSE_MAX_EVENT_BUFFER)
}

async fn run_live_direct_chat_sse(
    mut session: LiveDirectChatSession,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    let result = run_live_direct_chat_sse_inner(&mut session, &tx).await;
    match result {
        Ok(()) => {
            let _ = send_sse_done(&tx).await;
        }
        Err(err) => {
            let recovered = if is_client_disconnect_error(&err) {
                finish_live_direct_chat_after_client_disconnect(&mut session, err).await
            } else if err.retryable && err.partial.is_some() {
                recover_live_direct_chat_after_partial(&mut session, &tx, err).await
            } else {
                Err(err)
            };
            match recovered {
                Ok(()) => {
                    let _ = send_sse_done(&tx).await;
                }
                Err(err) => {
                    record_route_observation(
                        &session.state,
                        session.route.as_ref(),
                        observation_sample_from_error(session.attempt_started.elapsed()),
                    );
                    let _ = session
                        .bridge
                        .session_send(
                            &session.provider,
                            &session.invocation.session_id,
                            json!({
                                "t": "s.close",
                                "v": 1,
                                "session_id": session.invocation.session_id,
                                "reason": "stream_error",
                            }),
                        )
                        .await;
                    let _ = send_sse_error(&tx, &err.message).await;
                    let _ = send_sse_done(&tx).await;
                }
            }
        }
    }
}

fn is_client_disconnect_error(err: &GatewaySessionError) -> bool {
    err.message.contains("end-user disconnected")
        || err
            .partial
            .as_ref()
            .is_some_and(|partial| partial.reason == "client_disconnect")
}

async fn finish_live_direct_chat_after_client_disconnect(
    session: &mut LiveDirectChatSession,
    mut err: GatewaySessionError,
) -> Result<(), GatewaySessionError> {
    if let Some(partial) = err.partial.take() {
        session
            .state
            .record_partial_provider_receipt(
                &session.model,
                &session.request,
                &session.invocation,
                &partial,
            )
            .map_err(|err| GatewaySessionError::new(err.message))?;
    }
    let _ = session
        .bridge
        .session_send(
            &session.provider,
            &session.invocation.session_id,
            json!({
                "t": "s.close",
                "v": 1,
                "session_id": session.invocation.session_id,
                "reason": "client_disconnect",
            }),
        )
        .await;
    Ok(())
}

async fn recover_live_direct_chat_after_partial(
    session: &mut LiveDirectChatSession,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    mut err: GatewaySessionError,
) -> Result<(), GatewaySessionError> {
    let partial = err
        .partial
        .take()
        .map(|partial| *partial)
        .ok_or_else(|| GatewaySessionError::new(err.message.clone()))?;
    session
        .state
        .record_partial_provider_receipt(
            &session.model,
            &session.request,
            &session.invocation,
            &partial,
        )
        .map_err(|err| GatewaySessionError::new(err.message))?;
    if let Some(route) = session.route.as_ref() {
        session.state.cool_route_provider(route, now_millis_u64());
    }
    record_route_observation(
        &session.state,
        session.route.as_ref(),
        observation_sample_from_error(session.attempt_started.elapsed()),
    );
    let _ = session
        .bridge
        .session_send(
            &session.provider,
            &session.invocation.session_id,
            json!({
                "t": "s.close",
                "v": 1,
                "session_id": session.invocation.session_id,
                "reason": "redispatch",
            }),
        )
        .await;

    let partials = vec![partial];
    let retry_request = redispatch_request_with_partials(&session.request, &partials);
    let GatewaySessionRun {
        result,
        invocation,
        metering_request,
        metering_output,
    } = run_chat_with_route_retry(
        &session.state,
        &session.model,
        &retry_request,
        session.options.clone(),
    )
    .await
    .map_err(|err| GatewaySessionError::new(err.message))?;
    let receipt = session
        .state
        .meter_chat_session(
            &session.model,
            &metering_request,
            &metering_output,
            &invocation,
            result.provider_receipt.as_ref(),
        )
        .map_err(|err| GatewaySessionError::new(err.message))?;
    session
        .state
        .maybe_run_canary_probe_after_session(&session.model, &invocation)
        .await;

    let mut output = result.output.clone();
    if output.tool_call.is_none() {
        let content = output.content.take().unwrap_or_default();
        output.content = Some(strip_live_streamed_prefix(
            &content,
            &partial_text(&partials),
        ));
    }
    add_partial_usage_to_output(&mut output, &partials);
    let mayhem_meta = ResponseMayhemMeta {
        backend: &result.backend,
        direct_session: result.direct_session,
        billable: !session.state.dev_session_shim,
        dev_session: session.state.dev_session_shim,
        hedge: invocation.hedge.clone(),
    };
    let _ = send_live_chat_tail(
        tx,
        &session.id,
        session.created,
        &session.model.id,
        &output,
        Some(&receipt),
        mayhem_meta,
        session.include_usage,
    )
    .await;
    Ok(())
}

async fn run_live_direct_chat_sse_inner(
    session: &mut LiveDirectChatSession,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) -> Result<(), GatewaySessionError> {
    if !send_sse_value(
        tx,
        chat_chunk(
            &session.id,
            session.created,
            &session.model.id,
            json!({ "role": "assistant" }),
            None,
            None,
        ),
    )
    .await
    {
        return Err(GatewaySessionError::retryable(
            "end-user disconnected before first stream event",
        ));
    }

    let mut content = String::new();
    let mut tool_call = None;
    let mut finish_reason = None;
    let mut claimed_usage = None;
    let mut final_provider_receipt = None;
    let mut latest_checkpoint_receipt = None;
    let mut latest_checkpoint_ack_frame: Option<Value> = None;
    let mut pending_checkpoint_receipt: Option<ProviderSignedReceipt> = None;
    let mut token_ids = Vec::new();
    let mut artifact_builders = BTreeMap::new();
    let mut delta_payload_chunks = SessionDeltaPayloadChunks::default();
    let started_at_millis = now_millis_u64();
    let failover = session.invocation.failover;
    let mut watchdog = DirectSessionWatchdog::new(
        started_at_millis,
        failover.ttft_timeout(),
        failover.stall_timeout(),
        None,
        failover.min_tok_s,
    );

    while finish_reason.is_none() || final_provider_receipt.is_none() {
        let remaining_millis = watchdog
            .next_wait_millis(now_millis_u64())
            .map_err(|kind| direct_session_timeout_error(kind, &session.invocation.session_id))?;
        let wait_millis = if latest_checkpoint_ack_frame.is_some() {
            remaining_millis.min(DIRECT_SESSION_CHECKPOINT_ACK_RESEND_MS)
        } else {
            remaining_millis
        };
        let frame = match next_session_frame(
            &mut session.bridge,
            &session.invocation.session_id,
            Duration::from_millis(wait_millis),
            &[
                "s.delta",
                "s.delta_chunk",
                "s.receipt",
                "s.error",
                "s.close",
            ],
        )
        .await
        {
            Ok(frame) => frame,
            Err(err) if err.message.starts_with("timed out waiting") => {
                let now = now_millis_u64();
                if let Some(ack_frame) = latest_checkpoint_ack_frame.clone() {
                    if watchdog.next_wait_millis(now).is_ok() {
                        send_direct_session_frame_with_peer_reconnect(
                            &mut session.bridge,
                            &session.provider,
                            &session.invocation.session_id,
                            ack_frame,
                            session.invocation.failover.open_timeout(),
                            "resending checkpoint s.receipt_ack",
                        )
                        .await?;
                        continue;
                    }
                }
                let timeout = watchdog.timeout_error(&session.invocation.session_id, now);
                if let Some(partial) = interrupted_direct_session_partial(
                    &content,
                    tool_call.clone(),
                    latest_checkpoint_receipt.as_ref(),
                    &token_ids,
                    &watchdog,
                    now,
                    "mid_stream_timeout",
                ) {
                    return Err(GatewaySessionError::retryable_partial(
                        timeout.message,
                        partial,
                    ));
                }
                return Err(timeout);
            }
            Err(err) if err.retryable => {
                let now = now_millis_u64();
                return Err(retryable_interrupted_direct_session_error(
                    err,
                    &content,
                    tool_call.clone(),
                    latest_checkpoint_receipt.as_ref(),
                    &token_ids,
                    &watchdog,
                    now,
                    "bridge_closed",
                ));
            }
            Err(err) => return Err(err),
        };
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta_chunk")
                if frame.get("rid").and_then(Value::as_str)
                    == Some(session.request_id.as_str()) =>
            {
                latest_checkpoint_ack_frame = None;
                watchdog.record_delta(now_millis_u64());
                collect_session_delta_chunk(&frame, &mut delta_payload_chunks)?;
            }
            Some("s.delta")
                if frame.get("rid").and_then(Value::as_str)
                    == Some(session.request_id.as_str()) =>
            {
                let now = now_millis_u64();
                latest_checkpoint_ack_frame = None;
                watchdog.record_delta(now);
                if let Some(delta) = frame.get("d").and_then(Value::as_str) {
                    content.push_str(delta);
                }
                if let Some(ids) = token_ids_from_session_delta(&frame) {
                    token_ids = ids;
                } else if let Some(ids) =
                    token_ids_ref_from_session_delta(&frame, &mut delta_payload_chunks)?
                {
                    token_ids = ids;
                } else if let Some(ids) = token_ids_delta_from_session_delta(&frame) {
                    token_ids.extend(ids);
                } else if let Some(token_id) = token_id_from_session_delta(&frame) {
                    token_ids.push(token_id);
                }
                if let Some(delta) = frame.get("d").and_then(Value::as_str) {
                    if !delta.is_empty()
                        && !send_sse_value(
                            tx,
                            chat_chunk(
                                &session.id,
                                session.created,
                                &session.model.id,
                                json!({ "content": delta }),
                                None,
                                None,
                            ),
                        )
                        .await
                    {
                        return Err(client_disconnect_direct_session_error(
                            &content,
                            tool_call.clone(),
                            latest_checkpoint_receipt.as_ref(),
                            &token_ids,
                            &watchdog,
                            now,
                        ));
                    }
                }
                if let Some(receipt) = pending_checkpoint_receipt.take() {
                    if let Some(ack_frame) = maybe_ack_direct_session_checkpoint_receipt(
                        &mut session.bridge,
                        &session.provider,
                        &session.invocation.session_id,
                        &session.request,
                        &session.invocation,
                        &content,
                        tool_call.clone(),
                        &receipt,
                        &token_ids,
                        &watchdog,
                        now,
                        &session.model,
                    )
                    .await?
                    {
                        latest_checkpoint_receipt = Some(receipt);
                        latest_checkpoint_ack_frame = Some(ack_frame);
                    } else {
                        pending_checkpoint_receipt = Some(receipt);
                    }
                }
                if tool_call.is_none() {
                    if let Some(next_tool_call) =
                        tool_call_from_session_delta_resolving(&frame, &mut delta_payload_chunks)?
                    {
                        if !send_sse_value(
                            tx,
                            chat_chunk(
                                &session.id,
                                session.created,
                                &session.model.id,
                                json!({ "tool_calls": [tool_call_value(&next_tool_call)] }),
                                None,
                                None,
                            ),
                        )
                        .await
                        {
                            return Err(client_disconnect_direct_session_error(
                                &content,
                                tool_call.clone(),
                                latest_checkpoint_receipt.as_ref(),
                                &token_ids,
                                &watchdog,
                                now,
                            ));
                        }
                        tool_call = Some(next_tool_call);
                    }
                }
                collect_artifact_from_session_delta(&frame, &mut artifact_builders)?;
                if let Some(fin) = frame.get("fin").and_then(Value::as_str) {
                    finish_reason = Some(fin.to_owned());
                    claimed_usage = usage_from_session_delta(&frame);
                }
                if finish_reason.is_none() {
                    let output_tokens = streamed_output_token_count(&content, &token_ids);
                    if let Some(tok_s) = watchdog.throughput_floor_violation(output_tokens, now) {
                        let err = direct_session_throughput_floor_error(
                            &session.invocation.session_id,
                            tok_s,
                            failover.min_tok_s.expect("floor checked"),
                        );
                        if let Some(partial) = interrupted_direct_session_partial(
                            &content,
                            tool_call.clone(),
                            latest_checkpoint_receipt.as_ref(),
                            &token_ids,
                            &watchdog,
                            now,
                            "throughput_floor",
                        ) {
                            return Err(GatewaySessionError::retryable_partial(err, partial));
                        }
                        return Err(GatewaySessionError::retryable(err));
                    }
                }
            }
            Some("s.receipt") => {
                let receipt = provider_signed_receipt_from_frame(
                    &frame,
                    &session.invocation.session_id,
                    &session.enclave_pubkey,
                )?;
                if receipt.body.final_receipt {
                    if pending_checkpoint_receipt.is_some() {
                        return Err(GatewaySessionError::new(
                            "provider sent final receipt before pending checkpoint was acknowledged",
                        ));
                    }
                    final_provider_receipt = Some(receipt);
                } else {
                    if pending_checkpoint_receipt.is_some() {
                        return Err(GatewaySessionError::new(
                            "provider sent checkpoint receipt before previous checkpoint was acknowledged",
                        ));
                    }
                    let now = now_millis_u64();
                    if let Some(ack_frame) = maybe_ack_direct_session_checkpoint_receipt(
                        &mut session.bridge,
                        &session.provider,
                        &session.invocation.session_id,
                        &session.request,
                        &session.invocation,
                        &content,
                        tool_call.clone(),
                        &receipt,
                        &token_ids,
                        &watchdog,
                        now,
                        &session.model,
                    )
                    .await?
                    {
                        latest_checkpoint_receipt = Some(receipt);
                        latest_checkpoint_ack_frame = Some(ack_frame);
                    } else {
                        pending_checkpoint_receipt = Some(receipt);
                    }
                }
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
                let err = format!(
                    "provider returned {code} on session {}: {message}",
                    session.invocation.session_id
                );
                if let Some(partial) = interrupted_direct_session_partial(
                    &content,
                    tool_call.clone(),
                    latest_checkpoint_receipt.as_ref(),
                    &token_ids,
                    &watchdog,
                    now_millis_u64(),
                    "mid_stream_error",
                ) {
                    return Err(GatewaySessionError::retryable_partial(err, partial));
                }
                return Err(GatewaySessionError::new(err));
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if finish_reason.is_none() {
                    let err = format!(
                        "provider closed session {} before final delta: {reason}",
                        session.invocation.session_id
                    );
                    if let Some(partial) = interrupted_direct_session_partial(
                        &content,
                        tool_call.clone(),
                        latest_checkpoint_receipt.as_ref(),
                        &token_ids,
                        &watchdog,
                        now_millis_u64(),
                        "mid_stream_close",
                    ) {
                        return Err(GatewaySessionError::retryable_partial(err, partial));
                    }
                    return Err(GatewaySessionError::new(err));
                }
                return Err(GatewaySessionError::new(format!(
                    "provider closed session {} before s.receipt: {reason}",
                    session.invocation.session_id
                )));
            }
            _ => {}
        }
    }

    let usage = observed_chat_usage(&session.request, &content, &token_ids);
    if let Some(claimed) = claimed_usage {
        if claimed != usage {
            return Err(GatewaySessionError::new(format!(
                "provider reported usage {:?} did not match gateway-observed usage {:?}",
                claimed, usage
            )));
        }
    }
    let completed_at_millis = now_millis_u64();
    let quality =
        watchdog
            .first_delta_at_millis
            .map(|first_delta_at_millis| GatewaySessionQuality {
                ttft_ms: first_delta_at_millis.saturating_sub(started_at_millis),
                tok_s: generated_tokens_per_second(
                    usage.completion_tokens,
                    first_delta_at_millis,
                    completed_at_millis,
                ),
            });
    let artifacts = finish_session_artifacts(artifact_builders)?;
    let output = ChatOutput {
        content: tool_call.is_none().then_some(content),
        tool_call,
        artifacts,
        finish_reason: finish_reason.expect("loop ended with final delta"),
        usage,
    };
    let provider_receipt = final_provider_receipt.expect("loop ended with provider receipt");
    let receipt_ack = direct_session_receipt_ack(
        &session.request,
        &output,
        &session.invocation,
        &provider_receipt,
        &session.provider,
        &session.model,
    )?;
    send_direct_session_frame_with_peer_reconnect(
        &mut session.bridge,
        &session.provider,
        &session.invocation.session_id,
        json!({
            "t": "s.receipt_ack",
            "v": 1,
            "session_id": receipt_ack.session_id,
            "seq": receipt_ack.seq,
            "user_sig": receipt_ack.user_sig,
        }),
        session.invocation.failover.open_timeout(),
        "sending s.receipt_ack",
    )
    .await?;
    let _ = session
        .bridge
        .session_send(
            &session.provider,
            &session.invocation.session_id,
            json!({
                "t": "s.close",
                "v": 1,
                "session_id": session.invocation.session_id,
                "reason": "done",
            }),
        )
        .await;

    let result = GatewaySessionResult {
        output: output.clone(),
        backend: session.backend.clone(),
        direct_session: true,
        provider_receipt: Some(provider_receipt.clone()),
        token_ids,
        quality,
    };
    let receipt = session
        .state
        .meter_chat_session(
            &session.model,
            &session.request,
            &output,
            &session.invocation,
            Some(&provider_receipt),
        )
        .map_err(|err| GatewaySessionError::new(err.message))?;
    record_route_observation(
        &session.state,
        session.route.as_ref(),
        observation_sample_from_success(&result, session.attempt_started.elapsed()),
    );
    if let Some(route) = session.route.as_ref() {
        session
            .state
            .record_chat_affinity(&session.model, &session.request, route);
    }
    session
        .state
        .maybe_run_canary_probe_after_session(&session.model, &session.invocation)
        .await;

    let mut finish_chunk = chat_chunk(
        &session.id,
        session.created,
        &session.model.id,
        json!({}),
        Some(output.finish_reason.as_str()),
        None,
    );
    if !output.artifacts.is_empty() {
        finish_chunk["mayhem"] = json!({
            "artifacts": artifact_summaries(&output.artifacts),
        });
    }
    if !send_sse_value(tx, finish_chunk).await {
        return Ok(());
    }
    if session.include_usage {
        let mayhem_meta = ResponseMayhemMeta {
            backend: &session.backend,
            direct_session: true,
            billable: true,
            dev_session: false,
            hedge: session.invocation.hedge.clone(),
        };
        let usage_chunk = json!({
            "id": session.id,
            "object": "chat.completion.chunk",
            "created": session.created,
            "model": session.model.id,
            "choices": [],
            "usage": output.usage,
            "mayhem": {
                "backend": mayhem_meta.backend,
                "direct_session": mayhem_meta.direct_session,
                "billable": mayhem_meta.billable,
                "dev_session": mayhem_meta.dev_session,
                "artifacts": artifact_summaries(&output.artifacts),
                "hedge": {
                    "requested": mayhem_meta.hedge.requested,
                    "planned_probe_count": mayhem_meta.hedge.planned_probe_count,
                    "actual_probe_count": mayhem_meta.hedge.actual_probe_count,
                    "winner_provider": mayhem_meta.hedge.winner_provider,
                    "winner_ttft_ms": mayhem_meta.hedge.winner_ttft_ms,
                },
                "receipt": receipt_summary(&receipt),
            },
        });
        let _ = send_sse_value(tx, usage_chunk).await;
    }
    Ok(())
}

async fn send_live_chat_tail(
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    id: &str,
    created: u64,
    model_id: &str,
    output: &ChatOutput,
    receipt: Option<&StoredReceipt>,
    mayhem_meta: ResponseMayhemMeta<'_>,
    include_usage: bool,
) -> bool {
    if let Some(tool_call) = &output.tool_call {
        if !send_sse_value(
            tx,
            chat_chunk(
                id,
                created,
                model_id,
                json!({ "tool_calls": [tool_call_value(tool_call)] }),
                None,
                None,
            ),
        )
        .await
        {
            return false;
        }
    } else if let Some(content) = &output.content {
        for part in stream_parts(content) {
            if !send_sse_value(
                tx,
                chat_chunk(
                    id,
                    created,
                    model_id,
                    json!({ "content": part }),
                    None,
                    None,
                ),
            )
            .await
            {
                return false;
            }
        }
    }

    let mut finish_chunk = chat_chunk(
        id,
        created,
        model_id,
        json!({}),
        Some(output.finish_reason.as_str()),
        None,
    );
    if !output.artifacts.is_empty() {
        finish_chunk["mayhem"] = json!({
            "artifacts": artifact_summaries(&output.artifacts),
        });
    }
    if !send_sse_value(tx, finish_chunk).await {
        return false;
    }
    if include_usage {
        let usage_chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model_id,
            "choices": [],
            "usage": output.usage,
            "mayhem": {
                "backend": mayhem_meta.backend,
                "direct_session": mayhem_meta.direct_session,
                "billable": mayhem_meta.billable,
                "dev_session": mayhem_meta.dev_session,
                "artifacts": artifact_summaries(&output.artifacts),
                "hedge": {
                    "requested": mayhem_meta.hedge.requested,
                    "planned_probe_count": mayhem_meta.hedge.planned_probe_count,
                    "actual_probe_count": mayhem_meta.hedge.actual_probe_count,
                    "winner_provider": mayhem_meta.hedge.winner_provider,
                    "winner_ttft_ms": mayhem_meta.hedge.winner_ttft_ms,
                },
                "receipt": receipt.map(receipt_summary),
            },
        });
        return send_sse_value(tx, usage_chunk).await;
    }
    true
}

fn strip_live_streamed_prefix(candidate: &str, streamed_prefix: &str) -> String {
    if candidate.is_empty() || streamed_prefix.is_empty() {
        return candidate.to_owned();
    }
    if let Some(stripped) = candidate.strip_prefix(streamed_prefix) {
        return stripped.to_owned();
    }
    let max_overlap = candidate.len().min(streamed_prefix.len());
    for overlap in (1..=max_overlap).rev() {
        if !candidate.is_char_boundary(overlap) {
            continue;
        }
        let start = streamed_prefix.len().saturating_sub(overlap);
        if !streamed_prefix.is_char_boundary(start) {
            continue;
        }
        if streamed_prefix[start..] == candidate[..overlap] {
            return candidate[overlap..].to_owned();
        }
    }
    candidate.to_owned()
}

fn add_partial_usage_to_output(output: &mut ChatOutput, partials: &[GatewaySessionPartial]) {
    let partial_completion = partials
        .iter()
        .map(|partial| partial.output.usage.completion_tokens)
        .sum::<u64>();
    output.usage.completion_tokens = output
        .usage
        .completion_tokens
        .saturating_add(partial_completion);
    output.usage.total_tokens = output
        .usage
        .prompt_tokens
        .saturating_add(output.usage.completion_tokens);
}

async fn send_sse_value(
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    value: Value,
) -> bool {
    tx.send(Ok(sse_event_from_value(value))).await.is_ok()
}

async fn send_sse_error(
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    message: &str,
) -> bool {
    send_sse_value(
        tx,
        json!({
            "error": {
                "message": message,
                "type": "mayhem_stream_error",
            },
        }),
    )
    .await
}

async fn send_sse_done(tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>) -> bool {
    tx.send(Ok(Event::default().data("[DONE]"))).await.is_ok()
}

fn sse_event_from_value(value: Value) -> Event {
    Event::default()
        .json_data(value)
        .unwrap_or_else(|_| Event::default().data("{}"))
}

async fn run_chat_with_route_retry(
    state: &GatewayState,
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    options: GatewayRequestOptions,
) -> Result<GatewaySessionRun, ApiError> {
    let eligible_routes =
        ordered_route_candidates_for_request_with_options(state, model, request, &options);
    let RouteWaitOutcome {
        routes: mut eligible_routes,
        waited,
    } = wait_for_eligible_routes(state, model, &options, eligible_routes, || {
        ordered_route_candidates_for_request_with_options(state, model, request, &options)
    })
    .await;
    if !model.mayhem.route_candidates.is_empty() && eligible_routes.is_empty() {
        if waited {
            return Err(route_wait_expired_error(&options));
        }
        return Err(no_eligible_route_error(state, model, &options));
    }
    if eligible_routes.is_empty() && !state.dev_session_shim {
        return Err(ApiError::service_unavailable(
            "no provider available: production gateway requires an active provider joined to an admin-created room",
            Some("model"),
        ));
    }
    let mut attempt_request = request.clone();
    let hedge_probe =
        run_hedge_probes_if_requested(state, model, &attempt_request, &eligible_routes, &options)
            .await?;
    if let Some(winner) = hedge_probe.winner.as_ref() {
        if let Some(winner_index) = eligible_routes
            .iter()
            .position(|route| route.provider == winner.provider)
        {
            let winner_route = eligible_routes.remove(winner_index);
            eligible_routes.insert(0, winner_route);
        }
    }
    let attempt_count = if eligible_routes.is_empty() {
        1
    } else {
        eligible_routes
            .len()
            .min(usize::from(DEFAULT_MAX_OPEN_ATTEMPTS))
    };
    let mut last_retryable_error = None;
    let mut partials = Vec::new();

    for attempt_index in 0..attempt_count {
        let route = eligible_routes.get(attempt_index).copied();
        let invocation =
            state.prepare_chat_invocation_for_route(model, &attempt_request, route, &options)?;
        let invocation = invocation.with_hedge_probe_outcome(&hedge_probe);
        let attempt_started = Instant::now();
        match state
            .session_backend
            .run_chat(model, &attempt_request, &invocation)
            .await
        {
            Ok(mut result) => {
                if let Some(message) = throughput_floor_retry_message(&result, &invocation.failover)
                {
                    if result.provider_receipt.is_none() {
                        if let Some(route) = route {
                            state.cool_route_provider(route, now_millis_u64());
                        }
                        record_route_observation(
                            state,
                            route,
                            observation_sample_from_floor_failure(
                                &result,
                                attempt_started.elapsed(),
                            ),
                        );
                        last_retryable_error = Some(message);
                        continue;
                    }
                }
                record_route_observation(
                    state,
                    route,
                    observation_sample_from_success(&result, attempt_started.elapsed()),
                );
                if let Some(route) = route {
                    state.record_chat_affinity(model, request, route);
                }
                let metering_request = attempt_request.clone();
                let metering_output = result.output.clone();
                if !partials.is_empty() {
                    stitch_partials_into_result(&mut result, &partials);
                }
                return Ok(GatewaySessionRun {
                    result,
                    invocation,
                    metering_request,
                    metering_output,
                });
            }
            Err(err) if err.retryable => {
                record_retryable_route_attempt(state, route, attempt_started.elapsed(), &err);
                if let Some(partial) = err.partial.as_ref() {
                    state.record_partial_provider_receipt(
                        model,
                        &attempt_request,
                        &invocation,
                        partial,
                    )?;
                }
                if let Some(partial) = err.partial {
                    partials.push(*partial);
                    attempt_request = redispatch_request_with_partials(request, &partials);
                }
                last_retryable_error = Some(err.message);
            }
            Err(err) => {
                record_route_failure_attempt(state, route, attempt_started.elapsed());
                return Err(ApiError::bad_gateway(err.message, Some("model")));
            }
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

fn ordered_route_candidates_for_request_with_options<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    request: &ChatCompletionRequest,
    options: &GatewayRequestOptions,
) -> Vec<&'a GatewayRouteCandidate> {
    let seed = route_selection_seed(model, request, now_millis_u64());
    ordered_route_candidates_for_request_with_max_price_seed(
        state,
        model,
        request,
        options.min_att_tier,
        options.max_price_mu,
        options.min_ctx,
        options.quant.as_deref(),
        state.generation_floor_tok_s_for_model(model, options),
        seed,
    )
}

fn ordered_route_candidates_for_embedding_with_options<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    inputs: &[String],
    options: &GatewayRequestOptions,
) -> Vec<&'a GatewayRouteCandidate> {
    let seed = embedding_route_selection_seed(model, inputs, now_millis_u64());
    ordered_route_candidates_for_embedding_with_max_price_seed(
        state,
        model,
        inputs,
        options.min_att_tier,
        options.max_price_mu,
        options.quant.as_deref(),
        state.throughput_floor_for_model(
            model,
            options,
            DEFAULT_EMBEDDING_INPUT_TOKENS_FLOOR_PER_S,
        ),
        seed,
    )
}

fn ordered_route_candidates_for_image_generation_with_options<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    request: &ImageGenerationRequest,
    options: &GatewayRequestOptions,
) -> Vec<&'a GatewayRouteCandidate> {
    let seed = image_generation_route_selection_seed(model, request, now_millis_u64());
    ordered_route_candidates_for_image_generation_with_max_price_seed(
        state,
        model,
        request,
        options.min_att_tier,
        options.max_price_mu,
        options.quant.as_deref(),
        state.throughput_floor_for_model(model, options, DEFAULT_IMAGE_FLOOR_IMAGES_PER_S),
        seed,
    )
}

fn ordered_route_candidates_for_audio_speech_with_options<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    request: &AudioSpeechRequest,
    options: &GatewayRequestOptions,
) -> Vec<&'a GatewayRouteCandidate> {
    let seed = audio_speech_route_selection_seed(model, request, now_millis_u64());
    let now_millis = now_millis_u64();
    ordered_route_candidates_for_requirements_with_seed(
        state,
        model,
        options.min_att_tier,
        options.quant.as_deref(),
        seed,
        request_requirements_for_audio_speech(
            state,
            model,
            request,
            now_millis,
            options.max_price_mu,
            state.throughput_floor_for_model(model, options, DEFAULT_AUDIO_REALTIME_FACTOR_FLOOR),
        ),
        now_millis,
    )
}

fn ordered_route_candidates_for_audio_transcription_with_options<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    request: &AudioTranscriptionRequest,
    options: &GatewayRequestOptions,
) -> Vec<&'a GatewayRouteCandidate> {
    let seed = audio_transcription_route_selection_seed(model, request, now_millis_u64());
    let now_millis = now_millis_u64();
    ordered_route_candidates_for_requirements_with_seed(
        state,
        model,
        options.min_att_tier,
        options.quant.as_deref(),
        seed,
        request_requirements_for_audio_transcription(
            state,
            model,
            request,
            now_millis,
            options.max_price_mu,
            state.throughput_floor_for_model(model, options, DEFAULT_AUDIO_REALTIME_FACTOR_FLOOR),
        ),
        now_millis,
    )
}

async fn run_hedge_probes_if_requested(
    state: &GatewayState,
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    routes: &[&GatewayRouteCandidate],
    options: &GatewayRequestOptions,
) -> Result<GatewayHedgeProbeOutcome, ApiError> {
    if !options.hedge_requested || routes.len() < 2 {
        return Ok(GatewayHedgeProbeOutcome::default());
    }
    let first_invocation =
        state.prepare_chat_invocation_for_route(model, request, Some(routes[0]), options)?;
    let second_invocation =
        state.prepare_chat_invocation_for_route(model, request, Some(routes[1]), options)?;
    let (first, second) = tokio::join!(
        state
            .session_backend
            .hedge_probe(model, request, &first_invocation),
        state
            .session_backend
            .hedge_probe(model, request, &second_invocation)
    );
    let mut successes = [first.ok(), second.ok()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    successes.sort_by(|left, right| left.ttft_ms.cmp(&right.ttft_ms));
    Ok(GatewayHedgeProbeOutcome {
        actual_probe_count: 2,
        winner: successes.into_iter().next(),
    })
}

#[cfg(test)]
fn ordered_route_candidates_for_request_with_seed<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    request: &ChatCompletionRequest,
    min_att_tier: Option<u8>,
    seed: u64,
) -> Vec<&'a GatewayRouteCandidate> {
    ordered_route_candidates_for_request_with_max_price_seed(
        state,
        model,
        request,
        min_att_tier,
        None,
        None,
        None,
        state.generation_floor_tok_s_for_model(model, &GatewayRequestOptions::default()),
        seed,
    )
}

fn ordered_route_candidates_for_request_with_max_price_seed<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    request: &ChatCompletionRequest,
    min_att_tier: Option<u8>,
    max_price_mu: Option<u64>,
    min_ctx: Option<u32>,
    quant: Option<&str>,
    min_throughput: Option<f64>,
    seed: u64,
) -> Vec<&'a GatewayRouteCandidate> {
    let now_millis = now_millis_u64();
    let eligible_routes =
        eligible_route_candidates(model, min_att_tier, quant, &state.receipt_config.rail)
            .into_iter()
            .filter(|route| !state.route_provider_in_cooloff(route, now_millis))
            .collect::<Vec<_>>();
    if eligible_routes.is_empty() {
        return eligible_routes;
    }

    state.refresh_provider_table_routes(model);
    let requirements = request_requirements_for_chat(
        state,
        model,
        request,
        now_millis,
        max_price_mu,
        min_ctx,
        min_throughput,
    );
    let route_by_key = eligible_routes
        .iter()
        .map(|candidate| (route_key(candidate), *candidate))
        .collect::<BTreeMap<_, _>>();
    let mut remaining_entries = {
        let table = state
            .provider_table
            .lock()
            .expect("provider table poisoned");
        table
            .entries(now_millis)
            .into_iter()
            .filter(|entry| route_by_key.contains_key(&entry.key))
            .collect::<Vec<_>>()
    };
    let weights = SelectionWeights::default();
    let table_entry_keys = remaining_entries
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<BTreeSet<_>>();
    let table_eligible_keys =
        crate::provider_table::eligible_candidates(&remaining_entries, &requirements, &weights)
            .into_iter()
            .map(|candidate| candidate.entry.key)
            .collect::<BTreeSet<_>>();
    let eligible_routes = eligible_routes
        .into_iter()
        .filter(|candidate| {
            let key = route_key(candidate);
            table_entry_keys.contains(&key) && table_eligible_keys.contains(&key)
        })
        .collect::<Vec<_>>();
    if eligible_routes.len() <= 1 {
        return eligible_routes;
    }
    let mut rng = LcgBalancerRng::seeded(seed);
    let mut ordered = Vec::new();
    let mut selected_keys = BTreeSet::new();

    while !remaining_entries.is_empty() {
        let Some(selection) = crate::provider_table::select_weighted_p2c(
            &remaining_entries,
            &requirements,
            &weights,
            &mut rng,
        ) else {
            break;
        };
        let key = selection.selected.entry.key;
        if let Some(candidate) = route_by_key.get(&key).copied() {
            ordered.push(candidate);
            selected_keys.insert(key.clone());
        }
        remaining_entries.retain(|entry| entry.key != key);
    }

    for candidate in eligible_routes {
        let key = route_key(candidate);
        if table_entry_keys.contains(&key) && !table_eligible_keys.contains(&key) {
            continue;
        }
        if selected_keys.insert(key) {
            ordered.push(candidate);
        }
    }
    prioritize_material_reputation_gap(state.apply_chat_affinity(model, request, ordered))
}

fn ordered_route_candidates_for_embedding_with_max_price_seed<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    inputs: &[String],
    min_att_tier: Option<u8>,
    max_price_mu: Option<u64>,
    quant: Option<&str>,
    min_throughput: Option<f64>,
    seed: u64,
) -> Vec<&'a GatewayRouteCandidate> {
    let now_millis = now_millis_u64();
    let eligible_routes =
        eligible_route_candidates(model, min_att_tier, quant, &state.receipt_config.rail)
            .into_iter()
            .filter(|route| !state.route_provider_in_cooloff(route, now_millis))
            .collect::<Vec<_>>();
    if eligible_routes.is_empty() {
        return eligible_routes;
    }

    state.refresh_provider_table_routes(model);
    let requirements = request_requirements_for_embedding(
        state,
        model,
        inputs,
        now_millis,
        max_price_mu,
        min_throughput,
    );
    let route_by_key = eligible_routes
        .iter()
        .map(|candidate| (route_key(candidate), *candidate))
        .collect::<BTreeMap<_, _>>();
    let mut remaining_entries = {
        let table = state
            .provider_table
            .lock()
            .expect("provider table poisoned");
        table
            .entries(now_millis)
            .into_iter()
            .filter(|entry| route_by_key.contains_key(&entry.key))
            .collect::<Vec<_>>()
    };
    let weights = SelectionWeights::default();
    let table_entry_keys = remaining_entries
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<BTreeSet<_>>();
    let table_eligible_keys =
        crate::provider_table::eligible_candidates(&remaining_entries, &requirements, &weights)
            .into_iter()
            .map(|candidate| candidate.entry.key)
            .collect::<BTreeSet<_>>();
    let eligible_routes = eligible_routes
        .into_iter()
        .filter(|candidate| {
            let key = route_key(candidate);
            table_entry_keys.contains(&key) && table_eligible_keys.contains(&key)
        })
        .collect::<Vec<_>>();
    if eligible_routes.len() <= 1 {
        return eligible_routes;
    }
    let mut rng = LcgBalancerRng::seeded(seed);
    let mut ordered = Vec::new();
    let mut selected_keys = BTreeSet::new();

    while !remaining_entries.is_empty() {
        let Some(selection) = crate::provider_table::select_weighted_p2c(
            &remaining_entries,
            &requirements,
            &weights,
            &mut rng,
        ) else {
            break;
        };
        let key = selection.selected.entry.key;
        if let Some(candidate) = route_by_key.get(&key).copied() {
            ordered.push(candidate);
            selected_keys.insert(key.clone());
        }
        remaining_entries.retain(|entry| entry.key != key);
    }

    for candidate in eligible_routes {
        let key = route_key(candidate);
        if table_entry_keys.contains(&key) && !table_eligible_keys.contains(&key) {
            continue;
        }
        if selected_keys.insert(key) {
            ordered.push(candidate);
        }
    }
    prioritize_material_reputation_gap(ordered)
}

fn ordered_route_candidates_for_image_generation_with_max_price_seed<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    request: &ImageGenerationRequest,
    min_att_tier: Option<u8>,
    max_price_mu: Option<u64>,
    quant: Option<&str>,
    min_throughput: Option<f64>,
    seed: u64,
) -> Vec<&'a GatewayRouteCandidate> {
    let now_millis = now_millis_u64();
    let eligible_routes =
        eligible_route_candidates(model, min_att_tier, quant, &state.receipt_config.rail)
            .into_iter()
            .filter(|route| !state.route_provider_in_cooloff(route, now_millis))
            .collect::<Vec<_>>();
    if eligible_routes.is_empty() {
        return eligible_routes;
    }

    state.refresh_provider_table_routes(model);
    let requirements = request_requirements_for_image_generation(
        state,
        model,
        request,
        now_millis,
        max_price_mu,
        min_throughput,
    );
    let route_by_key = eligible_routes
        .iter()
        .map(|candidate| (route_key(candidate), *candidate))
        .collect::<BTreeMap<_, _>>();
    let mut remaining_entries = {
        let table = state
            .provider_table
            .lock()
            .expect("provider table poisoned");
        table
            .entries(now_millis)
            .into_iter()
            .filter(|entry| route_by_key.contains_key(&entry.key))
            .collect::<Vec<_>>()
    };
    let weights = SelectionWeights::default();
    let table_entry_keys = remaining_entries
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<BTreeSet<_>>();
    let table_eligible_keys =
        crate::provider_table::eligible_candidates(&remaining_entries, &requirements, &weights)
            .into_iter()
            .map(|candidate| candidate.entry.key)
            .collect::<BTreeSet<_>>();
    let eligible_routes = eligible_routes
        .into_iter()
        .filter(|candidate| {
            let key = route_key(candidate);
            table_entry_keys.contains(&key) && table_eligible_keys.contains(&key)
        })
        .collect::<Vec<_>>();
    if eligible_routes.len() <= 1 {
        return eligible_routes;
    }
    let mut rng = LcgBalancerRng::seeded(seed);
    let mut ordered = Vec::new();
    let mut selected_keys = BTreeSet::new();

    while !remaining_entries.is_empty() {
        let Some(selection) = crate::provider_table::select_weighted_p2c(
            &remaining_entries,
            &requirements,
            &weights,
            &mut rng,
        ) else {
            break;
        };
        let key = selection.selected.entry.key;
        if let Some(candidate) = route_by_key.get(&key).copied() {
            ordered.push(candidate);
            selected_keys.insert(key.clone());
        }
        remaining_entries.retain(|entry| entry.key != key);
    }

    for candidate in eligible_routes {
        let key = route_key(candidate);
        if table_entry_keys.contains(&key) && !table_eligible_keys.contains(&key) {
            continue;
        }
        if selected_keys.insert(key) {
            ordered.push(candidate);
        }
    }
    prioritize_material_reputation_gap(ordered)
}

fn ordered_route_candidates_for_requirements_with_seed<'a>(
    state: &GatewayState,
    model: &'a GatewayModel,
    min_att_tier: Option<u8>,
    quant: Option<&str>,
    seed: u64,
    requirements: RequestRequirements,
    now_millis: u64,
) -> Vec<&'a GatewayRouteCandidate> {
    let eligible_routes =
        eligible_route_candidates(model, min_att_tier, quant, &state.receipt_config.rail)
            .into_iter()
            .filter(|route| !state.route_provider_in_cooloff(route, now_millis))
            .collect::<Vec<_>>();
    if eligible_routes.is_empty() {
        return eligible_routes;
    }

    state.refresh_provider_table_routes(model);
    let route_by_key = eligible_routes
        .iter()
        .map(|candidate| (route_key(candidate), *candidate))
        .collect::<BTreeMap<_, _>>();
    let mut remaining_entries = {
        let table = state
            .provider_table
            .lock()
            .expect("provider table poisoned");
        table
            .entries(now_millis)
            .into_iter()
            .filter(|entry| route_by_key.contains_key(&entry.key))
            .collect::<Vec<_>>()
    };
    let weights = SelectionWeights::default();
    let table_entry_keys = remaining_entries
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<BTreeSet<_>>();
    let table_eligible_keys =
        crate::provider_table::eligible_candidates(&remaining_entries, &requirements, &weights)
            .into_iter()
            .map(|candidate| candidate.entry.key)
            .collect::<BTreeSet<_>>();
    let eligible_routes = eligible_routes
        .into_iter()
        .filter(|candidate| {
            let key = route_key(candidate);
            table_entry_keys.contains(&key) && table_eligible_keys.contains(&key)
        })
        .collect::<Vec<_>>();
    if eligible_routes.len() <= 1 {
        return eligible_routes;
    }
    let mut rng = LcgBalancerRng::seeded(seed);
    let mut ordered = Vec::new();
    let mut selected_keys = BTreeSet::new();
    while !remaining_entries.is_empty() {
        let Some(selection) = crate::provider_table::select_weighted_p2c(
            &remaining_entries,
            &requirements,
            &weights,
            &mut rng,
        ) else {
            break;
        };
        let key = selection.selected.entry.key;
        if let Some(candidate) = route_by_key.get(&key).copied() {
            ordered.push(candidate);
            selected_keys.insert(key.clone());
        }
        remaining_entries.retain(|entry| entry.key != key);
    }
    for candidate in eligible_routes {
        let key = route_key(candidate);
        if table_entry_keys.contains(&key) && !table_eligible_keys.contains(&key) {
            continue;
        }
        if selected_keys.insert(key) {
            ordered.push(candidate);
        }
    }
    prioritize_material_reputation_gap(ordered)
}

impl GatewayState {
    fn refresh_provider_table_routes(&self, model: &GatewayModel) {
        let now = now_millis_u64();
        let mut table = self.provider_table.lock().expect("provider table poisoned");
        for candidate in &model.mayhem.route_candidates {
            table.upsert_contract(contract_snapshot_for_route(
                model,
                candidate,
                self.receipt_config.rules_ver,
            ));
            table.upsert_fallback_heartbeat(heartbeat_for_route(model, candidate, now), now);
        }
    }

    fn cool_route_provider(&self, route: &GatewayRouteCandidate, now_millis: u64) -> u64 {
        let cooled_until = now_millis.saturating_add(DEFAULT_PROVIDER_COOLOFF_MILLIS);
        self.provider_cooloffs
            .lock()
            .expect("provider cooloff map poisoned")
            .insert(route_key(route), cooled_until);
        cooled_until
    }

    fn route_provider_in_cooloff(&self, route: &GatewayRouteCandidate, now_millis: u64) -> bool {
        self.provider_cooloffs
            .lock()
            .expect("provider cooloff map poisoned")
            .get(&route_key(route))
            .is_some_and(|cooled_until| *cooled_until > now_millis)
    }

    fn served_ctx_for_route(
        &self,
        model: &GatewayModel,
        route: Option<&GatewayRouteCandidate>,
    ) -> u32 {
        let Some(route) = route else {
            return model_served_ctx(model);
        };
        let key = route_key(route);
        let now = now_millis_u64();
        self.provider_table
            .lock()
            .expect("provider table poisoned")
            .entries(now)
            .into_iter()
            .find(|entry| entry.key == key)
            .and_then(|entry| entry.heartbeat.map(|heartbeat| heartbeat.caps.ctx))
            .filter(|ctx| *ctx > 0)
            .unwrap_or_else(|| route_caps_ctx(model, route))
    }

    fn ctx_bracket_terms_for_served_ctx(
        &self,
        served_ctx: u32,
        at: u64,
    ) -> Result<(String, u32), ApiError> {
        ctx_bracket_for_tokens_in_schedule(served_ctx, &self.ctx_bracket_schedule, at).ok_or_else(
            || {
                ApiError::bad_gateway(
                    "active context bracket table does not cover served_ctx",
                    Some("model"),
                )
            },
        )
    }

    fn apply_chat_affinity<'a>(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        mut ordered: Vec<&'a GatewayRouteCandidate>,
    ) -> Vec<&'a GatewayRouteCandidate> {
        let Some(key) = chat_affinity_key(model, request) else {
            return ordered;
        };
        let Some(sticky_provider) = self
            .chat_affinity
            .lock()
            .expect("chat affinity map poisoned")
            .get(&key)
            .cloned()
        else {
            return ordered;
        };
        let Some(index) = ordered
            .iter()
            .position(|candidate| route_key(candidate) == sticky_provider)
        else {
            return ordered;
        };
        if index > 0 {
            let sticky = ordered.remove(index);
            ordered.insert(0, sticky);
        }
        ordered
    }

    fn record_chat_affinity(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        route: &GatewayRouteCandidate,
    ) {
        let Some(key) = chat_affinity_key(model, request) else {
            return;
        };
        self.chat_affinity
            .lock()
            .expect("chat affinity map poisoned")
            .insert(key, route_key(route));
    }
}

fn prioritize_material_reputation_gap<'a>(
    mut ordered: Vec<&'a GatewayRouteCandidate>,
) -> Vec<&'a GatewayRouteCandidate> {
    if ordered.len() < 2 {
        return ordered;
    }
    let Some((best_index, best_reputation)) = ordered
        .iter()
        .enumerate()
        .map(|(idx, candidate)| (idx, candidate.reputation_bps.min(10_000)))
        .max_by_key(|(_, reputation_bps)| *reputation_bps)
    else {
        return ordered;
    };
    let first_reputation = ordered[0].reputation_bps.min(10_000);
    if best_index > 0
        && best_reputation.saturating_sub(first_reputation) >= ROUTE_REPUTATION_PRIORITY_BPS_DELTA
    {
        let best = ordered.remove(best_index);
        ordered.insert(0, best);
    }
    ordered
}

fn route_key(candidate: &GatewayRouteCandidate) -> ProviderKey {
    ProviderKey::new(
        candidate.provider.clone(),
        candidate.enclave_id.clone(),
        candidate.room_id.clone(),
    )
}

fn chat_affinity_key(
    model: &GatewayModel,
    request: &ChatCompletionRequest,
) -> Option<ChatAffinityKey> {
    for field in [
        "conversation_id",
        "thread_id",
        "session_id",
        "mayhem_conversation_id",
        "mayhem_session_id",
    ] {
        if let Some(id) = request
            .metadata
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(ChatAffinityKey {
                model_id: model.id.clone(),
                conversation_id: format!("metadata:{field}:{id}"),
            });
        }
    }

    request
        .user
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|user| ChatAffinityKey {
            model_id: model.id.clone(),
            conversation_id: format!("user:{user}"),
        })
}

fn request_requirements_for_chat(
    state: &GatewayState,
    _model: &GatewayModel,
    request: &ChatCompletionRequest,
    now_millis: u64,
    max_price_mu: Option<u64>,
    explicit_min_ctx: Option<u32>,
    min_throughput: Option<f64>,
) -> RequestRequirements {
    let prompt_text = chat_prompt_text(request);
    let input_tokens = rough_tokens(&prompt_text);
    let output_tokens = u64::from(request.max_tokens.unwrap_or(1024).max(1));
    let prompt_min_ctx = input_tokens
        .saturating_add(output_tokens)
        .min(u64::from(u32::MAX)) as u32;
    RequestRequirements {
        current_rules_ver: state.receipt_config.rules_ver,
        requires_tools: request
            .tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty()),
        requires_json: request.response_format.is_some(),
        requires_vision: chat_input_modalities(&request.messages).image,
        min_ctx: explicit_min_ctx.unwrap_or(0).max(prompt_min_ctx),
        input_tokens,
        output_tokens,
        min_throughput,
        now_millis,
        max_price_mu,
        ..RequestRequirements::default()
    }
}

fn request_requirements_for_embedding(
    state: &GatewayState,
    _model: &GatewayModel,
    inputs: &[String],
    now_millis: u64,
    max_price_mu: Option<u64>,
    min_throughput: Option<f64>,
) -> RequestRequirements {
    let input_tokens = embedding_input_token_count(inputs);
    RequestRequirements {
        current_rules_ver: state.receipt_config.rules_ver,
        requires_tools: false,
        requires_json: false,
        requires_vision: false,
        min_ctx: input_tokens.min(u64::from(u32::MAX)) as u32,
        input_tokens,
        output_tokens: 0,
        min_throughput,
        now_millis,
        max_price_mu,
        ..RequestRequirements::default()
    }
}

fn request_requirements_for_image_generation(
    state: &GatewayState,
    _model: &GatewayModel,
    request: &ImageGenerationRequest,
    now_millis: u64,
    max_price_mu: Option<u64>,
    min_throughput: Option<f64>,
) -> RequestRequirements {
    let input_tokens = rough_tokens(&request.prompt);
    RequestRequirements {
        current_rules_ver: state.receipt_config.rules_ver,
        requires_tools: false,
        requires_json: false,
        requires_vision: false,
        min_ctx: input_tokens.min(u64::from(u32::MAX)) as u32,
        input_tokens,
        output_tokens: 0,
        min_throughput,
        now_millis,
        max_price_mu,
        ..RequestRequirements::default()
    }
}

fn request_requirements_for_audio_speech(
    state: &GatewayState,
    _model: &GatewayModel,
    request: &AudioSpeechRequest,
    now_millis: u64,
    max_price_mu: Option<u64>,
    min_throughput: Option<f64>,
) -> RequestRequirements {
    let input_tokens = rough_tokens(&request.input);
    RequestRequirements {
        current_rules_ver: state.receipt_config.rules_ver,
        requires_tools: false,
        requires_json: false,
        requires_vision: false,
        min_ctx: input_tokens.min(u64::from(u32::MAX)) as u32,
        input_tokens,
        output_tokens: 0,
        min_throughput,
        now_millis,
        max_price_mu,
        ..RequestRequirements::default()
    }
}

fn request_requirements_for_audio_transcription(
    state: &GatewayState,
    _model: &GatewayModel,
    request: &AudioTranscriptionRequest,
    now_millis: u64,
    max_price_mu: Option<u64>,
    min_throughput: Option<f64>,
) -> RequestRequirements {
    RequestRequirements {
        current_rules_ver: state.receipt_config.rules_ver,
        requires_tools: false,
        requires_json: false,
        requires_vision: false,
        min_ctx: 1,
        input_tokens: audio_transcription_seconds(request),
        output_tokens: 0,
        min_throughput,
        now_millis,
        max_price_mu,
        ..RequestRequirements::default()
    }
}

fn route_selection_seed(
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    now_millis: u64,
) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.id.as_bytes());
    hasher.update(chat_prompt_text(request).as_bytes());
    hasher.update(&request.seed.unwrap_or_default().to_le_bytes());
    hasher.update(&now_millis.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn embedding_route_selection_seed(model: &GatewayModel, inputs: &[String], now_millis: u64) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.id.as_bytes());
    hasher.update(embedding_prompt_text(inputs).as_bytes());
    hasher.update(&now_millis.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn image_generation_route_selection_seed(
    model: &GatewayModel,
    request: &ImageGenerationRequest,
    now_millis: u64,
) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.id.as_bytes());
    hasher.update(image_generation_prompt_text(request).as_bytes());
    hasher.update(&now_millis.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn audio_speech_route_selection_seed(
    model: &GatewayModel,
    request: &AudioSpeechRequest,
    now_millis: u64,
) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.id.as_bytes());
    hasher.update(audio_speech_prompt_hash(request).as_bytes());
    hasher.update(&now_millis.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn audio_transcription_route_selection_seed(
    model: &GatewayModel,
    request: &AudioTranscriptionRequest,
    now_millis: u64,
) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.id.as_bytes());
    hasher.update(audio_transcription_prompt_hash(request).as_bytes());
    hasher.update(&now_millis.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn record_route_observation(
    state: &GatewayState,
    route: Option<&GatewayRouteCandidate>,
    sample: ProviderObservationSample,
) {
    let Some(route) = route else {
        return;
    };
    let key = route_key(route);
    let maybe_event = state
        .provider_table
        .lock()
        .expect("provider table poisoned")
        .record_observation_at(&key, sample, now_millis_u64());
    if let Some(event) = maybe_event {
        state.record_reputation_event(stored_underdelivery_reputation_event(
            event,
            state.epoch_seconds,
        ));
    }
}

fn stored_underdelivery_reputation_event(
    event: ProviderUnderdeliveryEvent,
    epoch_seconds: u64,
) -> StoredReputationEvent {
    let at = event.observed_at_millis / 1_000;
    let epoch_seconds = epoch_seconds.max(1);
    let epoch = at / epoch_seconds + 1;
    let provider = event.key.provider.clone();
    let enclave_id = event.key.enclave_id.clone();
    let evidence = json!({
        "source": "mayhem-gateway-throughput-fairness-v1",
        "provider": provider,
        "enclave_id": enclave_id,
        "room_id": event.key.room_id,
        "epoch_seconds": epoch_seconds,
        "advertised_throughput": event.advertised_throughput,
        "measured_throughput": event.measured_throughput,
        "ratio": event.ratio,
        "streak": event.streak,
        "observed_at_millis": event.observed_at_millis,
    });
    let evidence_hash = stable_value_hash(&evidence);
    let event_id = format!(
        "underdelivery-{}",
        evidence_hash.get(..32).unwrap_or(evidence_hash.as_str())
    );
    let command = json!({
        "op": "record_rep_event",
        "provider": provider.clone(),
        "event_id": event_id.clone(),
        "kind": "underdelivery",
        "epoch": epoch,
        "at": at,
        "evidence_hash": evidence_hash.clone(),
        "enclave_id": enclave_id,
    });
    StoredReputationEvent {
        provider,
        event_id,
        kind: "underdelivery".to_owned(),
        epoch,
        at,
        evidence_hash,
        evidence,
        command,
    }
}

fn stored_capacity_mismatch_reputation_event(
    event: ProviderCapacityMismatchEvent,
    epoch_seconds: u64,
) -> StoredReputationEvent {
    let at = event.observed_at_millis / 1_000;
    let epoch_seconds = epoch_seconds.max(1);
    let epoch = at / epoch_seconds + 1;
    let provider = event.key.provider.clone();
    let enclave_id = event.key.enclave_id.clone();
    let evidence = json!({
        "source": "mayhem-gateway-capacity-mismatch-v1",
        "provider": provider,
        "enclave_id": enclave_id,
        "room_id": event.key.room_id,
        "epoch_seconds": epoch_seconds,
        "advertised_free_slots": event.advertised_free_slots,
        "advertised_engine_backlog": event.advertised_engine_backlog,
        "refusal_code": "CAPACITY",
        "streak": event.streak,
        "observed_at_millis": event.observed_at_millis,
    });
    let evidence_hash = stable_value_hash(&evidence);
    let event_id = format!(
        "capacity-mismatch-{}",
        evidence_hash.get(..32).unwrap_or(evidence_hash.as_str())
    );
    let command = json!({
        "op": "record_rep_event",
        "provider": provider.clone(),
        "event_id": event_id.clone(),
        "kind": "underdelivery",
        "epoch": epoch,
        "at": at,
        "evidence_hash": evidence_hash.clone(),
        "enclave_id": enclave_id,
    });
    StoredReputationEvent {
        provider,
        event_id,
        kind: "underdelivery".to_owned(),
        epoch,
        at,
        evidence_hash,
        evidence,
        command,
    }
}

fn record_route_failure_attempt(
    state: &GatewayState,
    route: Option<&GatewayRouteCandidate>,
    elapsed: Duration,
) {
    if let Some(route) = route {
        state.cool_route_provider(route, now_millis_u64());
    }
    record_route_observation(state, route, observation_sample_from_error(elapsed));
}

fn record_retryable_route_attempt(
    state: &GatewayState,
    route: Option<&GatewayRouteCandidate>,
    elapsed: Duration,
    err: &GatewaySessionError,
) {
    if err.clean_refusal
        && err
            .clean_refusal_code
            .as_deref()
            .is_some_and(|code| code == "CAPACITY")
    {
        record_capacity_mismatch_if_advertised(state, route);
    }
    if !err.clean_refusal {
        record_route_failure_attempt(state, route, elapsed);
    }
}

fn record_capacity_mismatch_if_advertised(
    state: &GatewayState,
    route: Option<&GatewayRouteCandidate>,
) {
    let Some(route) = route else {
        return;
    };
    let key = route_key(route);
    let now_millis = now_millis_u64();
    let maybe_event = {
        let mut table = state
            .provider_table
            .lock()
            .expect("provider table poisoned");
        let advertised_free_capacity = table
            .entries(now_millis)
            .into_iter()
            .find(|entry| entry.key == key)
            .and_then(|entry| entry.heartbeat)
            .is_some_and(|heartbeat| {
                heartbeat.accepting_new
                    && heartbeat.sat < DEFAULT_SATURATION_CUTOFF
                    && (heartbeat.q.free_slots > 0 || heartbeat.slots.active < heartbeat.slots.max)
            });
        if advertised_free_capacity {
            table.record_capacity_mismatch_at(&key, now_millis)
        } else {
            None
        }
    };
    if let Some(event) = maybe_event {
        state.record_reputation_event(stored_capacity_mismatch_reputation_event(
            event,
            state.epoch_seconds,
        ));
    }
}

fn observation_sample_from_success(
    result: &GatewaySessionResult,
    elapsed: Duration,
) -> ProviderObservationSample {
    let elapsed_millis = duration_millis_u64(elapsed).max(1);
    let elapsed_seconds = elapsed.as_secs_f64().max(0.001);
    ProviderObservationSample {
        ttft_ms: result
            .quality
            .map(|quality| quality.ttft_ms)
            .unwrap_or(elapsed_millis),
        tok_s: result
            .quality
            .and_then(|quality| quality.tok_s)
            .or_else(|| {
                Some(result.output.usage.completion_tokens as f64 / elapsed_seconds)
                    .filter(|tok_s| tok_s.is_finite() && *tok_s >= 0.0)
            }),
        error: false,
    }
}

fn observation_sample_from_embedding_success(
    result: &GatewayEmbeddingResult,
    elapsed: Duration,
) -> ProviderObservationSample {
    let elapsed_seconds = elapsed.as_secs_f64().max(0.001);
    ProviderObservationSample {
        ttft_ms: result
            .quality
            .map(|quality| quality.ttft_ms)
            .unwrap_or_else(|| duration_millis_u64(elapsed).max(1)),
        tok_s: result
            .quality
            .and_then(|quality| quality.tok_s)
            .or_else(|| {
                Some(result.output.usage.prompt_tokens as f64 / elapsed_seconds)
                    .filter(|value| value.is_finite() && *value >= 0.0)
            }),
        error: false,
    }
}

fn observation_sample_from_image_generation_success(
    result: &GatewayImageGenerationResult,
    elapsed: Duration,
) -> ProviderObservationSample {
    let elapsed_seconds = elapsed.as_secs_f64().max(0.001);
    ProviderObservationSample {
        ttft_ms: result
            .quality
            .map(|quality| quality.ttft_ms)
            .unwrap_or_else(|| duration_millis_u64(elapsed).max(1)),
        tok_s: result
            .quality
            .and_then(|quality| quality.tok_s)
            .or_else(|| {
                Some(result.output.usage.get(USAGE_IMAGE) as f64 / elapsed_seconds)
                    .filter(|value| value.is_finite() && *value >= 0.0)
            }),
        error: false,
    }
}

fn observation_sample_from_audio_speech_success(
    result: &GatewayAudioSpeechResult,
    elapsed: Duration,
) -> ProviderObservationSample {
    let elapsed_seconds = elapsed.as_secs_f64().max(0.001);
    ProviderObservationSample {
        ttft_ms: result
            .quality
            .map(|quality| quality.ttft_ms)
            .unwrap_or_else(|| duration_millis_u64(elapsed).max(1)),
        tok_s: result
            .quality
            .and_then(|quality| quality.tok_s)
            .or_else(|| {
                Some(result.output.usage.get(USAGE_AUDIO_SECOND) as f64 / elapsed_seconds)
                    .filter(|value| value.is_finite() && *value >= 0.0)
            }),
        error: false,
    }
}

fn observation_sample_from_audio_transcription_success(
    result: &GatewayAudioTranscriptionResult,
    elapsed: Duration,
) -> ProviderObservationSample {
    let elapsed_seconds = elapsed.as_secs_f64().max(0.001);
    ProviderObservationSample {
        ttft_ms: result
            .quality
            .map(|quality| quality.ttft_ms)
            .unwrap_or_else(|| duration_millis_u64(elapsed).max(1)),
        tok_s: result
            .quality
            .and_then(|quality| quality.tok_s)
            .or_else(|| {
                Some(result.output.usage.get(USAGE_AUDIO_SECOND) as f64 / elapsed_seconds)
                    .filter(|value| value.is_finite() && *value >= 0.0)
            }),
        error: false,
    }
}

fn throughput_floor_retry_message(
    result: &GatewaySessionResult,
    failover: &GatewayFailoverInvocation,
) -> Option<String> {
    let min_tok_s = failover.min_tok_s?;
    let tok_s = result.quality.and_then(|quality| quality.tok_s)?;
    (tok_s < min_tok_s)
        .then(|| format!("provider throughput {tok_s:.2} tok/s below floor {min_tok_s:.2} tok/s"))
}

fn observation_sample_from_floor_failure(
    result: &GatewaySessionResult,
    elapsed: Duration,
) -> ProviderObservationSample {
    let mut sample = observation_sample_from_success(result, elapsed);
    sample.error = true;
    sample
}

fn observation_sample_from_error(elapsed: Duration) -> ProviderObservationSample {
    ProviderObservationSample {
        ttft_ms: duration_millis_u64(elapsed).max(1),
        tok_s: None,
        error: true,
    }
}

fn redispatch_request_with_partials(
    original: &ChatCompletionRequest,
    partials: &[GatewaySessionPartial],
) -> ChatCompletionRequest {
    let mut request = original.clone();
    let prefix = partial_text(partials);
    if !prefix.is_empty() {
        request.messages.push(ChatMessage {
            role: "assistant".to_owned(),
            content: json!(prefix),
            name: None,
            extra: BTreeMap::new(),
        });
        request.messages.push(ChatMessage {
            role: "user".to_owned(),
            content: json!(
                "Continue from the previous assistant message. Do not repeat text already written."
            ),
            name: None,
            extra: BTreeMap::new(),
        });
    }
    if let Some(max_tokens) = request.max_tokens {
        let delivered = partials
            .iter()
            .map(|partial| partial.output.usage.completion_tokens)
            .sum::<u64>();
        request.max_tokens = Some(
            max_tokens
                .saturating_sub(u32::try_from(delivered).unwrap_or(u32::MAX))
                .max(1),
        );
    }
    request
}

fn stitch_partials_into_result(
    result: &mut GatewaySessionResult,
    partials: &[GatewaySessionPartial],
) {
    let prefix = partial_text(partials);
    if !prefix.is_empty() && result.output.tool_call.is_none() {
        let suffix = result.output.content.take().unwrap_or_default();
        result.output.content = Some(format!("{prefix}{suffix}"));
    }
    let partial_completion = partials
        .iter()
        .map(|partial| partial.output.usage.completion_tokens)
        .sum::<u64>();
    result.output.usage.completion_tokens = result
        .output
        .usage
        .completion_tokens
        .saturating_add(partial_completion);
    result.output.usage.total_tokens = result
        .output
        .usage
        .prompt_tokens
        .saturating_add(result.output.usage.completion_tokens);
    let mut token_ids = partials
        .iter()
        .flat_map(|partial| partial.token_ids.iter().copied())
        .collect::<Vec<_>>();
    token_ids.extend(result.token_ids.iter().copied());
    result.token_ids = token_ids;
}

fn partial_text(partials: &[GatewaySessionPartial]) -> String {
    partials
        .iter()
        .filter_map(|partial| partial.output.content.as_deref())
        .collect::<String>()
}

fn eligible_route_candidates<'a>(
    model: &'a GatewayModel,
    min_att_tier: Option<u8>,
    quant: Option<&str>,
    rail: &str,
) -> Vec<&'a GatewayRouteCandidate> {
    model
        .mayhem
        .route_candidates
        .iter()
        .filter(|candidate| {
            candidate
                .accepted_rails
                .iter()
                .any(|candidate_rail| candidate_rail == rail)
                && min_att_tier
                    .map(|min_tier| candidate.att_tier >= min_tier)
                    .unwrap_or(true)
                && quant
                    .map(|quant| candidate.quant.eq_ignore_ascii_case(quant))
                    .unwrap_or(true)
        })
        .collect()
}

fn build_completion(
    state: &GatewayState,
    request: CompletionRequest,
) -> Result<ChatResponse, ApiError> {
    if !state.dev_session_shim {
        return Err(ApiError::service_unavailable(
            "no provider available: legacy completions cannot use the local dev shim in production mode",
            Some("model"),
        ));
    }
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
            "backend": "local-openai-shape",
            "billable": false,
            "dev_session": true,
            "receipt": null,
        },
    });
    if request.stream {
        Ok(ChatResponse::Sse(vec![chunk]))
    } else {
        Ok(ChatResponse::Json(chunk))
    }
}

async fn build_embedding(
    state: &GatewayState,
    request: EmbeddingRequest,
    options: GatewayRequestOptions,
) -> Result<Value, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_embeddings(&model) {
        return Err(ApiError::bad_request(
            "model does not support embeddings",
            Some("model"),
        ));
    }
    let inputs = embedding_input_texts(&request)?;
    let encoding_format = embedding_encoding_format(&request)?;
    let id = make_id("embd");
    let created = now_secs();
    let GatewayEmbeddingRun {
        result:
            GatewayEmbeddingResult {
                output,
                backend,
                direct_session,
                provider_receipt,
                quality,
            },
        invocation,
        metering_inputs,
        metering_output,
    } = run_embedding_with_route_retry(state, &model, &request, &inputs, options).await?;
    let receipt = if state.dev_session_shim {
        None
    } else {
        let receipt = state.meter_embedding_session(
            &model,
            &metering_inputs,
            &metering_output,
            &invocation,
            provider_receipt.as_ref(),
        )?;
        Some(receipt_summary(&receipt))
    };
    let data = output
        .embeddings
        .iter()
        .enumerate()
        .map(|(index, embedding)| {
            json!({
                "object": "embedding",
                "index": index,
                "embedding": embedding_response_value(embedding, encoding_format),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "id": id,
        "object": "list",
        "created": created,
        "model": model.id,
        "data": data,
        "usage": {
            "prompt_tokens": output.usage.prompt_tokens,
            "total_tokens": output.usage.total_tokens,
        },
        "mayhem": {
            "backend": backend,
            "direct_session": direct_session,
            "billable": !state.dev_session_shim,
            "dev_session": state.dev_session_shim,
            "quality": quality.map(|quality| json!({
                "ttft_ms": quality.ttft_ms,
                "tok_s": quality.tok_s,
            })),
            "receipt": receipt,
        },
    }))
}

async fn build_image_generation(
    state: &GatewayState,
    request: ImageGenerationRequest,
    options: GatewayRequestOptions,
) -> Result<Value, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_image_generation(&model) {
        return Err(ApiError::bad_request(
            "model does not support image generation",
            Some("model"),
        ));
    }
    validate_image_generation_request(&model, &request)?;
    let id = make_id("img");
    let created = now_secs();
    let GatewayImageGenerationRun {
        result:
            GatewayImageGenerationResult {
                output,
                backend,
                direct_session,
                provider_receipt,
                quality,
            },
        invocation,
        metering_request,
        metering_output,
    } = run_image_generation_with_route_retry(state, &model, &request, options).await?;
    let expected_count = usize::try_from(image_generation_count(&request)).unwrap_or(usize::MAX);
    if output.artifacts.len() != expected_count {
        return Err(ApiError::bad_gateway(
            format!(
                "provider returned {} image artifact(s), expected {expected_count}",
                output.artifacts.len()
            ),
            Some("model"),
        ));
    }
    let receipt = if state.dev_session_shim {
        None
    } else {
        let receipt = state.meter_image_generation_session(
            &model,
            &metering_request,
            &metering_output,
            &invocation,
            provider_receipt.as_ref(),
        )?;
        Some(receipt_summary(&receipt))
    };
    let data = image_generation_response_data(
        &output.artifacts,
        request.response_format.as_deref().unwrap_or("b64_json"),
    )?;
    Ok(json!({
        "id": id,
        "object": "images.response",
        "created": created,
        "model": model.id,
        "data": data,
        "usage": output.usage,
        "mayhem": {
            "backend": backend,
            "direct_session": direct_session,
            "billable": !state.dev_session_shim,
            "dev_session": state.dev_session_shim,
            "quality": quality.map(|quality| json!({
                "ttft_ms": quality.ttft_ms,
                "tok_s": quality.tok_s,
            })),
            "artifacts": artifact_summaries(&output.artifacts),
            "receipt": receipt,
        },
    }))
}

async fn build_audio_speech(
    state: &GatewayState,
    request: AudioSpeechRequest,
    options: GatewayRequestOptions,
) -> Result<Response, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_tts(&model) {
        return Err(ApiError::bad_request(
            "model does not support audio speech",
            Some("model"),
        ));
    }
    validate_audio_speech_request(&request)?;
    let GatewayAudioSpeechRun {
        result:
            GatewayAudioSpeechResult {
                output,
                backend,
                direct_session,
                provider_receipt,
                quality,
            },
        invocation,
        metering_request,
        metering_output,
    } = run_audio_speech_with_route_retry(state, &model, &request, options).await?;
    let receipt = if state.dev_session_shim {
        None
    } else {
        let receipt = state.meter_audio_speech_session(
            &model,
            &metering_request,
            &metering_output,
            &invocation,
            provider_receipt.as_ref(),
        )?;
        Some(receipt_summary(&receipt))
    };
    let artifact = output
        .artifacts
        .iter()
        .find(|artifact| artifact.content_type == "audio/wav")
        .or_else(|| output.artifacts.first())
        .ok_or_else(|| {
            ApiError::bad_gateway("provider returned no audio artifact", Some("model"))
        })?;
    let mut response = Body::from(artifact.bytes.clone()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&artifact.content_type).map_err(|err| {
            ApiError::bad_gateway(format!("invalid audio content type: {err}"), None)
        })?,
    );
    response.headers_mut().insert(
        "x-mayhem-backend",
        HeaderValue::from_str(&backend)
            .map_err(|err| ApiError::bad_gateway(format!("invalid backend header: {err}"), None))?,
    );
    response.headers_mut().insert(
        "x-mayhem-direct-session",
        HeaderValue::from_static(if direct_session { "true" } else { "false" }),
    );
    response.headers_mut().insert(
        "x-mayhem-usage",
        HeaderValue::from_str(&serde_json::to_string(&output.usage).map_err(ApiError::internal)?)
            .map_err(|err| ApiError::bad_gateway(format!("invalid usage header: {err}"), None))?,
    );
    if let Some(receipt) = receipt {
        response.headers_mut().insert(
            "x-mayhem-receipt",
            HeaderValue::from_str(&receipt.to_string()).map_err(|err| {
                ApiError::bad_gateway(format!("invalid receipt header: {err}"), None)
            })?,
        );
    }
    if let Some(quality) = quality {
        response.headers_mut().insert(
            "x-mayhem-ttft-ms",
            HeaderValue::from_str(&quality.ttft_ms.to_string()).map_err(|err| {
                ApiError::bad_gateway(format!("invalid quality header: {err}"), None)
            })?,
        );
    }
    Ok(response)
}

async fn parse_audio_transcription_multipart(
    mut multipart: Multipart,
) -> Result<AudioTranscriptionRequest, ApiError> {
    let mut model = None;
    let mut audio = None;
    let mut content_type = None;
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
                content_type = field.content_type().map(str::to_owned);
                filename = field.file_name().map(str::to_owned);
                let bytes = field.bytes().await.map_err(|err| {
                    ApiError::bad_request(format!("invalid file field: {err}"), Some("file"))
                })?;
                if !bytes.is_empty() {
                    audio = Some(bytes.to_vec());
                }
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
            .ok_or_else(|| ApiError::bad_request("multipart form missing file", Some("file")))?,
        content_type,
        filename,
        response_format,
        language,
        prompt,
    })
}

async fn build_audio_transcription(
    state: &GatewayState,
    request: AudioTranscriptionRequest,
    options: GatewayRequestOptions,
) -> Result<Value, ApiError> {
    let model = require_model(state, &request.model)?;
    if !model_supports_stt(&model) {
        return Err(ApiError::bad_request(
            "model does not support audio transcription",
            Some("model"),
        ));
    }
    let response_format = request.response_format.as_deref().unwrap_or("json");
    if response_format != "json" {
        return Err(ApiError::bad_request(
            "only response_format=json is supported",
            Some("response_format"),
        ));
    }
    let GatewayAudioTranscriptionRun {
        result:
            GatewayAudioTranscriptionResult {
                output,
                backend,
                direct_session,
                provider_receipt,
                quality,
            },
        invocation,
        metering_request,
        metering_output,
    } = run_audio_transcription_with_route_retry(state, &model, &request, options).await?;
    let receipt = if state.dev_session_shim {
        None
    } else {
        let receipt = state.meter_audio_transcription_session(
            &model,
            &metering_request,
            &metering_output,
            &invocation,
            provider_receipt.as_ref(),
        )?;
        Some(receipt_summary(&receipt))
    };
    Ok(json!({
        "text": output.text,
        "usage": output.usage,
        "mayhem": {
            "backend": backend,
            "direct_session": direct_session,
            "billable": !state.dev_session_shim,
            "dev_session": state.dev_session_shim,
            "quality": quality.map(|quality| json!({
                "ttft_ms": quality.ttft_ms,
                "tok_s": quality.tok_s,
            })),
            "receipt": receipt,
        },
    }))
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
            Ok(probe) => {
                self.record_probe(probe);
                if let Ok(Some(context_probe)) = self
                    .run_context_needle_probe_for_route(model, invocation, &config)
                    .await
                {
                    self.record_probe(context_probe);
                }
            }
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

    async fn run_context_needle_probe_for_route(
        &self,
        model: &GatewayModel,
        served_invocation: &GatewaySessionInvocation,
        config: &GatewayCanaryModelConfig,
    ) -> Result<Option<StoredProbeEvent>, ApiError> {
        if model.mayhem.model_class != DEFAULT_MODEL_CLASS {
            return Ok(None);
        }
        if served_invocation.served_ctx < CONTEXT_NEEDLE_MIN_CTX {
            return Ok(None);
        }
        let Some(route) = canary_served_route(model, served_invocation) else {
            return Ok(None);
        };
        let spec = context_needle_spec(model, served_invocation, config, self.canary_policy.seed);
        let request = context_needle_chat_request(&model.id, &spec);
        let invocation = match self.prepare_chat_invocation_for_route(
            model,
            &request,
            Some(&route),
            &GatewayRequestOptions::default(),
        ) {
            Ok(invocation) => invocation,
            Err(_) => return Ok(None),
        };
        let result = match self
            .session_backend
            .run_chat(model, &request, &invocation)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                return Ok(Some(failed_context_needle_runtime_probe(
                    model,
                    served_invocation,
                    config,
                    &spec,
                    err.message,
                    self.canary_policy.epoch,
                    &self.receipt_config.user_seed,
                )));
            }
        };
        let response = result.output.content.clone().unwrap_or_default();
        let pass = context_needle_response_matches(&response, &spec.answer);
        let match_bps = if pass { 10_000 } else { 0 };
        let token_fingerprint = token_fingerprint(result.token_ids.iter().copied()).digest;
        let receipt = self.meter_chat_session(
            model,
            &request,
            &result.output,
            &invocation,
            result.provider_receipt.as_ref(),
        )?;
        let receipt_hash = stable_value_hash(&json!(receipt));
        let provider = served_invocation
            .provider_pubkey
            .clone()
            .unwrap_or_else(|| verifying_key_hex(&self.receipt_config.provider_seed));
        let binary_hash = served_invocation
            .attestation
            .as_ref()
            .map(|attestation| attestation.contract.binary_hash.clone())
            .unwrap_or_default();
        let expected_fingerprint = stable_value_hash(&json!({
            "domain": "mayhem-context-needle-expected-v1",
            "answer": spec.answer.clone(),
        }));
        let observed_fingerprint = stable_value_hash(&json!({
            "domain": "mayhem-context-needle-observed-v1",
            "response": response,
            "token_fingerprint": token_fingerprint,
        }));
        let evidence = json!({
            "schema_version": 1,
            "kind": "mayhem-context-needle-probe-evidence",
            "model": model.id,
            "provider": provider,
            "enclave_id": served_invocation.enclave_id,
            "binary_hash": binary_hash,
            "canary_set": config.canary_set,
            "verification_method": CANARY_VERIFICATION_CONTEXT_NEEDLE,
            "served_ctx": served_invocation.served_ctx,
            "ctx_bracket": served_invocation.ctx_bracket,
            "ctx_bracket_table_ver": served_invocation.ctx_bracket_table_ver,
            "needle_position_tokens": spec.needle_position_tokens,
            "tail_tokens_after_needle": spec.tail_tokens_after_needle,
            "answer_hash": expected_fingerprint,
            "response": response,
            "response_token_fingerprint": token_fingerprint,
            "pass": pass,
            "match_bps": match_bps,
            "receipt_hash": receipt_hash,
        });
        let evidence_hash = stable_value_hash(&evidence);
        let at = now_secs();
        let probe_id = stable_value_hash(&json!({
            "provider": provider,
            "enclave_id": served_invocation.enclave_id,
            "canary_set": config.canary_set,
            "verification_method": CANARY_VERIFICATION_CONTEXT_NEEDLE,
            "epoch": self.canary_policy.epoch,
            "served_ctx": served_invocation.served_ctx,
            "needle_position_tokens": spec.needle_position_tokens,
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
            "verification_method": CANARY_VERIFICATION_CONTEXT_NEEDLE,
            "match_bps": match_bps,
            "pass": pass,
            "session_receipt_hash": receipt_hash,
            "evidence_hash": evidence_hash,
        });
        probe_command["auditor_sig"] = json!(probe_result_signature(
            &self.receipt_config.user_seed,
            &probe_command,
            &verifying_key_hex(&self.receipt_config.user_seed),
        ));
        Ok(Some(StoredProbeEvent {
            probe_id,
            model_id: model.id.clone(),
            provider,
            enclave_id: served_invocation.enclave_id.clone(),
            binary_hash,
            canary_set: config.canary_set.clone(),
            verification_method: CANARY_VERIFICATION_CONTEXT_NEEDLE.to_owned(),
            expected_fingerprint,
            observed_fingerprint,
            match_bps,
            pass,
            reputation_event_kind: if pass {
                ReputationEventKind::ProbeOk
            } else {
                ReputationEventKind::ProbeFail
            },
            session_receipt_hash: receipt_hash,
            evidence: json!({
                "evidence": evidence,
                "receipts": [receipt],
            }),
            evidence_hash,
            probe_command,
        }))
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
                &GatewayRequestOptions::default(),
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
        options: &GatewayRequestOptions,
    ) -> Result<GatewaySessionInvocation, ApiError> {
        let prompt_text = chat_prompt_text(request);
        let input_tokens = rough_tokens(&prompt_text);
        let failover = self.failover_thresholds_for_model(model, options, input_tokens);
        let session_id = session_id_for(&model.id, &prompt_text);
        let enclave_id = route
            .map(|candidate| candidate.enclave_id.clone())
            .unwrap_or_else(|| enclave_id_for_model(&model.id));
        let price = route_price_ref_mu(model, route);
        let price_ver = price.ver;
        let locked_rate_map = session_locked_rate_map(price);
        let attestation = route.map(|candidate| GatewaySessionAttestation {
            contract: EnclaveContractRecord {
                enclave_id: candidate.enclave_id.clone(),
                admin_pubkey: candidate.admin_pubkey.clone(),
                model_id: model.id.clone(),
                model_class: model.mayhem.model_class.clone(),
                artifact_root: candidate.artifact_root.clone(),
                artifact_sidecar_roots: candidate.artifact_sidecar_roots.clone(),
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
        let max_spend_mu = estimate_max_spend_mu(price, request, &prompt_text);
        ensure_max_price_allows(max_spend_mu, options.max_price_mu)?;
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }
        self.access_control
            .ensure_budget_allows(&options.access_token, max_spend_mu)?;
        let opened_at = now_secs();
        let served_ctx = self.served_ctx_for_route(model, route);
        let (ctx_bracket, ctx_bracket_table_ver) =
            self.ctx_bracket_terms_for_served_ctx(served_ctx, opened_at)?;
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            rail: self.receipt_config.rail.clone(),
            enclave_id: enclave_id.clone(),
            price_ver,
            locked_rate_map: locked_rate_map.clone(),
            locked_per_req_mu: price.per_req_mu,
            locked_min_session_mu: price.min_session_mu,
            served_ctx,
            ctx_bracket: ctx_bracket.clone(),
            ctx_bracket_table_ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        Ok(GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            rail: self.receipt_config.rail.clone(),
            user_pubkey: verifying_key_hex(&self.receipt_config.user_seed),
            provider_pubkey: route.map(|candidate| candidate.provider.clone()),
            enclave_id,
            price_ver,
            opened_at,
            served_ctx: voucher_body.served_ctx,
            ctx_bracket,
            ctx_bracket_table_ver,
            rules_ver: self.receipt_config.rules_ver,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
            },
            attestation,
            hedge: hedge_invocation_for_model(model, options, failover),
            failover,
            access_token: options.access_token.clone(),
            receipt_cosign_enabled: self.receipt_config.cosign_enabled,
            receipt_user_seed: self.receipt_config.user_seed,
        })
    }

    fn prepare_embedding_invocation_for_route(
        &self,
        model: &GatewayModel,
        inputs: &[String],
        route: Option<&GatewayRouteCandidate>,
        options: &GatewayRequestOptions,
    ) -> Result<GatewaySessionInvocation, ApiError> {
        let prompt_text = embedding_prompt_text(inputs);
        let failover = self.failover_thresholds_for_model(
            model,
            options,
            embedding_failover_work_units(inputs),
        );
        let session_id = session_id_for(&model.id, &prompt_text);
        let enclave_id = route
            .map(|candidate| candidate.enclave_id.clone())
            .unwrap_or_else(|| enclave_id_for_model(&model.id));
        let price = route_price_ref_mu(model, route);
        let price_ver = price.ver;
        let locked_rate_map = session_locked_rate_map(price);
        let attestation = route.map(|candidate| GatewaySessionAttestation {
            contract: EnclaveContractRecord {
                enclave_id: candidate.enclave_id.clone(),
                admin_pubkey: candidate.admin_pubkey.clone(),
                model_id: model.id.clone(),
                model_class: model.mayhem.model_class.clone(),
                artifact_root: candidate.artifact_root.clone(),
                artifact_sidecar_roots: candidate.artifact_sidecar_roots.clone(),
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
        let max_spend_mu = estimate_embedding_max_spend_mu(price, inputs);
        ensure_max_price_allows(max_spend_mu, options.max_price_mu)?;
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }
        self.access_control
            .ensure_budget_allows(&options.access_token, max_spend_mu)?;
        let opened_at = now_secs();
        let served_ctx = self.served_ctx_for_route(model, route);
        let (ctx_bracket, ctx_bracket_table_ver) =
            self.ctx_bracket_terms_for_served_ctx(served_ctx, opened_at)?;
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            rail: self.receipt_config.rail.clone(),
            enclave_id: enclave_id.clone(),
            price_ver,
            locked_rate_map: locked_rate_map.clone(),
            locked_per_req_mu: price.per_req_mu,
            locked_min_session_mu: price.min_session_mu,
            served_ctx,
            ctx_bracket: ctx_bracket.clone(),
            ctx_bracket_table_ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        Ok(GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            rail: self.receipt_config.rail.clone(),
            user_pubkey: verifying_key_hex(&self.receipt_config.user_seed),
            provider_pubkey: route.map(|candidate| candidate.provider.clone()),
            enclave_id,
            price_ver,
            opened_at,
            served_ctx: voucher_body.served_ctx,
            ctx_bracket,
            ctx_bracket_table_ver,
            rules_ver: self.receipt_config.rules_ver,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
            },
            attestation,
            hedge: GatewayHedgeInvocation::default(),
            failover,
            access_token: options.access_token.clone(),
            receipt_cosign_enabled: self.receipt_config.cosign_enabled,
            receipt_user_seed: self.receipt_config.user_seed,
        })
    }

    fn prepare_image_generation_invocation_for_route(
        &self,
        model: &GatewayModel,
        request: &ImageGenerationRequest,
        route: Option<&GatewayRouteCandidate>,
        options: &GatewayRequestOptions,
    ) -> Result<GatewaySessionInvocation, ApiError> {
        let prompt_text = image_generation_prompt_text(request);
        let failover = self.failover_thresholds_for_model(
            model,
            options,
            image_generation_failover_work_units(request)?,
        );
        let session_id = session_id_for(&model.id, &prompt_text);
        let enclave_id = route
            .map(|candidate| candidate.enclave_id.clone())
            .unwrap_or_else(|| enclave_id_for_model(&model.id));
        let price = route_price_ref_mu(model, route);
        let price_ver = price.ver;
        let locked_rate_map = session_locked_rate_map(price);
        let attestation = route.map(|candidate| GatewaySessionAttestation {
            contract: EnclaveContractRecord {
                enclave_id: candidate.enclave_id.clone(),
                admin_pubkey: candidate.admin_pubkey.clone(),
                model_id: model.id.clone(),
                model_class: model.mayhem.model_class.clone(),
                artifact_root: candidate.artifact_root.clone(),
                artifact_sidecar_roots: candidate.artifact_sidecar_roots.clone(),
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
        let max_spend_mu = estimate_image_generation_max_spend_mu(price, request);
        ensure_max_price_allows(max_spend_mu, options.max_price_mu)?;
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }
        self.access_control
            .ensure_budget_allows(&options.access_token, max_spend_mu)?;
        let opened_at = now_secs();
        let served_ctx = self.served_ctx_for_route(model, route);
        let (ctx_bracket, ctx_bracket_table_ver) =
            self.ctx_bracket_terms_for_served_ctx(served_ctx, opened_at)?;
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            rail: self.receipt_config.rail.clone(),
            enclave_id: enclave_id.clone(),
            price_ver,
            locked_rate_map: locked_rate_map.clone(),
            locked_per_req_mu: price.per_req_mu,
            locked_min_session_mu: price.min_session_mu,
            served_ctx,
            ctx_bracket: ctx_bracket.clone(),
            ctx_bracket_table_ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        Ok(GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            rail: self.receipt_config.rail.clone(),
            user_pubkey: verifying_key_hex(&self.receipt_config.user_seed),
            provider_pubkey: route.map(|candidate| candidate.provider.clone()),
            enclave_id,
            price_ver,
            opened_at,
            served_ctx: voucher_body.served_ctx,
            ctx_bracket,
            ctx_bracket_table_ver,
            rules_ver: self.receipt_config.rules_ver,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
            },
            attestation,
            hedge: GatewayHedgeInvocation::default(),
            failover,
            access_token: options.access_token.clone(),
            receipt_cosign_enabled: self.receipt_config.cosign_enabled,
            receipt_user_seed: self.receipt_config.user_seed,
        })
    }

    fn prepare_audio_speech_invocation_for_route(
        &self,
        model: &GatewayModel,
        request: &AudioSpeechRequest,
        route: Option<&GatewayRouteCandidate>,
        options: &GatewayRequestOptions,
    ) -> Result<GatewaySessionInvocation, ApiError> {
        let prompt_text = audio_speech_prompt_hash(request);
        let failover = self.failover_thresholds_for_model(
            model,
            options,
            audio_speech_failover_work_units(request),
        );
        let session_id = session_id_for(&model.id, &prompt_text);
        let enclave_id = route
            .map(|candidate| candidate.enclave_id.clone())
            .unwrap_or_else(|| enclave_id_for_model(&model.id));
        let price = route_price_ref_mu(model, route);
        let price_ver = price.ver;
        let locked_rate_map = session_locked_rate_map(price);
        let attestation = route.map(|candidate| GatewaySessionAttestation {
            contract: EnclaveContractRecord {
                enclave_id: candidate.enclave_id.clone(),
                admin_pubkey: candidate.admin_pubkey.clone(),
                model_id: model.id.clone(),
                model_class: model.mayhem.model_class.clone(),
                artifact_root: candidate.artifact_root.clone(),
                artifact_sidecar_roots: candidate.artifact_sidecar_roots.clone(),
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
        let max_spend_mu = estimate_audio_speech_max_spend_mu(price, request);
        ensure_max_price_allows(max_spend_mu, options.max_price_mu)?;
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }
        self.access_control
            .ensure_budget_allows(&options.access_token, max_spend_mu)?;
        let opened_at = now_secs();
        let served_ctx = self.served_ctx_for_route(model, route);
        let (ctx_bracket, ctx_bracket_table_ver) =
            self.ctx_bracket_terms_for_served_ctx(served_ctx, opened_at)?;
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            rail: self.receipt_config.rail.clone(),
            enclave_id: enclave_id.clone(),
            price_ver,
            locked_rate_map: locked_rate_map.clone(),
            locked_per_req_mu: price.per_req_mu,
            locked_min_session_mu: price.min_session_mu,
            served_ctx,
            ctx_bracket: ctx_bracket.clone(),
            ctx_bracket_table_ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        Ok(GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            rail: self.receipt_config.rail.clone(),
            user_pubkey: verifying_key_hex(&self.receipt_config.user_seed),
            provider_pubkey: route.map(|candidate| candidate.provider.clone()),
            enclave_id,
            price_ver,
            opened_at,
            served_ctx: voucher_body.served_ctx,
            ctx_bracket,
            ctx_bracket_table_ver,
            rules_ver: self.receipt_config.rules_ver,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
            },
            attestation,
            hedge: GatewayHedgeInvocation::default(),
            failover,
            access_token: options.access_token.clone(),
            receipt_cosign_enabled: self.receipt_config.cosign_enabled,
            receipt_user_seed: self.receipt_config.user_seed,
        })
    }

    fn prepare_audio_transcription_invocation_for_route(
        &self,
        model: &GatewayModel,
        request: &AudioTranscriptionRequest,
        route: Option<&GatewayRouteCandidate>,
        options: &GatewayRequestOptions,
    ) -> Result<GatewaySessionInvocation, ApiError> {
        let prompt_text = audio_transcription_prompt_hash(request);
        let failover = self.failover_thresholds_for_model(
            model,
            options,
            audio_transcription_failover_work_units(request),
        );
        let session_id = session_id_for(&model.id, &prompt_text);
        let enclave_id = route
            .map(|candidate| candidate.enclave_id.clone())
            .unwrap_or_else(|| enclave_id_for_model(&model.id));
        let price = route_price_ref_mu(model, route);
        let price_ver = price.ver;
        let locked_rate_map = session_locked_rate_map(price);
        let attestation = route.map(|candidate| GatewaySessionAttestation {
            contract: EnclaveContractRecord {
                enclave_id: candidate.enclave_id.clone(),
                admin_pubkey: candidate.admin_pubkey.clone(),
                model_id: model.id.clone(),
                model_class: model.mayhem.model_class.clone(),
                artifact_root: candidate.artifact_root.clone(),
                artifact_sidecar_roots: candidate.artifact_sidecar_roots.clone(),
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
        let max_spend_mu = estimate_audio_transcription_max_spend_mu(price, request);
        ensure_max_price_allows(max_spend_mu, options.max_price_mu)?;
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }
        self.access_control
            .ensure_budget_allows(&options.access_token, max_spend_mu)?;
        let opened_at = now_secs();
        let served_ctx = self.served_ctx_for_route(model, route);
        let (ctx_bracket, ctx_bracket_table_ver) =
            self.ctx_bracket_terms_for_served_ctx(served_ctx, opened_at)?;
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            rail: self.receipt_config.rail.clone(),
            enclave_id: enclave_id.clone(),
            price_ver,
            locked_rate_map: locked_rate_map.clone(),
            locked_per_req_mu: price.per_req_mu,
            locked_min_session_mu: price.min_session_mu,
            served_ctx,
            ctx_bracket: ctx_bracket.clone(),
            ctx_bracket_table_ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        Ok(GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            rail: self.receipt_config.rail.clone(),
            user_pubkey: verifying_key_hex(&self.receipt_config.user_seed),
            provider_pubkey: route.map(|candidate| candidate.provider.clone()),
            enclave_id,
            price_ver,
            opened_at,
            served_ctx: voucher_body.served_ctx,
            ctx_bracket,
            ctx_bracket_table_ver,
            rules_ver: self.receipt_config.rules_ver,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
            },
            attestation,
            hedge: GatewayHedgeInvocation::default(),
            failover,
            access_token: options.access_token.clone(),
            receipt_cosign_enabled: self.receipt_config.cosign_enabled,
            receipt_user_seed: self.receipt_config.user_seed,
        })
    }

    fn failover_thresholds_for_model(
        &self,
        model: &GatewayModel,
        options: &GatewayRequestOptions,
        prompt_tokens: u64,
    ) -> GatewayFailoverInvocation {
        let config = self
            .failover_policy
            .merged_with(model.mayhem.failover)
            .merged_with(GatewayFailoverPolicyConfig {
                min_tok_s: options.failover_overrides.min_tok_s,
                ..GatewayFailoverPolicyConfig::default()
            });
        GatewayFailoverInvocation::from_config_for_prompt(config, prompt_tokens)
    }

    fn generation_floor_tok_s_for_model(
        &self,
        model: &GatewayModel,
        options: &GatewayRequestOptions,
    ) -> Option<f64> {
        self.throughput_floor_for_model(model, options, DEFAULT_LLM_GENERATION_FLOOR_TOK_S)
    }

    fn throughput_floor_for_model(
        &self,
        model: &GatewayModel,
        options: &GatewayRequestOptions,
        default_floor: f64,
    ) -> Option<f64> {
        if let Some(user_floor) = options.failover_overrides.min_tok_s {
            return (user_floor > 0.0).then_some(user_floor);
        }
        model
            .mayhem
            .failover
            .min_tok_s
            .or(self.failover_policy.min_tok_s)
            .or(Some(default_floor))
            .filter(|value| value.is_finite() && *value > 0.0)
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
            let seq = provider_receipt.body.seq;
            let usage = expected_text_usage_for_provider(
                Some(&provider_receipt.body.usage),
                output.usage.prompt_tokens,
                output.usage.completion_tokens,
                &invocation.spend_voucher.body.locked_rate_map,
            )
            .map_err(|err| ApiError::bad_gateway(err.message, Some("model")))?;
            let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
            if mu_owed_cum > invocation.spend_voucher.body.max_spend_mu {
                return Err(ApiError::payment_required(
                    "session usage exceeded signed spend voucher",
                    Some("model"),
                ));
            }
            self.cosign_provider_receipt(
                model,
                invocation,
                provider_receipt,
                ExpectedProviderReceipt {
                    provider: &provider,
                    seq,
                    final_receipt: true,
                    usage: usage.clone(),
                    mu_owed_cum,
                    prompt_hash: blake3_hex(prompt_text.as_bytes()),
                },
            )?
        } else {
            let usage =
                ReceiptUsage::text(output.usage.prompt_tokens, output.usage.completion_tokens);
            let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
            if mu_owed_cum > invocation.spend_voucher.body.max_spend_mu {
                return Err(ApiError::payment_required(
                    "session usage exceeded signed spend voucher",
                    Some("model"),
                ));
            }
            let body = ReceiptBody {
                schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
                session_id: invocation.session_id.clone(),
                seq: 1,
                final_receipt: true,
                rail: invocation.rail.clone(),
                user: invocation.user_pubkey.clone(),
                provider,
                enclave_id: invocation.enclave_id.clone(),
                model_id: model.id.clone(),
                price_ver: invocation.price_ver,
                locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
                locked_per_req_mu: invocation.spend_voucher.body.locked_per_req_mu,
                locked_min_session_mu: invocation.spend_voucher.body.locked_min_session_mu,
                served_ctx: invocation.served_ctx,
                ctx_bracket: invocation.ctx_bracket.clone(),
                ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
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
            rail: invocation.rail.clone(),
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
            access_token: invocation.access_token.clone(),
        };
        self.record_receipt(stored.clone())?;
        Ok(stored)
    }

    fn meter_embedding_session(
        &self,
        model: &GatewayModel,
        inputs: &[String],
        output: &EmbeddingOutput,
        invocation: &GatewaySessionInvocation,
        provider_receipt: Option<&ProviderSignedReceipt>,
    ) -> Result<StoredReceipt, ApiError> {
        let usage = ReceiptUsage::text(output.usage.prompt_tokens, 0);
        let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
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
                    seq: 1,
                    final_receipt: true,
                    usage: usage.clone(),
                    mu_owed_cum,
                    prompt_hash: blake3_hex(embedding_prompt_text(inputs).as_bytes()),
                },
            )?
        } else {
            let body = ReceiptBody {
                schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
                session_id: invocation.session_id.clone(),
                seq: 1,
                final_receipt: true,
                rail: invocation.rail.clone(),
                user: invocation.user_pubkey.clone(),
                provider,
                enclave_id: invocation.enclave_id.clone(),
                model_id: model.id.clone(),
                price_ver: invocation.price_ver,
                locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
                locked_per_req_mu: invocation.spend_voucher.body.locked_per_req_mu,
                locked_min_session_mu: invocation.spend_voucher.body.locked_min_session_mu,
                served_ctx: invocation.served_ctx,
                ctx_bracket: invocation.ctx_bracket.clone(),
                ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
                rules_ver: invocation.rules_ver,
                usage,
                mu_owed_cum,
                prompt_hash: blake3_hex(embedding_prompt_text(inputs).as_bytes()),
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
            rail: invocation.rail.clone(),
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
            access_token: invocation.access_token.clone(),
        };
        self.record_receipt(stored.clone())?;
        Ok(stored)
    }

    fn meter_image_generation_session(
        &self,
        model: &GatewayModel,
        request: &ImageGenerationRequest,
        output: &ImageGenerationOutput,
        invocation: &GatewaySessionInvocation,
        provider_receipt: Option<&ProviderSignedReceipt>,
    ) -> Result<StoredReceipt, ApiError> {
        let usage = output.usage.clone();
        let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
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
                    seq: 1,
                    final_receipt: true,
                    usage: usage.clone(),
                    mu_owed_cum,
                    prompt_hash: image_generation_prompt_hash(request),
                },
            )?
        } else {
            let body = ReceiptBody {
                schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
                session_id: invocation.session_id.clone(),
                seq: 1,
                final_receipt: true,
                rail: invocation.rail.clone(),
                user: invocation.user_pubkey.clone(),
                provider,
                enclave_id: invocation.enclave_id.clone(),
                model_id: model.id.clone(),
                price_ver: invocation.price_ver,
                locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
                locked_per_req_mu: invocation.spend_voucher.body.locked_per_req_mu,
                locked_min_session_mu: invocation.spend_voucher.body.locked_min_session_mu,
                served_ctx: invocation.served_ctx,
                ctx_bracket: invocation.ctx_bracket.clone(),
                ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
                rules_ver: invocation.rules_ver,
                usage,
                mu_owed_cum,
                prompt_hash: image_generation_prompt_hash(request),
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
            rail: invocation.rail.clone(),
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
            access_token: invocation.access_token.clone(),
        };
        self.record_receipt(stored.clone())?;
        Ok(stored)
    }

    fn record_partial_provider_receipt(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        invocation: &GatewaySessionInvocation,
        partial: &GatewaySessionPartial,
    ) -> Result<StoredReceipt, ApiError> {
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
        let body = &partial.provider_receipt.body;
        let usage = ReceiptUsage::text(
            partial.output.usage.prompt_tokens,
            partial.output.usage.completion_tokens,
        );
        let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
        if mu_owed_cum > invocation.spend_voucher.body.max_spend_mu {
            return Err(ApiError::payment_required(
                "provider partial receipt exceeds signed spend voucher",
                Some("model"),
            ));
        }
        let receipt = self.cosign_provider_receipt(
            model,
            invocation,
            &partial.provider_receipt,
            ExpectedProviderReceipt {
                provider: &provider,
                seq: body.seq,
                final_receipt: false,
                mu_owed_cum,
                usage,
                prompt_hash: blake3_hex(chat_prompt_text(request).as_bytes()),
            },
        )?;
        let receipt_ack = ReceiptAck {
            session_id: receipt.body.session_id.clone(),
            seq: receipt.body.seq,
            user_sig: receipt.user_sig.clone(),
        };
        let stored = StoredReceipt {
            rail: invocation.rail.clone(),
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
            access_token: invocation.access_token.clone(),
        };
        self.record_receipt(stored.clone())?;
        Ok(stored)
    }

    fn meter_audio_speech_session(
        &self,
        model: &GatewayModel,
        request: &AudioSpeechRequest,
        output: &AudioSpeechOutput,
        invocation: &GatewaySessionInvocation,
        provider_receipt: Option<&ProviderSignedReceipt>,
    ) -> Result<StoredReceipt, ApiError> {
        let usage = output.usage.clone();
        let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
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
                    seq: 1,
                    final_receipt: true,
                    usage: usage.clone(),
                    mu_owed_cum,
                    prompt_hash: audio_speech_prompt_hash(request),
                },
            )?
        } else {
            let body = ReceiptBody {
                schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
                session_id: invocation.session_id.clone(),
                seq: 1,
                final_receipt: true,
                rail: invocation.rail.clone(),
                user: invocation.user_pubkey.clone(),
                provider,
                enclave_id: invocation.enclave_id.clone(),
                model_id: model.id.clone(),
                price_ver: invocation.price_ver,
                locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
                locked_per_req_mu: invocation.spend_voucher.body.locked_per_req_mu,
                locked_min_session_mu: invocation.spend_voucher.body.locked_min_session_mu,
                served_ctx: invocation.served_ctx,
                ctx_bracket: invocation.ctx_bracket.clone(),
                ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
                rules_ver: invocation.rules_ver,
                usage,
                mu_owed_cum,
                prompt_hash: audio_speech_prompt_hash(request),
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
            rail: invocation.rail.clone(),
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
            access_token: invocation.access_token.clone(),
        };
        self.record_receipt(stored.clone())?;
        Ok(stored)
    }

    fn meter_audio_transcription_session(
        &self,
        model: &GatewayModel,
        request: &AudioTranscriptionRequest,
        output: &AudioTranscriptionOutput,
        invocation: &GatewaySessionInvocation,
        provider_receipt: Option<&ProviderSignedReceipt>,
    ) -> Result<StoredReceipt, ApiError> {
        let usage = output.usage.clone();
        let mu_owed_cum = calculate_locked_mu_owed(invocation, &usage);
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
                    seq: 1,
                    final_receipt: true,
                    usage: usage.clone(),
                    mu_owed_cum,
                    prompt_hash: audio_transcription_prompt_hash(request),
                },
            )?
        } else {
            let body = ReceiptBody {
                schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
                session_id: invocation.session_id.clone(),
                seq: 1,
                final_receipt: true,
                rail: invocation.rail.clone(),
                user: invocation.user_pubkey.clone(),
                provider,
                enclave_id: invocation.enclave_id.clone(),
                model_id: model.id.clone(),
                price_ver: invocation.price_ver,
                locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
                locked_per_req_mu: invocation.spend_voucher.body.locked_per_req_mu,
                locked_min_session_mu: invocation.spend_voucher.body.locked_min_session_mu,
                served_ctx: invocation.served_ctx,
                ctx_bracket: invocation.ctx_bracket.clone(),
                ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
                rules_ver: invocation.rules_ver,
                usage,
                mu_owed_cum,
                prompt_hash: audio_transcription_prompt_hash(request),
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
            rail: invocation.rail.clone(),
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
            access_token: invocation.access_token.clone(),
        };
        self.record_receipt(stored.clone())?;
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

#[derive(Clone, Debug)]
struct ContextNeedleSpec {
    answer: String,
    prompt: String,
    needle_position_tokens: u32,
    tail_tokens_after_needle: u32,
}

fn context_needle_spec(
    model: &GatewayModel,
    invocation: &GatewaySessionInvocation,
    config: &GatewayCanaryModelConfig,
    seed: i64,
) -> ContextNeedleSpec {
    let tail_tokens =
        (invocation.served_ctx / 8).clamp(CONTEXT_NEEDLE_MIN_CTX / 2, invocation.served_ctx / 2);
    let needle_position_tokens = invocation.served_ctx.saturating_sub(tail_tokens).max(1);
    let digest = blake3::hash(
        format!(
            "mayhem-context-needle:{}:{}:{}:{}:{}",
            model.id, invocation.enclave_id, config.canary_set, invocation.served_ctx, seed
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let answer = format!("MAYHEM-CTX-{}", digest[..12].to_ascii_uppercase());
    let before_words = needle_position_tokens.saturating_sub(64) as usize;
    let after_words = tail_tokens.saturating_sub(64) as usize;
    let mut prompt = String::new();
    prompt.push_str("You are reading a Mayhem context canary. Return only the exact needle code requested at the end.\n\n");
    push_context_needle_filler(&mut prompt, before_words);
    prompt.push_str("\n\nCONTEXT NEEDLE CODE: ");
    prompt.push_str(&answer);
    prompt.push_str("\n\n");
    push_context_needle_filler(&mut prompt, after_words);
    prompt.push_str("\n\nQuestion: what is the CONTEXT NEEDLE CODE? Return only that code.");
    ContextNeedleSpec {
        answer,
        prompt,
        needle_position_tokens,
        tail_tokens_after_needle: tail_tokens,
    }
}

fn push_context_needle_filler(prompt: &mut String, words: usize) {
    for idx in 0..words {
        if idx > 0 {
            prompt.push(' ');
        }
        prompt.push_str("haystack");
        if (idx + 1) % CONTEXT_NEEDLE_FILLER_WORDS_PER_LINE == 0 {
            prompt.push('\n');
        }
    }
}

fn context_needle_response_matches(response: &str, answer: &str) -> bool {
    response.to_ascii_uppercase().contains(answer)
}

fn context_needle_chat_request(model_id: &str, spec: &ContextNeedleSpec) -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: model_id.to_owned(),
        messages: vec![
            ChatMessage {
                role: "system".to_owned(),
                content: json!("Return only the exact context needle code."),
                name: None,
                extra: BTreeMap::new(),
            },
            ChatMessage {
                role: "user".to_owned(),
                content: json!(spec.prompt.clone()),
                name: None,
                extra: BTreeMap::new(),
            },
        ],
        user: None,
        metadata: BTreeMap::new(),
        stream: true,
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
        tools: None,
        tool_choice: None,
        response_format: None,
        temperature: Some(DEFAULT_CANARY_TEMPERATURE),
        top_p: None,
        seed: Some(0),
        stop: None,
        max_tokens: Some(CONTEXT_NEEDLE_MAX_TOKENS),
    }
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
        user: None,
        metadata: BTreeMap::new(),
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

fn failed_context_needle_runtime_probe(
    model: &GatewayModel,
    invocation: &GatewaySessionInvocation,
    config: &GatewayCanaryModelConfig,
    spec: &ContextNeedleSpec,
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
    let expected_fingerprint = stable_value_hash(&json!({
        "domain": "mayhem-context-needle-expected-v1",
        "answer": spec.answer.clone(),
    }));
    let evidence = json!({
        "schema_version": 1,
        "kind": "mayhem-context-needle-runtime-failure",
        "model": model.id,
        "provider": provider,
        "enclave_id": invocation.enclave_id,
        "binary_hash": binary_hash,
        "canary_set": config.canary_set,
        "verification_method": CANARY_VERIFICATION_CONTEXT_NEEDLE,
        "served_ctx": invocation.served_ctx,
        "ctx_bracket": invocation.ctx_bracket,
        "ctx_bracket_table_ver": invocation.ctx_bracket_table_ver,
        "needle_position_tokens": spec.needle_position_tokens,
        "tail_tokens_after_needle": spec.tail_tokens_after_needle,
        "answer_hash": expected_fingerprint,
        "reason": reason,
    });
    let evidence_hash = stable_value_hash(&evidence);
    let at = now_secs();
    let session_receipt_hash = stable_value_hash(&json!({
        "domain": "mayhem-context-needle-runtime-failure-v1",
        "evidence_hash": evidence_hash,
    }));
    let probe_id = stable_value_hash(&json!({
        "provider": provider,
        "enclave_id": invocation.enclave_id,
        "canary_set": config.canary_set,
        "verification_method": CANARY_VERIFICATION_CONTEXT_NEEDLE,
        "epoch": epoch,
        "served_ctx": invocation.served_ctx,
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
        "verification_method": CANARY_VERIFICATION_CONTEXT_NEEDLE,
        "match_bps": 0,
        "pass": false,
        "session_receipt_hash": session_receipt_hash,
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
        verification_method: CANARY_VERIFICATION_CONTEXT_NEEDLE.to_owned(),
        expected_fingerprint,
        observed_fingerprint: String::new(),
        match_bps: 0,
        pass: false,
        reputation_event_kind: ReputationEventKind::ProbeFail,
        session_receipt_hash,
        evidence_hash,
        evidence,
        probe_command,
    }
}

fn hedge_invocation_for_model(
    model: &GatewayModel,
    options: &GatewayRequestOptions,
    failover: GatewayFailoverInvocation,
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
    let policy = FailoverPolicy {
        open_timeout_millis: failover.open_timeout_ms,
        stall_timeout_millis: failover.stall_timeout_ms,
        ..FailoverPolicy::default()
    };
    let failover_state = SessionFailoverState::new(
        policy,
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
        actual_probe_count: 0,
        winner_provider: None,
        winner_ttft_ms: None,
    }
}

fn dev_chat_output(model: &GatewayModel, request: &ChatCompletionRequest) -> ChatOutput {
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
            arguments: dev_tool_arguments(&name),
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
    receipt: Option<&StoredReceipt>,
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
            "billable": mayhem_meta.billable,
            "dev_session": mayhem_meta.dev_session,
            "artifacts": artifact_summaries(&output.artifacts),
            "hedge": {
                "requested": mayhem_meta.hedge.requested,
                "planned_probe_count": mayhem_meta.hedge.planned_probe_count,
                "actual_probe_count": mayhem_meta.hedge.actual_probe_count,
                "winner_provider": mayhem_meta.hedge.winner_provider,
                "winner_ttft_ms": mayhem_meta.hedge.winner_ttft_ms,
            },
            "model": model.mayhem,
            "receipt": receipt.map(receipt_summary),
        },
    })
}

fn chat_stream_chunks(
    id: &str,
    created: u64,
    model: &str,
    output: &ChatOutput,
    receipt: Option<&StoredReceipt>,
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
                "billable": mayhem_meta.billable,
                "dev_session": mayhem_meta.dev_session,
                "artifacts": artifact_summaries(&output.artifacts),
                "hedge": {
                    "requested": mayhem_meta.hedge.requested,
                    "planned_probe_count": mayhem_meta.hedge.planned_probe_count,
                    "actual_probe_count": mayhem_meta.hedge.actual_probe_count,
                    "winner_provider": mayhem_meta.hedge.winner_provider,
                    "winner_ttft_ms": mayhem_meta.hedge.winner_ttft_ms,
                },
                "receipt": receipt.map(receipt_summary),
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

fn image_generation_response_data(
    artifacts: &[GatewayArtifactOutput],
    response_format: &str,
) -> Result<Vec<Value>, ApiError> {
    artifacts
        .iter()
        .map(|artifact| {
            if !artifact.content_type.starts_with("image/") {
                return Err(ApiError::bad_gateway(
                    format!(
                        "provider artifact {} has non-image content type {}",
                        artifact.id, artifact.content_type
                    ),
                    Some("model"),
                ));
            }
            let encoded = BASE64_STANDARD.encode(&artifact.bytes);
            match response_format {
                "url" => Ok(json!({
                    "url": format!("data:{};base64,{encoded}", artifact.content_type),
                    "revised_prompt": null,
                    "mayhem": {
                        "artifact_id": artifact.id,
                        "content_type": artifact.content_type,
                        "blake3": artifact.blake3,
                    },
                })),
                _ => Ok(json!({
                    "b64_json": encoded,
                    "revised_prompt": null,
                    "mayhem": {
                        "artifact_id": artifact.id,
                        "content_type": artifact.content_type,
                        "blake3": artifact.blake3,
                    },
                })),
            }
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
        "rail": receipt.rail,
        "session_id": receipt.receipt.body.session_id,
        "seq": receipt.receipt.body.seq,
        "final": receipt.receipt.body.final_receipt,
        "mu_owed_cum": receipt.receipt.body.mu_owed_cum,
        "prompt_hash": receipt.receipt.body.prompt_hash,
        "receipt_ack": receipt.receipt_ack,
        "access_token": receipt.access_token,
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

fn sse_stream_response(events: SseEventStream) -> Response {
    let mut response = Sse::new(events).into_response();
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
                per_req_mu: price.get("per_req_mu").and_then(Value::as_u64).unwrap_or(0),
                min_session_mu: price
                    .get("min_session_mu")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                derivation: price_derivation_from_catalog_value(price),
            },
            attestation_tier_labels: attestation_tier_labels_from_catalog_value(model)
                .unwrap_or_else(|| attestation_tier_labels_for_counts(&tiers)),
            attestation_tiers: tiers,
            quant_buckets: quant_buckets_from_catalog_value(model),
            min_app_version: model
                .get("min_app_version")
                .and_then(Value::as_str)
                .map(str::to_owned),
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
                max_image_width: caps
                    .get("max_image_width")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                max_image_height: caps
                    .get("max_image_height")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                max_image_steps: caps.get("max_image_steps").and_then(Value::as_u64),
                output_modality: caps
                    .get("output_modality")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                output_modalities: caps_output_modalities(caps),
            },
            adapter: shape_adapter_from_catalog_value(model),
            failover: failover_policy_from_catalog_value(model),
            source: "catalog".to_owned(),
            kyb_identities: Vec::new(),
            route_candidates: Vec::new(),
        },
    })
}

fn failover_policy_from_catalog_value(model: &Value) -> GatewayFailoverPolicyConfig {
    let failover = model
        .get("failover")
        .or_else(|| {
            model
                .get("quality")
                .and_then(|quality| quality.get("failover"))
        })
        .unwrap_or(&Value::Null);
    GatewayFailoverPolicyConfig {
        open_timeout_ms: positive_u64_field(failover, "open_timeout_ms"),
        ttft_timeout_ms: positive_u64_field(failover, "ttft_timeout_ms"),
        stall_timeout_ms: positive_u64_field(failover, "stall_timeout_ms")
            .or_else(|| positive_u64_field(failover, "idle_timeout_ms")),
        min_tok_s: positive_f64_field(failover, "min_tok_s")
            .or_else(|| positive_f64_field(failover, "min_tokens_per_second")),
    }
}

fn positive_u64_field(value: &Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
}

fn positive_f64_field(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
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

fn price_derivation_from_catalog_value(price: &Value) -> Option<Value> {
    price
        .get("derivation")
        .or_else(|| price.get("price_derivation"))
        .or_else(|| price.get("market_derivation"))
        .or_else(|| price.get("market"))
        .filter(|value| value.is_object())
        .cloned()
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

fn quant_buckets_from_catalog_value(model: &Value) -> BTreeMap<String, u32> {
    if let Some(object) = model.get("quant_buckets").and_then(Value::as_object) {
        let buckets = object
            .iter()
            .filter_map(|(bucket, count)| {
                let bucket = normalize_quant_bucket(bucket).ok()?;
                let count = count.as_u64().and_then(|count| u32::try_from(count).ok())?;
                Some((bucket, count))
            })
            .collect::<BTreeMap<_, _>>();
        if !buckets.is_empty() {
            return buckets;
        }
    }
    let mut buckets = BTreeMap::new();
    if let Some(artifacts) = model.get("artifacts").and_then(Value::as_object) {
        for (name, artifact) in artifacts {
            let bucket = quant_bucket_from_catalog_artifact(name, artifact);
            *buckets.entry(bucket).or_insert(0) += 1;
        }
    }
    if buckets.is_empty() {
        buckets.insert(DEFAULT_QUANT_BUCKET.to_owned(), 0);
    }
    buckets
}

fn quant_bucket_from_catalog_artifact(name: &str, artifact: &Value) -> String {
    let mut descriptor = name.to_ascii_lowercase();
    if let Some(engine) = artifact.get("engine").and_then(Value::as_str) {
        descriptor.push(' ');
        descriptor.push_str(&engine.to_ascii_lowercase());
    }
    if let Some(path) = artifact.get("path").and_then(Value::as_str) {
        descriptor.push(' ');
        descriptor.push_str(&path.to_ascii_lowercase());
    }
    quant_bucket_from_descriptor(&descriptor)
}

fn quant_bucket_from_descriptor(descriptor: &str) -> String {
    let descriptor = descriptor.replace('_', "-");
    if descriptor.contains("nvfp4") {
        "nvfp4".to_owned()
    } else if descriptor.contains("fp8") {
        "fp8".to_owned()
    } else if descriptor.contains("bf16") {
        "bf16".to_owned()
    } else if descriptor.contains("fp16") || descriptor.contains("f16") {
        "fp16".to_owned()
    } else if descriptor.contains("int8")
        || descriptor.contains("8bit")
        || descriptor.contains("q8")
    {
        "int8".to_owned()
    } else if descriptor.contains("int4")
        || descriptor.contains("4bit")
        || descriptor.contains("q4")
    {
        "int4".to_owned()
    } else {
        DEFAULT_QUANT_BUCKET.to_owned()
    }
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

fn route_price_ref_mu<'a>(
    model: &'a GatewayModel,
    route: Option<&'a GatewayRouteCandidate>,
) -> &'a PriceRefMu {
    route
        .and_then(|candidate| candidate.price_ref_mu.as_ref())
        .unwrap_or(&model.mayhem.price_ref_mu)
}

fn session_locked_rate_map(price: &PriceRefMu) -> Vec<RateMapEntry> {
    normalize_rate_map(price.rate_map.clone())
}

fn calculate_locked_mu_owed(invocation: &GatewaySessionInvocation, usage: &ReceiptUsage) -> u64 {
    priced_usage_mu(
        &invocation.spend_voucher.body.locked_rate_map,
        invocation.spend_voucher.body.locked_per_req_mu,
        invocation.spend_voucher.body.locked_min_session_mu,
        usage,
    )
}

fn calculate_mu_owed(price: &PriceRefMu, usage: &ReceiptUsage) -> u64 {
    priced_usage_mu(
        &price.rate_map,
        price.per_req_mu,
        price.min_session_mu,
        usage,
    )
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

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn stable_value_hash(value: &Value) -> String {
    blake3::hash(stable_json_value(value).to_string().as_bytes())
        .to_hex()
        .to_string()
}

fn wav_duration_seconds_ceil(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut data_len = None;
    while offset.saturating_add(8) <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_len = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start.saturating_add(chunk_len).min(bytes.len());
        if chunk_id == b"fmt " && chunk_len >= 16 && chunk_end <= bytes.len() {
            channels = Some(u16::from_le_bytes([
                bytes[chunk_start + 2],
                bytes[chunk_start + 3],
            ]));
            sample_rate = Some(u32::from_le_bytes([
                bytes[chunk_start + 4],
                bytes[chunk_start + 5],
                bytes[chunk_start + 6],
                bytes[chunk_start + 7],
            ]));
            bits_per_sample = Some(u16::from_le_bytes([
                bytes[chunk_start + 14],
                bytes[chunk_start + 15],
            ]));
        } else if chunk_id == b"data" {
            data_len = Some(chunk_len);
        }
        let padded = chunk_len + (chunk_len % 2);
        offset = chunk_start.saturating_add(padded);
    }
    let sample_rate = u64::from(sample_rate?);
    let channels = u64::from(channels?);
    let bits_per_sample = u64::from(bits_per_sample?);
    let data_len = u64::try_from(data_len?).ok()?;
    let bytes_per_second = sample_rate
        .saturating_mul(channels)
        .saturating_mul(bits_per_sample)
        .checked_div(8)?;
    if bytes_per_second == 0 {
        return None;
    }
    Some(data_len.div_ceil(bytes_per_second).max(1))
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

fn dev_tool_arguments(name: &str) -> String {
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

fn embedding_input_texts(request: &EmbeddingRequest) -> Result<Vec<String>, ApiError> {
    embedding_input_texts_from_value(&request.input)
        .map_err(|message| ApiError::bad_request(message, Some("input")))
}

fn embedding_input_texts_from_value(value: &Value) -> Result<Vec<String>, String> {
    match value {
        Value::String(text) => {
            if text.is_empty() {
                Err("embedding input must not be empty".to_owned())
            } else {
                Ok(vec![text.clone()])
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                return Err("embedding input array must not be empty".to_owned());
            }
            let mut inputs = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(text) if !text.is_empty() => inputs.push(text.clone()),
                    Value::String(_) => {
                        return Err("embedding input strings must not be empty".to_owned())
                    }
                    _ => {
                        return Err(
                            "embedding input must be a string or an array of strings".to_owned()
                        )
                    }
                }
            }
            Ok(inputs)
        }
        _ => Err("embedding input must be a string or an array of strings".to_owned()),
    }
}

fn embedding_prompt_text(inputs: &[String]) -> String {
    stable_json_value(&json!({
        "kind": "embedding",
        "input": inputs,
    }))
    .to_string()
}

fn image_generation_prompt_text(request: &ImageGenerationRequest) -> String {
    stable_json_value(&direct_session_image_generation_request_body(request)).to_string()
}

fn image_generation_prompt_hash(request: &ImageGenerationRequest) -> String {
    stable_value_hash(&direct_session_image_generation_request_body(request))
}

fn audio_speech_prompt_hash(request: &AudioSpeechRequest) -> String {
    stable_value_hash(&direct_session_audio_speech_request_body(request))
}

fn audio_transcription_prompt_hash(request: &AudioTranscriptionRequest) -> String {
    stable_value_hash(&direct_session_audio_transcription_request_body(request))
}

fn validate_image_generation_request(
    model: &GatewayModel,
    request: &ImageGenerationRequest,
) -> Result<(), ApiError> {
    if request.prompt.trim().is_empty() {
        return Err(ApiError::bad_request(
            "prompt must not be empty",
            Some("prompt"),
        ));
    }
    let (width, height) = parse_image_generation_size(request)?;
    if let Some(max_width) = model.mayhem.caps.max_image_width {
        if width > max_width {
            return Err(ApiError::bad_request(
                format!("size width exceeds model maximum {max_width}"),
                Some("size"),
            ));
        }
    }
    if let Some(max_height) = model.mayhem.caps.max_image_height {
        if height > max_height {
            return Err(ApiError::bad_request(
                format!("size height exceeds model maximum {max_height}"),
                Some("size"),
            ));
        }
    }
    let _ = image_generation_count(request);
    let steps = image_generation_steps(request);
    if let Some(max_steps) = model.mayhem.caps.max_image_steps {
        if steps > max_steps {
            return Err(ApiError::bad_request(
                format!("steps exceed model maximum {max_steps}"),
                Some("steps"),
            ));
        }
    }
    let _ = image_generation_cfg_scale(request);
    match request.response_format.as_deref().unwrap_or("b64_json") {
        "b64_json" | "url" => Ok(()),
        _ => Err(ApiError::bad_request(
            "only response_format=b64_json or response_format=url is supported",
            Some("response_format"),
        )),
    }
}

fn image_generation_count(request: &ImageGenerationRequest) -> u32 {
    request.n.unwrap_or(1).clamp(1, 4)
}

fn image_generation_steps(request: &ImageGenerationRequest) -> u64 {
    request.steps.unwrap_or(1).clamp(1, 150)
}

fn image_generation_cfg_scale(request: &ImageGenerationRequest) -> f32 {
    request.cfg_scale.unwrap_or(1.0).clamp(0.0, 50.0)
}

fn parse_image_generation_size(request: &ImageGenerationRequest) -> Result<(u32, u32), ApiError> {
    let size = request.size.as_deref().unwrap_or("512x512");
    let (width, height) = size
        .split_once('x')
        .ok_or_else(|| ApiError::bad_request("size must be WIDTHxHEIGHT", Some("size")))?;
    let width = width
        .parse::<u32>()
        .map_err(|_| ApiError::bad_request("size width is not a positive integer", Some("size")))?;
    let height = height.parse::<u32>().map_err(|_| {
        ApiError::bad_request("size height is not a positive integer", Some("size"))
    })?;
    if width == 0 || height == 0 {
        return Err(ApiError::bad_request(
            "size dimensions must be greater than zero",
            Some("size"),
        ));
    }
    Ok((width, height))
}

fn embedding_failover_work_units(inputs: &[String]) -> u64 {
    embedding_input_token_count(inputs).max(u64::try_from(inputs.len()).unwrap_or(u64::MAX))
}

fn image_generation_failover_work_units(request: &ImageGenerationRequest) -> Result<u64, ApiError> {
    let (width, height) = parse_image_generation_size(request)?;
    let pixels = u64::from(width).saturating_mul(u64::from(height)).max(1);
    let resolution_scale = pixels.div_ceil(512 * 512).max(1);
    let image_work = u64::from(image_generation_count(request))
        .saturating_mul(image_generation_steps(request).max(1))
        .saturating_mul(resolution_scale)
        .saturating_mul(1_000);
    Ok(rough_tokens(&request.prompt).saturating_add(image_work))
}

fn image_generation_usage_for_request(request: &ImageGenerationRequest) -> ReceiptUsage {
    let resolution_scale = image_generation_resolution_scale(request).unwrap_or(1);
    image_generation_usage_for_count(
        u64::from(image_generation_count(request)),
        image_generation_steps(request),
        resolution_scale,
    )
}

fn image_generation_usage_for_observed(
    request: &ImageGenerationRequest,
    observed_artifacts: usize,
) -> ReceiptUsage {
    let resolution_scale = image_generation_resolution_scale(request).unwrap_or(1);
    image_generation_usage_for_count(
        u64::try_from(observed_artifacts).unwrap_or(u64::MAX),
        image_generation_steps(request),
        resolution_scale,
    )
}

fn image_generation_resolution_scale(request: &ImageGenerationRequest) -> Result<u64, ApiError> {
    let (width, height) = parse_image_generation_size(request)?;
    let pixels = u64::from(width).saturating_mul(u64::from(height)).max(1);
    Ok(pixels.div_ceil(512 * 512).max(1))
}

fn image_generation_usage_for_count(
    image_count: u64,
    steps: u64,
    resolution_scale: u64,
) -> ReceiptUsage {
    let billed_steps = image_count
        .saturating_mul(steps)
        .saturating_mul(resolution_scale.max(1));
    ReceiptUsage::from_units([(USAGE_IMAGE, image_count), (USAGE_STEP, billed_steps)])
}

fn validate_audio_speech_request(request: &AudioSpeechRequest) -> Result<(), ApiError> {
    if request.input.trim().is_empty() {
        return Err(ApiError::bad_request(
            "input must not be empty",
            Some("input"),
        ));
    }
    if !matches!(audio_speech_response_format(request), "wav") {
        return Err(ApiError::bad_request(
            "only response_format=wav is supported by the launch TTS backend",
            Some("response_format"),
        ));
    }
    if request
        .speed
        .is_some_and(|speed| !speed.is_finite() || speed <= 0.0 || speed > 4.0)
    {
        return Err(ApiError::bad_request(
            "speed must be in the range (0, 4]",
            Some("speed"),
        ));
    }
    Ok(())
}

fn audio_speech_response_format(request: &AudioSpeechRequest) -> &str {
    request.response_format.as_deref().unwrap_or("wav")
}

fn audio_speech_usage_for_request(request: &AudioSpeechRequest) -> ReceiptUsage {
    ReceiptUsage::from_units([
        (
            USAGE_INPUT_CHARACTER,
            u64::try_from(request.input.chars().count()).unwrap_or(u64::MAX),
        ),
        (USAGE_AUDIO_SECOND, estimate_audio_speech_seconds(request)),
    ])
}

fn estimate_audio_speech_seconds(request: &AudioSpeechRequest) -> u64 {
    u64::try_from(request.input.chars().count())
        .unwrap_or(u64::MAX)
        .div_ceil(12)
        .max(1)
}

fn audio_speech_failover_work_units(request: &AudioSpeechRequest) -> u64 {
    estimate_audio_speech_seconds(request).saturating_mul(1_000)
}

fn audio_speech_usage_for_observed(
    request: &AudioSpeechRequest,
    artifacts: &[GatewayArtifactOutput],
) -> ReceiptUsage {
    let input_characters = u64::try_from(request.input.chars().count()).unwrap_or(u64::MAX);
    let audio_seconds = artifacts
        .iter()
        .filter(|artifact| artifact.content_type == "audio/wav")
        .map(|artifact| wav_duration_seconds_ceil(&artifact.bytes).unwrap_or(1))
        .fold(0_u64, u64::saturating_add)
        .max(1);
    ReceiptUsage::from_units([
        (USAGE_INPUT_CHARACTER, input_characters),
        (USAGE_AUDIO_SECOND, audio_seconds),
    ])
}

fn audio_transcription_usage_for_request(request: &AudioTranscriptionRequest) -> ReceiptUsage {
    ReceiptUsage::from_units([(USAGE_AUDIO_SECOND, audio_transcription_seconds(request))])
}

fn audio_transcription_seconds(request: &AudioTranscriptionRequest) -> u64 {
    wav_duration_seconds_ceil(&request.audio).unwrap_or(1)
}

fn audio_transcription_failover_work_units(request: &AudioTranscriptionRequest) -> u64 {
    audio_transcription_seconds(request).saturating_mul(1_000)
}

fn embedding_input_token_count(inputs: &[String]) -> u64 {
    inputs
        .iter()
        .map(|input| rough_tokens(input))
        .fold(0_u64, u64::saturating_add)
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

fn embedding_usage_for_inputs(inputs: &[String]) -> Usage {
    let prompt_tokens = embedding_input_token_count(inputs);
    Usage {
        prompt_tokens,
        completion_tokens: 0,
        total_tokens: prompt_tokens,
    }
}

fn embedding_encoding_format(request: &EmbeddingRequest) -> Result<&'static str, ApiError> {
    match request.encoding_format.as_deref().map(str::trim) {
        None | Some("") => Ok("float"),
        Some(value) if value.eq_ignore_ascii_case("float") => Ok("float"),
        Some(value) if value.eq_ignore_ascii_case("base64") => Ok("base64"),
        Some(_) => Err(ApiError::bad_request(
            "only encoding_format=float or encoding_format=base64 is supported",
            Some("encoding_format"),
        )),
    }
}

fn embedding_response_value(embedding: &[f32], encoding_format: &str) -> Value {
    if encoding_format.eq_ignore_ascii_case("base64") {
        let mut bytes = Vec::with_capacity(embedding.len().saturating_mul(4));
        for value in embedding {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Value::String(BASE64_STANDARD.encode(bytes))
    } else {
        json!(embedding)
    }
}

fn estimate_max_spend_mu(
    price: &PriceRefMu,
    request: &ChatCompletionRequest,
    prompt_text: &str,
) -> u64 {
    let usage = ReceiptUsage::text(
        rough_tokens(prompt_text),
        u64::from(request.max_tokens.unwrap_or(1024).max(1)),
    );
    calculate_mu_owed(price, &usage).max(1_000)
}

fn estimate_embedding_max_spend_mu(price: &PriceRefMu, inputs: &[String]) -> u64 {
    let usage = ReceiptUsage::text(embedding_input_token_count(inputs), 0);
    calculate_mu_owed(price, &usage).max(1_000)
}

fn estimate_image_generation_max_spend_mu(
    price: &PriceRefMu,
    request: &ImageGenerationRequest,
) -> u64 {
    calculate_mu_owed(price, &image_generation_usage_for_request(request)).max(1_000)
}

fn estimate_audio_speech_max_spend_mu(price: &PriceRefMu, request: &AudioSpeechRequest) -> u64 {
    calculate_mu_owed(price, &audio_speech_usage_for_request(request)).max(1_000)
}

fn estimate_audio_transcription_max_spend_mu(
    price: &PriceRefMu,
    request: &AudioTranscriptionRequest,
) -> u64 {
    calculate_mu_owed(price, &audio_transcription_usage_for_request(request)).max(1_000)
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
    fn unauthorized(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
            param,
        }
    }

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

    fn forbidden(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            param,
        }
    }

    fn service_unavailable(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
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

    fn too_many_requests(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
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
        attestation_signing_bytes, ctx_bracket_for_tokens, reassemble_json_payload,
        receipt_signing_bytes_for_version, AttestationSigner, CTX_BRACKET_TABLE_VERSION,
    };

    #[test]
    fn sc_bridge_direct_session_defaults_match_f11_failover_timeouts() {
        let config = ScBridgeGatewaySessionConfig::new("ws://127.0.0.1:49222", "token");
        assert_eq!(
            config.open_timeout,
            Duration::from_millis(DEFAULT_OPEN_TIMEOUT_MILLIS)
        );
        assert_eq!(
            config.ttft_timeout,
            Duration::from_millis(DEFAULT_TTFT_BASE_TIMEOUT_MILLIS)
        );
        assert_eq!(
            config.frame_timeout,
            Duration::from_millis(DEFAULT_STALL_TIMEOUT_MILLIS)
        );
        assert_eq!(config.min_tok_s, None);
        assert_eq!(config.open_timeout, Duration::from_secs(10));
        assert_eq!(config.ttft_timeout, Duration::from_secs(30));
        assert_eq!(config.frame_timeout, Duration::from_secs(30));
    }

    #[test]
    fn direct_session_watchdog_resets_idle_gap_per_delta() {
        let mut watchdog = DirectSessionWatchdog::new(
            0,
            Duration::from_millis(15),
            Duration::from_millis(15),
            None,
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
            None,
        );
        watchdog.record_delta(25);
        assert_eq!(watchdog.next_wait_millis(30), Ok(1));
        assert_eq!(
            watchdog.next_wait_millis(31),
            Err(DirectSessionTimeoutKind::OverallBudget)
        );
    }

    #[test]
    fn direct_session_watchdog_detects_throughput_floor_after_sample_window() {
        let mut watchdog = DirectSessionWatchdog::new(
            0,
            Duration::from_millis(100),
            Duration::from_millis(100),
            None,
            Some(5.0),
        );
        watchdog.record_delta(100);
        assert_eq!(watchdog.throughput_floor_violation(1, 999), None);
        assert_eq!(watchdog.throughput_floor_violation(1, 1_000), None);
        assert_eq!(watchdog.throughput_floor_violation(1, 1_100), Some(1.0));
        assert_eq!(watchdog.throughput_floor_violation(5, 1_000), None);
    }

    #[test]
    fn live_stream_recovery_strips_already_sent_prefix_without_eating_new_text() {
        assert_eq!(strip_live_streamed_prefix("hello world", "hello "), "world");
        assert_eq!(strip_live_streamed_prefix("world", "hello "), "world");
        assert_eq!(strip_live_streamed_prefix("lo world", "hello"), " world");
        assert_eq!(
            strip_live_streamed_prefix("fresh continuation", "hello "),
            "fresh continuation"
        );
    }

    #[test]
    fn failover_thresholds_merge_admin_model_and_user_owned_throughput_overrides() {
        let mut model = test_model();
        model.mayhem.failover = GatewayFailoverPolicyConfig {
            ttft_timeout_ms: Some(7_000),
            min_tok_s: Some(20.0),
            ..GatewayFailoverPolicyConfig::default()
        };
        let state = GatewayState::from_models(vec![model.clone()]).with_failover_policy(
            GatewayFailoverPolicyConfig {
                open_timeout_ms: Some(4_000),
                stall_timeout_ms: Some(6_000),
                min_tok_s: Some(10.0),
                ..GatewayFailoverPolicyConfig::default()
            },
        );
        let options = GatewayRequestOptions {
            failover_overrides: GatewayFailoverPolicyConfig {
                stall_timeout_ms: Some(9_000),
                min_tok_s: Some(30.0),
                ..GatewayFailoverPolicyConfig::default()
            },
            ..GatewayRequestOptions::default()
        };
        let request = test_chat_request(&model.id);
        let invocation = state
            .prepare_chat_invocation_for_route(&model, &request, None, &options)
            .expect("invocation");

        assert_eq!(invocation.failover.open_timeout_ms, 4_000);
        assert_eq!(invocation.failover.ttft_timeout_ms, 7_000);
        assert_eq!(invocation.failover.stall_timeout_ms, 6_000);
        assert_eq!(invocation.failover.min_tok_s, Some(30.0));
    }

    #[test]
    fn failover_thresholds_scale_default_ttft_with_prompt_tokens() {
        let model = test_model();
        let state = GatewayState::from_models(vec![model.clone()]);
        let mut request = test_chat_request(&model.id);
        request.messages = vec![ChatMessage {
            role: "user".to_owned(),
            content: json!("x ".repeat(100_000)),
            name: None,
            extra: BTreeMap::new(),
        }];
        let invocation = state
            .prepare_chat_invocation_for_route(
                &model,
                &request,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("invocation");
        assert_eq!(invocation.failover.open_timeout_ms, 10_000);
        assert_eq!(invocation.failover.stall_timeout_ms, 30_000);
        assert_eq!(invocation.failover.ttft_timeout_ms, 130_000);
    }

    #[test]
    fn one_shot_failover_deadlines_scale_with_work_size() {
        let model = test_model();
        let state = GatewayState::from_models(vec![model.clone()]);

        let small_image = ImageGenerationRequest {
            model: model.id.clone(),
            prompt: "small".to_owned(),
            n: Some(1),
            size: Some("512x512".to_owned()),
            response_format: None,
            steps: Some(1),
            cfg_scale: None,
            seed: None,
            user: None,
        };
        let mut large_image = small_image.clone();
        large_image.n = Some(4);
        large_image.size = Some("1024x1024".to_owned());
        large_image.steps = Some(40);
        let small_image_invocation = state
            .prepare_image_generation_invocation_for_route(
                &model,
                &small_image,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("small image invocation");
        let large_image_invocation = state
            .prepare_image_generation_invocation_for_route(
                &model,
                &large_image,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("large image invocation");
        assert!(
            large_image_invocation.failover.ttft_timeout_ms
                > small_image_invocation.failover.ttft_timeout_ms
        );

        let small_embedding = vec!["alpha".to_owned()];
        let large_embedding = vec!["alpha ".repeat(5_000)];
        let small_embedding_invocation = state
            .prepare_embedding_invocation_for_route(
                &model,
                &small_embedding,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("small embedding invocation");
        let large_embedding_invocation = state
            .prepare_embedding_invocation_for_route(
                &model,
                &large_embedding,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("large embedding invocation");
        assert!(
            large_embedding_invocation.failover.ttft_timeout_ms
                > small_embedding_invocation.failover.ttft_timeout_ms
        );

        let small_audio = AudioSpeechRequest {
            model: model.id.clone(),
            input: "short".to_owned(),
            voice: None,
            response_format: Some("wav".to_owned()),
            speed: None,
        };
        let mut large_audio = small_audio.clone();
        large_audio.input = "long ".repeat(1_000);
        let small_audio_invocation = state
            .prepare_audio_speech_invocation_for_route(
                &model,
                &small_audio,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("small audio invocation");
        let large_audio_invocation = state
            .prepare_audio_speech_invocation_for_route(
                &model,
                &large_audio,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("large audio invocation");
        assert!(
            large_audio_invocation.failover.ttft_timeout_ms
                > small_audio_invocation.failover.ttft_timeout_ms
        );
    }

    #[test]
    fn receipt_wallet_balance_and_rail_are_configurable() {
        let model = test_model();
        let request = test_chat_request(&model.id);
        let user_seed = [12_u8; 32];
        let state = GatewayState::from_models(vec![model.clone()])
            .with_receipt_user_seed(user_seed)
            .with_receipt_balance_mu(2_000_000)
            .with_receipt_rail("tnk");
        let invocation = state
            .prepare_chat_invocation_for_route(
                &model,
                &request,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("configured wallet has enough balance");

        assert_eq!(invocation.rail, "tnk");
        assert_eq!(invocation.user_pubkey, verifying_key_hex(&user_seed));
        assert_eq!(invocation.receipt_user_seed, user_seed);

        let checkpointed = GatewayState::from_models(vec![model.clone()])
            .with_receipt_user_seed(user_seed)
            .with_receipt_balance_mu(2_000_000)
            .with_receipt_checkpoint_every(CheckpointPolicy {
                tokens: 32,
                ms: 2500,
            })
            .prepare_chat_invocation_for_route(
                &model,
                &request,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("configured checkpoint policy should be accepted");
        assert_eq!(checkpointed.spend_voucher.body.checkpoint_every.tokens, 32);
        assert_eq!(checkpointed.spend_voucher.body.checkpoint_every.ms, 2500);

        let low_balance = GatewayState::from_models(vec![model.clone()])
            .with_receipt_user_seed(user_seed)
            .with_receipt_balance_mu(1);
        let err = low_balance
            .prepare_chat_invocation_for_route(
                &model,
                &request,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect_err("gateway rejects spend vouchers above the startup balance snapshot");
        assert!(err.message.contains("insufficient local balance"));

        let max_bid = GatewayRequestOptions {
            max_price_mu: Some(999),
            ..GatewayRequestOptions::default()
        };
        let err = state
            .prepare_chat_invocation_for_route(&model, &request, None, &max_bid)
            .expect_err("gateway rejects quotes above the user max-bid");
        assert_eq!(err.param, Some("X-Mayhem-Max-Price-Mu"));
    }

    #[test]
    fn spend_voucher_uses_admin_ctx_bracket_schedule() {
        let mut model = test_model();
        model.mayhem.caps.ctx = 12_000;
        let request = test_chat_request(&model.id);
        let schedule = CtxBracketSchedule {
            current: mayhem_proto::CtxBracketTableRecord {
                ver: 2,
                submitted_at: 1,
                effective_at: 1,
                brackets: vec![
                    mayhem_proto::CtxBracketEntry {
                        id: "le16k".to_owned(),
                        max_ctx: Some(16_384),
                    },
                    mayhem_proto::CtxBracketEntry {
                        id: "gt16k".to_owned(),
                        max_ctx: None,
                    },
                ],
            },
            pending: None,
        };
        let invocation = GatewayState::from_models(vec![model.clone()])
            .with_ctx_bracket_schedule(schedule)
            .prepare_chat_invocation_for_route(
                &model,
                &request,
                None,
                &GatewayRequestOptions::default(),
            )
            .expect("invocation uses custom context table");

        assert_eq!(invocation.served_ctx, 12_000);
        assert_eq!(invocation.ctx_bracket, "le16k");
        assert_eq!(invocation.ctx_bracket_table_ver, 2);
        assert!(invocation.opened_at > 0);
        assert_eq!(invocation.spend_voucher.body.ctx_bracket, "le16k");
        assert_eq!(invocation.spend_voucher.body.ctx_bracket_table_ver, 2);
    }

    #[test]
    fn catalog_model_parses_per_model_failover_policy() {
        let model = model_from_catalog_value(
            &json!({
                "model_id": "admin/quality-test",
                "failover": {
                    "open_timeout_ms": 2_000,
                    "ttft_timeout_ms": 5_000,
                    "stall_timeout_ms": 8_000,
                    "min_tok_s": 12.5
                }
            }),
            1,
        )
        .expect("catalog model");

        assert_eq!(model.mayhem.failover.open_timeout_ms, Some(2_000));
        assert_eq!(model.mayhem.failover.ttft_timeout_ms, Some(5_000));
        assert_eq!(model.mayhem.failover.stall_timeout_ms, Some(8_000));
        assert_eq!(model.mayhem.failover.min_tok_s, Some(12.5));
    }

    #[test]
    fn request_failover_headers_parse_and_reject_invalid_values() {
        let mut headers = HeaderMap::new();
        headers.insert("x-mayhem-min-tok-s", HeaderValue::from_static("17.5"));
        headers.insert("x-mayhem-max-price-mu", HeaderValue::from_static("1234"));
        headers.insert("x-mayhem-max-wait-ms", HeaderValue::from_static("0"));
        headers.insert("x-mayhem-quant", HeaderValue::from_static("Q4_K_M"));
        let options = GatewayRequestOptions::from_headers(&headers).expect("headers parse");
        assert_eq!(options.failover_overrides.open_timeout_ms, None);
        assert_eq!(options.failover_overrides.ttft_timeout_ms, None);
        assert_eq!(options.failover_overrides.stall_timeout_ms, None);
        assert_eq!(options.failover_overrides.min_tok_s, Some(17.5));
        assert_eq!(options.max_price_mu, Some(1_234));
        assert_eq!(options.max_wait_ms, 0);
        assert_eq!(options.quant.as_deref(), Some("int4"));

        headers.insert("x-mayhem-open-timeout-ms", HeaderValue::from_static("2500"));
        let err =
            GatewayRequestOptions::from_headers(&headers).expect_err("timeout header rejects");
        assert_eq!(err.param, Some("X-Mayhem-Open-Timeout-Ms"));
        assert!(err.message.contains("admin catalog controlled"));
        headers.remove("x-mayhem-open-timeout-ms");

        headers.insert("x-mayhem-max-price-mu", HeaderValue::from_static("0"));
        let err =
            GatewayRequestOptions::from_headers(&headers).expect_err("zero max price rejects");
        assert_eq!(err.param, Some("X-Mayhem-Max-Price-Mu"));
        headers.insert("x-mayhem-max-price-mu", HeaderValue::from_static("1234"));

        headers.insert("x-mayhem-max-wait-ms", HeaderValue::from_static("60001"));
        let err = GatewayRequestOptions::from_headers(&headers).expect_err("too-long wait rejects");
        assert_eq!(err.param, Some("X-Mayhem-Max-Wait-Ms"));
        headers.insert("x-mayhem-max-wait-ms", HeaderValue::from_static("1000"));

        headers.insert("x-mayhem-min-tok-s", HeaderValue::from_static("0"));
        let options = GatewayRequestOptions::from_headers(&headers).expect("zero floor disables");
        assert_eq!(options.failover_overrides.min_tok_s, Some(0.0));
        headers.insert("x-mayhem-min-tok-s", HeaderValue::from_static("17.5"));

        headers.insert("x-mayhem-quant", HeaderValue::from_static("potato"));
        let err = GatewayRequestOptions::from_headers(&headers).expect_err("bad quant rejects");
        assert_eq!(err.param, Some("X-Mayhem-Quant"));
    }

    #[test]
    fn gateway_default_max_price_applies_when_header_is_absent() {
        let state = GatewayState::from_models(vec![test_model()])
            .with_default_max_price_mu(Some(777))
            .with_default_max_wait_ms(Some(3_000));
        let headers = HeaderMap::new();
        let options = state
            .request_options_from_headers(&headers)
            .expect("empty headers parse");
        assert_eq!(options.max_price_mu, Some(777));
        assert_eq!(options.max_wait_ms, 3_000);

        let mut headers = HeaderMap::new();
        headers.insert("x-mayhem-max-price-mu", HeaderValue::from_static("1234"));
        headers.insert("x-mayhem-max-wait-ms", HeaderValue::from_static("0"));
        let options = state
            .request_options_from_headers(&headers)
            .expect("headers parse");
        assert_eq!(options.max_price_mu, Some(1_234));
        assert_eq!(options.max_wait_ms, 0);
    }

    #[test]
    fn shared_gateway_bind_requires_active_tokens_and_enforced_auth() {
        let loopback = "127.0.0.1:11435".parse().unwrap();
        let shared = "0.0.0.0:11435".parse().unwrap();

        assert!(validate_gateway_bind_access(loopback, false, false).is_ok());
        assert!(validate_gateway_bind_access(loopback, true, false).is_err());
        assert!(validate_gateway_bind_access(shared, true, false).is_err());
        assert!(validate_gateway_bind_access(shared, false, true).is_err());
        assert!(validate_gateway_bind_access(shared, true, true).is_ok());
    }

    #[test]
    fn gateway_access_control_authorizes_hashes_and_rejects_bad_tokens() {
        let raw_token = "sk-mayhem-test-token";
        let store = GatewayTokenStore {
            version: 1,
            tokens: vec![GatewayTokenRecord {
                name: "agent".to_owned(),
                token_hash: gateway_token_hash(raw_token),
                token_id: "tok_test".to_owned(),
                created_at: 1,
                expires_at: None,
                budget_mu: None,
                budget_period: None,
                spent_total_mu: 0,
                spent_period_mu: 0,
                period_started_at: Some(1),
                max_rate_per_minute: Some(1),
                models: vec!["mayhem/dev-chat-tools".to_owned()],
                last_used_at: None,
                revoked_at: None,
            }],
        };
        let access = GatewayAccessControl::new(true, store, None);
        let missing = HeaderMap::new();
        assert_eq!(
            access
                .authorize(&missing, Some("mayhem/dev-chat-tools"))
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {raw_token}")).unwrap(),
        );
        let attribution = access
            .authorize(&headers, Some("mayhem/dev-chat-tools"))
            .expect("valid token accepted")
            .expect("token attribution");
        assert_eq!(attribution.name, "agent");

        assert_eq!(
            access
                .authorize(&headers, Some("mayhem/other-model"))
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            access
                .authorize(&headers, Some("mayhem/dev-chat-tools"))
                .unwrap_err()
                .status,
            StatusCode::TOO_MANY_REQUESTS
        );

        let mut wrong = HeaderMap::new();
        wrong.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-mayhem-wrong"),
        );
        assert_eq!(
            access
                .authorize(&wrong, Some("mayhem/dev-chat-tools"))
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );

        let rejected_token = |mut token: GatewayTokenRecord| {
            token.token_hash = gateway_token_hash("sk-mayhem-rejected");
            let store = GatewayTokenStore {
                version: 1,
                tokens: vec![token],
            };
            let access = GatewayAccessControl::new(true, store, None);
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer sk-mayhem-rejected"),
            );
            access
                .authorize(&headers, Some("mayhem/dev-chat-tools"))
                .unwrap_err()
                .status
        };
        let base = GatewayTokenRecord {
            name: "agent".to_owned(),
            token_hash: gateway_token_hash("unused"),
            token_id: "tok_rejected".to_owned(),
            created_at: 1,
            expires_at: None,
            budget_mu: None,
            budget_period: None,
            spent_total_mu: 0,
            spent_period_mu: 0,
            period_started_at: Some(1),
            max_rate_per_minute: None,
            models: Vec::new(),
            last_used_at: None,
            revoked_at: None,
        };
        assert_eq!(
            rejected_token(GatewayTokenRecord {
                revoked_at: Some(2),
                ..base.clone()
            }),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            rejected_token(GatewayTokenRecord {
                expires_at: Some(1),
                ..base.clone()
            }),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            rejected_token(GatewayTokenRecord {
                budget_mu: Some(7),
                budget_period: Some(GatewayTokenBudgetPeriod::Total),
                spent_total_mu: 7,
                ..base
            }),
            StatusCode::PAYMENT_REQUIRED
        );
    }

    #[test]
    fn gateway_receipt_attribution_records_token_spend_delta() {
        let model = test_model();
        let request = test_chat_request(&model.id);
        let output = test_chat_output();
        let access_token = GatewayTokenAttribution {
            name: "agent".to_owned(),
            token_id: "tok_delta".to_owned(),
        };
        let store = GatewayTokenStore {
            version: 1,
            tokens: vec![GatewayTokenRecord {
                name: access_token.name.clone(),
                token_hash: gateway_token_hash("sk-mayhem-delta"),
                token_id: access_token.token_id.clone(),
                created_at: 1,
                expires_at: None,
                budget_mu: Some(1_000_000),
                budget_period: Some(GatewayTokenBudgetPeriod::Total),
                spent_total_mu: 0,
                spent_period_mu: 0,
                period_started_at: Some(1),
                max_rate_per_minute: None,
                models: Vec::new(),
                last_used_at: None,
                revoked_at: None,
            }],
        };
        let state = GatewayState::from_models(vec![model.clone()])
            .with_access_control(GatewayAccessControl::new(false, store, None));
        let mut invocation = test_invocation();
        invocation.access_token = Some(access_token);

        let stored = state
            .meter_chat_session(&model, &request, &output, &invocation, None)
            .expect("metering succeeds");
        let access = state.access_summary();
        let spent = access["tokens"][0]["spent_total_mu"].as_u64().unwrap();
        assert_eq!(spent, stored.receipt.body.mu_owed_cum);

        state
            .record_receipt(stored.clone())
            .expect("duplicate cumulative receipt is accepted");
        let access = state.access_summary();
        assert_eq!(
            access["tokens"][0]["spent_total_mu"].as_u64().unwrap(),
            spent
        );
    }

    #[tokio::test]
    async fn route_wait_rechecks_until_route_becomes_eligible_or_expires() {
        let model = test_routed_model(1);
        let state = GatewayState::from_models(vec![model.clone()]);
        let options = GatewayRequestOptions {
            max_wait_ms: 40,
            ..GatewayRequestOptions::default()
        };
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let outcome = wait_for_eligible_routes_with_poll(
            &state,
            &model,
            &options,
            Vec::new(),
            || {
                if calls_for_refresh.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1 {
                    vec![&model.mayhem.route_candidates[0]]
                } else {
                    Vec::new()
                }
            },
            Duration::from_millis(1),
        )
        .await;
        assert!(outcome.waited);
        assert_eq!(outcome.routes.len(), 1);

        let instant = GatewayRequestOptions {
            max_wait_ms: 0,
            ..GatewayRequestOptions::default()
        };
        let outcome = wait_for_eligible_routes_with_poll(
            &state,
            &model,
            &instant,
            Vec::new(),
            || vec![&model.mayhem.route_candidates[0]],
            Duration::from_millis(1),
        )
        .await;
        assert!(!outcome.waited);
        assert!(outcome.routes.is_empty());
    }

    #[test]
    fn embedding_canary_matches_exact_vector_and_cosine_tolerance() {
        let expected = test_embedding_vector("admin/embed-fixture", "fixed canary", 16);
        let same = test_embedding_vector("admin/embed-fixture", "fixed canary", 16);
        let different = test_embedding_vector("admin/embed-fixture", "different canary", 16);

        assert_eq!(expected, same);
        assert_eq!(
            embedding_cosine_similarity_bps(&expected, &same),
            Some(10_000)
        );
        assert!(embedding_canary_matches(&expected, &same, 0));
        assert!(!embedding_canary_matches(&expected, &different, 1));
        assert_eq!(embedding_cosine_similarity_bps(&expected, &[]), None);
    }

    fn test_embedding_vector(model_id: &str, input: &str, dimensions: usize) -> Vec<f32> {
        let mut values = (0..dimensions)
            .map(|index| {
                let digest = blake3::hash(
                    format!("mayhem-embedding-test:{model_id}:{input}:{index}").as_bytes(),
                );
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

        let checkpointed_final =
            test_provider_receipt_with_finality(&model, &request, &output, &invocation, 3, true);
        let checkpointed_ack = direct_session_receipt_ack(
            &request,
            &output,
            &invocation,
            &checkpointed_final,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect("checkpointed final receipt seq should be accepted");
        assert_eq!(checkpointed_ack.seq, 3);
        let checkpointed_stored = state
            .meter_chat_session(
                &model,
                &request,
                &output,
                &invocation,
                Some(&checkpointed_final),
            )
            .expect("checkpointed final receipt should be co-signed");
        assert_eq!(checkpointed_stored.receipt.body.seq, 3);

        let cached_output = ChatOutput {
            usage: Usage {
                prompt_tokens: 1_000,
                completion_tokens: 0,
                total_tokens: 1_000,
            },
            ..output.clone()
        };
        let mut cached_receipt =
            test_provider_receipt(&model, &request, &cached_output, &invocation);
        cached_receipt.body.usage = ReceiptUsage::text_with_cached(500, 500, 0);
        cached_receipt.body.mu_owed_cum =
            calculate_locked_mu_owed(&invocation, &cached_receipt.body.usage);
        cached_receipt.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&cached_receipt.body).unwrap(),
        );
        let cached_stored = state
            .meter_chat_session(
                &model,
                &request,
                &cached_output,
                &invocation,
                Some(&cached_receipt),
            )
            .expect("cached input receipt should be co-signed");
        assert_eq!(
            cached_stored
                .receipt
                .body
                .usage
                .get(USAGE_CACHED_INPUT_TOKEN),
            500
        );
        assert_eq!(cached_stored.receipt.body.usage.prompt_tokens(), 1_000);
        assert_eq!(cached_stored.receipt.body.mu_owed_cum, 13);

        let mut false_cache = cached_receipt.clone();
        false_cache.body.usage = ReceiptUsage::text_with_cached(100, 500, 0);
        false_cache.body.mu_owed_cum =
            calculate_locked_mu_owed(&invocation, &false_cache.body.usage);
        false_cache.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&false_cache.body).unwrap(),
        );
        let err = state
            .meter_chat_session(
                &model,
                &request,
                &cached_output,
                &invocation,
                Some(&false_cache),
            )
            .expect_err("false cache hit must not be co-signed");
        assert!(err.message.contains("cached prompt usage mismatch"));

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

        let mut wrong_usage = provider_receipt.clone();
        wrong_usage.body.usage = ReceiptUsage::text(
            output.usage.prompt_tokens,
            output.usage.completion_tokens.saturating_add(1),
        );
        wrong_usage.body.mu_owed_cum =
            calculate_locked_mu_owed(&invocation, &wrong_usage.body.usage);
        wrong_usage.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&wrong_usage.body).unwrap(),
        );
        let err = direct_session_receipt_ack(
            &request,
            &output,
            &invocation,
            &wrong_usage,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect_err("inflated provider usage must not receive a receipt ack");
        assert!(err.message.contains("usage"));

        let mut wrong_sig = provider_receipt;
        wrong_sig.enclave_sig = "11".repeat(64);
        let err = state
            .meter_chat_session(&model, &request, &output, &invocation, Some(&wrong_sig))
            .expect_err("wrong enclave signature must be rejected");
        assert!(err.message.contains("signature"));
    }

    #[test]
    fn partial_provider_receipt_must_match_gateway_observed_usage_before_ack() {
        let model = test_model();
        let request = test_chat_request(&model.id);
        let output = test_chat_output();
        let invocation = test_invocation();
        let provider_receipt =
            test_provider_receipt_with_finality(&model, &request, &output, &invocation, 1, false);
        let partial = GatewaySessionPartial {
            output: output.clone(),
            provider_receipt: provider_receipt.clone(),
            token_ids: vec![1, 2, 3],
            quality: Some(GatewaySessionQuality {
                ttft_ms: 10,
                tok_s: Some(20.0),
            }),
            reason: "checkpoint".to_owned(),
            redispatch_mode: RedispatchMode::FullMessageHistoryClientSide,
        };

        let ack = direct_session_partial_receipt_ack(
            &request,
            &invocation,
            &partial,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect("matching partial receipt should be acked");
        assert_eq!(ack.seq, provider_receipt.body.seq);

        let mut inflated = partial;
        inflated.provider_receipt.body.usage = ReceiptUsage::text(
            output.usage.prompt_tokens,
            output.usage.completion_tokens.saturating_add(1),
        );
        inflated.provider_receipt.body.mu_owed_cum =
            calculate_locked_mu_owed(&invocation, &inflated.provider_receipt.body.usage);
        inflated.provider_receipt.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&inflated.provider_receipt.body).unwrap(),
        );
        let err = direct_session_partial_receipt_ack(
            &request,
            &invocation,
            &inflated,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect_err("inflated partial receipt must not receive a receipt ack");
        assert!(err.message.contains("usage"));
    }

    #[test]
    fn client_disconnect_uses_last_checkpoint_partial_without_redispatch_marker() {
        let model = test_model();
        let request = test_chat_request(&model.id);
        let output = test_chat_output();
        let invocation = test_invocation();
        let provider_receipt =
            test_provider_receipt_with_finality(&model, &request, &output, &invocation, 2, false);
        let mut watchdog = DirectSessionWatchdog::new(
            1_000,
            Duration::from_secs(5),
            Duration::from_secs(5),
            None,
            None,
        );
        watchdog.record_delta(1_100);

        let err = client_disconnect_direct_session_error(
            output.content.as_deref().unwrap(),
            None,
            Some(&provider_receipt),
            &[1, 2, 3],
            &watchdog,
            1_250,
        );

        assert!(err.retryable);
        assert!(is_client_disconnect_error(&err));
        let partial = err
            .partial
            .expect("checkpointed disconnect should carry partial");
        assert_eq!(partial.reason, "client_disconnect");
        assert_eq!(partial.output.content.as_deref(), output.content.as_deref());
        assert_eq!(partial.output.usage, output.usage);
        assert_eq!(partial.token_ids, vec![1, 2, 3]);
        assert_eq!(partial.provider_receipt.body.seq, 2);
        assert!(!partial.provider_receipt.body.final_receipt);
    }

    #[test]
    fn embedding_provider_receipt_must_match_gateway_observed_usage() {
        let state = GatewayState::fixture();
        let model = test_model();
        let inputs = vec!["alpha".to_owned(), "beta gamma".to_owned()];
        let output = EmbeddingOutput {
            embeddings: vec![vec![0.1, 0.2, 0.3], vec![0.2, 0.3, 0.4]],
            usage: Usage {
                prompt_tokens: 3,
                completion_tokens: 0,
                total_tokens: 3,
            },
        };
        let invocation = test_invocation();
        let provider_receipt =
            test_embedding_provider_receipt(&model, &inputs, &output, &invocation);

        let stored = state
            .meter_embedding_session(
                &model,
                &inputs,
                &output,
                &invocation,
                Some(&provider_receipt),
            )
            .expect("matching embedding provider receipt is co-signed");
        let live_ack = direct_session_embedding_receipt_ack(
            &inputs,
            &output,
            &invocation,
            &provider_receipt,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect("embedding receipt ack signs the accepted receipt");
        assert_eq!(live_ack, stored.receipt_ack);

        let mut inflated_usage = provider_receipt.clone();
        inflated_usage.body.usage = ReceiptUsage::text(9, 0);
        inflated_usage.body.mu_owed_cum =
            calculate_locked_mu_owed(&invocation, &inflated_usage.body.usage);
        inflated_usage.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&inflated_usage.body).unwrap(),
        );
        let err = direct_session_embedding_receipt_ack(
            &inputs,
            &output,
            &invocation,
            &inflated_usage,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect_err("inflated usage must not receive a receipt ack");
        assert!(err.message.contains("usage"));

        let mut wrong_amount = provider_receipt;
        wrong_amount.body.mu_owed_cum = wrong_amount.body.mu_owed_cum.saturating_add(1);
        wrong_amount.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&wrong_amount.body).unwrap(),
        );
        let err = state
            .meter_embedding_session(&model, &inputs, &output, &invocation, Some(&wrong_amount))
            .expect_err("wrong embedding amount must be rejected");
        assert!(err.message.contains("amount"));
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
            artifact_sidecar_roots: identity.artifact_sidecar_roots.clone(),
            manifest_hash: identity.manifest_hash.clone(),
            binary_hash: identity.binary_hash.clone(),
            att_tier: 1,
            caps: json!({}),
        };
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            rail: "fiat".to_owned(),
            enclave_id: enclave_id.clone(),
            price_ver: 7,
            locked_rate_map: text_generation_rate_map(20, 60),
            locked_per_req_mu: 0,
            locked_min_session_mu: 0,
            served_ctx: 4096,
            ctx_bracket: ctx_bracket_for_tokens(4096).to_owned(),
            ctx_bracket_table_ver: CTX_BRACKET_TABLE_VERSION,
            max_spend_mu: 1000,
            checkpoint_every: CheckpointPolicy {
                tokens: 128,
                ms: 30_000,
            },
        };
        GatewaySessionInvocation {
            contract_version: CONTRACT_VERSION,
            session_id,
            rail: "fiat".to_owned(),
            user_pubkey: verifying_key_hex(&test_user_seed()),
            provider_pubkey: Some(verifying_key_hex(&test_provider_seed())),
            enclave_id,
            price_ver: 7,
            opened_at: 1,
            served_ctx: voucher_body.served_ctx,
            ctx_bracket: voucher_body.ctx_bracket.clone(),
            ctx_bracket_table_ver: voucher_body.ctx_bracket_table_ver,
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
            failover: GatewayFailoverInvocation::default(),
            access_token: None,
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
                    per_req_mu: 0,
                    min_session_mu: 0,
                    derivation: None,
                },
                attestation_tiers: BTreeMap::from([("T1".to_owned(), 1)]),
                attestation_tier_labels: attestation_tier_labels_for_counts(&BTreeMap::from([(
                    "T1".to_owned(),
                    1,
                )])),
                quant_buckets: BTreeMap::from([(DEFAULT_QUANT_BUCKET.to_owned(), 1)]),
                min_app_version: None,
                caps: ModelCaps {
                    tools: true,
                    json: true,
                    ctx: 8192,
                    vision: false,
                    image: false,
                    video: false,
                    audio: false,
                    max_image_width: None,
                    max_image_height: None,
                    max_image_steps: None,
                    output_modality: Some("text".to_owned()),
                    output_modalities: vec!["text".to_owned()],
                },
                adapter: ShapeAdapterInfo::default(),
                failover: GatewayFailoverPolicyConfig::default(),
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
            user: None,
            metadata: BTreeMap::new(),
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
    fn live_sse_event_buffer_scales_with_token_budget() {
        let mut request = test_chat_request("mayhem/test-model");
        assert_eq!(
            live_sse_event_buffer_capacity(&request),
            LIVE_SSE_DEFAULT_MAX_TOKENS * 2 + 64
        );

        request.max_tokens = Some(32);
        assert_eq!(
            live_sse_event_buffer_capacity(&request),
            LIVE_SSE_MIN_EVENT_BUFFER
        );

        request.max_tokens = Some(2048);
        assert_eq!(live_sse_event_buffer_capacity(&request), 4160);

        request.max_tokens = Some(u32::MAX);
        assert_eq!(
            live_sse_event_buffer_capacity(&request),
            LIVE_SSE_MAX_EVENT_BUFFER
        );
    }

    #[test]
    fn session_delta_token_ids_support_full_and_delta_lists() {
        let full = json!({ "token_ids": [1, 2, 3] });
        assert_eq!(token_ids_from_session_delta(&full), Some(vec![1, 2, 3]));
        assert_eq!(token_ids_delta_from_session_delta(&full), None);

        let delta = json!({ "token_ids_delta": [4, 5] });
        assert_eq!(token_ids_from_session_delta(&delta), None);
        assert_eq!(token_ids_delta_from_session_delta(&delta), Some(vec![4, 5]));
    }

    fn test_routed_model(provider_count: u8) -> GatewayModel {
        test_routed_model_with_id("mayhem/routing-test", 0, provider_count)
    }

    fn test_routed_model_with_id(
        model_id: &str,
        provider_offset: u8,
        provider_count: u8,
    ) -> GatewayModel {
        let mut model = test_model();
        model.id = model_id.to_owned();
        model.mayhem.source = "contract".to_owned();
        model.mayhem.providers_online = u32::from(provider_count);
        model.mayhem.rooms = u32::from(provider_count);
        model.mayhem.route_candidates = (0..provider_count)
            .map(|idx| test_route_candidate(provider_offset.wrapping_add(idx)))
            .collect::<Vec<_>>();
        model
    }

    fn test_route_candidate(idx: u8) -> GatewayRouteCandidate {
        GatewayRouteCandidate {
            provider: format!("{:02x}", idx.wrapping_add(1)).repeat(32),
            accepted_rails: vec!["fiat".to_owned(), "tap".to_owned(), "tnk".to_owned()],
            enclave_id: format!("{:02x}", idx.wrapping_add(80)).repeat(32),
            room_id: format!("{:02x}", idx.wrapping_add(160)).repeat(16),
            price_ver: 7,
            price_ref_mu: None,
            min_ask_mu: 0,
            att_tier: 1,
            quant: DEFAULT_QUANT_BUCKET.to_owned(),
            admin_pubkey: "33".repeat(32),
            artifact_root: format!("{:02x}", idx.wrapping_add(180)).repeat(32),
            artifact_sidecar_roots: BTreeMap::new(),
            manifest_hash: format!("{:02x}", idx.wrapping_add(190)).repeat(32),
            binary_hash: format!("{:02x}", idx.wrapping_add(200)).repeat(32),
            kyb: None,
            reputation_bps: 10_000,
            probation: None,
            caps: json!({}),
            local_run: None,
        }
    }

    #[derive(Debug)]
    struct PartialThenSuccessBackend {
        attempts: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
        providers: Arc<Mutex<Vec<String>>>,
    }

    impl GatewaySessionBackend for PartialThenSuccessBackend {
        fn name(&self) -> &str {
            "test-partial-then-success"
        }

        fn run_chat<'a>(
            &'a self,
            model: &'a GatewayModel,
            request: &'a ChatCompletionRequest,
            invocation: &'a GatewaySessionInvocation,
        ) -> GatewaySessionFuture<'a> {
            Box::pin(async move {
                let attempt = {
                    self.providers
                        .lock()
                        .expect("providers lock")
                        .push(invocation.provider_pubkey.clone().unwrap_or_default());
                    let mut attempts = self.attempts.lock().expect("attempts lock");
                    attempts.push(request.messages.clone());
                    attempts.len()
                };
                if attempt == 1 {
                    let output = ChatOutput {
                        content: Some("hello ".to_owned()),
                        tool_call: None,
                        artifacts: Vec::new(),
                        finish_reason: "interrupted".to_owned(),
                        usage: Usage {
                            prompt_tokens: rough_tokens(&chat_prompt_text(request)),
                            completion_tokens: 1,
                            total_tokens: rough_tokens(&chat_prompt_text(request)) + 1,
                        },
                    };
                    let provider_receipt = test_provider_receipt_with_finality(
                        model, request, &output, invocation, 1, false,
                    );
                    return Err(GatewaySessionError::retryable_partial(
                        "simulated mid-stream stall after checkpoint",
                        GatewaySessionPartial {
                            output,
                            provider_receipt,
                            token_ids: vec![11],
                            quality: Some(GatewaySessionQuality {
                                ttft_ms: 25,
                                tok_s: Some(20.0),
                            }),
                            reason: "mid_stream_stall".to_owned(),
                            redispatch_mode: RedispatchMode::FullMessageHistoryClientSide,
                        },
                    ));
                }

                assert!(
                    request.messages.iter().any(|message| {
                        message.role == "assistant" && message.content == json!("hello ")
                    }),
                    "redispatch request should include the checkpointed assistant prefix"
                );
                let output = ChatOutput {
                    content: Some("world".to_owned()),
                    tool_call: None,
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens: rough_tokens(&chat_prompt_text(request)),
                        completion_tokens: 1,
                        total_tokens: rough_tokens(&chat_prompt_text(request)) + 1,
                    },
                };
                Ok(GatewaySessionResult {
                    output: output.clone(),
                    backend: self.name().to_owned(),
                    direct_session: true,
                    provider_receipt: Some(test_provider_receipt(
                        model, request, &output, invocation,
                    )),
                    token_ids: vec![22],
                    quality: Some(GatewaySessionQuality {
                        ttft_ms: 30,
                        tok_s: Some(25.0),
                    }),
                })
            })
        }
    }

    #[derive(Debug)]
    struct SlowQualityThenSuccessBackend {
        providers: Arc<Mutex<Vec<String>>>,
    }

    impl GatewaySessionBackend for SlowQualityThenSuccessBackend {
        fn name(&self) -> &str {
            "test-slow-quality-then-success"
        }

        fn run_chat<'a>(
            &'a self,
            _model: &'a GatewayModel,
            request: &'a ChatCompletionRequest,
            invocation: &'a GatewaySessionInvocation,
        ) -> GatewaySessionFuture<'a> {
            Box::pin(async move {
                let attempt = {
                    let mut providers = self.providers.lock().expect("providers lock");
                    providers.push(invocation.provider_pubkey.clone().unwrap_or_default());
                    providers.len()
                };
                let prompt_tokens = rough_tokens(&chat_prompt_text(request));
                let (content, tok_s) = if attempt == 1 {
                    ("slow", 1.0)
                } else {
                    ("fast", 25.0)
                };
                Ok(GatewaySessionResult {
                    output: ChatOutput {
                        content: Some(content.to_owned()),
                        tool_call: None,
                        artifacts: Vec::new(),
                        finish_reason: "stop".to_owned(),
                        usage: Usage {
                            prompt_tokens,
                            completion_tokens: 1,
                            total_tokens: prompt_tokens + 1,
                        },
                    },
                    backend: self.name().to_owned(),
                    direct_session: true,
                    provider_receipt: None,
                    token_ids: vec![attempt as i32],
                    quality: Some(GatewaySessionQuality {
                        ttft_ms: 10,
                        tok_s: Some(tok_s),
                    }),
                })
            })
        }
    }

    #[derive(Debug)]
    struct CleanRefusalThenSuccessBackend {
        providers: Arc<Mutex<Vec<String>>>,
    }

    impl GatewaySessionBackend for CleanRefusalThenSuccessBackend {
        fn name(&self) -> &str {
            "test-clean-refusal-then-success"
        }

        fn run_chat<'a>(
            &'a self,
            _model: &'a GatewayModel,
            request: &'a ChatCompletionRequest,
            invocation: &'a GatewaySessionInvocation,
        ) -> GatewaySessionFuture<'a> {
            Box::pin(async move {
                let attempt = {
                    let mut providers = self.providers.lock().expect("providers lock");
                    providers.push(invocation.provider_pubkey.clone().unwrap_or_default());
                    providers.len()
                };
                if attempt == 1 {
                    return Err(GatewaySessionError::clean_refusal(
                        "provider rejected session with CAPACITY: provider full",
                    ));
                }
                let prompt_tokens = rough_tokens(&chat_prompt_text(request));
                Ok(GatewaySessionResult {
                    output: ChatOutput {
                        content: Some("recovered".to_owned()),
                        tool_call: None,
                        artifacts: Vec::new(),
                        finish_reason: "stop".to_owned(),
                        usage: Usage {
                            prompt_tokens,
                            completion_tokens: 1,
                            total_tokens: prompt_tokens + 1,
                        },
                    },
                    backend: self.name().to_owned(),
                    direct_session: true,
                    provider_receipt: None,
                    token_ids: vec![2],
                    quality: Some(GatewaySessionQuality {
                        ttft_ms: 10,
                        tok_s: Some(40.0),
                    }),
                })
            })
        }
    }

    #[derive(Debug)]
    struct SuccessBackend {
        providers: Arc<Mutex<Vec<String>>>,
    }

    impl GatewaySessionBackend for SuccessBackend {
        fn name(&self) -> &str {
            "test-success"
        }

        fn run_chat<'a>(
            &'a self,
            _model: &'a GatewayModel,
            request: &'a ChatCompletionRequest,
            invocation: &'a GatewaySessionInvocation,
        ) -> GatewaySessionFuture<'a> {
            Box::pin(async move {
                self.providers
                    .lock()
                    .expect("providers lock")
                    .push(invocation.provider_pubkey.clone().unwrap_or_default());
                let prompt_tokens = rough_tokens(&chat_prompt_text(request));
                Ok(GatewaySessionResult {
                    output: ChatOutput {
                        content: Some("ok".to_owned()),
                        tool_call: None,
                        artifacts: Vec::new(),
                        finish_reason: "stop".to_owned(),
                        usage: Usage {
                            prompt_tokens,
                            completion_tokens: 1,
                            total_tokens: prompt_tokens + 1,
                        },
                    },
                    backend: self.name().to_owned(),
                    direct_session: true,
                    provider_receipt: None,
                    token_ids: vec![1],
                    quality: Some(GatewaySessionQuality {
                        ttft_ms: 10,
                        tok_s: Some(40.0),
                    }),
                })
            })
        }
    }

    #[test]
    fn route_selection_uses_weighted_p2c_not_array_order() {
        let model = test_routed_model(4);
        let state = GatewayState::from_models(vec![model.clone()]);
        let request = test_chat_request(&model.id);
        let mut counts = BTreeMap::new();

        for seed in 0..256 {
            let selected = ordered_route_candidates_for_request_with_seed(
                &state, &model, &request, None, seed,
            )
            .into_iter()
            .next()
            .expect("route selected")
            .provider
            .clone();
            *counts.entry(selected).or_insert(0_u32) += 1;
        }

        assert!(
            counts.len() > 1,
            "P2C should distribute first picks instead of always using catalog array order"
        );
        assert!(
            counts[&model.mayhem.route_candidates[0].provider] < 256,
            "catalog array first provider must not win every live selection"
        );
    }

    #[test]
    fn route_selection_uses_anchored_reputation_weight() {
        let neutral_model = test_routed_model(2);
        let neutral_state = GatewayState::from_models(vec![neutral_model.clone()]);
        let request = test_chat_request(&neutral_model.id);
        let penalized_provider = neutral_model.mayhem.route_candidates[0].provider.clone();
        let neutral_wins = (0..512)
            .filter(|seed| {
                ordered_route_candidates_for_request_with_seed(
                    &neutral_state,
                    &neutral_model,
                    &request,
                    None,
                    *seed,
                )
                .into_iter()
                .next()
                .map(|candidate| candidate.provider == penalized_provider)
                .unwrap_or(false)
            })
            .count();

        let mut anchored_model = neutral_model.clone();
        anchored_model.mayhem.route_candidates[0].reputation_bps = 3_100;
        let anchored_state = GatewayState::from_models(vec![anchored_model.clone()]);
        let anchored_wins = (0..512)
            .filter(|seed| {
                ordered_route_candidates_for_request_with_seed(
                    &anchored_state,
                    &anchored_model,
                    &request,
                    None,
                    *seed,
                )
                .into_iter()
                .next()
                .map(|candidate| candidate.provider == penalized_provider)
                .unwrap_or(false)
            })
            .count();

        assert!(
            neutral_wins > 150,
            "neutral reputation should select the provider at a visible baseline"
        );
        assert_eq!(
            anchored_wins, 0,
            "anchored low reputation should not outrank a materially healthier route"
        );

        let snapshot = contract_snapshot_for_route(
            &anchored_model,
            &anchored_model.mayhem.route_candidates[0],
            3,
        );
        assert!((snapshot.reputation - 0.31).abs() < f64::EPSILON);
    }

    #[test]
    fn route_selection_penalizes_observed_slow_error_provider() {
        let model = test_routed_model(4);
        let state = GatewayState::from_models(vec![model.clone()]);
        let request = test_chat_request(&model.id);
        let penalized_provider = model.mayhem.route_candidates[0].provider.clone();
        let before = (0..256)
            .filter(|seed| {
                ordered_route_candidates_for_request_with_seed(
                    &state, &model, &request, None, *seed,
                )
                .into_iter()
                .next()
                .map(|candidate| candidate.provider == penalized_provider)
                .unwrap_or(false)
            })
            .count();

        let penalized_key = route_key(&model.mayhem.route_candidates[0]);
        {
            let mut table = state
                .provider_table
                .lock()
                .expect("provider table poisoned");
            for _ in 0..5 {
                table.record_observation(
                    &penalized_key,
                    ProviderObservationSample {
                        ttft_ms: 5_000,
                        tok_s: None,
                        error: true,
                    },
                );
            }
        }

        let after = (0..256)
            .filter(|seed| {
                ordered_route_candidates_for_request_with_seed(
                    &state, &model, &request, None, *seed,
                )
                .into_iter()
                .next()
                .map(|candidate| candidate.provider == penalized_provider)
                .unwrap_or(false)
            })
            .count();

        assert!(before > 0, "baseline should pick the provider sometimes");
        assert!(
            after < before,
            "slow/error observations should lower future P2C selection weight"
        );
        assert_eq!(after, 0);
    }

    #[test]
    fn route_selection_prefers_previous_provider_for_same_conversation_model() {
        let model = test_routed_model(4);
        let state = GatewayState::from_models(vec![model.clone()]);
        let mut request = test_chat_request(&model.id);
        request
            .metadata
            .insert("conversation_id".to_owned(), json!("agent-loop-1"));
        let seed = 0x5eed;
        let baseline =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, seed);
        let baseline_first = baseline
            .first()
            .expect("baseline route selected")
            .provider
            .clone();
        let sticky_route = model
            .mayhem
            .route_candidates
            .iter()
            .find(|candidate| candidate.provider != baseline_first)
            .expect("alternate route");

        state.record_chat_affinity(&model, &request, sticky_route);
        let selected =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, seed);

        assert_eq!(selected[0].provider, sticky_route.provider);
        assert_eq!(selected.len(), model.mayhem.route_candidates.len());
    }

    #[test]
    fn route_selection_keeps_conversation_affinity_scoped_to_model() {
        let model_a = test_routed_model_with_id("mayhem/model-a", 0, 4);
        let mut model_b = model_a.clone();
        model_b.id = "mayhem/model-b".to_owned();
        let state = GatewayState::from_models(vec![model_a.clone(), model_b.clone()]);
        let mut request_a = test_chat_request(&model_a.id);
        request_a
            .metadata
            .insert("conversation_id".to_owned(), json!("agent-loop-1"));
        let mut request_b = test_chat_request(&model_b.id);
        request_b
            .metadata
            .insert("conversation_id".to_owned(), json!("agent-loop-1"));
        let seed = 0x5eed;
        let baseline_b = ordered_route_candidates_for_request_with_seed(
            &state, &model_b, &request_b, None, seed,
        )
        .into_iter()
        .map(|candidate| candidate.provider.clone())
        .collect::<Vec<_>>();
        let baseline_b_first = baseline_b.first().expect("baseline route selected").clone();
        let sticky_route = model_a
            .mayhem
            .route_candidates
            .iter()
            .find(|candidate| candidate.provider != baseline_b_first)
            .expect("alternate route");

        state.record_chat_affinity(&model_a, &request_a, sticky_route);
        let selected_b = ordered_route_candidates_for_request_with_seed(
            &state, &model_b, &request_b, None, seed,
        )
        .into_iter()
        .map(|candidate| candidate.provider.clone())
        .collect::<Vec<_>>();

        assert_eq!(selected_b, baseline_b);
    }

    #[test]
    fn route_selection_ignores_conversation_affinity_for_cooled_provider() {
        let model = test_routed_model(3);
        let state = GatewayState::from_models(vec![model.clone()]);
        let mut request = test_chat_request(&model.id);
        request
            .metadata
            .insert("conversation_id".to_owned(), json!("agent-loop-1"));
        let cooled_route = &model.mayhem.route_candidates[0];
        let cooled_provider = cooled_route.provider.clone();

        state.record_chat_affinity(&model, &request, cooled_route);
        state.cool_route_provider(cooled_route, now_millis_u64());
        let selected =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);

        assert!(!selected
            .iter()
            .any(|candidate| candidate.provider == cooled_provider));
    }

    #[test]
    fn route_selection_excludes_provider_during_cooloff_and_readmits_after_expiry() {
        let model = test_routed_model(3);
        let state = GatewayState::from_models(vec![model.clone()]);
        let request = test_chat_request(&model.id);
        let cooled_route = &model.mayhem.route_candidates[0];
        let cooled_provider = cooled_route.provider.clone();

        state.cool_route_provider(cooled_route, now_millis_u64());
        let cooled_order =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert!(!cooled_order
            .iter()
            .any(|candidate| candidate.provider == cooled_provider));

        state
            .provider_cooloffs
            .lock()
            .expect("provider cooloff map poisoned")
            .insert(route_key(cooled_route), 0);
        let readmitted_order =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert!(readmitted_order
            .iter()
            .any(|candidate| candidate.provider == cooled_provider));
    }

    #[test]
    fn route_selection_honors_route_tool_caps_even_for_single_provider() {
        let mut model = test_routed_model(1);
        model.mayhem.route_candidates[0].caps = json!({
            "tools": false,
            "json": true,
            "ctx": 8192,
            "vision": false,
        });
        let state = GatewayState::from_models(vec![model.clone()]);
        let mut request = test_chat_request(&model.id);
        request.tools = Some(vec![json!({
            "type": "function",
            "function": {
                "name": "lookup",
                "parameters": { "type": "object" },
            },
        })]);

        let selected =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert!(selected.is_empty());

        let mut model = test_routed_model(2);
        let dropped_provider = model.mayhem.route_candidates[0].provider.clone();
        model.mayhem.route_candidates[0].caps = json!({
            "tools": false,
            "json": true,
            "ctx": 8192,
            "vision": false,
        });
        model.mayhem.route_candidates[1].caps = json!({
            "tools": true,
            "json": true,
            "ctx": 8192,
            "vision": false,
        });
        let state = GatewayState::from_models(vec![model.clone()]);
        let selected =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert_eq!(selected.len(), 1);
        assert_ne!(selected[0].provider, dropped_provider);
    }

    #[test]
    fn route_selection_honors_user_max_bid_and_provider_min_ask() {
        let mut model = test_routed_model(2);
        let request = test_chat_request(&model.id);
        let high_ask_provider = model.mayhem.route_candidates[0].provider.clone();
        model.mayhem.route_candidates[0].min_ask_mu = u64::MAX;
        let state = GatewayState::from_models(vec![model.clone()]);

        let selected =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert_eq!(selected.len(), 1);
        assert_ne!(selected[0].provider, high_ask_provider);

        let priced_out = ordered_route_candidates_for_request_with_max_price_seed(
            &state,
            &model,
            &request,
            None,
            Some(1),
            None,
            None,
            None,
            0xfeed,
        );
        assert!(priced_out.is_empty());

        let clearing = ordered_route_candidates_for_request_with_max_price_seed(
            &state,
            &model,
            &request,
            None,
            Some(u64::MAX),
            None,
            None,
            None,
            0xfeed,
        );
        assert_eq!(clearing.len(), 1);
    }

    #[test]
    fn route_selection_honors_user_generation_throughput_floor() {
        let model = test_routed_model(2);
        let request = test_chat_request(&model.id);
        let state = GatewayState::from_models(vec![model.clone()]);

        let default_routes = ordered_route_candidates_for_request_with_options(
            &state,
            &model,
            &request,
            &GatewayRequestOptions::default(),
        );
        assert_eq!(default_routes.len(), 2);

        let strict_floor = GatewayRequestOptions {
            failover_overrides: GatewayFailoverPolicyConfig {
                min_tok_s: Some(60.0),
                ..GatewayFailoverPolicyConfig::default()
            },
            ..GatewayRequestOptions::default()
        };
        assert!(ordered_route_candidates_for_request_with_options(
            &state,
            &model,
            &request,
            &strict_floor,
        )
        .is_empty());

        let disabled_floor = GatewayRequestOptions {
            failover_overrides: GatewayFailoverPolicyConfig {
                min_tok_s: Some(0.0),
                ..GatewayFailoverPolicyConfig::default()
            },
            ..GatewayRequestOptions::default()
        };
        assert_eq!(
            ordered_route_candidates_for_request_with_options(
                &state,
                &model,
                &request,
                &disabled_floor,
            )
            .len(),
            2
        );
    }

    #[test]
    fn route_selection_applies_modality_throughput_floors() {
        let model = test_routed_model(2);
        let state = GatewayState::from_models(vec![model.clone()]);
        let inputs = vec!["alpha beta".to_owned(), "gamma".to_owned()];

        assert_eq!(
            ordered_route_candidates_for_embedding_with_options(
                &state,
                &model,
                &inputs,
                &GatewayRequestOptions::default(),
            )
            .len(),
            2
        );

        let strict = GatewayRequestOptions {
            failover_overrides: GatewayFailoverPolicyConfig {
                min_tok_s: Some(60.0),
                ..GatewayFailoverPolicyConfig::default()
            },
            ..GatewayRequestOptions::default()
        };
        assert!(ordered_route_candidates_for_embedding_with_options(
            &state, &model, &inputs, &strict,
        )
        .is_empty());

        let image_request = ImageGenerationRequest {
            model: model.id.clone(),
            prompt: "quiet launch panel".to_owned(),
            n: Some(1),
            size: Some("512x512".to_owned()),
            response_format: None,
            steps: Some(4),
            cfg_scale: None,
            seed: None,
            user: None,
        };
        assert_eq!(
            ordered_route_candidates_for_image_generation_with_options(
                &state,
                &model,
                &image_request,
                &GatewayRequestOptions::default(),
            )
            .len(),
            2
        );
        assert!(ordered_route_candidates_for_image_generation_with_options(
            &state,
            &model,
            &image_request,
            &strict,
        )
        .is_empty());

        let audio_request = AudioSpeechRequest {
            model: model.id.clone(),
            input: "hello from mayhem".to_owned(),
            voice: None,
            response_format: Some("wav".to_owned()),
            speed: None,
        };
        assert_eq!(
            ordered_route_candidates_for_audio_speech_with_options(
                &state,
                &model,
                &audio_request,
                &GatewayRequestOptions::default(),
            )
            .len(),
            2
        );
        assert!(ordered_route_candidates_for_audio_speech_with_options(
            &state,
            &model,
            &audio_request,
            &strict,
        )
        .is_empty());
    }

    #[test]
    fn modality_observations_use_natural_throughput_units() {
        let embedding = GatewayEmbeddingResult {
            output: EmbeddingOutput {
                embeddings: vec![vec![0.1, 0.2]],
                usage: Usage {
                    prompt_tokens: 40,
                    completion_tokens: 0,
                    total_tokens: 40,
                },
            },
            backend: "direct".to_owned(),
            direct_session: true,
            provider_receipt: None,
            quality: None,
        };
        let sample = observation_sample_from_embedding_success(&embedding, Duration::from_secs(2));
        assert_eq!(sample.tok_s, Some(20.0));

        let image = GatewayImageGenerationResult {
            output: ImageGenerationOutput {
                artifacts: vec![GatewayArtifactOutput {
                    id: "img".to_owned(),
                    content_type: "image/png".to_owned(),
                    bytes: vec![1, 2, 3],
                    blake3: "00".repeat(32),
                }],
                usage: ReceiptUsage::from_units([(USAGE_IMAGE, 2), (USAGE_STEP, 8)]),
            },
            backend: "direct".to_owned(),
            direct_session: true,
            provider_receipt: None,
            quality: None,
        };
        let sample =
            observation_sample_from_image_generation_success(&image, Duration::from_secs(4));
        assert_eq!(sample.tok_s, Some(0.5));

        let audio = GatewayAudioTranscriptionResult {
            output: AudioTranscriptionOutput {
                text: "hello".to_owned(),
                usage: ReceiptUsage::from_units([(USAGE_AUDIO_SECOND, 6)]),
            },
            backend: "direct".to_owned(),
            direct_session: true,
            provider_receipt: None,
            quality: Some(GatewaySessionQuality {
                ttft_ms: 250,
                tok_s: Some(3.0),
            }),
        };
        let sample =
            observation_sample_from_audio_transcription_success(&audio, Duration::from_secs(99));
        assert_eq!(sample.ttft_ms, 250);
        assert_eq!(sample.tok_s, Some(3.0));
    }

    #[test]
    fn underdelivery_streak_emits_contract_ready_reputation_event() {
        let model = test_routed_model(1);
        let state = GatewayState::from_models(vec![model.clone()]).with_epoch_seconds(7_200);
        let route = &model.mayhem.route_candidates[0];

        for _ in 0..3 {
            record_route_observation(
                &state,
                Some(route),
                ProviderObservationSample {
                    ttft_ms: 100,
                    tok_s: Some(10.0),
                    error: false,
                },
            );
        }

        let events = state.reputation_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.kind, "underdelivery");
        assert_eq!(event.command["op"], json!("record_rep_event"));
        assert_eq!(event.command["provider"], json!(route.provider));
        assert_eq!(event.command["kind"], json!("underdelivery"));
        assert_eq!(event.command["evidence_hash"], json!(event.evidence_hash));
        assert_eq!(event.command["epoch"], json!(event.at / 7_200 + 1));
        assert_eq!(event.evidence["epoch_seconds"], json!(7_200));
        assert_eq!(event.evidence["streak"], json!(3));
        assert_eq!(event.evidence["ratio"], json!(0.2));
    }

    #[test]
    fn route_selection_locks_matching_tier_market_price() {
        let mut model = test_routed_model(2);
        let request = test_chat_request(&model.id);
        model.mayhem.price_ref_mu = PriceRefMu {
            denom: "mu_usd".to_owned(),
            ver: 1,
            rate_map: text_generation_rate_map(10, 20),
            per_req_mu: 0,
            min_session_mu: 0,
            derivation: None,
        };
        model.mayhem.route_candidates[0].att_tier = 1;
        model.mayhem.route_candidates[0].quant = "int4".to_owned();
        model.mayhem.route_candidates[0].price_ref_mu = Some(PriceRefMu {
            denom: "mu_usd".to_owned(),
            ver: 1,
            rate_map: text_generation_rate_map(10, 20),
            per_req_mu: 0,
            min_session_mu: 0,
            derivation: None,
        });
        model.mayhem.route_candidates[1].att_tier = 3;
        model.mayhem.route_candidates[1].quant = "fp16".to_owned();
        model.mayhem.route_candidates[1].price_ref_mu = Some(PriceRefMu {
            denom: "mu_usd".to_owned(),
            ver: 9,
            rate_map: text_generation_rate_map(90, 180),
            per_req_mu: 123,
            min_session_mu: 456,
            derivation: None,
        });
        let state = GatewayState::from_models(vec![model.clone()]).with_receipt_balance_mu(10_000);

        let selected = ordered_route_candidates_for_request_with_max_price_seed(
            &state,
            &model,
            &request,
            Some(3),
            Some(u64::MAX),
            None,
            None,
            None,
            0xfeed,
        );
        assert_eq!(selected.len(), 1);
        let route = selected[0];
        assert_eq!(route.att_tier, 3);
        assert_eq!(route.quant, "fp16");

        let snapshot = contract_snapshot_for_route(&model, route, state.receipt_config.rules_ver);
        assert_eq!(snapshot.price_ver, 9);
        let expected_rate_map = normalize_rate_map(text_generation_rate_map(90, 180));
        assert_eq!(
            normalize_rate_map(snapshot.rate_map.clone()),
            expected_rate_map
        );
        assert_eq!(snapshot.per_req_mu, 123);
        assert_eq!(snapshot.min_session_mu, 456);

        let heartbeat = heartbeat_for_route(&model, route, now_millis_u64());
        assert_eq!(heartbeat.price_ver, 9);

        let invocation = state
            .prepare_chat_invocation_for_route(
                &model,
                &request,
                Some(route),
                &GatewayRequestOptions {
                    min_att_tier: Some(3),
                    max_price_mu: Some(u64::MAX),
                    ..GatewayRequestOptions::default()
                },
            )
            .expect("tier-3 route price clears max bid");
        assert_eq!(invocation.enclave_id, route.enclave_id);
        assert_eq!(invocation.price_ver, 9);
        assert_eq!(
            invocation.spend_voucher.body.locked_rate_map,
            expected_rate_map
        );
        assert_eq!(invocation.spend_voucher.body.locked_per_req_mu, 123);
        assert_eq!(invocation.spend_voucher.body.locked_min_session_mu, 456);
    }

    #[test]
    fn route_selection_filters_by_enclave_quant_bucket() {
        let mut model = test_routed_model(2);
        let request = test_chat_request(&model.id);
        model.mayhem.route_candidates[0].quant = "int4".to_owned();
        model.mayhem.route_candidates[1].quant = "fp16".to_owned();
        let state = GatewayState::from_models(vec![model.clone()]);

        let selected = ordered_route_candidates_for_request_with_max_price_seed(
            &state,
            &model,
            &request,
            None,
            Some(u64::MAX),
            None,
            Some("fp16"),
            None,
            0xfeed,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].quant, "fp16");

        let missing = ordered_route_candidates_for_request_with_max_price_seed(
            &state,
            &model,
            &request,
            None,
            Some(u64::MAX),
            None,
            Some("fp8"),
            None,
            0xfeed,
        );
        assert!(missing.is_empty());
    }

    #[test]
    fn route_selection_excludes_circuit_open_provider_and_readmits_after_expiry() {
        let model = test_routed_model(3);
        let state = GatewayState::from_models(vec![model.clone()]);
        let request = test_chat_request(&model.id);
        let circuit_route = &model.mayhem.route_candidates[0];
        let circuit_provider = circuit_route.provider.clone();
        let key = route_key(circuit_route);
        let now = now_millis_u64();
        {
            let mut table = state
                .provider_table
                .lock()
                .expect("provider table poisoned");
            for offset in
                0..crate::provider_table::DEFAULT_ERROR_CIRCUIT_BREAKER_CONSECUTIVE_FAILURES
            {
                table.record_observation_at(
                    &key,
                    ProviderObservationSample {
                        ttft_ms: 5_000,
                        tok_s: None,
                        error: true,
                    },
                    now + u64::from(offset),
                );
            }
        }

        let circuit_order =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert!(!circuit_order
            .iter()
            .any(|candidate| candidate.provider == circuit_provider));

        state
            .provider_table
            .lock()
            .expect("provider table poisoned")
            .record_observation_at(
                &key,
                ProviderObservationSample {
                    ttft_ms: 100,
                    tok_s: Some(50.0),
                    error: false,
                },
                now + crate::provider_table::DEFAULT_ERROR_CIRCUIT_BREAKER_COOLOFF_MILLIS + 10,
            );
        let readmitted_order =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert!(readmitted_order
            .iter()
            .any(|candidate| candidate.provider == circuit_provider));
    }

    #[tokio::test]
    async fn route_retry_records_partial_receipt_and_redispatches_remaining_history() {
        let model = test_routed_model(2);
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let providers = Arc::new(Mutex::new(Vec::new()));
        let state = GatewayState::from_models(vec![model.clone()]).with_session_backend(Arc::new(
            PartialThenSuccessBackend {
                attempts: attempts.clone(),
                providers: providers.clone(),
            },
        ));
        let mut request = test_chat_request(&model.id);
        request.max_tokens = Some(8);

        let run =
            run_chat_with_route_retry(&state, &model, &request, GatewayRequestOptions::default())
                .await
                .expect("redispatch should recover");

        assert_eq!(run.result.output.content.as_deref(), Some("hello world"));
        assert_eq!(run.result.output.usage.completion_tokens, 2);
        assert_eq!(run.result.token_ids, vec![11, 22]);
        assert_eq!(run.metering_output.content.as_deref(), Some("world"));
        assert!(run
            .metering_request
            .messages
            .iter()
            .any(|message| message.role == "assistant" && message.content == json!("hello ")));
        assert_eq!(attempts.lock().expect("attempts lock").len(), 2);
        let providers = providers.lock().expect("providers lock").clone();
        assert_eq!(providers.len(), 2);
        let failed_provider = &providers[0];
        let post_failure_order =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert!(!post_failure_order
            .iter()
            .any(|candidate| &candidate.provider == failed_provider));

        let partial_receipts = state.receipts();
        assert_eq!(partial_receipts.len(), 1);
        assert!(!partial_receipts[0].receipt.body.final_receipt);
        assert_eq!(partial_receipts[0].receipt.body.usage.output_tokens(), 1);

        state
            .meter_chat_session(
                &model,
                &run.metering_request,
                &run.metering_output,
                &run.invocation,
                run.result.provider_receipt.as_ref(),
            )
            .expect("final receipt should validate against redispatch attempt only");
        let receipts = state.receipts();
        assert_eq!(receipts.len(), 2);
        assert!(receipts[1].receipt.body.final_receipt);
        assert_eq!(receipts[1].receipt.body.usage.output_tokens(), 1);
    }

    #[tokio::test]
    async fn route_retry_abandons_below_floor_provider_and_reroutes() {
        let model = test_routed_model(2);
        let providers = Arc::new(Mutex::new(Vec::new()));
        let state = GatewayState::from_models(vec![model.clone()]).with_session_backend(Arc::new(
            SlowQualityThenSuccessBackend {
                providers: providers.clone(),
            },
        ));
        let request = test_chat_request(&model.id);
        let options = GatewayRequestOptions {
            failover_overrides: GatewayFailoverPolicyConfig {
                min_tok_s: Some(10.0),
                ..GatewayFailoverPolicyConfig::default()
            },
            ..GatewayRequestOptions::default()
        };

        let run = run_chat_with_route_retry(&state, &model, &request, options)
            .await
            .expect("slow provider should be rerouted");

        assert_eq!(run.result.output.content.as_deref(), Some("fast"));
        let providers = providers.lock().expect("providers lock").clone();
        assert_eq!(providers.len(), 2);
        assert_ne!(providers[0], providers[1]);
        let post_failure_order =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, 0xfeed);
        assert!(!post_failure_order
            .iter()
            .any(|candidate| candidate.provider == providers[0]));
    }

    #[test]
    fn provider_reject_session_error_marks_self_protection_codes_clean() {
        for code in [
            "CAPACITY",
            "BUSY",
            "RATE",
            "QUOTA",
            "PRICE_FLOOR",
            "DRAINING",
        ] {
            let err = provider_reject_session_error(
                &json!({
                    "t": "s.reject",
                    "code": code,
                    "reason": "self protection",
                }),
                "session-a",
            );
            assert!(err.retryable);
            assert!(err.clean_refusal, "{code} should be a clean refusal");
            assert_eq!(err.clean_refusal_code.as_deref(), Some(code));
        }

        let err = provider_reject_session_error(
            &json!({
                "t": "s.reject",
                "code": "SIGNATURE",
                "reason": "bad open frame",
            }),
            "session-a",
        );
        assert!(err.retryable);
        assert!(!err.clean_refusal);
        assert_eq!(err.clean_refusal_code, None);
    }

    #[test]
    fn capacity_refusal_only_penalizes_sustained_false_free_capacity() {
        let model = test_routed_model(1);
        let state = GatewayState::from_models(vec![model.clone()]);
        let route = model.mayhem.route_candidates.first().expect("route");
        let capacity = GatewaySessionError::clean_refusal_with_code(
            "provider rejected session session-a with CAPACITY: full",
            Some("CAPACITY"),
        );

        record_retryable_route_attempt(&state, Some(route), Duration::from_millis(5), &capacity);
        record_retryable_route_attempt(&state, Some(route), Duration::from_millis(5), &capacity);
        assert!(state.reputation_events().is_empty());
        record_retryable_route_attempt(&state, Some(route), Duration::from_millis(5), &capacity);

        let events = state.reputation_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].event_id.starts_with("capacity-mismatch-"));
        assert_eq!(events[0].kind, "underdelivery");
        assert_eq!(
            events[0].evidence["source"],
            json!("mayhem-gateway-capacity-mismatch-v1")
        );

        let honest_state = GatewayState::from_models(vec![model.clone()]);
        let mut saturated = heartbeat_for_route(&model, route, now_millis_u64());
        saturated.sat = 1.0;
        saturated.slots.active = 1;
        saturated.slots.active_requests = 1;
        saturated.slots.max = 1;
        saturated.q.free_slots = 0;
        saturated.q.engine_backlog = 0;
        honest_state
            .provider_table
            .lock()
            .expect("provider table poisoned")
            .upsert_heartbeat(saturated, now_millis_u64());
        for _ in 0..3 {
            record_retryable_route_attempt(
                &honest_state,
                Some(route),
                Duration::from_millis(5),
                &capacity,
            );
        }
        assert!(honest_state.reputation_events().is_empty());
        let entry = honest_state
            .provider_table
            .lock()
            .expect("provider table poisoned")
            .entries(now_millis_u64())
            .into_iter()
            .next()
            .expect("provider entry");
        assert_eq!(entry.observed.capacity_mismatch_streak, 0);
        assert_eq!(entry.observed.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn route_retry_clean_refusal_reroutes_without_failure_penalty() {
        let model = test_routed_model(2);
        let providers = Arc::new(Mutex::new(Vec::new()));
        let state = GatewayState::from_models(vec![model.clone()]).with_session_backend(Arc::new(
            CleanRefusalThenSuccessBackend {
                providers: providers.clone(),
            },
        ));
        let request = test_chat_request(&model.id);

        let run =
            run_chat_with_route_retry(&state, &model, &request, GatewayRequestOptions::default())
                .await
                .expect("clean refusal should reroute");

        assert_eq!(run.result.output.content.as_deref(), Some("recovered"));
        let providers = providers.lock().expect("providers lock").clone();
        assert_eq!(providers.len(), 2);
        assert_ne!(providers[0], providers[1]);
        let refused_route = model
            .mayhem
            .route_candidates
            .iter()
            .find(|candidate| candidate.provider == providers[0])
            .expect("refused provider route");
        assert!(!state.route_provider_in_cooloff(refused_route, now_millis_u64()));
        let refused_entry = state
            .provider_table
            .lock()
            .expect("provider table lock")
            .entries(now_millis_u64())
            .into_iter()
            .find(|entry| entry.key.provider == providers[0])
            .expect("refused provider table entry");
        assert_eq!(refused_entry.observed.samples, 0);
        assert_eq!(refused_entry.observed.consecutive_failures, 0);
        assert_eq!(refused_entry.observed.ewma_error_rate, 0.0);
        assert_eq!(refused_entry.observed.circuit_open_until_millis, None);
    }

    #[tokio::test]
    async fn route_retry_records_conversation_affinity_after_success() {
        let model = test_routed_model(4);
        let providers = Arc::new(Mutex::new(Vec::new()));
        let state = GatewayState::from_models(vec![model.clone()]).with_session_backend(Arc::new(
            SuccessBackend {
                providers: providers.clone(),
            },
        ));
        let mut request = test_chat_request(&model.id);
        request
            .metadata
            .insert("conversation_id".to_owned(), json!("agent-loop-1"));

        run_chat_with_route_retry(&state, &model, &request, GatewayRequestOptions::default())
            .await
            .expect("request should succeed");

        let served_provider = providers
            .lock()
            .expect("providers lock")
            .first()
            .cloned()
            .expect("provider recorded");
        let affinity_key = chat_affinity_key(&model, &request).expect("affinity key");
        let sticky_key = state
            .chat_affinity
            .lock()
            .expect("chat affinity map poisoned")
            .get(&affinity_key)
            .cloned()
            .expect("affinity recorded");
        assert_eq!(sticky_key.provider, served_provider);

        let seed = (0..256_u64)
            .find(|seed| {
                ordered_route_candidates_for_request_with_seed(
                    &state, &model, &request, None, *seed,
                )
                .into_iter()
                .next()
                .map(|candidate| candidate.provider == served_provider)
                .unwrap_or(false)
            })
            .expect("sticky provider should remain eligible");
        let selected =
            ordered_route_candidates_for_request_with_seed(&state, &model, &request, None, seed);
        assert_eq!(selected[0].provider, served_provider);
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

    #[test]
    fn direct_session_request_body_preserves_conversation_hints() {
        let mut request = test_chat_request("mayhem/chat");
        request.user = Some("terminal-user-1".to_owned());
        request
            .metadata
            .insert("conversation_id".to_owned(), json!("agent-loop-1"));

        let body = direct_session_request_body(&request);

        assert_eq!(body["user"], json!("terminal-user-1"));
        assert_eq!(body["metadata"]["conversation_id"], json!("agent-loop-1"));
    }

    #[test]
    fn direct_session_request_frames_chunk_large_body_under_transport_limit() {
        let mut request = test_chat_request("mayhem/chat");
        request.messages[0].content = json!("hello ".repeat(20_000));
        let body = direct_session_request_body(&request);
        let max_frame_bytes = 12 * 1024;

        let frames = direct_session_request_frames("rid-large", &body, max_frame_bytes).unwrap();

        assert!(frames.len() > 2);
        assert!(frames.iter().all(|frame| {
            session_frame_json_len(frame).expect("frame serializes") <= max_frame_bytes
        }));
        assert!(frames[..frames.len() - 1]
            .iter()
            .all(|frame| frame.get("t").and_then(Value::as_str) == Some("s.req_chunk")));
        let final_frame = frames.last().expect("final frame");
        assert_eq!(final_frame.get("t").and_then(Value::as_str), Some("s.req"));
        let manifest: PayloadChunkManifest =
            serde_json::from_value(final_frame["body_ref"].clone()).unwrap();
        let chunks = frames[..frames.len() - 1]
            .iter()
            .map(|frame| serde_json::from_value::<PayloadChunk>(frame["chunk"].clone()).unwrap())
            .collect::<Vec<_>>();
        let restored = reassemble_json_payload(&manifest, &chunks).unwrap();
        assert_eq!(restored, body);
    }

    #[test]
    fn session_delta_refs_reassemble_large_tool_embeddings_and_token_ids() {
        let mut pending = SessionDeltaPayloadChunks::default();
        let tool = json!({
            "id": "call-large",
            "name": "write_file",
            "arguments": "x".repeat(10_000),
        });
        let embeddings = json!([vec![0.25_f32; 2048], vec![0.5_f32; 2048]]);
        let token_ids = json!((0..4096).collect::<Vec<i32>>());
        let fields = [
            ("tool", tool.clone()),
            ("embeddings", embeddings.clone()),
            ("token_ids", token_ids.clone()),
        ];
        let mut manifests = BTreeMap::new();
        for (field, value) in fields {
            let (manifest, chunks) = chunk_json_payload(&value, 512).unwrap();
            for chunk in chunks {
                collect_session_delta_chunk(
                    &json!({
                        "t": "s.delta_chunk",
                        "rid": "rid-large",
                        "field": field,
                        "payload_id": manifest.blake3.clone(),
                        "chunk": chunk,
                    }),
                    &mut pending,
                )
                .unwrap();
            }
            manifests.insert(field, manifest);
        }
        let frame = json!({
            "t": "s.delta",
            "rid": "rid-large",
            "tool": null,
            "tool_ref": manifests["tool"],
            "embeddings": null,
            "embeddings_ref": manifests["embeddings"],
            "token_ids": null,
            "token_ids_ref": manifests["token_ids"],
        });

        let restored_tool = tool_call_from_session_delta_resolving(&frame, &mut pending)
            .unwrap()
            .unwrap();
        let restored_embeddings = embeddings_from_session_delta(&frame, &mut pending)
            .unwrap()
            .unwrap();
        let restored_token_ids = token_ids_ref_from_session_delta(&frame, &mut pending)
            .unwrap()
            .unwrap();

        assert_eq!(restored_tool.id, "call-large");
        assert_eq!(restored_tool.name, "write_file");
        assert_eq!(restored_tool.arguments, "x".repeat(10_000));
        assert_eq!(restored_embeddings.len(), 2);
        assert_eq!(restored_embeddings[0].len(), 2048);
        assert_eq!(restored_token_ids.len(), 4096);
        assert!(pending.chunks.is_empty());
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
        test_provider_receipt_with_finality(model, request, output, invocation, 1, true)
    }

    fn test_embedding_provider_receipt(
        model: &GatewayModel,
        inputs: &[String],
        output: &EmbeddingOutput,
        invocation: &GatewaySessionInvocation,
    ) -> ProviderSignedReceipt {
        let usage = ReceiptUsage::text(output.usage.prompt_tokens, 0);
        let body = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: invocation.session_id.clone(),
            seq: 1,
            final_receipt: true,
            rail: invocation.rail.clone(),
            user: invocation.user_pubkey.clone(),
            provider: invocation.provider_pubkey.clone().unwrap(),
            enclave_id: invocation.enclave_id.clone(),
            model_id: model.id.clone(),
            price_ver: invocation.price_ver,
            locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
            locked_per_req_mu: invocation.spend_voucher.body.locked_per_req_mu,
            locked_min_session_mu: invocation.spend_voucher.body.locked_min_session_mu,
            served_ctx: invocation.served_ctx,
            ctx_bracket: invocation.ctx_bracket.clone(),
            ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
            rules_ver: invocation.rules_ver,
            usage: usage.clone(),
            mu_owed_cum: calculate_locked_mu_owed(invocation, &usage),
            prompt_hash: blake3_hex(embedding_prompt_text(inputs).as_bytes()),
            ts: 123,
        };
        ProviderSignedReceipt {
            enclave_sig: sign_hex(&test_enclave_seed(), &receipt_signing_bytes(&body).unwrap()),
            body,
            enclave_pubkey: verifying_key_hex(&test_enclave_seed()),
        }
    }

    fn test_provider_receipt_with_finality(
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        output: &ChatOutput,
        invocation: &GatewaySessionInvocation,
        seq: u64,
        final_receipt: bool,
    ) -> ProviderSignedReceipt {
        let usage = ReceiptUsage::text(output.usage.prompt_tokens, output.usage.completion_tokens);
        let body = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: invocation.session_id.clone(),
            seq,
            final_receipt,
            rail: invocation.rail.clone(),
            user: invocation.user_pubkey.clone(),
            provider: invocation.provider_pubkey.clone().unwrap(),
            enclave_id: invocation.enclave_id.clone(),
            model_id: model.id.clone(),
            price_ver: invocation.price_ver,
            locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
            locked_per_req_mu: invocation.spend_voucher.body.locked_per_req_mu,
            locked_min_session_mu: invocation.spend_voucher.body.locked_min_session_mu,
            served_ctx: invocation.served_ctx,
            ctx_bracket: invocation.ctx_bracket.clone(),
            ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
            rules_ver: invocation.rules_ver,
            usage: usage.clone(),
            mu_owed_cum: calculate_locked_mu_owed(invocation, &usage),
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
            artifact_sidecar_roots: std::collections::BTreeMap::new(),
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
