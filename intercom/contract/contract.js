import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { Contract } from 'trac-peer';

export const CONTRACT_VERSION = 7;
const SIGNING_MESSAGE_VERSION = 2;
const CURRENT_RULES_KEY = 'rules/current';
const PROVIDER_ACCEPTED_RAILS = new Set(['fiat', 'tap', 'tnk']);
const PROVIDER_ACCEPTED_RAIL_ORDER = Object.freeze(['fiat', 'tap', 'tnk']);
const PROVIDER_RAIL_SCHEMA_VERSION = 1;
const PAYOUT_TARGET_METHODS = new Set(['tnk', 'stripe', 'tap']);
const FIAT_DEPOSIT_RAILS = new Set(['stripe']);
const FIAT_CURRENCIES = new Set(['usd', 'eur']);
const PRICE_DENOMINATION = 'au_usd';
const RATE_SOURCES = new Set(['gate-spot', 'mexc-spot']);
const TAP_RATE_SOURCES = new Set(['uniswap-v2', 'config', 'stale']);
const CATALOG_SOURCE_KINDS = new Set(['https', 'huggingface']);
const PROVIDER_LIFECYCLE_OPS = new Set([
  'register_provider',
  'join_enclave',
  'leave_enclave',
  'join_room',
  'leave_room',
  'set_provider_rails',
]);
const DAY_SECONDS = 24 * 60 * 60;
const DEFAULT_PRICE_RATE_LIMIT_SECONDS = 6 * 60 * 60;
const DEFAULT_MARKET_PRICE_TARGET_UTILIZATION_BPS = 8_500;
const DEFAULT_MARKET_PRICE_EMA_ALPHA_BPS = 2_500;
const DEFAULT_MARKET_PRICE_GAIN_BPS = 5_000;
const DEFAULT_MARKET_PRICE_MAX_STEP_BPS = 1_000;
const DEFAULT_MARKET_PRICE_COLD_START_MIN_PROVIDERS = 2;
const ZERO_AU = '0';
const ONE_USD_AU = '1000000000000000000';
const FIVE_MILLI_USD_AU = '5000000000000000';
const DEFAULT_MARKET_PRICE_PROVIDER_EPOCH_TARGET_AU = ONE_USD_AU;
const DEFAULT_MARKET_PRICE_MAX_UTILIZATION_BPS = 50_000;
const DEFAULT_MARKET_PRICE_BELOW_TARGET_DISCOUNT_BPS = 2_500;
const DEFAULT_MARKET_PRICE_ABOVE_TARGET_SLOPE_BPS = 15_000;
const DEFAULT_PARAM_ACTIVATION_DELAY_SECONDS = DAY_SECONDS;
const PROBATION_SECONDS = 7 * DAY_SECONDS;
const DEFAULT_FRAUD_SLASH_BPS = 10_000;
const DEFAULT_DISPUTE_LOST_SLASH_BPS = 2_000;
const MAX_OPERATOR_FEE_BPS = 1_500;
const MAX_LAUNCH_ENCLAVE_ATTESTATION_TIER = 3;
const DEFAULT_DISPUTE_DEPOSIT_AU = ONE_USD_AU;
const DEFAULT_DISPUTE_TIMEOUT_EPOCHS = 168;
const DEFAULT_MAX_APPLY_BATCH = 2_000;
const DEFAULT_MAX_MARKET_USAGE_ENTRIES = 5_000;
const DEFAULT_MAX_TNK_SETTLEMENT_OUTPUTS = 5_000;
const DEFAULT_MAX_FIAT_SETTLEMENT_OUTPUTS = 5_000;
const MIN_TAP_CONFIRMATION_DEPTH = 12;
const DISPUTE_EVIDENCE_MAX_BYTES = 4_096;
const FRAUD_PROOF_MAX_BYTES = 4_096;
export const SESSION_RECEIPT_SCHEMA_VERSION = 8;
const CTX_BRACKET_TABLE_VERSION = 1;
const CTX_BRACKETS = Object.freeze([
  { id: 'le8k', max_ctx: 8_192 },
  { id: 'le32k', max_ctx: 32_768 },
  { id: 'le128k', max_ctx: 131_072 },
  { id: 'le256k', max_ctx: 262_144 },
  { id: 'gt256k', max_ctx: null },
]);
const TNK_E18 = 1_000_000_000_000_000_000n;
const TAP_WEI = 1_000_000_000_000_000_000n;
const USD_AU = 1_000_000_000_000_000_000n;
const FIAT_MINOR_AU = 10_000_000_000_000_000n;
const TAP_DEPOSIT_EVENT_SIGNATURE = '0xe1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c';
const TAP_DEPOSIT_WATCHER_ID = 'tap-deposit-watcher-v1';
const PARAM_DEFINITIONS = Object.freeze({
  probation_successful_sessions: { default: 50, min: 0, max: 1_000_000 },
  probation_seconds: { default: PROBATION_SECONDS, min: 0, max: 365 * 24 * 60 * 60 },
  probation_max_concurrent_sessions_per_user: { default: 2, min: 1, max: 1_000_000 },
  probation_price_max_bps: { default: 10_000, min: 0, max: 1_000_000 },
  probation_weight_bps: { default: 5_000, min: 0, max: 10_000 },
  auditor_min_reputation_bps: { default: 8_000, min: 0, max: 10_000 },
  auditor_min_age_seconds: { default: 30 * DAY_SECONDS, min: 0, max: 10 * 365 * DAY_SECONDS },
  canary_match_min_bps: { default: 9_000, min: 0, max: 10_000 },
  canary_probe_holdback_bps: { default: 0, min: 0, max: 10_000 },
  canary_probe_release_min_passes: { default: 1, min: 0, max: 1_000_000 },
  probe_reward_au: { default: FIVE_MILLI_USD_AU, min: ZERO_AU, money: true },
  uptime_tick_seconds: { default: 6 * 60 * 60, min: 60, max: 30 * DAY_SECONDS },
  fraud_slash_bps: { default: DEFAULT_FRAUD_SLASH_BPS, min: 0, max: 10_000 },
  dispute_lost_slash_bps: { default: DEFAULT_DISPUTE_LOST_SLASH_BPS, min: 0, max: 10_000 },
  new_provider_holdback_epochs: { default: 168, min: 0, max: 1_000_000 },
  holdback_epochs: { default: 24, min: 0, max: 1_000_000 },
  min_tier_notice_epochs: { default: 24, min: 1, max: 1_000_000 },
  fee_bps: { default: 1_500, min: 0, max: MAX_OPERATOR_FEE_BPS },
  dispute_deposit_au: { default: DEFAULT_DISPUTE_DEPOSIT_AU, min: '1', money: true },
  dispute_timeout_epochs: { default: DEFAULT_DISPUTE_TIMEOUT_EPOCHS, min: 1, max: 1_000_000 },
  payout_min_au: { default: ONE_USD_AU, min: ZERO_AU, money: true },
  price_min_bps: { default: 2_500, min: 1, max: 1_000_000 },
  price_max_bps: { default: 40_000, min: 1, max: 1_000_000 },
  price_rate_limit_seconds: { default: DEFAULT_PRICE_RATE_LIMIT_SECONDS, min: 0, max: 365 * DAY_SECONDS },
  market_target_utilization_bps: { default: DEFAULT_MARKET_PRICE_TARGET_UTILIZATION_BPS, min: 1, max: 9_999 },
  market_ema_alpha_bps: { default: DEFAULT_MARKET_PRICE_EMA_ALPHA_BPS, min: 1, max: 10_000 },
  market_gain_bps: { default: DEFAULT_MARKET_PRICE_GAIN_BPS, min: 1, max: 10_000 },
  market_max_step_bps: { default: DEFAULT_MARKET_PRICE_MAX_STEP_BPS, min: 1, max: 10_000 },
  market_cold_start_min_providers: { default: DEFAULT_MARKET_PRICE_COLD_START_MIN_PROVIDERS, min: 0, max: 1_000_000 },
  market_provider_epoch_target_au: { default: DEFAULT_MARKET_PRICE_PROVIDER_EPOCH_TARGET_AU, min: '1', money: true },
  market_max_utilization_bps: { default: DEFAULT_MARKET_PRICE_MAX_UTILIZATION_BPS, min: 1, max: 1_000_000 },
  market_below_target_discount_bps: { default: DEFAULT_MARKET_PRICE_BELOW_TARGET_DISCOUNT_BPS, min: 0, max: 10_000 },
  market_above_target_slope_bps: { default: DEFAULT_MARKET_PRICE_ABOVE_TARGET_SLOPE_BPS, min: 0, max: 1_000_000 },
  epoch_seconds: { default: 3_600, min: 60, max: 86_400 },
  rate_staleness_seconds: { default: 45 * 60, min: 60, max: 86_400 },
  rules_grace_seconds: { default: 14 * 24 * 60 * 60, min: 0, max: 365 * 24 * 60 * 60 },
  challenge_epochs: { default: 6, min: 0, max: 1_000_000 },
  max_apply_batch: { default: DEFAULT_MAX_APPLY_BATCH, min: 1, max: Number.MAX_SAFE_INTEGER },
  max_market_usage_entries: { default: DEFAULT_MAX_MARKET_USAGE_ENTRIES, min: 0, max: Number.MAX_SAFE_INTEGER },
  max_tnk_settlement_outputs: { default: DEFAULT_MAX_TNK_SETTLEMENT_OUTPUTS, min: 1, max: Number.MAX_SAFE_INTEGER },
  max_fiat_settlement_outputs: { default: DEFAULT_MAX_FIAT_SETTLEMENT_OUTPUTS, min: 1, max: Number.MAX_SAFE_INTEGER },
  param_activation_delay_seconds: { default: DEFAULT_PARAM_ACTIVATION_DELAY_SECONDS, min: 0, max: 30 * DAY_SECONDS },
});
const EPOCH_ADMIN_PARAM_KEYS = Object.freeze([
  'probation_successful_sessions',
  'probation_seconds',
  'probation_max_concurrent_sessions_per_user',
  'probation_price_max_bps',
  'probation_weight_bps',
  'auditor_min_reputation_bps',
  'auditor_min_age_seconds',
  'canary_match_min_bps',
  'canary_probe_holdback_bps',
  'canary_probe_release_min_passes',
  'probe_reward_au',
  'uptime_tick_seconds',
  'fraud_slash_bps',
  'dispute_lost_slash_bps',
  'new_provider_holdback_epochs',
  'holdback_epochs',
  'min_tier_notice_epochs',
  'fee_bps',
  'dispute_deposit_au',
  'dispute_timeout_epochs',
  'payout_min_au',
  'price_min_bps',
  'price_max_bps',
  'price_rate_limit_seconds',
  'market_target_utilization_bps',
  'market_ema_alpha_bps',
  'market_gain_bps',
  'market_max_step_bps',
  'market_cold_start_min_providers',
  'market_provider_epoch_target_au',
  'market_max_utilization_bps',
  'market_below_target_discount_bps',
  'market_above_target_slope_bps',
  'epoch_seconds',
  'rate_staleness_seconds',
  'rules_grace_seconds',
  'challenge_epochs',
  'max_apply_batch',
  'max_market_usage_entries',
  'max_tnk_settlement_outputs',
  'max_fiat_settlement_outputs',
  'param_activation_delay_seconds',
]);
const REPUTATION_EVENT_KINDS = new Set([
  'session_ok',
  'session_partial',
  'session_fail',
  'probe_ok',
  'probe_fail',
  'uptime_tick',
  'underdelivery',
  'dispute_lost',
  'provenance_violation',
]);
const PROBE_KINDS = new Set(['canary', 'uptime_tick']);
const PROBE_VERIFICATION_METHODS = new Set([
  'token_fingerprint',
  'context_needle',
  'seed_perceptual_hash',
  'attestation_of_compute',
]);
const BAN_TARGET_TYPES = new Set(['provider', 'device', 'fingerprint', 'committer']);
const FRAUD_PROOF_REASONS = new Set(['over_credit', 'price_derivation']);
const DISPUTE_OUTCOMES = new Set(['provider_fault', 'opener_fault', 'no_fault']);
const DISPUTE_DEPOSIT_ACTIONS = new Set(['refund', 'forfeit']);
const EPOCH_ROOT_KEYS = ['dep', 'use', 'earn', 'fee', 'price'];
const EPOCH_TOTAL_KEYS = [
  'dep_count',
  'dep_au',
  'use_count',
  'use_au',
  'provider_count',
  'earn_au',
  'fee_au',
  'fee_cum_au',
  'price_count',
];
const EPOCH_TOTAL_MONEY_KEYS = new Set(['dep_au', 'use_au', 'earn_au', 'fee_au', 'fee_cum_au']);
const ENCLAVE_UPDATE_FIELDS = [
  'model_class',
  'backend',
  'artifact_root',
  'artifact_root_kind',
  'artifact_source',
  'artifact_sidecars',
  'source_sha256',
  'manifest_hash',
  'att_tier',
  'quant',
  'binary_hash',
  'launch_measurements',
  'caps',
];
const TIER3_MEASUREMENT_MAX_NAMES = 32;
const TIER3_MEASUREMENT_MAX_VALUES = 128;
const ENCLAVE_BACKENDS = new Set([
  'llama.cpp',
  'mlx',
  'trt-llm',
  'vllm',
  'diffusers',
  'stable-diffusion.cpp',
  'comfyui',
  'whisper.cpp',
  'piper',
  'kokoro',
]);
const ENCLAVE_QUANT_BUCKETS = new Set([
  'unknown',
  'fp32',
  'fp16',
  'bf16',
  'fp8',
  'nvfp4',
  'int8',
  'int4',
]);
const DEFAULT_MODEL_CLASS = 'text-generation';
const MODEL_CLASSES = new Set([
  DEFAULT_MODEL_CLASS,
  'embedding',
  'image-generation',
  'video-generation',
  'tts',
  'stt',
  'audio-generation',
  'music-generation',
]);
const RATE_MAP_MAX_ENTRIES = 16;
const MODEL_CLASS_RATE_UNITS = Object.freeze({
  [DEFAULT_MODEL_CLASS]: new Set(['input_token', 'cached_input_token', 'output_token']),
  embedding: new Set(['input_token', 'embedding']),
  'image-generation': new Set(['image', 'step']),
  'video-generation': new Set(['video_second', 'frame']),
  tts: new Set(['input_character', 'audio_second']),
  stt: new Set(['audio_second']),
  'audio-generation': new Set(['input_character', 'audio_second']),
  'music-generation': new Set(['input_character', 'audio_second']),
});
const CAP_OUTPUT_MODALITIES = new Set(['text', 'embedding', 'image', 'video', 'audio']);
const MODEL_CLASS_OUTPUT_MODALITIES = Object.freeze({
  [DEFAULT_MODEL_CLASS]: new Set(['text']),
  embedding: new Set(['embedding']),
  'image-generation': new Set(['image']),
  'video-generation': new Set(['video']),
  tts: new Set(['audio']),
  stt: new Set(['text']),
  'audio-generation': new Set(['audio']),
  'music-generation': new Set(['audio']),
});
const ENCLAVE_ARTIFACT_ROOT_KIND = 'blake3_merkle_v1';
const ENCLAVE_CAP_BOOLEAN_FIELDS = [
  'chat',
  'tools',
  'json',
  'embeddings',
  'vision',
  'image',
  'video',
  'audio',
];
const ENCLAVE_CAP_INTEGER_FIELDS = [
  'ctx',
  'ctx_max',
  'tp_degree',
  'max_batch_size',
  'max_num_tokens',
  'vllm_gpu_memory_utilization_pct',
  'max_image_width',
  'max_image_height',
  'max_image_steps',
  'max_video_width',
  'max_video_height',
  'max_video_frames',
  'max_video_seconds',
  'max_audio_seconds',
  'sample_rate_hz',
];
const ENCLAVE_CAP_STRING_FIELDS = [
  'vllm_dtype',
];
const ENCLAVE_CAP_FIELDS = new Set([
  ...ENCLAVE_CAP_BOOLEAN_FIELDS,
  ...ENCLAVE_CAP_INTEGER_FIELDS,
  ...ENCLAVE_CAP_STRING_FIELDS,
  'output_modality',
  'output_modalities',
]);
const ROOM_POLICY_FIELDS = new Set([
  'region_hint',
  'canary_set',
  'min_reputation',
  'max_price_mult',
]);

export const signingMessageVersions = () => [SIGNING_MESSAGE_VERSION];
export const consentMessage = (ver, hash, signingVersion = SIGNING_MESSAGE_VERSION) => {
  if (signingVersion !== SIGNING_MESSAGE_VERSION) {
    throw new Error(`Unsupported signing message version: ${signingVersion}`);
  }
  return JSON.stringify({
    domain: 'mayhem-consent',
    signing_version: SIGNING_MESSAGE_VERSION,
    rules_ver: ver,
    rules_hash: hash,
  });
};
export const providerLifecycleIntentMessage = (intent, signingVersion = SIGNING_MESSAGE_VERSION) => {
  if (signingVersion !== SIGNING_MESSAGE_VERSION) {
    throw new Error(`Unsupported signing message version: ${signingVersion}`);
  }
  return JSON.stringify({
    domain: 'mayhem-provider-lifecycle',
    signing_version: SIGNING_MESSAGE_VERSION,
    intent: stableValue(intent),
  });
};
export const depositTnkIntentMessage = (intent) =>
  `mayhem-deposit-tnk-intent-v1${stableJson(intent)}`;
const canonicalSpendVoucherBody = (body) => {
  if (!body || typeof body !== 'object' || Array.isArray(body)) return body;
  const canonical = {
    session_id: body.session_id,
    rail: body.rail,
    enclave_id: body.enclave_id,
    price_ver: body.price_ver,
    locked_rate_map: body.locked_rate_map,
    locked_per_req_au: body.locked_per_req_au,
    locked_min_session_au: body.locked_min_session_au,
    served_ctx: body.served_ctx,
  };
  if (hasOwn(body, 'ctx_bracket')) canonical.ctx_bracket = body.ctx_bracket;
  if (hasOwn(body, 'ctx_bracket_table_ver')) canonical.ctx_bracket_table_ver = body.ctx_bracket_table_ver;
  canonical.max_spend_au = body.max_spend_au;
  canonical.checkpoint_every = body.checkpoint_every;
  return canonical;
};
const canonicalReceiptBody = (body) => {
  if (!body || typeof body !== 'object' || Array.isArray(body)) return body;
  const canonical = {
    schema_version: body.schema_version,
    session_id: body.session_id,
    seq: body.seq,
    final: body.final,
    rail: body.rail,
    user: body.user,
    provider: body.provider,
    enclave_id: body.enclave_id,
    model_id: body.model_id,
    price_ver: body.price_ver,
    locked_rate_map: body.locked_rate_map,
    locked_per_req_au: body.locked_per_req_au,
    locked_min_session_au: body.locked_min_session_au,
    served_ctx: body.served_ctx,
  };
  if (hasOwn(body, 'ctx_bracket')) canonical.ctx_bracket = body.ctx_bracket;
  if (hasOwn(body, 'ctx_bracket_table_ver')) canonical.ctx_bracket_table_ver = body.ctx_bracket_table_ver;
  canonical.rules_ver = body.rules_ver;
  canonical.usage = body.usage;
  canonical.au_owed_cum = body.au_owed_cum;
  canonical.prompt_hash = body.prompt_hash;
  canonical.ts = body.ts;
  return canonical;
};
export const receiptMessage = (body, signingVersion = SIGNING_MESSAGE_VERSION) => {
  if (signingVersion !== SIGNING_MESSAGE_VERSION) {
    throw new Error(`Unsupported signing message version: ${signingVersion}`);
  }
  return JSON.stringify({
    domain: 'mayhem-session-receipt',
    signing_version: SIGNING_MESSAGE_VERSION,
    body: canonicalReceiptBody(body),
  });
};
export const spendVoucherMessage = (body, signingVersion = SIGNING_MESSAGE_VERSION) => {
  if (signingVersion !== SIGNING_MESSAGE_VERSION) {
    throw new Error(`Unsupported signing message version: ${signingVersion}`);
  }
  return JSON.stringify({
    domain: 'mayhem-spend-voucher',
    signing_version: SIGNING_MESSAGE_VERSION,
    body: canonicalSpendVoucherBody(body),
  });
};
export const spendReservationEvidence = (value) => ({
  contract_version: value.contract_version,
  session_id: value.session_id,
  epoch: value.epoch,
  at: value.at,
  rail: value.rail,
  user: value.user,
  provider: value.provider,
  enclave_id: value.enclave_id,
  price_ver: value.price_ver,
  rules_ver: value.rules_ver,
  served_ctx: value.served_ctx,
  ctx_bracket: value.ctx_bracket,
  ctx_bracket_table_ver: value.ctx_bracket_table_ver,
  max_spend_au: value.max_spend_au,
  voucher: stableValue(value.voucher),
});
export const spendReservationMessage = (value) =>
  `mayhem-spend-reservation-v1${stableJson(spendReservationEvidence(value))}`;
export const probeResultEvidence = (value, auditor) => ({
  auditor,
  probe_id: value.probe_id,
  probe_kind: value.probe_kind,
  provider: value.provider,
  enclave_id: value.enclave_id,
  binary_hash: value.binary_hash,
  canary_set: value.canary_set,
  verification_method: value.verification_method,
  session_receipt_hash: value.session_receipt_hash,
  evidence_hash: value.evidence_hash,
  match_bps: value.match_bps,
  pass: value.pass,
  epoch: value.epoch,
  at: value.at,
});
export const probeResultMessage = (value, auditor) =>
  `mayhem-probe-result-v1${stableJson(probeResultEvidence(value, auditor))}`;
export const providerKybEvidence = (value) => ({
  provider: value.provider,
  legal_name: value.legal_name,
  jurisdiction: value.jurisdiction,
  proof_hash: value.proof_hash,
  kyb_ref: value.kyb_ref,
  verified_at: value.verified_at,
  schema_version: value.schema_version,
});
export const providerKybMessage = (value) =>
  `mayhem-provider-kyb-v1${stableJson(providerKybEvidence(value))}`;
export const roomSidechannelName = (roomId) => `mx/room/${roomId}`;
export const deriveRoomId = async (modelId, creator, nonce) => {
  const digest = await blake3(b4a.from(`${modelId}${creator}${nonce}`));
  return b4a.toString(digest, 'hex').slice(0, 32);
};

const cloneValue = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));
const hasOwn = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
const compareCodepoint = (left, right) => {
  const a = String(left);
  const b = String(right);
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
};
const stableValue = (value) => {
  if (Array.isArray(value)) return value.map((item) => stableValue(item));
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stableValue(value[key])])
    );
  }
  return value;
};
const stableJson = (value) => JSON.stringify(stableValue(value));

export const contractParamDefinitions = () => cloneValue(PARAM_DEFINITIONS);
export const contractEpochAdminParamKeys = () => [...EPOCH_ADMIN_PARAM_KEYS];
export const contractEpochAdminParamDefinitions = () => Object.fromEntries(
  EPOCH_ADMIN_PARAM_KEYS.map((key) => [key, cloneValue(PARAM_DEFINITIONS[key])])
);
export const contractCtxBracketTable = () => ({
  ver: CTX_BRACKET_TABLE_VERSION,
  brackets: cloneValue(CTX_BRACKETS),
});

const ctxBracketForTokens = (tokens, table = CTX_BRACKETS) => {
  if (!Number.isSafeInteger(tokens) || tokens < 0) return null;
  const bracket = table.find((entry) => entry.max_ctx === null || tokens <= entry.max_ctx);
  return bracket?.id ?? null;
};

class MayhemContract extends Contract {
  constructor(protocol, options = {}) {
    super(protocol, options);
    const self = this;

    this.addFeature('mayhem_feature', async function () {
      return await self.mayhemFeature();
    });

    this.addSchema('noop', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 32 },
      },
    });

    this.addSchema('gatedNoop', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 32 },
      },
    });

    this.addSchema('readKey', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        key: { type: 'string', min: 1, max: 256 },
      },
    });

    this.addSchema('setRules', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        ver: { type: 'number', integer: true, min: 1 },
        hash: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('setParams', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        submitted_at: { type: 'number', integer: true, min: 0 },
        effective_at: { type: 'number', integer: true, min: 0 },
        values: { type: 'any' },
      },
    });

    this.addSchema('readParams', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        at: { type: 'number', integer: true, min: 0 },
        keys: {
          type: 'array',
          max: 64,
          items: { type: 'string', min: 1, max: 64 },
          optional: true,
        },
      },
    });

    this.addSchema('setCtxBrackets', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        submitted_at: { type: 'number', integer: true, min: 0 },
        effective_at: { type: 'number', integer: true, min: 0 },
        brackets: { type: 'array', min: 1, max: 32, items: { type: 'any' } },
      },
    });

    this.addSchema('readCtxBrackets', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        at: { type: 'number', integer: true, min: 0, optional: true },
        ver: { type: 'number', integer: true, min: 1, optional: true },
      },
    });

    this.addSchema('consent', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        ver: { type: 'number', integer: true, min: 1 },
        hash: { type: 'string', min: 1, max: 128 },
        sig: { type: 'string', min: 1, max: 256 },
      },
    });

    this.addFunction('registerProvider');

    this.addSchema('setProviderPayout', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        payout_addr: { type: 'string', min: 1, max: 256 },
        payout_method: { type: 'string', min: 1, max: 32 },
        payout_currency: { type: 'string', min: 3, max: 3, optional: true },
      },
    });

    this.addSchema('setProviderRails', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        rails: {
          type: 'array',
          min: 1,
          max: 3,
          items: { type: 'string', min: 1, max: 16 },
        },
      },
    });

    this.addSchema('setProviderKyb', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        legal_name: { type: 'string', min: 1, max: 160 },
        jurisdiction: { type: 'string', min: 1, max: 64 },
        proof_hash: { type: 'string', min: 1, max: 128 },
        kyb_ref: { type: 'string', min: 1, max: 128 },
        verified_at: { type: 'number', integer: true, min: 0 },
        schema_version: { type: 'number', integer: true, min: 1, optional: true },
        admin_sig: { type: 'string', min: 1, max: 256 },
      },
    });

    this.addSchema('revokeProviderKyb', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        reason_hash: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('banProvider', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        reason_hash: { type: 'string', min: 1, max: 128, optional: true },
        device_key: { type: 'string', min: 1, max: 128, optional: true },
        hardware_fingerprint: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('unban', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        target_type: { type: 'string', min: 1, max: 32 },
        target: { type: 'string', min: 1, max: 128 },
        reason_hash: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('deviceRebind', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        device_key: { type: 'string', min: 1, max: 128 },
        provider: { type: 'string', min: 1, max: 128 },
        reason_hash: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('setModelRef', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        model_id: { type: 'string', min: 1, max: 256 },
        model_class: { type: 'string', min: 1, max: 64 },
        rate_map: { type: 'array', min: 1, max: RATE_MAP_MAX_ENTRIES, items: { type: 'any' } },
        source_hash: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('publishCatalog', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        catalog_id: { type: 'string', min: 1, max: 128 },
        source_kind: { type: 'string', min: 1, max: 32 },
        catalog_url: { type: 'string', min: 1, max: 512 },
        signature_url: { type: 'string', min: 1, max: 512 },
        catalog_hash: { type: 'string', min: 1, max: 128 },
        signature_hash: { type: 'string', min: 1, max: 128 },
        key_id: { type: 'string', min: 1, max: 128 },
        public_key: { type: 'string', min: 1, max: 128 },
        model_count: { type: 'number', integer: true, min: 1 },
        artifact_count: { type: 'number', integer: true, min: 1 },
        canaries: { type: 'array', max: 64, items: { type: 'any' } },
      },
    });

    this.addSchema('registerEnclave', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        model_id: { type: 'string', min: 1, max: 256 },
        model_class: { type: 'string', min: 1, max: 64 },
        backend: { type: 'string', min: 1, max: 64 },
        artifact_root: { type: 'string', min: 1, max: 256 },
        artifact_root_kind: { type: 'string', min: 1, max: 64 },
        artifact_source: { type: 'any' },
        artifact_sidecars: { type: 'any', optional: true },
        source_sha256: { type: 'string', min: 1, max: 128, optional: true },
        manifest_hash: { type: 'string', min: 1, max: 128 },
        att_tier: { type: 'number', integer: true, min: 1, max: 4 },
        quant: { type: 'string', min: 1, max: 32, optional: true },
        binary_hash: { type: 'string', min: 1, max: 128 },
        launch_measurements: { type: 'any', optional: true },
        caps: { type: 'any' },
      },
    });

    this.addSchema('updateEnclave', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        model_class: { type: 'string', min: 1, max: 64, optional: true },
        backend: { type: 'string', min: 1, max: 64, optional: true },
        artifact_root: { type: 'string', min: 1, max: 256, optional: true },
        artifact_root_kind: { type: 'string', min: 1, max: 64, optional: true },
        artifact_source: { type: 'any', optional: true },
        artifact_sidecars: { type: 'any', optional: true },
        source_sha256: { type: 'string', min: 1, max: 128, optional: true },
        manifest_hash: { type: 'string', min: 1, max: 128, optional: true },
        att_tier: { type: 'number', integer: true, min: 1, max: 4, optional: true },
        quant: { type: 'string', min: 1, max: 32, optional: true },
        binary_hash: { type: 'string', min: 1, max: 128, optional: true },
        launch_measurements: { type: 'any', optional: true },
        caps: { type: 'any', optional: true },
      },
    });

    this.addSchema('setEnclaveMinTier', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        min_att_tier: { type: 'number', integer: true, min: 1, max: 4 },
        submitted_epoch: { type: 'number', integer: true, min: 0 },
        effective_epoch: { type: 'number', integer: true, min: 0 },
        submitted_at: { type: 'number', integer: true, min: 0 },
        reason_hash: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('joinEnclave', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        hardware_fingerprint: { type: 'string', min: 1, max: 128, optional: true },
        device_key: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('leaveEnclave', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('joinRoom', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        room_id: { type: 'string', min: 1, max: 128 },
        enclave_id: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('leaveRoom', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        room_id: { type: 'string', min: 1, max: 128 },
        enclave_id: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('retireEnclave', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('openRoom', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        model_id: { type: 'string', min: 1, max: 256, optional: true },
        nonce: { type: 'string', min: 1, max: 128 },
        label: { type: 'string', min: 1, max: 64 },
        policy: { type: 'any' },
      },
    });

    this.addSchema('closeRoom', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        room_id: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('setPrice', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        rate_map: { type: 'array', min: 1, max: RATE_MAP_MAX_ENTRIES, items: { type: 'any' } },
        per_req_au: { type: 'string', min: 1, max: 80 },
        min_session_au: { type: 'string', min: 1, max: 80 },
        effective_at: { type: 'number', integer: true, min: 0 },
        ctx_bracket: { type: 'string', min: 1, max: 64, optional: true },
      },
    });

    this.addSchema('readPrice', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        at: { type: 'number', integer: true, min: 0 },
        ctx_bracket: { type: 'string', min: 1, max: 64, optional: true },
      },
    });

    this.addSchema('recordReputationEvent', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        event_id: { type: 'string', min: 1, max: 128 },
        kind: { type: 'string', min: 1, max: 64 },
        epoch: { type: 'number', integer: true, min: 0 },
        at: { type: 'number', integer: true, min: 0 },
        paid_au: { type: 'string', min: 1, max: 80, optional: true },
        max_spend_au: { type: 'string', min: 1, max: 80, optional: true },
        evidence_hash: { type: 'string', min: 1, max: 128, optional: true },
        beneficiary: { type: 'string', min: 1, max: 128, optional: true },
        enclave_id: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('anchorReputation', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        epoch: { type: 'number', integer: true, min: 0 },
        folded_at: { type: 'number', integer: true, min: 0 },
        events_head: { type: 'string', min: 1, max: 128 },
        r_bps: { type: 'number', integer: true, min: 0, max: 10_000 },
        raw_milli: { type: 'number', integer: true },
        successful_sessions: { type: 'number', integer: true, min: 0 },
        provenance_violation: { type: 'boolean', optional: true },
      },
    });

    this.addSchema('auditorRegister', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        auditor: { type: 'string', min: 1, max: 128, optional: true },
        registered_at_seconds: { type: 'number', integer: true, min: 0, optional: true },
      },
    });

    this.addSchema('probeResult', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        probe_id: { type: 'string', min: 1, max: 128 },
        probe_kind: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        enclave_id: { type: 'string', min: 1, max: 128, optional: true },
        binary_hash: { type: 'string', min: 1, max: 128, optional: true },
        epoch: { type: 'number', integer: true, min: 0 },
        at: { type: 'number', integer: true, min: 0 },
        canary_set: { type: 'string', min: 1, max: 128, optional: true },
        verification_method: { type: 'string', min: 1, max: 64, optional: true },
        match_bps: { type: 'number', integer: true, min: 0, max: 10_000, optional: true },
        pass: { type: 'boolean', optional: true },
        session_receipt_hash: { type: 'string', min: 1, max: 128, optional: true },
        evidence_hash: { type: 'string', min: 1, max: 128, optional: true },
        auditor_sig: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('epochCommit', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
        roots: { type: 'any' },
        totals: { type: 'any' },
      },
    });

    this.addSchema('epochSealEmpty', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
        reason_hash: { type: 'string', min: 64, max: 64 },
      },
    });

    this.addSchema('fraudProof', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        epoch: { type: 'number', integer: true, min: 1 },
        proof_epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
        reason: { type: 'string', min: 1, max: 64 },
        receipt: { type: 'any', optional: true },
        claimed_au_owed_cum: { type: 'string', min: 1, max: 80, optional: true },
        previous_au_owed_cum: { type: 'string', min: 1, max: 80, optional: true },
        price_usage: { type: 'any', optional: true },
      },
    });

    this.addSchema('dispute', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        rail: { type: 'string', min: 1, max: 16 },
        session_id: { type: 'string', min: 1, max: 128 },
        reason: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128, optional: true },
        counterparty: { type: 'string', min: 1, max: 128, optional: true },
        enclave_id: { type: 'string', min: 1, max: 128, optional: true },
        epoch: { type: 'number', integer: true, min: 0, optional: true },
        at: { type: 'number', integer: true, min: 0 },
        evidence_hash: { type: 'string', min: 1, max: 128, optional: true },
        evidence: { type: 'any', optional: true },
      },
    });

    this.addSchema('disputeResolve', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        dispute_id: { type: 'number', integer: true, min: 1 },
        outcome: { type: 'string', min: 1, max: 64 },
        deposit_action: { type: 'string', min: 1, max: 64 },
        rationale_hash: { type: 'string', min: 1, max: 128 },
        at: { type: 'number', integer: true, min: 0 },
        slash: { type: 'boolean', optional: true },
        beneficiary: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('disputeExpire', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        dispute_id: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
      },
    });

    this.addSchema('fiatDeposit', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        rail: { type: 'string', min: 1, max: 64 },
        who: { type: 'string', min: 1, max: 128 },
        au: { type: 'string', min: 1, max: 80 },
        ext_ref_hash: { type: 'string', min: 1, max: 128 },
        fiat_currency: { type: 'string', min: 3, max: 3 },
        fiat_amount_minor: { type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER },
        epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
      },
    });

    this.addSchema('fiatChargeback', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        rail: { type: 'string', min: 1, max: 64 },
        who: { type: 'string', min: 1, max: 128 },
        au: { type: 'string', min: 1, max: 80 },
        ext_ref_hash: { type: 'string', min: 1, max: 128 },
        dispute_ref_hash: { type: 'string', min: 1, max: 128 },
        fiat_currency: { type: 'string', min: 3, max: 3 },
        fiat_amount_minor: { type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER },
        epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
      },
    });

  }

  async noop() {
    const result = {
      ok: true,
      op: 'noop',
      contract: 'mayhem',
      version: CONTRACT_VERSION,
      address: this.address,
    };
    console.log('mayhem noop', result);
    return result;
  }

  async gatedNoop() {
    const consentError = await this.requireConsent();
    if (consentError) return consentError;

    const result = {
      ok: true,
      op: 'gatedNoop',
      contract: 'mayhem',
      version: CONTRACT_VERSION,
      address: this.address,
    };
    console.log('mayhem gatedNoop', result);
    return result;
  }

  async readKey() {
    const key = this.value?.key;
    const value = typeof key === 'string' ? await this.get(key) : null;
    console.log('mayhem readKey', key, '=>', value);
    return value;
  }

  async mayhemFeature() {
    this._mayhemLastFeatureResult = undefined;
    const rawKey = this.op?.key;
    const key = typeof rawKey === 'string' && rawKey.startsWith('mayhem_')
      ? rawKey.slice('mayhem_'.length)
      : rawKey;
    const value = this.value;
    if (typeof key !== 'string' || !value || typeof value !== 'object' || Array.isArray(value)) {
      return;
    }
    if (value.op === 'deposit_tnk') {
      const result = await this.applyDepositTnkFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'consent') {
      const result = await this.applyConsentFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'provider_lifecycle') {
      const result = await this.applyProviderLifecycleFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'spend_reserve') {
      const result = await this.applySpendReserveFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    const adminError = await this.requireAdmin(this.address);
    if (adminError) {
      this._mayhemLastFeatureResult = adminError;
      return adminError;
    }
    if (value.op === 'epoch_apply') {
      const result = await this.applyEpochApplyFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (['tnk_deposit', 'tap_deposit', 'fiat_deposit'].includes(value.op)) {
      const result = await this.applyDepositCreditFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'rate_oracle' || value.op === 'tap_rate_oracle') {
      const result = await this.applyRateOracleFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'tnk_settlement') {
      const result = await this.applyTnkSettlementFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'fiat_settlement') {
      const result = await this.applyFiatSettlementFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'anchor_reputation') {
      const result = await this.applyReputationAnchorFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
    if (value.op === 'tier3_bless_measurement') {
      const result = await this.applyTier3MeasurementFeature(key, value);
      this._mayhemLastFeatureResult = result;
      return result;
    }
  }

  async applyConsentFeature(key, value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      ['op', 'sender', 'ver', 'hash', 'sig'],
      'consent feature'
    );
    if (shapeError) return;
    if (!this.isHexBytes(value.sender, 32)) return;
    if (!this.isHexBytes(value.sig, 64)) return;
    const rules = await this.currentRules();
    if (!rules || value.ver !== rules.ver || value.hash !== rules.hash) return;
    if (key !== `consent/${value.sender}/${value.ver}/${value.hash}`) return;
    if (!this.verifyConsentSignature(value.sender, value.ver, value.hash, value.sig)) return;

    const record = {
      ver: value.ver,
      hash: value.hash,
      at: key,
      via: 'feature',
    };
    await this.put(`consent/${value.sender}`, record);
    console.log('mayhem consent feature', { address: value.sender, ...record });
    return { ok: true, op: 'consentFeature', address: value.sender, ...record };
  }

  async applyProviderLifecycleFeature(key, value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      ['op', 'intent', 'sig'],
      'provider lifecycle feature'
    );
    if (shapeError) return;
    const intent = value.intent;
    if (!intent || typeof intent !== 'object' || Array.isArray(intent)) return;
    const intentError = this.validateProviderLifecycleIntent(intent);
    if (intentError) return;
    if (!this.isHexBytes(value.sig, 64)) return;
    if (!(await this.providerLifecycleFeatureKeys(intent)).includes(key)) return;
    if (!this.verifyProviderLifecycleSignature(intent.provider, intent, value.sig)) return;

    switch (intent.op) {
      case 'register_provider':
        return await this.applyRegisterProvider(intent.provider, key);
      case 'join_enclave':
        return await this.applyJoinEnclave(
          intent.provider,
          intent.enclave_id,
          key,
          intent.hardware_fingerprint ?? null,
          intent.device_key ?? null,
          {
            served_ctx: intent.served_ctx,
            ctx_bracket: intent.ctx_bracket,
            ctx_bracket_table_ver: intent.ctx_bracket_table_ver,
          }
        );
      case 'leave_enclave':
        return await this.applyLeaveEnclave(intent.provider, intent.enclave_id, key);
      case 'join_room':
        return await this.applyJoinRoom(intent.provider, intent.room_id, intent.enclave_id, key);
      case 'leave_room':
        return await this.applyLeaveRoom(intent.provider, intent.room_id, intent.enclave_id, key);
      case 'set_provider_rails':
        return await this.applySetProviderRails(intent.provider, intent.rails, key);
      default:
        return;
    }
  }

  async applySpendReserveFeature(key, value) {
    const normalized = await this.normalizeSpendReserveValue(value);
    if (normalized instanceof Error) return normalized;
    normalized.voucher_hash = await this.opaqueHash('mayhem-spend-voucher-record-v1', normalized.voucher_body);
    const expectedKey = await this.spendReservationFeatureKey(normalized);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;
    if (!this.verifySpendVoucherSignature(normalized.user, normalized.voucher_body, normalized.voucher.user_sig)) {
      return new Error('Invalid spend voucher signature.');
    }
    if (!this.verifySpendReservationSignature(normalized.provider, normalized)) {
      return new Error('Invalid spend reservation provider signature.');
    }

    const applyState = await this.epochApplyStateRecord();
    const activeEpoch = applyState.updated_epoch + 1;
    if (normalized.epoch !== activeEpoch) {
      return new Error('Spend reservation epoch is not the active billing epoch.');
    }

    const provider = await this.get(`prov/${normalized.provider}`);
    if (!provider || provider.status !== 'active') return new Error('Provider registration required.');
    if (!Array.isArray(provider.accepted_rails) || !provider.accepted_rails.includes(normalized.rail)) {
      return new Error('Provider does not accept payment rail.');
    }
    const enclave = await this.get(`enclave/${normalized.enclave_id}`);
    if (!enclave || enclave.status !== 'active') return new Error('Admin enclave is not active.');
    const enclaveError = this.requireAdminCreatedEnclave(enclave);
    if (enclaveError) return enclaveError;
    const serve = await this.get(`serve/${normalized.provider}/${normalized.enclave_id}`);
    if (!serve || serve.status !== 'active') {
      return new Error('Provider is not actively serving this admin enclave.');
    }
    const serveTermsError = this.validateCommittedServeTerms(serve, normalized);
    if (serveTermsError) return serveTermsError;
    const priceCtxBracket = normalized.ctx_bracket ?? null;
    const priceError = await this.requireCurrentAdminPrice(normalized.enclave_id, priceCtxBracket);
    if (priceError) return priceError;
    const lockedPrice = await this.get(
      this.priceRecordKey(normalized.enclave_id, normalized.price_ver, priceCtxBracket)
    );
    if (!lockedPrice || lockedPrice.denom !== PRICE_DENOMINATION) {
      return new Error('Spend reservation locked price version is not known.');
    }
    if (lockedPrice.set_by_role !== 'admin') {
      return new Error('Spend reservation locked price is not admin-set.');
    }
    if ((lockedPrice.ctx_bracket ?? null) !== priceCtxBracket) {
      return new Error('Spend reservation locked price context bracket mismatch.');
    }
    if ((lockedPrice.ctx_bracket_table_ver ?? null) !== (normalized.ctx_bracket_table_ver ?? null)) {
      return new Error('Spend reservation locked price context bracket table mismatch.');
    }
    if (stableJson(normalized.locked_rate_map) !== stableJson(lockedPrice.rate_map)) {
      return new Error('Spend reservation locked rate_map does not match price version.');
    }
    if (this.compareAu(normalized.locked_per_req_au, lockedPrice.per_req_au ?? ZERO_AU) !== 0) {
      return new Error('Spend reservation locked per_req_au does not match price version.');
    }
    if (this.compareAu(normalized.locked_min_session_au, lockedPrice.min_session_au ?? ZERO_AU) !== 0) {
      return new Error('Spend reservation locked min_session_au does not match price version.');
    }
    const rules = await this.currentRules();
    if (!rules || rules.ver !== normalized.rules_ver) {
      return new Error('Spend reservation rules version is not current.');
    }

    const balance = await this.balanceRecord(normalized.user, normalized.rail);
    if (balance instanceof Error) return balance;
    const balanceError = this.guardianValidateBalanceRecord(balance, normalized.user, normalized.rail);
    if (balanceError) return balanceError;

    const holdKey = this.spendHoldKey(normalized.user, normalized.rail, normalized.epoch);
    const hold = await this.normalizeSpendHoldRecord(
      (await this.get(holdKey)) ?? null,
      normalized.user,
      normalized.rail,
      normalized.epoch
    );
    if (hold instanceof Error) return hold;
    const existing = hold.sessions.find((session) => session.session_id === normalized.session_id);
    if (existing) {
      if (
        existing.provider !== normalized.provider ||
        existing.enclave_id !== normalized.enclave_id ||
        existing.price_ver !== normalized.price_ver ||
        stableJson(existing.locked_rate_map) !== stableJson(normalized.locked_rate_map) ||
        this.compareAu(existing.locked_per_req_au, normalized.locked_per_req_au) !== 0 ||
        this.compareAu(existing.locked_min_session_au, normalized.locked_min_session_au) !== 0 ||
        existing.served_ctx !== normalized.served_ctx ||
        existing.ctx_bracket !== normalized.ctx_bracket ||
        existing.ctx_bracket_table_ver !== normalized.ctx_bracket_table_ver ||
        this.compareAu(existing.max_spend_au, normalized.max_spend_au) !== 0 ||
        existing.voucher_hash !== normalized.voucher_hash
      ) {
        return new Error('Spend reservation session already exists with different terms.');
      }
      const availableAu = this.compareAu(hold.reserved_au, balance.au) >= 0
        ? ZERO_AU
        : this.safeSubAu(balance.au, hold.reserved_au);
      if (availableAu instanceof Error) return availableAu;
      return {
        ok: true,
        op: 'spendReserve',
        session_id: normalized.session_id,
        epoch: normalized.epoch,
        rail: normalized.rail,
        user: normalized.user,
        provider: normalized.provider,
        reserved_au: hold.reserved_au,
        available_au: availableAu,
        idempotent: true,
      };
    }

    const nextReservedAu = this.safeAddAu(hold.reserved_au, normalized.max_spend_au);
    if (nextReservedAu instanceof Error) return nextReservedAu;
    if (this.compareAu(nextReservedAu, balance.au) > 0) {
      return new Error('Insufficient unreserved credit balance.');
    }
    const session = {
      session_id: normalized.session_id,
      provider: normalized.provider,
      enclave_id: normalized.enclave_id,
      price_ver: normalized.price_ver,
      locked_rate_map: normalized.locked_rate_map,
      locked_per_req_au: normalized.locked_per_req_au,
      locked_min_session_au: normalized.locked_min_session_au,
      served_ctx: normalized.served_ctx,
      ctx_bracket: normalized.ctx_bracket,
      ctx_bracket_table_ver: normalized.ctx_bracket_table_ver,
      rules_ver: normalized.rules_ver,
      max_spend_au: normalized.max_spend_au,
      voucher_hash: normalized.voucher_hash,
      feature_key: key,
      reserved_at: normalized.at,
      recorded_at: this.tx,
    };
    const nextHold = {
      ...hold,
      balance_au_at_last_reserve: balance.au,
      reserved_au: nextReservedAu,
      sessions: [...hold.sessions, session].sort((a, b) => compareCodepoint(a.session_id, b.session_id)),
      updated_at: this.tx,
    };
    await this.put(holdKey, nextHold);
    const availableAu = this.safeSubAu(balance.au, nextHold.reserved_au);
    if (availableAu instanceof Error) return availableAu;
    return {
      ok: true,
      op: 'spendReserve',
      session_id: normalized.session_id,
      epoch: normalized.epoch,
      rail: normalized.rail,
      user: normalized.user,
      provider: normalized.provider,
      reserved_au: nextHold.reserved_au,
      available_au: availableAu,
      idempotent: false,
    };
  }

  async applyEpochApplyFeature(key, value) {
    const expectedKey = await this.epochApplyFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousTx = this.tx;
    this.tx = key;
    try {
      return await this.epochApply();
    } finally {
      this.tx = previousTx;
    }
  }

  async applyDepositTnkFeature(key, value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      ['op', 'sender', 'intent', 'sig'],
      'deposit TNK feature'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'deposit_tnk') return new Error('Invalid deposit TNK feature op.');
    if (!this.isHexBytes(value.sender, 32)) return new Error('Invalid deposit TNK sender.');
    if (!this.isHexBytes(value.sig, 64)) return new Error('Invalid deposit TNK signature.');
    const intentError = this.validateDepositTnkIntent(value.intent);
    if (intentError) return intentError;
    if (!this.verifyDepositTnkSignature(value.sender, value.intent, value.sig)) {
      return new Error('Invalid deposit TNK signature.');
    }
    const expectedKey = await this.depositFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousAddress = this.address;
    const previousTx = this.tx;
    const previousValue = this.value;
    this.address = value.sender;
    this.tx = key;
    this.value = value.intent;
    try {
      return await this.depositTnk();
    } finally {
      this.address = previousAddress;
      this.tx = previousTx;
      this.value = previousValue;
    }
  }

  async applyDepositCreditFeature(key, value) {
    const expectedKey = await this.depositFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousTx = this.tx;
    this.tx = key;
    try {
      if (value.op === 'tnk_deposit') return await this.tnkDeposit();
      if (value.op === 'tap_deposit') return await this.tapDeposit();
      if (value.op === 'fiat_deposit') return await this.fiatDeposit();
      return;
    } finally {
      this.tx = previousTx;
    }
  }

  async applyRateOracleFeature(key, value) {
    const expectedKey = await this.rateFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousTx = this.tx;
    this.tx = key;
    try {
      if (value.op === 'rate_oracle') return await this.rateOracle();
      if (value.op === 'tap_rate_oracle') return await this.tapRateOracle();
      return;
    } finally {
      this.tx = previousTx;
    }
  }

  async applyTnkSettlementFeature(key, value) {
    const expectedKey = await this.tnkSettlementFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousTx = this.tx;
    this.tx = key;
    try {
      return await this.tnkSettlement();
    } finally {
      this.tx = previousTx;
    }
  }

  async applyFiatSettlementFeature(key, value) {
    const expectedKey = await this.fiatSettlementFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousTx = this.tx;
    this.tx = key;
    try {
      return await this.fiatSettlement();
    } finally {
      this.tx = previousTx;
    }
  }

  async applyReputationAnchorFeature(key, value) {
    const expectedKey = await this.reputationAnchorFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousTx = this.tx;
    const previousValue = this.value;
    this.tx = key;
    this.value = value;
    try {
      return await this.anchorReputation();
    } finally {
      this.tx = previousTx;
      this.value = previousValue;
    }
  }

  async applyTier3MeasurementFeature(key, value) {
    const expectedKey = await this.tier3MeasurementFeatureKey(value);
    if (expectedKey instanceof Error) return expectedKey;
    if (key !== expectedKey) return;

    const previousTx = this.tx;
    const previousValue = this.value;
    this.tx = key;
    this.value = value;
    try {
      return await this.tier3BlessMeasurement();
    } finally {
      this.tx = previousTx;
      this.value = previousValue;
    }
  }

  async tier3BlessMeasurement() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const validationError = this.validateTier3MeasurementBlessValue(this.value);
    if (validationError) return validationError;

    const platform = this.value.platform;
    const layer = this.value.layer;
    const measurementName = this.value.measurement_name;
    const measurement = this.value.measurement.toLowerCase();
    const key = `tier3/measurement/${platform}`;
    const current = await this.get(key);
    const record = current
      ? cloneValue(current)
      : {
          schema_version: 1,
          platform,
          entries: [],
          measurements: {},
          created_at: this.tx,
          created_by: this.address,
        };
    if (record.platform !== platform) return new Error('Tier-3 measurement platform mismatch.');
    if (!Array.isArray(record.entries)) record.entries = [];
    if (!record.measurements || typeof record.measurements !== 'object' || Array.isArray(record.measurements)) {
      record.measurements = {};
    }
    if (!record.measurements[layer] || typeof record.measurements[layer] !== 'object' || Array.isArray(record.measurements[layer])) {
      record.measurements[layer] = {};
    }

    const already = record.entries.find((entry) =>
      entry.layer === layer && entry.measurement_name === measurementName && entry.measurement === measurement
    );
    if (already) {
      return {
        ok: true,
        op: 'tier3BlessMeasurement',
        platform,
        layer,
        measurement_name: measurementName,
        measurement,
        status: 'already_blessed',
      };
    }

    const entry = {
      layer,
      measurement_name: measurementName,
      measurement,
      effective_epoch: this.value.effective_epoch,
      region: this.value.region ?? null,
      derivation_hash: this.value.derivation_hash ?? null,
      source: this.value.source ?? 'admin-derive',
      blessed_at: this.tx,
      blessed_by: this.address,
    };
    record.entries.push(entry);
    const values = Array.isArray(record.measurements[layer][measurementName])
      ? record.measurements[layer][measurementName]
      : [];
    if (!values.includes(measurement)) values.push(measurement);
    values.sort();
    record.measurements[layer][measurementName] = values;
    record.updated_at = this.tx;
    record.updated_by = this.address;
    await this.put(key, record);
    await this.put(`tier3/measurement/${platform}/${layer}/${measurementName}/${measurement}`, entry);
    console.log('mayhem tier3BlessMeasurement', entry);
    return {
      ok: true,
      op: 'tier3BlessMeasurement',
      platform,
      layer,
      measurement_name: measurementName,
      measurement,
      status: 'blessed',
    };
  }

  async setRules() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const current = await this.currentRules();
    if (current && this.value.ver <= current.ver) {
      return new Error('Rules version must increase.');
    }

    const rules = {
      ver: this.value.ver,
      hash: this.value.hash,
      set_by: this.address,
      set_by_role: 'admin',
      activated_at: this.tx,
    };
    await this.put(`rules/${rules.ver}`, rules);
    await this.put(CURRENT_RULES_KEY, rules);
    console.log('mayhem setRules', rules);
    return { ok: true, op: 'setRules', rules };
  }

  async setParams() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const governanceParams = await this.activeParamsAt(this.value.submitted_at, [
      'param_activation_delay_seconds',
    ]);
    if (
      this.value.effective_at - this.value.submitted_at <
      governanceParams.param_activation_delay_seconds
    ) {
      return new Error('Parameter changes require at least the active param_activation_delay_seconds.');
    }

    const valuesError = this.validateParamValues(this.value.values);
    if (valuesError) return valuesError;
    const normalizedValues = this.normalizeParamValues(this.value.values);
    if (normalizedValues instanceof Error) return normalizedValues;

    const existingAtEffective = await this.activeParamsAt(this.value.effective_at);
    const mergedAtEffective = { ...existingAtEffective, ...normalizedValues };
    const boundsError = this.validateParamBounds(mergedAtEffective);
    if (boundsError) return boundsError;

    const meta = await this.get('params/current');
    const ver = meta ? meta.ver + 1 : 1;
    const keys = Object.keys(normalizedValues).sort();
    const update = {
      ver,
      values: cloneValue(normalizedValues),
      submitted_at: this.value.submitted_at,
      effective_at: this.value.effective_at,
      set_by: this.address,
      set_by_role: 'admin',
      tx: this.tx,
    };

    for (const key of keys) {
      const record = await this.paramRecord(key);
      if (record.pending && record.pending.effective_at > this.value.submitted_at) {
        return new Error(`Pending parameter change already scheduled for ${key}.`);
      }

      const current = this.paramActiveEntry(record, this.value.submitted_at);
      const updated = {
        key,
        current,
        pending: {
          value: normalizedValues[key],
          ver,
          submitted_at: this.value.submitted_at,
          effective_at: this.value.effective_at,
          set_by: this.address,
          set_by_role: 'admin',
          set_at: this.tx,
        },
      };
      await this.put(`params/${key}`, updated);
    }

    await this.put(`params/update/${ver}`, update);
    await this.put('params/current', {
      ver,
      keys,
      set_by: this.address,
      set_by_role: 'admin',
      updated_at: this.tx,
      effective_at: this.value.effective_at,
    });
    console.log('mayhem setParams', update);
    return { ok: true, op: 'setParams', ver, effective_at: this.value.effective_at, keys };
  }

  async readParams() {
    const keys = this.value.keys ?? Object.keys(PARAM_DEFINITIONS);
    const keyError = this.validateParamKeys(keys);
    if (keyError) return keyError;

    const params = await this.activeParamsAt(this.value.at, keys);
    console.log('mayhem readParams', { at: this.value.at, params });
    return { ok: true, op: 'readParams', at: this.value.at, params };
  }

  async setCtxBrackets() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const governanceParams = await this.activeParamsAt(this.value.submitted_at, [
      'param_activation_delay_seconds',
    ]);
    if (
      this.value.effective_at - this.value.submitted_at <
      governanceParams.param_activation_delay_seconds
    ) {
      return new Error('Context bracket changes require at least the active param_activation_delay_seconds.');
    }

    const brackets = this.normalizeCtxBracketTable(this.value.brackets);
    if (brackets instanceof Error) return brackets;

    const schedule = await this.ctxBracketSchedule();
    if (schedule.pending && schedule.pending.effective_at > this.value.submitted_at) {
      return new Error('Pending context bracket table already scheduled.');
    }
    const latest = this.ctxBracketLatestEntry(schedule);
    const record = {
      ver: latest.ver + 1,
      brackets,
      submitted_at: this.value.submitted_at,
      effective_at: this.value.effective_at,
      effective_from: this.tx,
      updated_at: this.tx,
      set_by: this.address,
      set_by_role: 'admin',
    };
    const updated = {
      current: schedule.current,
      pending: schedule.pending,
    };
    if (updated.pending && updated.pending.effective_at <= this.value.effective_at) {
      updated.current = updated.pending;
      updated.pending = null;
    }
    if (updated.pending) return new Error('Pending context bracket table already scheduled.');
    updated.pending = record;

    await this.put('ctx_brackets', updated);
    await this.put(`ctx_brackets/v/${record.ver}`, record);
    console.log('mayhem setCtxBrackets', record);
    return { ok: true, op: 'setCtxBrackets', ver: record.ver, effective_at: record.effective_at };
  }

  async readCtxBrackets() {
    if (hasOwn(this.value, 'ver') && hasOwn(this.value, 'at')) {
      return new Error('Read context brackets by either ver or at, not both.');
    }
    const table = hasOwn(this.value, 'ver')
      ? await this.ctxBracketTableByVersion(this.value.ver)
      : await this.ctxBracketTableAt(this.value.at ?? 0);
    if (table instanceof Error) return table;
    console.log('mayhem readCtxBrackets', table);
    return { ok: true, op: 'readCtxBrackets', table };
  }

  async consent() {
    const rules = await this.currentRules();
    if (!rules) return new Error('Rules are not set.');
    if (this.value.ver !== rules.ver || this.value.hash !== rules.hash) {
      return new Error('Consent must match the current rules.');
    }
    if (!this.verifyConsentSignature(this.address, this.value.ver, this.value.hash, this.value.sig)) {
      return new Error('Invalid consent signature.');
    }

    const record = {
      ver: this.value.ver,
      hash: this.value.hash,
      at: this.tx,
    };
    await this.put(`consent/${this.address}`, record);
    console.log('mayhem consent', { address: this.address, ...record });
    return { ok: true, op: 'consent', address: this.address, ...record };
  }

  async registerProvider() {
    const shapeError = this.validateExactCommandValue(['op'], 'register_provider');
    if (shapeError) return shapeError;
    if (this.value.op !== 'register_provider') return new Error('Invalid provider registration op.');

    return this.applyRegisterProvider(this.address, this.tx);
  }

  async applyRegisterProvider(providerId, stamp) {
    const consentError = await this.requireConsent(providerId);
    if (consentError) return consentError;

    const auditor = await this.get(`auditor/${providerId}`);
    if (auditor?.status === 'active') {
      return new Error('Auditor keys cannot register as providers.');
    }

    const key = `prov/${providerId}`;
    if ((await this.get(key)) !== null) return new Error('Provider already registered.');

    const record = {
      provider: providerId,
      payout: null,
      accepted_rails: ['fiat'],
      accepted_rails_schema_version: PROVIDER_RAIL_SCHEMA_VERSION,
      accepted_rails_set_by: providerId,
      accepted_rails_set_at: stamp,
      status: 'active',
      enclaves: [],
      probation: {
        since: stamp,
        since_seconds: 0,
        successful_sessions: 0,
      },
      registered_at: stamp,
      updated_at: stamp,
    };
    await this.put(key, record);
    console.log('mayhem registerProvider', record);
    return { ok: true, op: 'registerProvider', provider: providerId };
  }

  normalizeProviderAcceptedRails(rails) {
    if (!Array.isArray(rails) || rails.length === 0) {
      return new Error('Provider accepted rails cannot be empty.');
    }
    const accepted = [];
    for (const rawRail of rails) {
      if (typeof rawRail !== 'string') return new Error('Invalid provider rail.');
      const rail = rawRail.toLowerCase();
      if (!PROVIDER_ACCEPTED_RAILS.has(rail)) {
        return new Error('Unsupported provider payment rail.');
      }
      if (accepted.includes(rail)) return new Error('Duplicate provider payment rail.');
      accepted.push(rail);
    }
    if (accepted.length === 0) return new Error('Provider accepted rails cannot be empty.');
    accepted.sort(
      (left, right) =>
        PROVIDER_ACCEPTED_RAIL_ORDER.indexOf(left) - PROVIDER_ACCEPTED_RAIL_ORDER.indexOf(right)
    );
    return accepted;
  }

  normalizeLedgerRail(value, label = 'ledger rail') {
    if (typeof value !== 'string') return new Error(`Invalid ${label}.`);
    const rail = value.toLowerCase();
    if (!PROVIDER_ACCEPTED_RAILS.has(rail)) return new Error(`Unsupported ${label}.`);
    return rail;
  }

  balanceKey(user, rail) {
    return `bal/${user}/${rail}`;
  }

  spendHoldKey(user, rail, epoch) {
    return `hold/${rail}/${user}/${epoch}`;
  }

  earningKey(provider, rail) {
    return `earn/${rail}/${provider}`;
  }

  feeCumKey(rail) {
    return `fee/${rail}/cum`;
  }

  async setProviderRails() {
    const shapeError = this.validateExactCommandValue(['op', 'rails'], 'set_provider_rails');
    if (shapeError) return shapeError;
    return await this.applySetProviderRails(this.address, this.value.rails, this.tx);
  }

  async applySetProviderRails(providerId, acceptedRails, stamp) {
    const consentError = await this.requireConsent(providerId);
    if (consentError) return consentError;
    const provider = await this.get(`prov/${providerId}`);
    if (!provider || provider.status !== 'active') return new Error('Provider registration required.');
    const rails = this.normalizeProviderAcceptedRails(acceptedRails);
    if (rails instanceof Error) return rails;

    const updated = {
      ...provider,
      accepted_rails: rails,
      accepted_rails_schema_version: PROVIDER_RAIL_SCHEMA_VERSION,
      accepted_rails_set_by: providerId,
      accepted_rails_set_at: stamp,
      updated_at: stamp,
    };
    await this.put(`prov/${providerId}`, updated);
    console.log('mayhem setProviderRails', updated);
    return { ok: true, op: 'setProviderRails', provider: providerId, rails };
  }

  async setProviderPayout() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.provider)) return new Error('Invalid provider id.');
    if (!PAYOUT_TARGET_METHODS.has(this.value.payout_method)) {
      return new Error('Unsupported payout method.');
    }
    let payoutCurrency = null;
    if (this.value.payout_method === 'tnk' || this.value.payout_method === 'tap') {
      if (this.value.payout_currency !== undefined) {
        return new Error('Crypto payout target must not include fiat currency.');
      }
    } else {
      if (this.value.payout_currency === undefined) {
        return new Error('Fiat payout target requires payout_currency.');
      }
      payoutCurrency = this.normalizeFiatCurrency(this.value.payout_currency);
      if (payoutCurrency instanceof Error) return payoutCurrency;
    }

    const key = `prov/${this.value.provider}`;
    const record = await this.get(key);
    if (!record) return new Error('Provider not found.');

    const updated = {
      ...record,
      payout: {
        addr: this.value.payout_addr,
        method: this.value.payout_method,
        ...(payoutCurrency ? { currency: payoutCurrency } : {}),
        set_by: this.address,
        set_by_role: 'admin',
        set_at: this.tx,
      },
      updated_at: this.tx,
    };
    await this.put(key, updated);
    console.log('mayhem setProviderPayout', updated);
    return { ok: true, op: 'setProviderPayout', provider: this.value.provider };
  }

  async setProviderKyb() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateExactCommandValue(
      [
        'op',
        'provider',
        'legal_name',
        'jurisdiction',
        'proof_hash',
        'kyb_ref',
        'verified_at',
        'admin_sig',
      ],
      'set_provider_kyb',
      ['schema_version']
    );
    if (shapeError) return shapeError;

    const normalized = this.normalizeProviderKybValue(this.value);
    if (normalized instanceof Error) return normalized;
    if (!(await this.verifyProviderKybSignature(normalized))) {
      return new Error('Invalid provider KYB admin signature.');
    }

    const key = `prov/${normalized.provider}`;
    const provider = await this.get(key);
    if (!provider) return new Error('Provider not found.');
    if (provider.status === 'banned') return new Error('Provider is banned.');
    const kybBanError = await this.rejectBannedProviderKyb(normalized);
    if (kybBanError) return kybBanError;

    const record = {
      status: 'verified',
      provider: normalized.provider,
      legal_name: normalized.legal_name,
      jurisdiction: normalized.jurisdiction,
      proof_hash: normalized.proof_hash,
      kyb_ref: normalized.kyb_ref,
      verified_at: normalized.verified_at,
      verified_by: this.address,
      verified_by_role: 'admin',
      admin_sig: normalized.admin_sig,
      schema_version: normalized.schema_version,
      updated_at: this.tx,
    };
    const providerSummary = {
      status: 'verified',
      legal_name: record.legal_name,
      jurisdiction: record.jurisdiction,
      proof_hash: record.proof_hash,
      kyb_ref: record.kyb_ref,
      verified_at: record.verified_at,
      verified_by: record.verified_by,
      verified_by_role: 'admin',
      schema_version: record.schema_version,
      set_at: this.tx,
    };
    const updatedProvider = {
      ...provider,
      kyb: providerSummary,
      updated_at: this.tx,
    };

    await this.put(`kyb/${normalized.provider}`, record);
    await this.put(key, updatedProvider);
    console.log('mayhem setProviderKyb', record);
    return {
      ok: true,
      op: 'setProviderKyb',
      provider: normalized.provider,
      att_tier: 4,
    };
  }

  async revokeProviderKyb() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateExactCommandValue(
      ['op', 'provider'],
      'revoke_provider_kyb',
      ['reason_hash']
    );
    if (shapeError) return shapeError;
    if (!this.isHexBytes(this.value.provider, 32)) return new Error('Invalid provider id.');
    if (
      this.value.reason_hash !== undefined &&
      !this.isHexBytes(this.value.reason_hash, 32)
    ) {
      return new Error('Invalid provider KYB revoke reason hash.');
    }

    const key = `prov/${this.value.provider}`;
    const provider = await this.get(key);
    if (!provider) return new Error('Provider not found.');
    const current = await this.get(`kyb/${this.value.provider}`);
    if (!current || current.status !== 'verified') return new Error('Active provider KYB not found.');

    const revoked = {
      ...current,
      status: 'revoked',
      revoked_at: this.tx,
      revoked_by: this.address,
      revoked_by_role: 'admin',
      revoke_reason_hash: this.value.reason_hash ?? null,
      updated_at: this.tx,
    };
    const updatedProvider = {
      ...provider,
      kyb: {
        ...(provider.kyb ?? {}),
        status: 'revoked',
        revoked_at: this.tx,
        revoked_by: this.address,
        revoked_by_role: 'admin',
        revoke_reason_hash: this.value.reason_hash ?? null,
      },
      updated_at: this.tx,
    };

    await this.put(`kyb/${this.value.provider}`, revoked);
    await this.put(key, updatedProvider);
    const kybBanIndexError = await this.writeProviderKybBanIndexes(revoked, {
      status: 'revoked',
      source: 'revoke_provider_kyb',
      provider: this.value.provider,
      reason_hash: this.value.reason_hash ?? null,
      recorded_at: this.tx,
      recorded_by: this.address,
      recorded_by_role: 'admin',
    });
    if (kybBanIndexError instanceof Error) return kybBanIndexError;
    console.log('mayhem revokeProviderKyb', revoked);
    return {
      ok: true,
      op: 'revokeProviderKyb',
      provider: this.value.provider,
    };
  }

  async banProvider() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.provider)) return new Error('Invalid provider id.');
    if (this.value.device_key !== undefined && !this.isHexBytes(this.value.device_key, 32)) {
      return new Error('Invalid provider device key.');
    }
    if (this.value.hardware_fingerprint !== undefined && !this.isHexBytes(this.value.hardware_fingerprint, 32)) {
      return new Error('Invalid provider hardware fingerprint.');
    }

    const key = `prov/${this.value.provider}`;
    const record = await this.get(key);
    if (!record) return new Error('Provider not found.');
    if (record.status === 'banned') return new Error('Provider already banned.');

    const tombstones = await this.tombstoneProviderEnclaves(
      this.value.provider,
      this.providerActiveEnclaves(record),
      this.value.reason_hash ?? null
    );
    if (tombstones instanceof Error) return tombstones;

    const updated = {
      ...record,
      status: 'banned',
      enclaves: [],
      tombstoned_enclaves: tombstones.map((tombstone) => tombstone.enclave_id),
      banned_at: this.tx,
      banned_by: this.address,
      banned_by_role: 'admin',
      ban_reason_hash: this.value.reason_hash ?? null,
      updated_at: this.tx,
    };
    const providerBan = {
      target_type: 'provider',
      target: this.value.provider,
      provider: this.value.provider,
      status: 'banned',
      reason_hash: this.value.reason_hash ?? null,
      banned_at: this.tx,
      banned_by: this.address,
      banned_by_role: 'admin',
      reversible: true,
    };
    const deviceKey = this.value.device_key ?? record.device_key ?? null;
    const fingerprint = this.value.hardware_fingerprint ?? record.hardware_fingerprint ?? null;
    await this.put(key, updated);
    await this.put(`ban/provider/${this.value.provider}`, providerBan);
    if (deviceKey) {
      await this.put(`ban/device/${deviceKey}`, {
        target_type: 'device',
        target: deviceKey,
        provider: this.value.provider,
        status: 'banned',
        reason_hash: this.value.reason_hash ?? null,
        banned_at: this.tx,
        banned_by: this.address,
        banned_by_role: 'admin',
        reversible: true,
      });
    }
    if (fingerprint) {
      const fpKey = `ban/fingerprint/${fingerprint}`;
      const current = await this.get(fpKey);
      const wallets = {
        ...(current?.wallets ?? {}),
        [this.value.provider]: {
          provider: this.value.provider,
          reason_hash: this.value.reason_hash ?? null,
          banned_at: this.tx,
        },
      };
      await this.put(fpKey, {
        target_type: 'fingerprint',
        target: fingerprint,
        status: 'banned',
        wallets,
        reason_hash: this.value.reason_hash ?? null,
        banned_at: current?.banned_at ?? this.tx,
        updated_at: this.tx,
        banned_by: this.address,
        banned_by_role: 'admin',
        reversible: true,
        auto_reject: true,
      });
    }
    if (record.kyb?.status === 'verified') {
      const kybBanIndexError = await this.writeProviderKybBanIndexes(record.kyb, {
        status: 'banned',
        source: 'ban_provider',
        provider: this.value.provider,
        reason_hash: this.value.reason_hash ?? null,
        recorded_at: this.tx,
        recorded_by: this.address,
        recorded_by_role: 'admin',
      });
      if (kybBanIndexError instanceof Error) return kybBanIndexError;
    }
    console.log('mayhem banProvider', updated);
    return {
      ok: true,
      op: 'banProvider',
      provider: this.value.provider,
      tombstoned_enclaves: updated.tombstoned_enclaves,
      device_key: deviceKey,
      hardware_fingerprint: fingerprint,
    };
  }

  async unban() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateExactCommandValue(
      ['op', 'target_type', 'target', 'reason_hash'],
      'unban'
    );
    if (shapeError) return shapeError;
    const targetType = String(this.value.target_type).toLowerCase();
    if (!BAN_TARGET_TYPES.has(targetType)) return new Error('Unsupported ban target type.');
    if (!this.isHexBytes(this.value.target, 32)) return new Error('Invalid ban target.');
    if (!this.isHexBytes(this.value.reason_hash, 32)) return new Error('Invalid unban reason hash.');

    if (targetType === 'provider') {
      const provider = await this.get(`prov/${this.value.target}`);
      if (!provider) return new Error('Provider not found.');
      if (provider.status === 'banned') {
        await this.put(`prov/${this.value.target}`, {
          ...provider,
          status: 'active',
          unbanned_at: this.tx,
          unbanned_by: this.address,
          unbanned_by_role: 'admin',
          unban_reason_hash: this.value.reason_hash,
          updated_at: this.tx,
        });
      }
    }

    const key = this.banRecordKey(targetType, this.value.target);
    const current = await this.get(key);
    const record = {
      ...(current ?? {}),
      target_type: targetType,
      target: this.value.target,
      status: 'unbanned',
      unbanned_at: this.tx,
      unbanned_by: this.address,
      unbanned_by_role: 'admin',
      unban_reason_hash: this.value.reason_hash,
      reversible: true,
    };
    await this.put(key, record);
    console.log('mayhem unban', record);
    return {
      ok: true,
      op: 'unban',
      target_type: targetType,
      target: this.value.target,
    };
  }

  async deviceRebind() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateExactCommandValue(
      ['op', 'device_key', 'provider', 'reason_hash'],
      'device_rebind'
    );
    if (shapeError) return shapeError;
    if (!this.isHexBytes(this.value.device_key, 32)) return new Error('Invalid device key.');
    if (!this.isHexBytes(this.value.provider, 32)) return new Error('Invalid provider id.');
    if (!this.isHexBytes(this.value.reason_hash, 32)) return new Error('Invalid rebind reason hash.');
    const provider = await this.get(`prov/${this.value.provider}`);
    if (!provider || provider.status !== 'active') return new Error('Active provider not found.');
    const deviceBan = await this.get(`ban/device/${this.value.device_key}`);
    if (deviceBan?.status === 'banned') return new Error('Device key is banned.');
    const current = await this.get(`device/${this.value.device_key}`);
    const record = {
      ...(current ?? {}),
      device_key: this.value.device_key,
      provider: this.value.provider,
      status: 'active',
      rebound_at: this.tx,
      rebound_by: this.address,
      rebound_by_role: 'admin',
      rebind_reason_hash: this.value.reason_hash,
      previous_provider: current?.provider ?? null,
    };
    await this.put(`device/${this.value.device_key}`, record);
    await this.put(`prov/${this.value.provider}`, {
      ...provider,
      device_key: this.value.device_key,
      device_key_bound_at: this.tx,
      updated_at: this.tx,
    });
    console.log('mayhem deviceRebind', record);
    return {
      ok: true,
      op: 'deviceRebind',
      device_key: this.value.device_key,
      provider: this.value.provider,
    };
  }

  async setModelRef() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const validationError = this.validateModelRef(this.value);
    if (validationError) return validationError;

    const key = `modelref/${this.value.model_id}`;
    const current = await this.get(key);
    const record = {
      model_id: this.value.model_id,
      model_class: this.modelClassFor(this.value),
      denom: PRICE_DENOMINATION,
      rate_map: this.normalizeRateMap(this.value.rate_map),
      ver: (current?.ver ?? 0) + 1,
      source_hash: this.value.source_hash ?? null,
      updated_at: this.tx,
      set_by: this.address,
      set_by_role: 'admin',
    };
    await this.put(key, record);
    console.log('mayhem setModelRef', record);
    return { ok: true, op: 'setModelRef', model_id: record.model_id, ver: record.ver };
  }

  async publishCatalog() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const validationError = this.validateCatalogRelease(this.value);
    if (validationError) return validationError;

    const current = await this.get('catalog/current');
    const record = {
      catalog_id: this.value.catalog_id,
      source_kind: this.value.source_kind,
      catalog_url: this.value.catalog_url,
      signature_url: this.value.signature_url,
      catalog_hash: this.value.catalog_hash,
      signature_hash: this.value.signature_hash,
      key_id: this.value.key_id,
      public_key: this.value.public_key,
      model_count: this.value.model_count,
      artifact_count: this.value.artifact_count,
      canaries: cloneValue(this.value.canaries),
      ver: (current?.ver ?? 0) + 1,
      supersedes: current?.catalog_hash ?? null,
      status: 'active',
      published_at: this.tx,
      published_by: this.address,
      published_by_role: 'admin',
    };
    await this.put(`catalog/release/${record.catalog_hash}`, record);
    await this.put('catalog/current', record);
    console.log('mayhem publishCatalog', record);
    return {
      ok: true,
      op: 'publishCatalog',
      catalog_hash: record.catalog_hash,
      ver: record.ver,
    };
  }

  async registerEnclave() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');
    if (!this.isSafeModelId(this.value.model_id)) return new Error('Invalid model id.');

    const key = `enclave/${this.value.enclave_id}`;
    if ((await this.get(key)) !== null) return new Error('Enclave already registered.');
    const modelClass = this.modelClassFor(this.value);
    const classError = this.validateModelClass(modelClass, 'Enclave model_class');
    if (classError) return classError;
    const tierError = this.validateLaunchEnclaveAttestationTier(this.value.att_tier);
    if (tierError) return tierError;
    const capsError = this.validateEnclaveCaps(this.value.caps, modelClass);
    if (capsError) return capsError;
    const artifactError = this.validateEnclaveArtifactBinding(this.value);
    if (artifactError) return artifactError;
    const launchMeasurements = this.normalizeEnclaveLaunchMeasurements(this.value.launch_measurements);
    const measurementsError = this.validateEnclaveLaunchMeasurements(launchMeasurements, this.value.att_tier);
    if (measurementsError) return measurementsError;
    const quant = this.normalizeEnclaveQuant(this.value.quant ?? 'unknown');
    const quantError = this.validateEnclaveQuant(quant);
    if (quantError) return quantError;

    const record = {
      enclave_id: this.value.enclave_id,
      model_id: this.value.model_id,
      model_class: modelClass,
      backend: this.value.backend,
      artifact_root: this.value.artifact_root,
      artifact_root_kind: this.value.artifact_root_kind,
      artifact_source: cloneValue(this.value.artifact_source),
      artifact_sidecars: cloneValue(this.value.artifact_sidecars ?? {}),
      source_sha256: this.value.source_sha256 ?? null,
      manifest_hash: this.value.manifest_hash,
      att_tier: this.value.att_tier,
      min_att_tier: this.value.att_tier,
      pending_min_att_tier: null,
      quant,
      binary_hash: this.value.binary_hash,
      launch_measurements: launchMeasurements,
      caps: cloneValue(this.value.caps),
      status: 'active',
      providers: [],
      created_by: this.address,
      created_by_role: 'admin',
      registered_at: this.tx,
      updated_at: this.tx,
      retired_at: null,
    };
    await this.put(key, record);
    console.log('mayhem registerEnclave', record);
    return { ok: true, op: 'registerEnclave', enclave_id: record.enclave_id };
  }

  async updateEnclave() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');

    const key = `enclave/${this.value.enclave_id}`;
    const record = await this.get(key);
    if (!record) return new Error('Enclave not found.');
    if (record.status === 'retired') return new Error('Enclave is retired.');

    let changed = false;
    const updated = cloneValue(record);
    for (const field of ENCLAVE_UPDATE_FIELDS) {
      if (!hasOwn(this.value, field)) continue;
      updated[field] = cloneValue(this.value[field]);
      changed = true;
    }
    if (!changed) return new Error('No enclave fields to update.');
    if (hasOwn(this.value, 'quant')) {
      updated.quant = this.normalizeEnclaveQuant(updated.quant);
    }
    const modelClass = this.modelClassFor(updated);
    const classError = this.validateModelClass(modelClass, 'Enclave model_class');
    if (classError) return classError;
    const tierError = this.validateLaunchEnclaveAttestationTier(updated.att_tier);
    if (tierError) return tierError;
    const capsError = this.validateEnclaveCaps(updated.caps, modelClass);
    if (capsError) return capsError;
    const artifactError = this.validateEnclaveArtifactBinding(updated);
    if (artifactError) return artifactError;
    updated.launch_measurements = this.normalizeEnclaveLaunchMeasurements(updated.launch_measurements);
    const measurementsError = this.validateEnclaveLaunchMeasurements(updated.launch_measurements, updated.att_tier);
    if (measurementsError) return measurementsError;
    const quantError = this.validateEnclaveQuant(updated.quant ?? 'unknown');
    if (quantError) return quantError;
    updated.quant = this.normalizeEnclaveQuant(updated.quant ?? 'unknown');

    updated.updated_by = this.address;
    updated.updated_by_role = 'admin';
    updated.updated_at = this.tx;
    await this.put(key, updated);
    console.log('mayhem updateEnclave', updated);
    return { ok: true, op: 'updateEnclave', enclave_id: updated.enclave_id };
  }

  async setEnclaveMinTier() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateExactCommandValue(
      [
        'op',
        'enclave_id',
        'min_att_tier',
        'submitted_epoch',
        'effective_epoch',
        'submitted_at',
        'reason_hash',
      ],
      'set_enclave_min_tier'
    );
    if (shapeError) return shapeError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');
    if (!this.isHexBytes(this.value.reason_hash, 32)) return new Error('Invalid min-tier reason hash.');

    const tierError = this.validateLaunchEnclaveAttestationTier(this.value.min_att_tier);
    if (tierError) return tierError;
    if (this.value.effective_epoch <= this.value.submitted_epoch) {
      return new Error('Enclave min-tier effective_epoch must be after submitted_epoch.');
    }

    const enclaveKey = `enclave/${this.value.enclave_id}`;
    const enclave = await this.get(enclaveKey);
    if (!enclave) return new Error('Enclave not found.');
    if (enclave.status !== 'active') return new Error('Enclave is not active.');
    const enclaveRoleError = this.requireAdminCreatedEnclave(enclave);
    if (enclaveRoleError) return enclaveRoleError;

    const params = await this.activeParamsAt(this.value.submitted_at, ['min_tier_notice_epochs']);
    if (
      this.value.effective_epoch - this.value.submitted_epoch <
      params.min_tier_notice_epochs
    ) {
      return new Error('Enclave min-tier changes require at least min_tier_notice_epochs notice.');
    }

    const currentEpoch = await this.currentAppliedEpoch();
    const activePolicy = await this.enclaveMinTierPolicy(enclave, currentEpoch);
    const pending = {
      min_att_tier: this.value.min_att_tier,
      previous_min_att_tier: activePolicy.min_att_tier,
      submitted_epoch: this.value.submitted_epoch,
      effective_epoch: this.value.effective_epoch,
      submitted_at: this.value.submitted_at,
      reason_hash: this.value.reason_hash,
      scheduled_at: this.tx,
      scheduled_by: this.address,
      scheduled_by_role: 'admin',
    };
    const record = {
      enclave_id: this.value.enclave_id,
      current_min_att_tier: activePolicy.min_att_tier,
      current_epoch: currentEpoch,
      pending,
      updated_at: this.tx,
      updated_by: this.address,
      updated_by_role: 'admin',
    };
    await this.put(`tierpolicy/enclave/${this.value.enclave_id}`, record);
    await this.put(enclaveKey, {
      ...enclave,
      min_att_tier: activePolicy.min_att_tier,
      pending_min_att_tier: pending,
      updated_at: this.tx,
      updated_by: this.address,
      updated_by_role: 'admin',
    });
    console.log('mayhem setEnclaveMinTier', record);
    return {
      ok: true,
      op: 'setEnclaveMinTier',
      enclave_id: this.value.enclave_id,
      min_att_tier: this.value.min_att_tier,
      effective_epoch: this.value.effective_epoch,
    };
  }

  async retireEnclave() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');

    const key = `enclave/${this.value.enclave_id}`;
    const record = await this.get(key);
    if (!record) return new Error('Enclave not found.');
    if (record.status === 'retired') return new Error('Enclave already retired.');

    const activeProviders = this.enclaveActiveProviders(record);
    const tombstones = await this.tombstoneEnclaveProviders(
      this.value.enclave_id,
      activeProviders,
      null
    );
    if (tombstones instanceof Error) return tombstones;

    const updated = {
      ...record,
      status: 'retired',
      providers: [],
      tombstoned_providers: tombstones
        .filter((tombstone) => tombstone.serve_tombstoned)
        .map((tombstone) => tombstone.provider),
      retired_at: this.tx,
      retired_by: this.address,
      retired_by_role: 'admin',
      updated_by: this.address,
      updated_by_role: 'admin',
      updated_at: this.tx,
    };
    await this.put(key, updated);
    console.log('mayhem retireEnclave', updated);
    return {
      ok: true,
      op: 'retireEnclave',
      enclave_id: updated.enclave_id,
      tombstoned_providers: updated.tombstoned_providers,
    };
  }

  async joinEnclave() {
    const shapeError = this.validateExactCommandValue(
      ['op', 'enclave_id'],
      'join_enclave',
      ['hardware_fingerprint', 'device_key', 'served_ctx', 'ctx_bracket', 'ctx_bracket_table_ver']
    );
    if (shapeError) return shapeError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');
    const hasServeTerms =
      hasOwn(this.value, 'served_ctx') ||
      hasOwn(this.value, 'ctx_bracket') ||
      hasOwn(this.value, 'ctx_bracket_table_ver');

    return this.applyJoinEnclave(
      this.address,
      this.value.enclave_id,
      this.tx,
      this.value.hardware_fingerprint ?? null,
      this.value.device_key ?? null,
      hasServeTerms
        ? {
            served_ctx: this.value.served_ctx,
            ctx_bracket: this.value.ctx_bracket,
            ctx_bracket_table_ver: this.value.ctx_bracket_table_ver,
          }
        : null
    );
  }

  async applyJoinEnclave(
    providerId,
    enclaveId,
    stamp,
    hardwareFingerprint = null,
    deviceKey = null,
    serveTerms = null
  ) {
    const consentError = await this.requireConsent(providerId);
    if (consentError) return consentError;
    const providerError = await this.requireProvider(providerId);
    if (providerError) return providerError;
    if (hardwareFingerprint !== null && !this.isHexBytes(hardwareFingerprint, 32)) {
      return new Error('Invalid provider hardware fingerprint.');
    }
    if (deviceKey !== null && !this.isHexBytes(deviceKey, 32)) {
      return new Error('Invalid provider device key.');
    }

    const enclave = await this.get(`enclave/${enclaveId}`);
    if (!enclave) return new Error('Enclave not found.');
    if (enclave.status !== 'active') return new Error('Enclave is not active.');
    const enclaveRoleError = this.requireAdminCreatedEnclave(enclave);
    if (enclaveRoleError) return enclaveRoleError;
    const normalizedServeTerms = await this.normalizeProviderServeTerms(
      enclaveId,
      serveTerms,
      'provider serve'
    );
    if (normalizedServeTerms instanceof Error) return normalizedServeTerms;
    const priceError = await this.requireCurrentAdminPrice(
      enclaveId,
      normalizedServeTerms?.ctx_bracket ?? null
    );
    if (priceError) return priceError;
    const minTierPolicy = await this.enclaveMinTierPolicy(enclave);
    if (enclave.att_tier < minTierPolicy.min_att_tier) {
      return new Error(
        `Enclave now requires minimum attestation tier ${minTierPolicy.min_att_tier}; lower-tier providers finish in-flight work and held earnings settle on the normal schedule.`
      );
    }

    const key = `serve/${providerId}/${enclaveId}`;
    const existing = await this.get(key);
    if (existing && existing.status === 'active') return new Error('Provider already serving enclave.');
    const provider = await this.get(`prov/${providerId}`);
    if (!provider || provider.status !== 'active') return new Error('Provider registration required.');
    if (deviceKey !== null) {
      const deviceBan = await this.get(`ban/device/${deviceKey}`);
      if (deviceBan?.status === 'banned') return new Error('Provider device key is banned.');
      const binding = await this.get(`device/${deviceKey}`);
      if (binding?.provider && binding.provider !== providerId) {
        return new Error('Provider device key is bound to a different wallet; admin rebind required.');
      }
    }

    if (hardwareFingerprint !== null) {
      const fingerprintBan = await this.get(`ban/fingerprint/${hardwareFingerprint}`);
      if (fingerprintBan?.status === 'banned') {
        return new Error('Provider hardware fingerprint is banned.');
      }
    }

    const record = {
      provider: providerId,
      enclave_id: enclaveId,
      model_id: enclave.model_id,
      status: 'active',
      ...(normalizedServeTerms ?? {}),
      ...(hardwareFingerprint !== null ? { hardware_fingerprint: hardwareFingerprint } : {}),
      ...(deviceKey !== null ? { device_key: deviceKey } : {}),
      joined_at: existing?.joined_at ?? stamp,
      updated_at: stamp,
      left_at: null,
      rooms: Array.isArray(existing?.rooms) ? existing.rooms.slice() : [],
    };
    await this.put(key, record);
    await this.put(`enclave/${enclaveId}`, {
      ...enclave,
      providers: this.enclaveProvidersWith(enclave, providerId),
      updated_at: stamp,
    });
    if (deviceKey !== null) {
      const currentDevice = await this.get(`device/${deviceKey}`);
      await this.put(`device/${deviceKey}`, {
        ...(currentDevice ?? {}),
        device_key: deviceKey,
        provider: providerId,
        status: 'active',
        bound_at: stamp,
        updated_at: stamp,
      });
    }
    await this.put(`prov/${providerId}`, {
      ...provider,
      enclaves: this.providerEnclavesWith(provider, enclaveId),
      ...(hardwareFingerprint !== null ? { hardware_fingerprint: hardwareFingerprint } : {}),
      ...(deviceKey !== null ? { device_key: deviceKey, device_key_bound_at: stamp } : {}),
      updated_at: stamp,
    });
    console.log('mayhem joinEnclave', record);
    return { ok: true, op: 'joinEnclave', provider: providerId, enclave_id: enclaveId };
  }

  async leaveEnclave() {
    const shapeError = this.validateExactCommandValue(['op', 'enclave_id'], 'leave_enclave');
    if (shapeError) return shapeError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');

    return this.applyLeaveEnclave(this.address, this.value.enclave_id, this.tx);
  }

  async applyLeaveEnclave(providerId, enclaveId, stamp) {
    const key = `serve/${providerId}/${enclaveId}`;
    const record = await this.get(key);
    if (!record || record.status !== 'active') return new Error('Provider is not serving enclave.');
    if (Array.isArray(record.rooms) && record.rooms.length > 0) {
      return new Error('Provider must leave rooms before leaving enclave.');
    }

    const updated = {
      ...record,
      status: 'inactive',
      updated_at: stamp,
      left_at: stamp,
    };
    await this.put(key, updated);
    const provider = await this.get(`prov/${providerId}`);
    if (provider) {
      await this.put(`prov/${providerId}`, {
        ...provider,
        enclaves: this.providerEnclavesWithout(provider, enclaveId),
        updated_at: stamp,
      });
    }
    const enclave = await this.get(`enclave/${enclaveId}`);
    if (enclave) {
      await this.put(`enclave/${enclaveId}`, {
        ...enclave,
        providers: this.enclaveProvidersWithout(enclave, providerId),
        updated_at: stamp,
      });
    }
    console.log('mayhem leaveEnclave', updated);
    return { ok: true, op: 'leaveEnclave', provider: providerId, enclave_id: enclaveId };
  }

  async joinRoom() {
    const shapeError = this.validateExactCommandValue(
      ['op', 'room_id', 'enclave_id'],
      'join_room'
    );
    if (shapeError) return shapeError;
    if (!this.isSafeKeyPart(this.value.room_id)) return new Error('Invalid room id.');
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');

    return this.applyJoinRoom(this.address, this.value.room_id, this.value.enclave_id, this.tx);
  }

  async applyJoinRoom(providerId, roomId, enclaveId, stamp) {
    const consentError = await this.requireConsent(providerId);
    if (consentError) return consentError;
    const providerError = await this.requireProvider(providerId);
    if (providerError) return providerError;

    const room = await this.get(`room/${roomId}`);
    if (!room) return new Error('Room not found.');
    if (room.status !== 'open') return new Error('Room is not open.');

    const serving = await this.get(`serve/${providerId}/${enclaveId}`);
    if (!serving || serving.status !== 'active') return new Error('Provider is not serving enclave.');
    const enclave = await this.get(`enclave/${enclaveId}`);
    if (!enclave || enclave.status !== 'active') return new Error('Enclave is not active.');
    const enclaveRoleError = this.requireAdminCreatedEnclave(enclave);
    if (enclaveRoleError) return enclaveRoleError;
    const roomRoleError = this.requireAdminCreatedRoom(room);
    if (roomRoleError) return roomRoleError;
    const priceError = await this.requireCurrentAdminPrice(enclaveId, serving.ctx_bracket ?? null);
    if (priceError) return priceError;
    if (room.enclave_id !== enclaveId) {
      return new Error('Room enclave does not match served enclave.');
    }
    if (serving.model_id !== enclave.model_id || enclave.model_id !== room.model_id) {
      return new Error('Enclave model does not match room model.');
    }

    const key = `roomserve/${roomId}/${providerId}/${enclaveId}`;
    const existing = await this.get(key);
    if (existing && existing.status === 'active') return new Error('Provider already joined room with enclave.');

    const rooms = Array.isArray(serving.rooms) ? serving.rooms.slice() : [];
    if (!rooms.includes(roomId)) rooms.push(roomId);
    rooms.sort();
    const record = {
      room_id: roomId,
      sidechannel: room.sidechannel,
      provider: providerId,
      enclave_id: enclaveId,
      model_id: enclave.model_id,
      status: 'active',
      joined_at: existing?.joined_at ?? stamp,
      updated_at: stamp,
      left_at: null,
    };
    await this.put(key, record);
    await this.put(`serve/${providerId}/${enclaveId}`, {
      ...serving,
      rooms,
      updated_at: stamp,
    });
    await this.put(`room/${roomId}`, {
      ...room,
      serves: this.roomServesWith(room, providerId, enclaveId),
      serves_updated_at: stamp,
    });
    console.log('mayhem joinRoom', record);
    return {
      ok: true,
      op: 'joinRoom',
      room_id: roomId,
      provider: providerId,
      enclave_id: enclaveId,
      sidechannel: room.sidechannel,
    };
  }

  async leaveRoom() {
    const shapeError = this.validateExactCommandValue(
      ['op', 'room_id', 'enclave_id'],
      'leave_room'
    );
    if (shapeError) return shapeError;
    if (!this.isSafeKeyPart(this.value.room_id)) return new Error('Invalid room id.');
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');

    return this.applyLeaveRoom(this.address, this.value.room_id, this.value.enclave_id, this.tx);
  }

  async applyLeaveRoom(providerId, roomId, enclaveId, stamp) {
    const key = `roomserve/${roomId}/${providerId}/${enclaveId}`;
    const record = await this.get(key);
    if (!record) return new Error('Provider has not joined room with enclave.');
    if (record.status !== 'active') {
      return {
        ok: true,
        op: 'leaveRoom',
        room_id: roomId,
        provider: providerId,
        enclave_id: enclaveId,
        sidechannel: record.sidechannel,
        status: record.status,
        idempotent: true,
      };
    }

    const servingKey = `serve/${providerId}/${enclaveId}`;
    const serving = await this.get(servingKey);
    const rooms = Array.isArray(serving?.rooms)
      ? serving.rooms.filter((servingRoomId) => servingRoomId !== roomId)
      : [];
    const roomKey = `room/${roomId}`;
    const room = await this.get(roomKey);
    const updated = {
      ...record,
      status: 'inactive',
      updated_at: stamp,
      left_at: stamp,
    };
    await this.put(key, updated);
    if (serving) {
      await this.put(servingKey, {
        ...serving,
        rooms,
        updated_at: stamp,
      });
    }
    if (room) {
      await this.put(roomKey, {
        ...room,
        serves: this.roomServesWithout(room, providerId, enclaveId),
        serves_updated_at: stamp,
      });
    }
    console.log('mayhem leaveRoom', updated);
    return {
      ok: true,
      op: 'leaveRoom',
      room_id: roomId,
      provider: providerId,
      enclave_id: enclaveId,
      sidechannel: updated.sidechannel,
    };
  }

  async openRoom() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');
    if (this.value.model_id && !this.isSafeModelId(this.value.model_id)) {
      return new Error('Invalid model id.');
    }

    const policyError = this.validateRoomPolicy(this.value.policy);
    if (policyError) return policyError;

    let recordModelId = this.value.model_id;
    const enclave = await this.get(`enclave/${this.value.enclave_id}`);
    if (!enclave) return new Error('Enclave not found.');
    if (enclave.status !== 'active') return new Error('Enclave is not active.');
    const enclaveRoleError = this.requireAdminCreatedEnclave(enclave);
    if (enclaveRoleError) return enclaveRoleError;
    if (this.value.model_id && this.value.model_id !== enclave.model_id) {
      return new Error('Room model does not match enclave model.');
    }
    recordModelId = enclave.model_id;

    const roomId = await deriveRoomId(this.value.enclave_id, this.address, this.value.nonce);
    const key = `room/${roomId}`;
    const existing = await this.get(key);
    if (existing && existing.status !== 'closed') return new Error('Room already open.');

    const record = {
      room_id: roomId,
      sidechannel: roomSidechannelName(roomId),
      enclave_id: this.value.enclave_id,
      model_id: recordModelId,
      label: this.value.label,
      creator: this.address,
      creator_role: 'admin',
      policy: cloneValue(this.value.policy),
      serves: [],
      serves_updated_at: null,
      created_at: this.tx,
      updated_at: this.tx,
      closed_at: null,
      status: 'open',
    };
    await this.put(key, record);
    console.log('mayhem openRoom', record);
    return { ok: true, op: 'openRoom', room_id: roomId, sidechannel: record.sidechannel };
  }

  async closeRoom() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.room_id)) return new Error('Invalid room id.');

    const key = `room/${this.value.room_id}`;
    const record = await this.get(key);
    if (!record) return new Error('Room not found.');
    if (record.status === 'closed') return new Error('Room already closed.');

    const tombstones = await this.tombstoneRoomServes(
      this.value.room_id,
      this.roomServingEntries(record),
      null
    );
    if (tombstones instanceof Error) return tombstones;
    const current = (await this.get(key)) ?? record;
    const updated = {
      ...current,
      status: 'closed',
      serves: [],
      serves_updated_at: this.tx,
      tombstoned_serves: tombstones
        .filter((tombstone) => tombstone.roomserve_tombstoned)
        .map(({ provider, enclave_id }) => ({ provider, enclave_id })),
      updated_at: this.tx,
      closed_at: this.tx,
      closed_by: this.address,
      closed_by_role: 'admin',
    };
    await this.put(key, updated);
    console.log('mayhem closeRoom', updated);
    return {
      ok: true,
      op: 'closeRoom',
      room_id: updated.room_id,
      sidechannel: updated.sidechannel,
      tombstoned_serves: updated.tombstoned_serves,
    };
  }

  async setPrice() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');

    const enclave = await this.get(`enclave/${this.value.enclave_id}`);
    if (!enclave) return new Error('Enclave not found.');
    if (enclave.status === 'retired') return new Error('Enclave is retired.');

    const modelRef = await this.get(`modelref/${enclave.model_id}`);
    if (!modelRef) return new Error('Model reference not found.');
    const enclaveClass = this.modelClassFor(enclave);
    const enclaveClassError = this.validateModelClass(enclaveClass, 'Enclave model_class');
    if (enclaveClassError) return enclaveClassError;
    const modelRefClass = this.modelClassFor(modelRef);
    const modelRefClassError = this.validateModelClass(modelRefClass, 'Model reference model_class');
    if (modelRefClassError) return modelRefClassError;
    if (modelRefClass !== enclaveClass) {
      return new Error('Model reference model_class must match enclave model_class.');
    }

    const rateError = this.validateRateMap(this.value.rate_map, enclaveClass, 'Enclave price rate_map', {
      allowZeroPrice: true,
    });
    if (rateError) return rateError;
    const priceRateMap = this.normalizeRateMap(this.value.rate_map);
    const perReqAu = this.normalizeAu(this.value.per_req_au, 'Enclave price per_req_au');
    if (perReqAu instanceof Error) return perReqAu;
    const minSessionAu = this.normalizeAu(this.value.min_session_au, 'Enclave price min_session_au');
    if (minSessionAu instanceof Error) return minSessionAu;
    const params = await this.activeParamsAt(this.value.effective_at, [
      'price_min_bps',
      'price_max_bps',
      'price_rate_limit_seconds',
    ]);
    const boundsError = this.validateRateMapBounds(priceRateMap, modelRef.rate_map, params);
    if (boundsError) return boundsError;

    const ctxMeta = await this.priceCtxMetaForEnclave(enclave, this.value.ctx_bracket, this.value.effective_at, 'Enclave price');
    if (ctxMeta instanceof Error) return ctxMeta;
    const key = this.priceScheduleKey(this.value.enclave_id, ctxMeta?.ctx_bracket ?? null);
    const schedule = await this.priceSchedule(key, enclave, ctxMeta);
    const latest = this.priceLatestEntry(schedule);
    const latestSeed = this.priceLatestSeedEntry(schedule);
    if (
      latestSeed &&
      this.value.effective_at - latestSeed.effective_at < params.price_rate_limit_seconds
    ) {
      return new Error('Price seed changes are limited by price_rate_limit_seconds.');
    }

    const record = {
      enclave_id: this.value.enclave_id,
      model_id: enclave.model_id,
      denom: PRICE_DENOMINATION,
      ver: latest ? latest.ver + 1 : 1,
      rate_map: priceRateMap,
      per_req_au: perReqAu,
      min_session_au: minSessionAu,
      effective_at: this.value.effective_at,
      effective_from: this.tx,
      updated_at: this.tx,
      set_by: this.address,
      set_by_role: 'admin',
      ...(ctxMeta ? {
        ctx_bracket: ctxMeta.ctx_bracket,
        ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
      } : {}),
    };

    const updated = {
      enclave_id: this.value.enclave_id,
      model_id: enclave.model_id,
      denom: PRICE_DENOMINATION,
      ...(ctxMeta ? {
        ctx_bracket: ctxMeta.ctx_bracket,
        ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
      } : {}),
      current: schedule.current,
      pending: schedule.pending,
    };
    if (!updated.current) {
      updated.current = record;
    } else {
      if (updated.pending && updated.pending.effective_at <= this.value.effective_at) {
        updated.current = updated.pending;
        updated.pending = null;
      }
      if (updated.pending) return new Error('Pending price change already scheduled.');
      updated.pending = record;
    }

    await this.put(key, updated);
    await this.put(this.priceRecordKey(this.value.enclave_id, record.ver, ctxMeta?.ctx_bracket ?? null), record);
    console.log('mayhem setPrice', { schedule: updated, record });
    return {
      ok: true,
      op: 'setPrice',
      enclave_id: record.enclave_id,
      ver: record.ver,
    };
  }

  async readPrice() {
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');
    const enclave = await this.get(`enclave/${this.value.enclave_id}`);
    if (!enclave) return new Error('Enclave not found.');
    const ctxMeta = await this.priceCtxMetaForEnclave(enclave, this.value.ctx_bracket, this.value.at, 'Enclave price');
    if (ctxMeta instanceof Error) return ctxMeta;
    const key = this.priceScheduleKey(this.value.enclave_id, ctxMeta?.ctx_bracket ?? null);
    const schedule = await this.get(key);
    if (!schedule) {
      return {
        ok: true,
        op: 'readPrice',
        enclave_id: this.value.enclave_id,
        at: this.value.at,
        ...(ctxMeta ? {
          ctx_bracket: ctxMeta.ctx_bracket,
          ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
        } : {}),
        price: null,
      };
    }

    const price = this.priceActiveEntry(schedule, this.value.at);
    console.log('mayhem readPrice', { enclave_id: this.value.enclave_id, at: this.value.at, price });
    return {
      ok: true,
      op: 'readPrice',
      enclave_id: this.value.enclave_id,
      at: this.value.at,
      ...(ctxMeta ? {
        ctx_bracket: ctxMeta.ctx_bracket,
        ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
      } : {}),
      price,
    };
  }

  async recordReputationEvent() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const validationError = this.validateReputationEvent(this.value);
    if (validationError) return validationError;

    const provider = await this.get(`prov/${this.value.provider}`);
    if (!provider) return new Error('Provider not found.');

    const record = await this.appendReputationEvent({
      provider: this.value.provider,
      event_id: this.value.event_id,
      kind: this.value.kind,
      epoch: this.value.epoch,
      at: this.value.at,
      paid_au: this.value.paid_au !== undefined
        ? this.normalizeAu(this.value.paid_au, 'reputation paid amount')
        : null,
      max_spend_au: this.value.max_spend_au !== undefined
        ? this.normalizeAu(this.value.max_spend_au, 'reputation max spend')
        : null,
      evidence_hash: this.value.evidence_hash ?? null,
    });
    if (record instanceof Error) return record;

    let slash = null;
    if (this.value.kind === 'dispute_lost') {
      const params = await this.activeParamsAt(this.value.at, ['dispute_lost_slash_bps']);
      slash = await this.applyProviderSlash({
        providerId: this.value.provider,
        source: 'dispute',
        reason: 'dispute_lost',
        evidenceHash: this.value.evidence_hash ?? null,
        epoch: this.value.epoch,
        at: this.value.at,
        slashBps: params.dispute_lost_slash_bps,
        beneficiary: this.value.beneficiary ?? null,
        enclaveId: this.value.enclave_id ?? null,
        banProvider: false,
        tombstoneEnclave: false,
      });
      if (slash instanceof Error) return slash;
    }

    console.log('mayhem recordReputationEvent', record);
    return {
      ok: true,
      op: 'recordReputationEvent',
      provider: this.value.provider,
      event_id: this.value.event_id,
      head: record.head,
      slash,
    };
  }

  async anchorReputation() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const validationError = this.validateReputationAnchor(this.value);
    if (validationError) return validationError;

    const providerKey = `prov/${this.value.provider}`;
    const provider = await this.get(providerKey);
    if (!provider) return new Error('Provider not found.');

    const head = await this.get(`ev/rep/head/${this.value.provider}`);
    if (!head || head.head !== this.value.events_head) {
      return new Error('Reputation events head mismatch.');
    }
    if (this.value.successful_sessions < (provider.probation?.successful_sessions ?? 0)) {
      return new Error('Successful sessions must not decrease.');
    }

    const params = await this.activeParamsAt(this.value.folded_at, [
      'probation_successful_sessions',
      'probation_seconds',
      'probation_max_concurrent_sessions_per_user',
      'probation_price_max_bps',
      'probation_weight_bps',
    ]);
    const sinceSeconds = provider.probation?.since_seconds ?? 0;
    const probationActive = (
      this.value.successful_sessions < params.probation_successful_sessions ||
      this.value.folded_at - sinceSeconds < params.probation_seconds
    );
    const probation = {
      active: probationActive,
      since: provider.probation?.since ?? provider.registered_at,
      since_seconds: sinceSeconds,
      successful_sessions: this.value.successful_sessions,
      required_successful_sessions: params.probation_successful_sessions,
      required_seconds: params.probation_seconds,
      caps: {
        max_concurrent_sessions_per_user: params.probation_max_concurrent_sessions_per_user,
        price_max_bps: params.probation_price_max_bps,
        weight_bps: params.probation_weight_bps,
      },
    };
    const snapshot = {
      provider: this.value.provider,
      r: this.value.r_bps / 10_000,
      r_bps: this.value.r_bps,
      raw: this.value.raw_milli / 1_000,
      raw_milli: this.value.raw_milli,
      events_head: this.value.events_head,
      epoch: this.value.epoch,
      folded_at: this.value.folded_at,
      updated_at: this.tx,
      probation,
      provenance_violation: this.value.provenance_violation === true,
    };
    const updatedProvider = {
      ...provider,
      probation: {
        ...(provider.probation ?? {}),
        successful_sessions: this.value.successful_sessions,
        since_seconds: sinceSeconds,
      },
      updated_at: this.tx,
    };

    await this.put(`rep/${this.value.provider}`, snapshot);
    await this.put(providerKey, updatedProvider);
    console.log('mayhem anchorReputation', snapshot);
    return {
      ok: true,
      op: 'anchorReputation',
      provider: this.value.provider,
      epoch: this.value.epoch,
      events_head: this.value.events_head,
    };
  }

  async auditorRegister() {
    const target = this.value.auditor ?? this.address;
    if (!this.isSafeKeyPart(target)) return new Error('Invalid auditor id.');

    const consentError = await this.requireConsent(target);
    if (consentError) return consentError;

    const provider = await this.get(`prov/${target}`);
    if (provider) return new Error('Provider keys cannot register as auditors.');

    const adminRegistersOther = target !== this.address;
    if (adminRegistersOther) {
      const adminError = await this.requireAdmin();
      if (adminError) return adminError;
    } else {
      const eligibilityError = await this.requireAuditorEligibility(
        target,
        this.value.registered_at_seconds ?? 0
      );
      if (eligibilityError) return eligibilityError;
    }

    const key = `auditor/${target}`;
    const existing = await this.get(key);
    if (existing?.status === 'active') return new Error('Auditor already registered.');

    const record = {
      auditor: target,
      status: 'active',
      registered_at: this.tx,
      registered_at_seconds: this.value.registered_at_seconds ?? 0,
      accredited_by: adminRegistersOther ? this.address : null,
      successful_probes: existing?.successful_probes ?? 0,
      submitted_probes: existing?.submitted_probes ?? 0,
      false_reports: existing?.false_reports ?? 0,
      updated_at: this.tx,
    };
    await this.put(key, record);
    console.log('mayhem auditorRegister', record);
    return { ok: true, op: 'auditorRegister', auditor: target };
  }

  async probeResult() {
    const auditor = await this.get(`auditor/${this.address}`);
    if (!auditor || auditor.status !== 'active') return new Error('Auditor registration required.');
    if ((await this.get(`prov/${this.address}`)) !== null) {
      return new Error('Provider keys cannot submit auditor probes.');
    }

    const validationError = this.validateProbeResult(this.value);
    if (validationError) return validationError;
    if ((await this.get(`ev/probe/${this.value.probe_id}`)) !== null) {
      return new Error('Probe result already recorded.');
    }

    const providerKey = `prov/${this.value.provider}`;
    const provider = await this.get(providerKey);
    if (!provider) return new Error('Provider not found.');

    if (this.value.probe_kind === 'canary') {
      const canaryBindingError = await this.requireBoundCanaryProbe(this.value, this.address);
      if (canaryBindingError) return canaryBindingError;
    }

    const params = await this.activeParamsAt(this.value.at, [
      'canary_match_min_bps',
      'probe_reward_au',
      'uptime_tick_seconds',
      'fraud_slash_bps',
    ]);
    const pass = this.probePass(this.value, params);
    if (this.value.pass !== undefined && this.value.pass !== pass) {
      return new Error('Probe pass flag does not match contract threshold.');
    }

    const reputationEvent = await this.appendReputationEvent({
      provider: this.value.provider,
      event_id: `probe-${this.value.probe_id}`,
      kind: this.value.probe_kind === 'uptime_tick'
        ? 'uptime_tick'
        : pass
          ? 'probe_ok'
          : 'probe_fail',
      epoch: this.value.epoch,
      at: this.value.at,
      paid_au: null,
      max_spend_au: null,
      evidence_hash: this.value.evidence_hash ?? null,
    });
    if (reputationEvent instanceof Error) return reputationEvent;

    let provenanceViolation = false;
    let slash = null;
    if (this.value.probe_kind === 'canary' && !pass) {
      provenanceViolation = true;
      const violationEvent = await this.appendReputationEvent({
        provider: this.value.provider,
        event_id: `probe-${this.value.probe_id}-violation`,
        kind: 'provenance_violation',
        epoch: this.value.epoch,
        at: this.value.at,
        paid_au: null,
        max_spend_au: null,
        evidence_hash: this.value.evidence_hash ?? null,
      });
      if (violationEvent instanceof Error) return violationEvent;

      slash = await this.applyProviderSlash({
        providerId: this.value.provider,
        source: 'probe',
        reason: 'canary_mismatch',
        evidenceHash: this.value.evidence_hash ?? null,
        epoch: this.value.epoch,
        at: this.value.at,
        slashBps: params.fraud_slash_bps,
        beneficiary: this.address,
        enclaveId: this.value.enclave_id ?? null,
        probeId: this.value.probe_id,
        banProvider: true,
        tombstoneEnclave: true,
      });
      if (slash instanceof Error) return slash;
    }

    const record = {
      probe_id: this.value.probe_id,
      probe_kind: this.value.probe_kind,
      auditor: this.address,
      provider: this.value.provider,
      enclave_id: this.value.enclave_id ?? null,
      epoch: this.value.epoch,
      at: this.value.at,
      canary_set: this.value.canary_set ?? null,
      verification_method: this.value.verification_method ?? null,
      binary_hash: this.value.binary_hash ?? null,
      match_bps: this.value.match_bps ?? null,
      pass,
      session_receipt_hash: this.value.session_receipt_hash ?? null,
      evidence_hash: this.value.evidence_hash ?? null,
      auditor_sig: this.value.auditor_sig ?? null,
      reputation_head: (await this.get(`ev/rep/head/${this.value.provider}`))?.head ?? null,
      provenance_violation: provenanceViolation,
      probe_reward_au: params.probe_reward_au,
      slash,
      recorded_at: this.tx,
    };
    await this.put(`ev/probe/${this.value.probe_id}`, record);
    let probePassRecord = null;
    if (this.value.probe_kind === 'canary' && pass) {
      probePassRecord = await this.recordCanaryProbePass(this.value);
      if (probePassRecord instanceof Error) return probePassRecord;
    }
    await this.put(`auditor/${this.address}`, {
      ...auditor,
      submitted_probes: (auditor.submitted_probes ?? 0) + 1,
      successful_probes: (auditor.successful_probes ?? 0) + (pass ? 1 : 0),
      updated_at: this.tx,
    });
    console.log('mayhem probeResult', record);
    return {
      ok: true,
      op: 'probeResult',
      probe_id: this.value.probe_id,
      provider: this.value.provider,
      pass,
      ...(probePassRecord ? { probe_pass_record: probePassRecord } : {}),
      provenance_violation: provenanceViolation,
    };
  }

  async epochApply() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const shapeError = this.validateEpochApplyShape(this.value);
    if (shapeError) return shapeError;
    const roots = this.value.roots === undefined ? null : this.normalizeEpochRoots(this.value.roots);
    if (roots instanceof Error) return roots;
    const totals = this.value.totals === undefined ? null : this.normalizeEpochTotals(this.value.totals);
    if (totals instanceof Error) return totals;
    if ((roots && !totals) || (!roots && totals)) {
      return new Error('Epoch apply roots and totals must be provided together.');
    }
    const page = this.epochApplyPage(this.value);
    if (page instanceof Error) return page;
    const lastPage = this.epochApplyLastPage(this.value);
    if (lastPage instanceof Error) return lastPage;
    const pagedApply = hasOwn(this.value, 'page');
    if (pagedApply && (roots || totals)) {
      return new Error('Paged epochApply pages must omit aggregate roots and totals.');
    }

    const params = await this.activeParamsAt(this.value.at, [
      'epoch_seconds',
      'fee_bps',
      'max_apply_batch',
      'max_market_usage_entries',
      'probation_successful_sessions',
      'new_provider_holdback_epochs',
      'holdback_epochs',
      'challenge_epochs',
      'probation_successful_sessions',
      'new_provider_holdback_epochs',
      'canary_probe_holdback_bps',
      'canary_probe_release_min_passes',
    ]);
    if (this.value.debits.length + this.value.earnings.length > params.max_apply_batch) {
      return new Error('Epoch apply batch exceeds max_apply_batch.');
    }
    if ((this.value.market_usage?.length ?? 0) > params.max_market_usage_entries) {
      return new Error('Epoch market usage batch exceeds max_market_usage_entries.');
    }

    const debitMap = this.aggregateRailLedgerEntries(this.value.debits, 'user', 'au', 'debit');
    if (debitMap instanceof Error) return debitMap;
    const grossEarningMap = this.aggregateRailLedgerEntries(
      this.value.earnings,
      'provider',
      'gross_au',
      'earning'
    );
    if (grossEarningMap instanceof Error) return grossEarningMap;

    const debitTotal = this.sumRailAu(debitMap, 'au');
    if (debitTotal instanceof Error) return debitTotal;
    const grossTotal = this.sumRailAu(grossEarningMap, 'gross_au');
    if (grossTotal instanceof Error) return grossTotal;
    const debitRailTotals = this.railTotals(debitMap, 'au');
    if (debitRailTotals instanceof Error) return debitRailTotals;
    const grossRailTotals = this.railTotals(grossEarningMap, 'gross_au');
    if (grossRailTotals instanceof Error) return grossRailTotals;
    const railTotalError = this.assertMatchingRailTotals(debitRailTotals, grossRailTotals);
    if (railTotalError) return railTotalError;
    const marketUsageProvided = hasOwn(this.value, 'market_usage');
    const marketUsageMap = marketUsageProvided
      ? this.aggregateMarketUsageEntries(this.value.market_usage)
      : new Map();
    if (marketUsageMap instanceof Error) return marketUsageMap;
    const marketUsageTotal = this.sumMarketDemandAu(marketUsageMap);
    if (marketUsageTotal instanceof Error) return marketUsageTotal;
    if (marketUsageProvided && this.compareAu(marketUsageTotal, grossTotal) !== 0) {
      return new Error('Epoch market usage demand must equal gross provider earnings.');
    }

    const applyState = await this.epochApplyStateRecord();
    const previousApplyHash = page === 0 ? null : applyState.last_apply_hash;
    const normalized = {
      epoch: this.value.epoch,
      page,
      last_page: lastPage,
      at: this.value.at,
      epoch_seconds: params.epoch_seconds,
      fee_bps: params.fee_bps,
      debits: this.mapRailEntriesForHash(debitMap, 'user', 'au'),
      earnings: this.mapRailEntriesForHash(grossEarningMap, 'provider', 'gross_au'),
      roots,
      totals,
    };
    if (previousApplyHash !== null) {
      normalized.previous_apply_hash = previousApplyHash;
    }
    if (marketUsageProvided) {
      normalized.market_usage = this.mapMarketUsageEntriesForHash(marketUsageMap);
    }
    const applyHash = await this.epochApplyHash(normalized);
    if (this.isIdempotentEpochApplyPage(applyState, this.value.epoch, page, lastPage, applyHash)) {
      const result = {
        ok: true,
        op: 'epochApply',
        epoch: this.value.epoch,
        idempotent: true,
        debited_au: ZERO_AU,
        earned_au: ZERO_AU,
        fee_au: ZERO_AU,
      };
      if (pagedApply) {
        result.page = page;
        result.last_page = lastPage;
      }
      return result;
    }
    const pageOrderError = this.validateEpochApplyPageOrder(applyState, this.value.epoch, page);
    if (pageOrderError) return pageOrderError;
    const reservationDebitTotals = this.nextReservationDebitTotals(applyState, this.value.epoch, page, debitMap);
    if (reservationDebitTotals instanceof Error) return reservationDebitTotals;
    const reservationError = await this.validateEpochDebitReservations(this.value.epoch, reservationDebitTotals);
    if (reservationError) return reservationError;

    const balances = new Map();
    for (const debit of debitMap.values()) {
      const { rail, user, au: debitAu } = debit;
      const balance = await this.balanceRecord(user, rail);
      if (balance instanceof Error) return balance;
      const balanceError = this.guardianValidateBalanceRecord(balance, user, rail);
      if (balanceError) return balanceError;
      if (this.compareAu(balance.au, debitAu) < 0) return new Error('Insufficient credit balance.');
      const nextBalanceAu = this.safeSubAu(balance.au, debitAu);
      if (nextBalanceAu instanceof Error) return nextBalanceAu;
      balances.set(stableJson([rail, user]), {
        ...balance,
        rail,
        au: nextBalanceAu,
        updated_epoch: this.value.epoch,
        updated_at: this.tx,
      });
    }

    const earningDeltas = new Map();
    const feeDeltaByRail = new Map(PROVIDER_ACCEPTED_RAIL_ORDER.map((rail) => [rail, ZERO_AU]));
    let feeDeltaTotalAu = ZERO_AU;
    for (const earning of grossEarningMap.values()) {
      const { rail, provider, gross_au: grossAu } = earning;
      const providerRecord = await this.get(`prov/${provider}`);
      if (!providerRecord) return new Error('Provider not found.');
      if (!Array.isArray(providerRecord.accepted_rails)) {
        return new Error('Provider accepted rails are not set.');
      }
      if (!providerRecord.accepted_rails.includes(rail)) {
        return new Error('Provider does not accept payment rail.');
      }

      const feeAu = this.safeMulDivAu(grossAu, params.fee_bps, 10_000);
      if (feeAu instanceof Error) return feeAu;
      const providerAu = this.safeSubAu(grossAu, feeAu);
      if (providerAu instanceof Error) return providerAu;
      feeDeltaTotalAu = this.safeAddAu(feeDeltaTotalAu, feeAu);
      if (feeDeltaTotalAu instanceof Error) return feeDeltaTotalAu;
      const nextRailFee = this.safeAddAu(feeDeltaByRail.get(rail) ?? ZERO_AU, feeAu);
      if (nextRailFee instanceof Error) return nextRailFee;
      feeDeltaByRail.set(rail, nextRailFee);
      const key = stableJson([rail, provider]);
      const current = earningDeltas.get(key) ?? { rail, provider, provider_record: providerRecord, au: ZERO_AU };
      const next = this.safeAddAu(current.au, providerAu);
      if (next instanceof Error) return next;
      earningDeltas.set(key, { ...current, au: next });
    }

    const earnings = new Map();
    let earnCumTotal = ZERO_AU;
    for (const delta of earningDeltas.values()) {
      const { rail, provider, provider_record: providerRecord, au: deltaAu } = delta;
      const current = await this.earningRecord(provider, rail);
      if (current instanceof Error) return current;
      const currentError = this.guardianValidateEarningRecord(current, provider, rail);
      if (currentError) return currentError;
      const probeGate = await this.probeGateForEarning(provider, current, params);
      if (probeGate instanceof Error) return probeGate;
      const lockedEarningEpochs = this.providerLockedEarningEpochs(providerRecord, params);
      if (lockedEarningEpochs instanceof Error) return lockedEarningEpochs;
      const refreshed = this.refreshEarningHoldback(
        current,
        this.value.epoch,
        lockedEarningEpochs,
        probeGate
      );
      if (refreshed instanceof Error) return refreshed;
      const totalAu = this.safeAddAu(refreshed.total_au, deltaAu);
      if (totalAu instanceof Error) return totalAu;
      const heldAu = this.safeAddAu(refreshed.held_au, deltaAu);
      if (heldAu instanceof Error) return heldAu;
      const holdbacks = this.appendHoldbackBucket(
        refreshed.holdbacks,
        this.value.epoch,
        deltaAu,
        lockedEarningEpochs
      );
      if (holdbacks instanceof Error) return holdbacks;
      earnings.set(stableJson([rail, provider]), {
        ...refreshed,
        rail,
        total_au: totalAu,
        held_au: heldAu,
        holdbacks,
        updated_epoch: this.value.epoch,
        updated_at: this.tx,
      });
      earnCumTotal = this.safeAddAu(earnCumTotal, totalAu);
      if (earnCumTotal instanceof Error) return earnCumTotal;
    }

    const feeRecords = new Map();
    const nextFeeRecords = new Map();
    let nextFeeCumTotal = ZERO_AU;
    const touchedRails = new Set([
      ...Array.from(debitRailTotals.entries()).filter(([, au]) => this.compareAu(au, ZERO_AU) > 0).map(([rail]) => rail),
      ...Array.from(grossRailTotals.entries()).filter(([, au]) => this.compareAu(au, ZERO_AU) > 0).map(([rail]) => rail),
    ]);
    for (const rail of PROVIDER_ACCEPTED_RAIL_ORDER) {
      const fee = await this.feeCumRecord(rail);
      if (fee instanceof Error) return fee;
      const feeError = this.guardianValidateFeeRecord(fee, rail);
      if (feeError) return feeError;
      feeRecords.set(rail, fee);
      const feeDeltaAu = feeDeltaByRail.get(rail) ?? ZERO_AU;
      const nextFeeCum = this.safeAddAu(fee.cum_au, feeDeltaAu);
      if (nextFeeCum instanceof Error) return nextFeeCum;
      const nextFee = touchedRails.has(rail)
        ? {
            ...fee,
            cum_au: nextFeeCum,
            updated_epoch: this.value.epoch,
            updated_at: this.tx,
            last_apply_hash: applyHash,
            last_fee_bps: params.fee_bps,
          }
        : fee;
      nextFeeRecords.set(rail, nextFee);
      nextFeeCumTotal = this.safeAddAu(nextFeeCumTotal, nextFee.cum_au);
      if (nextFeeCumTotal instanceof Error) return nextFeeCumTotal;
    }

    const providerDeltaTotal = this.safeSubAu(grossTotal, feeDeltaTotalAu);
    if (providerDeltaTotal instanceof Error) return providerDeltaTotal;
    const guardian = this.guardianCheckEpochApply({
      epoch: this.value.epoch,
      page,
      applyState,
      feeRecords,
      debitTotal,
      debitRailTotals,
      feeDeltaByRail,
      providerDeltaTotal,
      nextFeeRecords,
      balances,
      earnings,
    });
    if (guardian instanceof Error) return guardian;

    let marketPriceUpdates = [];
    if (marketUsageProvided) {
      marketPriceUpdates = await this.computeMarketPriceUpdates(marketUsageMap, {
        epoch: this.value.epoch,
        at: this.value.at,
        epochSeconds: params.epoch_seconds,
      });
      if (marketPriceUpdates instanceof Error) return marketPriceUpdates;
    }
    const priceDerivations = await this.priceDerivationsFromMarketUpdates(marketPriceUpdates, {
      epoch: this.value.epoch,
      at: this.value.at,
      epochSeconds: params.epoch_seconds,
      usageRoot: roots?.use ?? null,
    });
    if (priceDerivations instanceof Error) return priceDerivations;

    if (totals) {
      const totalsError = await this.validateEpochApplyTotals({
        epoch: this.value.epoch,
        roots,
        totals,
        debitTotal,
        feeDeltaAu: feeDeltaTotalAu,
        nextFeeCum: nextFeeCumTotal,
        providerCount: grossEarningMap.size,
        earnCumTotal,
        epochSeconds: params.epoch_seconds,
        priceDerivations,
      });
      if (totalsError) return totalsError;
    }

    for (const balance of this.sortedRailRecords(balances, 'user')) {
      await this.put(this.balanceKey(balance.user, balance.rail), balance);
    }
    for (const earning of this.sortedRailRecords(earnings, 'provider')) {
      await this.put(this.earningKey(earning.provider, earning.rail), earning);
    }
    for (const rail of PROVIDER_ACCEPTED_RAIL_ORDER.filter((rail) => touchedRails.has(rail))) {
      const feeRecord = {
        ...nextFeeRecords.get(rail),
        settled_cum_au: guardian.next_settled_cum_by_rail.get(rail),
      };
      await this.put(this.feeCumKey(rail), feeRecord);
    }
    await this.put('epoch/apply/state', this.nextEpochApplyState({
      applyState,
      epoch: this.value.epoch,
      page,
      lastPage,
      applyHash,
      epochSeconds: params.epoch_seconds,
      reservationDebitTotals,
    }));
    if (totals) {
      await this.writeEpochEvidenceRoots({
        epoch: this.value.epoch,
        at: this.value.at,
        epoch_seconds: params.epoch_seconds,
        roots,
        totals,
        feeDeltaAu: feeDeltaTotalAu,
        feeCumAu: nextFeeCumTotal,
        priceDerivations,
      });
    } else if (priceDerivations.length > 0) {
      await this.writePriceDerivationEvidence({
        epoch: this.value.epoch,
        at: this.value.at,
        epoch_seconds: params.epoch_seconds,
        root: await this.priceDerivationRoot(priceDerivations),
        count: priceDerivations.length,
        derivations: priceDerivations,
      });
    }
    for (const update of marketPriceUpdates) {
      await this.put(update.schedule_key, update.schedule);
      await this.put(update.record_key, update.record);
    }

    const result = {
      ok: true,
      op: 'epochApply',
      epoch: this.value.epoch,
      idempotent: false,
      debited_au: debitTotal,
      earned_au: providerDeltaTotal,
      fee_au: feeDeltaTotalAu,
      rails: Array.from(touchedRails).sort(
        (left, right) => PROVIDER_ACCEPTED_RAIL_ORDER.indexOf(left) - PROVIDER_ACCEPTED_RAIL_ORDER.indexOf(right)
      ),
    };
    if (pagedApply) {
      result.page = page;
      result.last_page = lastPage;
    }
    if (marketPriceUpdates.length > 0) {
      result.market_prices = marketPriceUpdates.map((update) => ({
        enclave_id: update.enclave_id,
        ...(update.ctx_bracket ? {
          ctx_bracket: update.ctx_bracket,
          ctx_bracket_table_ver: update.ctx_bracket_table_ver,
        } : {}),
        ver: update.ver,
        utilization_bps: update.utilization_bps,
        ema_utilization_bps: update.ema_utilization_bps,
        active_supply: update.active_supply,
        active_demand_au: update.active_demand_au,
        frozen: update.frozen,
        derivation_hash: update.derivation_hash,
      }));
      result.price_root = await this.priceDerivationRoot(priceDerivations);
    }
    console.log('mayhem epochApply', result);
    return result;
  }

  async epochSealEmpty() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const shapeError = this.validateExactCommandValue(
      ['op', 'epoch', 'at', 'reason_hash'],
      'epoch_seal_empty'
    );
    if (shapeError) return shapeError;
    if (!Number.isSafeInteger(this.value.epoch) || this.value.epoch < 1) {
      return new Error('Invalid empty epoch seal epoch.');
    }
    if (!Number.isSafeInteger(this.value.at) || this.value.at < 0) {
      return new Error('Invalid empty epoch seal timestamp.');
    }
    if (!this.isHexBytes(this.value.reason_hash, 32)) {
      return new Error('Invalid empty epoch seal reason hash.');
    }

    const params = await this.activeParamsAt(this.value.at, ['epoch_seconds']);
    const applyState = await this.epochApplyStateRecord();
    const reasonHash = this.value.reason_hash.toLowerCase();
    const key = `epoch/seal/${this.value.epoch}`;
    const existing = await this.get(key);
    if (existing) {
      const existingHash = await this.epochEmptySealHash(this.epochEmptySealHashValue(existing));
      if (
        existing.seal_hash === existingHash &&
        existing.reason_hash === reasonHash &&
        existing.at === this.value.at &&
        existing.sealed_by === this.address &&
        applyState.updated_epoch === this.value.epoch &&
        applyState.last_apply_hash === existing.seal_hash &&
        (applyState.pending_epoch ?? null) === null
      ) {
        return {
          ok: true,
          op: 'epochSealEmpty',
          epoch: this.value.epoch,
          idempotent: true,
          seal_hash: existing.seal_hash,
        };
      }
      return new Error('Empty epoch seal already exists.');
    }

    const notYetActiveAt = this.value.epoch * params.epoch_seconds;
    if (this.value.at < notYetActiveAt) {
      return new Error('Empty epoch seal is not active until the epoch window ends.');
    }
    const pageOrderError = this.validateEpochApplyPageOrder(applyState, this.value.epoch, 0);
    if (pageOrderError) return pageOrderError;

    const sealValue = {
      type: 'epoch_empty_seal',
      epoch: this.value.epoch,
      at: this.value.at,
      epoch_seconds: params.epoch_seconds,
      previous_apply_hash: applyState.last_apply_hash ?? null,
      reason_hash: reasonHash,
      sealed_by: this.address,
      sealed_by_role: 'admin',
      totals: {
        debited_au: ZERO_AU,
        earned_au: ZERO_AU,
        fee_au: ZERO_AU,
      },
    };
    const sealHash = await this.epochEmptySealHash(sealValue);
    const record = {
      ...sealValue,
      seal_hash: sealHash,
      sealed_at: this.tx,
    };
    await this.put(key, record);
    await this.put('epoch/apply/state', this.nextEpochApplyState({
      applyState,
      epoch: this.value.epoch,
      page: 0,
      lastPage: true,
      applyHash: sealHash,
      epochSeconds: params.epoch_seconds,
    }));
    const result = {
      ok: true,
      op: 'epochSealEmpty',
      epoch: this.value.epoch,
      idempotent: false,
      seal_hash: sealHash,
    };
    console.log('mayhem epochSealEmpty', result);
    return result;
  }

  async epochCommit() {
    const banned = await this.get(`committer/ban/${this.address}`);
    if (banned?.status === 'banned') return new Error('Epoch committer is banned.');

    const roots = this.normalizeEpochRoots(this.value.roots);
    if (roots instanceof Error) return roots;
    const totals = this.normalizeEpochTotals(this.value.totals);
    if (totals instanceof Error) return totals;
    const params = await this.activeParamsAt(this.value.at, ['challenge_epochs', 'epoch_seconds']);
    const normalized = {
      epoch: this.value.epoch,
      epoch_seconds: params.epoch_seconds,
      roots,
      totals,
    };
    const commitHash = await this.epochCommitHash(normalized);
    const key = `epoch/commit/${this.value.epoch}`;
    const existing = await this.get(key);
    if (existing) {
      if (existing.commit_hash === commitHash) {
        return {
          ok: true,
          op: 'epochCommit',
          epoch: this.value.epoch,
          idempotent: true,
          commit_hash: commitHash,
        };
      }
      return new Error('Epoch commit already exists.');
    }

    const record = {
      type: 'epoch_commit',
      epoch: this.value.epoch,
      epoch_seconds: params.epoch_seconds,
      roots,
      totals,
      status: 'provisional',
      challenge_epochs: params.challenge_epochs,
      provisional_until_epoch: this.value.epoch + params.challenge_epochs,
      commit_hash: commitHash,
      submitted_by: this.address,
      submitted_at: this.tx,
      at: this.value.at,
    };
    await this.put(key, record);
    console.log('mayhem epochCommit', record);
    return {
      ok: true,
      op: 'epochCommit',
      epoch: this.value.epoch,
      idempotent: false,
      commit_hash: commitHash,
    };
  }

  async fraudProof() {
    if (!FRAUD_PROOF_REASONS.has(this.value.reason)) {
      return new Error('Unsupported fraud proof reason.');
    }
    if (b4a.from(stableJson(this.value)).byteLength > FRAUD_PROOF_MAX_BYTES) {
      return new Error('Fraud proof exceeds 4096 bytes.');
    }

    const commitKey = `epoch/commit/${this.value.epoch}`;
    const commit = await this.get(commitKey);
    if (!commit) return new Error('Epoch commit not found.');

    let proofHashPayload;
    let proof;
    let proofHash = null;
    let slashReason = 'receipt_forgery';
    let slashEnclaveId = null;

    if (this.value.reason === 'over_credit') {
      if (!hasOwn(this.value, 'receipt')) return new Error('Over-credit fraud proof requires a receipt.');
      if (!hasOwn(this.value, 'claimed_au_owed_cum')) {
        return new Error('Over-credit fraud proof requires claimed_au_owed_cum.');
      }
      const receipt = await this.normalizeReceiptEnvelope(this.value.receipt);
      if (receipt instanceof Error) return receipt;
      if (!this.verifyReceiptEnvelope(receipt)) {
        return new Error('Invalid receipt signature.');
      }
      proofHashPayload = {
        epoch: this.value.epoch,
        proof_epoch: this.value.proof_epoch,
        reason: this.value.reason,
        receipt,
        claimed_au_owed_cum: this.value.claimed_au_owed_cum,
        previous_au_owed_cum: this.value.previous_au_owed_cum ?? ZERO_AU,
      };
      proofHash = await this.fraudProofHash(proofHashPayload);
      const existingProof = await this.get(`ev/fraud/${this.value.epoch}/${proofHash}`);
      if (existingProof) {
        return {
          ok: true,
          op: 'fraudProof',
          epoch: this.value.epoch,
          idempotent: true,
          proof_hash: proofHash,
          voided_commit: commit.commit_hash,
          banned_submitter: commit.submitted_by,
        };
      }
      proof = await this.validateOverCreditFraudProof(commit, receipt);
      if (proof instanceof Error) return proof;
      slashEnclaveId = receipt.body.enclave_id;
    } else if (this.value.reason === 'price_derivation') {
      if (!hasOwn(this.value, 'price_usage')) {
        return new Error('Price derivation fraud proof requires price_usage.');
      }
      proof = await this.validatePriceDerivationFraudProof(commit);
      if (proof instanceof Error) return proof;
      proofHashPayload = {
        epoch: this.value.epoch,
        proof_epoch: this.value.proof_epoch,
        reason: this.value.reason,
        price_usage: proof.price_usage,
        expected_price_root: proof.expected_price_root,
        committed_price_root: proof.committed_price_root,
        price_derivation_hash: proof.price_derivation_hash,
      };
      slashReason = 'price_forgery';
      slashEnclaveId = proof.enclave_id;
    }

    proofHash ??= await this.fraudProofHash(proofHashPayload);
    const proofKey = `ev/fraud/${this.value.epoch}/${proofHash}`;
    const existingProof = await this.get(proofKey);
    if (existingProof) {
      return {
        ok: true,
        op: 'fraudProof',
        epoch: this.value.epoch,
        idempotent: true,
        proof_hash: proofHash,
        voided_commit: commit.commit_hash,
        banned_submitter: commit.submitted_by,
      };
    }

    if (commit.status === 'void') return new Error('Epoch commit is already void.');
    if (this.value.proof_epoch > commit.provisional_until_epoch) {
      return new Error('Epoch commit challenge window has closed.');
    }

    const record = {
      type: 'fraud_proof',
      epoch: this.value.epoch,
      proof_epoch: this.value.proof_epoch,
      reason: this.value.reason,
      proof_hash: proofHash,
      submitted_by: this.address,
      submitted_at: this.tx,
      at: this.value.at,
      voided_commit: commit.commit_hash,
      banned_submitter: commit.submitted_by,
    };
    if (this.value.reason === 'over_credit') {
      record.receipt_hash = proof.receipt_hash;
      record.actual_au = proof.actual_au;
      record.claimed_au = proof.claimed_au;
      record.committed_use_root = commit.roots.use;
    } else if (this.value.reason === 'price_derivation') {
      record.price_usage = proof.price_usage;
      record.committed_price_root = proof.committed_price_root;
      record.expected_price_root = proof.expected_price_root;
      record.price_derivation_hash = proof.price_derivation_hash;
      record.price_derivation = proof.price_derivation;
    }
    const updatedCommit = {
      ...commit,
      status: 'void',
      voided_at: this.tx,
      voided_by: this.address,
      fraud_reason: this.value.reason,
      fraud_proof_hash: proofHash,
    };
    const banKey = `committer/ban/${commit.submitted_by}`;
    const existingBan = await this.get(banKey);
    const ban = existingBan?.status === 'banned' ? existingBan : {
      ...(existingBan ?? {}),
      submitter: commit.submitted_by,
      status: 'banned',
      reason: 'fraud_proof',
      epoch: this.value.epoch,
      proof_hash: proofHash,
      banned_at: this.tx,
      banned_by: this.address,
    };

    let slash = null;
    if ((await this.get(`prov/${commit.submitted_by}`)) !== null) {
      const params = await this.activeParamsAt(this.value.at, ['fraud_slash_bps']);
      slash = await this.applyProviderSlash({
        providerId: commit.submitted_by,
        source: 'fraud_proof',
        reason: slashReason,
        evidenceHash: proofHash,
        epoch: this.value.epoch,
        at: this.value.at,
        slashBps: params.fraud_slash_bps,
        beneficiary: this.address,
        enclaveId: slashEnclaveId,
        banProvider: true,
        tombstoneEnclave: true,
      });
      if (slash instanceof Error) return slash;
    }
    record.slash = slash;

    await this.put(proofKey, record);
    await this.put(commitKey, updatedCommit);
    await this.put(banKey, ban);
    console.log('mayhem fraudProof', record);
    return {
      ok: true,
      op: 'fraudProof',
      epoch: this.value.epoch,
      idempotent: false,
      proof_hash: proofHash,
      voided_commit: commit.commit_hash,
      banned_submitter: commit.submitted_by,
      slash,
    };
  }

  async dispute() {
    if (!(await this.isAdmin())) {
      const consentError = await this.requireConsent();
      if (consentError) return consentError;
    }
    const validationError = this.validateDisputeOpen(this.value);
    if (validationError) return validationError;

    const rail = this.normalizeLedgerRail(this.value.rail, 'dispute rail');
    if (rail instanceof Error) return rail;
    const balance = await this.balanceRecord(this.address, rail);
    if (balance instanceof Error) return balance;
    const balanceError = this.guardianValidateBalanceRecord(balance, this.address, rail);
    if (balanceError) return balanceError;
    const params = await this.activeParamsAt(this.value.at, [
      'dispute_deposit_au',
      'dispute_timeout_epochs',
    ]);
    const depositAu = params.dispute_deposit_au;
    if (this.compareAu(balance.au, depositAu) < 0) return new Error('Insufficient balance for dispute deposit.');
    const applyState = await this.epochApplyStateRecord();
    const disputeEpoch = this.value.epoch ?? applyState.updated_epoch;
    const expiresAfterEpoch = disputeEpoch + params.dispute_timeout_epochs;
    if (!Number.isSafeInteger(expiresAfterEpoch)) return new Error('Dispute timeout epoch overflow.');

    const nextBalanceAu = this.safeSubAu(balance.au, depositAu);
    if (nextBalanceAu instanceof Error) return nextBalanceAu;
    const nextBalance = {
      ...balance,
      rail,
      au: nextBalanceAu,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch ?? 0),
      updated_at: this.tx,
    };
    const nextBalanceError = this.guardianValidateBalanceRecord(nextBalance, this.address, rail);
    if (nextBalanceError) return nextBalanceError;

    const next = await this.get('disp/next');
    const disputeId = next?.next ?? 1;
    const record = {
      type: 'dispute',
      dispute_id: disputeId,
      status: 'open',
      rail,
      opened_by: this.address,
      session_id: this.value.session_id,
      reason: this.value.reason,
      provider: this.value.provider ?? null,
      counterparty: this.value.counterparty ?? null,
      enclave_id: this.value.enclave_id ?? null,
      epoch: disputeEpoch,
      at: this.value.at,
      evidence_hash: this.value.evidence_hash ?? null,
      evidence: cloneValue(this.value.evidence ?? null),
      deposit_au: depositAu,
      deposit_holder: this.address,
      timeout_epochs: params.dispute_timeout_epochs,
      expires_after_epoch: expiresAfterEpoch,
      opened_at: this.tx,
      updated_at: this.tx,
    };
    record.dispute_hash = await this.opaqueHash('mayhem-dispute-v1', record);

    await this.put(this.balanceKey(this.address, rail), nextBalance);
    await this.put(`disp/${disputeId}`, record);
    await this.put('disp/next', { next: disputeId + 1, updated_at: this.tx });
    console.log('mayhem dispute', record);
    return {
      ok: true,
      op: 'dispute',
      dispute_id: disputeId,
      deposit_au: depositAu,
      expires_after_epoch: expiresAfterEpoch,
      dispute_hash: record.dispute_hash,
    };
  }

  async disputeResolve() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!DISPUTE_OUTCOMES.has(this.value.outcome)) return new Error('Unsupported dispute outcome.');
    if (!DISPUTE_DEPOSIT_ACTIONS.has(this.value.deposit_action)) {
      return new Error('Unsupported dispute deposit action.');
    }

    const key = `disp/${this.value.dispute_id}`;
    const dispute = await this.get(key);
    if (!dispute || dispute.type !== 'dispute') return new Error('Dispute not found.');
    if (dispute.status !== 'open') return new Error('Dispute is not open.');
    if (this.value.beneficiary !== undefined && !this.isSafeKeyPart(this.value.beneficiary)) {
      return new Error('Invalid slash beneficiary.');
    }
    if (this.value.outcome === 'provider_fault' && !dispute.provider) {
      return new Error('Provider fault disputes require a provider.');
    }
    if (this.value.outcome === 'provider_fault') {
      const provider = await this.get(`prov/${dispute.provider}`);
      if (!provider) return new Error('Provider not found.');
    }
    if (this.value.slash === true && !dispute.provider) {
      return new Error('Dispute slash requires a provider.');
    }
    if (this.value.slash === true && this.value.outcome !== 'provider_fault') {
      return new Error('Only provider_fault disputes may slash a provider.');
    }

    let depositRefundedAu = ZERO_AU;
    let depositForfeitedAu = ZERO_AU;
    const rail = this.normalizeLedgerRail(dispute.rail, 'dispute rail');
    if (rail instanceof Error) return rail;
    if (this.value.deposit_action === 'refund') {
      depositRefundedAu = dispute.deposit_au;
      const balance = await this.balanceRecord(dispute.opened_by, rail);
      if (balance instanceof Error) return balance;
      const balanceError = this.guardianValidateBalanceRecord(balance, dispute.opened_by, rail);
      if (balanceError) return balanceError;
      const nextAu = this.safeAddAu(balance.au, depositRefundedAu);
      if (nextAu instanceof Error) return nextAu;
      await this.put(this.balanceKey(dispute.opened_by, rail), {
        ...balance,
        rail,
        au: nextAu,
        updated_epoch: Math.max(balance.updated_epoch, dispute.epoch ?? 0),
        updated_at: this.tx,
      });
    } else {
      depositForfeitedAu = dispute.deposit_au;
      const fee = await this.feeCumRecord(rail);
      if (fee instanceof Error) return fee;
      const feeError = this.guardianValidateFeeRecord(fee, rail);
      if (feeError) return feeError;
      const cumAu = this.safeAddAu(fee.cum_au, depositForfeitedAu);
      if (cumAu instanceof Error) return cumAu;
      const settledCumAu = this.safeAddAu(fee.settled_cum_au ?? fee.cum_au, depositForfeitedAu);
      if (settledCumAu instanceof Error) return settledCumAu;
      const updatedFee = {
        ...fee,
        cum_au: cumAu,
        settled_cum_au: settledCumAu,
        updated_epoch: Math.max(fee.updated_epoch, dispute.epoch ?? 0),
        updated_at: this.tx,
        last_dispute_forfeit_at: this.tx,
      };
      const updatedFeeError = this.guardianValidateFeeRecord(updatedFee, rail);
      if (updatedFeeError) return updatedFeeError;
      await this.put(this.feeCumKey(rail), updatedFee);
    }

    let reputationEvent = null;
    let slash = null;
    if (this.value.outcome === 'provider_fault' && dispute.provider) {
      reputationEvent = await this.appendReputationEvent({
        provider: dispute.provider,
        event_id: `dispute-${dispute.dispute_id}-lost`,
        kind: 'dispute_lost',
        epoch: dispute.epoch ?? 0,
        at: this.value.at,
        paid_au: null,
        max_spend_au: null,
        evidence_hash: this.value.rationale_hash,
      });
      if (reputationEvent instanceof Error) return reputationEvent;

      if (this.value.slash === true) {
        const params = await this.activeParamsAt(this.value.at, ['dispute_lost_slash_bps']);
        slash = await this.applyProviderSlash({
          providerId: dispute.provider,
          source: 'dispute',
          reason: 'dispute_lost',
          evidenceHash: this.value.rationale_hash,
          epoch: dispute.epoch ?? 0,
          at: this.value.at,
          slashBps: params.dispute_lost_slash_bps,
          beneficiary: this.value.beneficiary ?? dispute.opened_by,
          enclaveId: dispute.enclave_id,
          eventId: reputationEvent.event_id,
          banProvider: false,
          tombstoneEnclave: false,
        });
        if (slash instanceof Error) return slash;
      }
    }

    const resolved = {
      ...dispute,
      status: 'resolved',
      outcome: this.value.outcome,
      deposit_action: this.value.deposit_action,
      rationale_hash: this.value.rationale_hash,
      resolved_by: this.address,
      resolved_at: this.tx,
      resolved_at_seconds: this.value.at,
      deposit_refunded_au: depositRefundedAu,
      deposit_forfeited_au: depositForfeitedAu,
      reputation_event: reputationEvent,
      slash,
      updated_at: this.tx,
    };
    resolved.resolution_hash = await this.opaqueHash('mayhem-dispute-resolution-v1', resolved);
    await this.put(key, resolved);
    console.log('mayhem disputeResolve', resolved);
    return {
      ok: true,
      op: 'disputeResolve',
      dispute_id: dispute.dispute_id,
      outcome: resolved.outcome,
      deposit_action: resolved.deposit_action,
      deposit_refunded_au: depositRefundedAu,
      deposit_forfeited_au: depositForfeitedAu,
      slash,
      resolution_hash: resolved.resolution_hash,
    };
  }

  async disputeExpire() {
    const key = `disp/${this.value.dispute_id}`;
    const dispute = await this.get(key);
    if (!dispute || dispute.type !== 'dispute') return new Error('Dispute not found.');
    if (dispute.status !== 'open') return new Error('Dispute is not open.');
    if (!Number.isSafeInteger(dispute.expires_after_epoch) || dispute.expires_after_epoch < 0) {
      return new Error('Dispute has no valid timeout epoch.');
    }

    const applyState = await this.epochApplyStateRecord();
    if (applyState.updated_epoch < dispute.expires_after_epoch) {
      return new Error('Dispute timeout epoch not reached.');
    }

    const rail = this.normalizeLedgerRail(dispute.rail, 'dispute rail');
    if (rail instanceof Error) return rail;
    const balance = await this.balanceRecord(dispute.opened_by, rail);
    if (balance instanceof Error) return balance;
    const balanceError = this.guardianValidateBalanceRecord(balance, dispute.opened_by, rail);
    if (balanceError) return balanceError;
    const depositRefundedAu = dispute.deposit_au;
    const nextAu = this.safeAddAu(balance.au, depositRefundedAu);
    if (nextAu instanceof Error) return nextAu;
    const nextBalance = {
      ...balance,
      rail,
      au: nextAu,
      updated_epoch: Math.max(balance.updated_epoch, applyState.updated_epoch, dispute.epoch ?? 0),
      updated_at: this.tx,
    };
    const nextBalanceError = this.guardianValidateBalanceRecord(nextBalance, dispute.opened_by, rail);
    if (nextBalanceError) return nextBalanceError;

    const expired = {
      ...dispute,
      status: 'expired',
      outcome: 'timeout_refund',
      deposit_action: 'refund',
      deposit_holder: null,
      expired_by: this.address,
      expired_at: this.tx,
      expired_at_epoch: applyState.updated_epoch,
      expired_at_seconds: this.value.at,
      deposit_refunded_au: depositRefundedAu,
      deposit_forfeited_au: ZERO_AU,
      reputation_event: null,
      slash: null,
      updated_at: this.tx,
    };
    expired.expiry_hash = await this.opaqueHash('mayhem-dispute-expiry-v1', expired);
    await this.put(this.balanceKey(dispute.opened_by, rail), nextBalance);
    await this.put(key, expired);
    console.log('mayhem disputeExpire', expired);
    return {
      ok: true,
      op: 'disputeExpire',
      dispute_id: dispute.dispute_id,
      outcome: expired.outcome,
      deposit_action: expired.deposit_action,
      deposit_refunded_au: depositRefundedAu,
      expired_at_epoch: expired.expired_at_epoch,
      expiry_hash: expired.expiry_hash,
    };
  }

  async rateOracle() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateRateOracleValue(this.value);
    if (shapeError) return shapeError;
    if (!RATE_SOURCES.has(this.value.source)) return new Error('Unsupported rate source.');

    const current = await this.get('rate/latest');
    if (current && this.value.ts < current.ts) {
      return new Error('Rate timestamp must not decrease.');
    }

    const record = {
      denom: 'tnk_usd_au',
      tnk_usd_au: this.normalizeAu(this.value.tnk_usd_au, 'TNK/USD atto-rate', { allowZero: false }),
      source: this.value.source,
      ts: this.value.ts,
      updated_at: this.tx,
      posted_by: this.address,
      posted_by_role: 'admin',
    };
    await this.put('rate/latest', record);
    console.log('mayhem rateOracle', record);
    return { ok: true, op: 'rateOracle', ts: record.ts, source: record.source };
  }

  async tapRateOracle() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateTapRateOracleValue(this.value);
    if (shapeError) return shapeError;
    if (!TAP_RATE_SOURCES.has(this.value.source)) return new Error('Unsupported TAP rate source.');

    const current = await this.get('tap/rate/latest');
    if (current && this.value.ts < current.ts) {
      return new Error('TAP rate timestamp must not decrease.');
    }

    const record = {
      denom: 'tap_usd_au',
      tap_usd_au: this.normalizeAu(this.value.tap_usd_au, 'TAP/USD atto-rate', { allowZero: false }),
      source: this.value.source,
      ts: this.value.ts,
      updated_at: this.tx,
      posted_by: this.address,
      posted_by_role: 'admin',
    };
    await this.put('tap/rate/latest', record);
    console.log('mayhem tapRateOracle', record);
    return { ok: true, op: 'tapRateOracle', ts: record.ts, source: record.source };
  }

  validateRateOracleValue(value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      ['op', 'tnk_usd_au', 'source', 'ts'],
      'rate oracle'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'rate_oracle') return new Error('Invalid rate oracle op.');
    const rate = this.normalizeAu(value.tnk_usd_au, 'TNK/USD atto-rate', { allowZero: false });
    if (rate instanceof Error) {
      return new Error('Invalid TNK/USD atto-rate.');
    }
    if (typeof value.source !== 'string' || value.source.length < 1 || value.source.length > 64) {
      return new Error('Invalid rate source.');
    }
    if (!Number.isSafeInteger(value.ts) || value.ts < 0) {
      return new Error('Invalid rate timestamp.');
    }
    return null;
  }

  validateTapRateOracleValue(value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      ['op', 'tap_usd_au', 'source', 'ts'],
      'TAP rate oracle'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'tap_rate_oracle') return new Error('Invalid TAP rate oracle op.');
    const rate = this.normalizeAu(value.tap_usd_au, 'TAP/USD atto-rate', { allowZero: false });
    if (rate instanceof Error) {
      return new Error('Invalid TAP/USD atto-rate.');
    }
    if (typeof value.source !== 'string' || value.source.length < 1 || value.source.length > 64) {
      return new Error('Invalid TAP rate source.');
    }
    if (!Number.isSafeInteger(value.ts) || value.ts < 0) {
      return new Error('Invalid TAP rate timestamp.');
    }
    return null;
  }

  async tnkSettlement() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateTnkSettlementValue(this.value);
    if (shapeError) return shapeError;
    const settlementParams = await this.activeParamsAt(this.value.at, ['max_tnk_settlement_outputs']);
    if (this.value.outputs.length > settlementParams.max_tnk_settlement_outputs) {
      return new Error('TNK settlement output count exceeds max_tnk_settlement_outputs.');
    }

    const outputs = this.normalizeTnkSettlementOutputs(this.value.outputs);
    if (outputs instanceof Error) return outputs;
    if (stableJson(outputs) !== stableJson(this.value.outputs)) {
      return new Error('TNK settlement outputs must be canonical.');
    }
    const totals = this.tnkSettlementTotals(outputs, this.value.rate_tnk_usd_au);
    if (totals instanceof Error) return totals;
    if (totals.provider_count !== this.value.provider_count) {
      return new Error('TNK settlement provider count does not match outputs.');
    }
    if (this.compareAu(totals.provider_au, this.value.provider_au) !== 0) {
      return new Error('TNK settlement provider amount does not match outputs.');
    }
    if (this.compareAu(totals.operator_fee_au, this.value.operator_fee_au) !== 0) {
      return new Error('TNK settlement operator fee does not match outputs.');
    }
    if (this.compareAu(totals.gross_au, this.value.gross_au) !== 0) {
      return new Error('TNK settlement gross amount does not match outputs.');
    }
    if (totals.tnk_e18 !== this.value.tnk_e18) {
      return new Error('TNK settlement TNK amount does not match outputs.');
    }

    const transferRoot = await this.tnkSettlementTransferRoot(outputs);
    if (transferRoot !== this.value.transfer_root) {
      return new Error('TNK settlement transfer root does not match outputs.');
    }

    const record = this.tnkSettlementRecord(outputs);
    const key = `settle/tnk/${this.value.epoch}`;
    const existing = await this.get(key);
    if (existing) {
      if (stableJson(existing) === stableJson(record)) {
        return {
          ok: true,
          op: 'tnkSettlement',
          epoch: this.value.epoch,
          rail: 'tnk',
          idempotent: true,
          msb_tx_hashes: this.value.msb_tx_hashes,
        };
      }
      return new Error('TNK settlement already exists for epoch.');
    }

    const applyState = await this.epochApplyStateRecord();
    if (
      applyState.updated_epoch !== this.value.epoch ||
      applyState.last_apply_hash !== this.value.epoch_apply_hash
    ) {
      return new Error('TNK settlement epoch apply hash does not match current state.');
    }

    const rate = await this.guardianRequireFreshRate(this.value.at);
    if (rate instanceof Error) return rate;
    if (
      this.compareAu(rate.tnk_usd_au, this.value.rate_tnk_usd_au) !== 0 ||
      rate.source !== this.value.rate_source ||
      rate.ts !== this.value.rate_ts
    ) {
      return new Error('TNK settlement rate does not match current oracle.');
    }

    const params = await this.activeParamsAt(this.value.at, [
      'holdback_epochs',
      'challenge_epochs',
      'canary_probe_holdback_bps',
      'canary_probe_release_min_passes',
    ]);
    const earningUpdates = [];
    for (const [outputIndex, output] of outputs.entries()) {
      if (output.role !== 'provider') continue;
      const provider = await this.get(`prov/${output.provider}`);
      if (!provider) return new Error('Provider not found.');
      if (provider.status !== 'active' && provider.status !== 'banned') {
        return new Error('TNK settlement provider status is not payable.');
      }
      const payoutError = await this.requireAdminSetPayoutTarget(provider);
      if (payoutError) return payoutError;
      if (provider.payout.method !== 'tnk') {
        return new Error('TNK settlement provider payout target must be TNK.');
      }
      if (provider.payout.addr !== output.to) {
        return new Error('TNK settlement provider payout target mismatch.');
      }

      const earning = await this.earningRecord(output.provider, 'tnk');
      if (earning instanceof Error) return earning;
      const earningError = this.guardianValidateEarningRecord(earning, output.provider, 'tnk');
      if (earningError) return earningError;
      const probeGate = await this.probeGateForEarning(output.provider, earning, params);
      if (probeGate instanceof Error) return probeGate;
      const lockedEarningEpochs = this.providerLockedEarningEpochs(provider, params);
      if (lockedEarningEpochs instanceof Error) return lockedEarningEpochs;
      const refreshed = this.refreshEarningHoldback(
        earning,
        this.value.epoch,
        lockedEarningEpochs,
        probeGate
      );
      if (refreshed instanceof Error) return refreshed;
      const payable = this.safeSubAu(
        this.safeSubAu(refreshed.total_au, refreshed.held_au),
        refreshed.paid_cum_au
      );
      if (payable instanceof Error) return payable;
      if (this.isZeroAu(payable)) return new Error('TNK settlement provider has no payable earnings.');
      if (this.compareAu(output.au, payable) !== 0) {
        return new Error('TNK settlement provider amount does not match payable earnings.');
      }
      const paidCumAu = this.safeAddAu(refreshed.paid_cum_au, output.au);
      if (paidCumAu instanceof Error) return paidCumAu;
      const nextEarning = {
        ...refreshed,
        paid_cum_au: paidCumAu,
        updated_epoch: Math.max(refreshed.updated_epoch, this.value.epoch),
        last_settlement_epoch: this.value.epoch,
        last_settlement_msb_tx_hash: this.value.msb_tx_hashes[outputIndex],
      };
      const nextError = this.guardianValidateEarningRecord(nextEarning, output.provider, 'tnk');
      if (nextError) return nextError;
      earningUpdates.push(nextEarning);
    }

    const fee = await this.feeCumRecord('tnk');
    if (fee instanceof Error) return fee;
    const feeError = this.guardianValidateFeeRecord(fee, 'tnk');
    if (feeError) return feeError;
    const payableFee = this.safeSubAu(fee.cum_au, fee.swept_cum_au);
    if (payableFee instanceof Error) return payableFee;
    if (this.compareAu(payableFee, this.value.operator_fee_au) !== 0) {
      return new Error('TNK settlement operator fee does not match fee state.');
    }
    const operatorOutputs = outputs.filter((entry) => entry.role === 'operator_fee');
    if (this.isZeroAu(payableFee) && operatorOutputs.length !== 0) {
      return new Error('TNK settlement has unexpected operator fee output.');
    }
    if (this.compareAu(payableFee, ZERO_AU) > 0) {
      if (operatorOutputs.length !== 1) return new Error('TNK settlement missing operator fee output.');
      if (operatorOutputs[0].to !== this.value.operator_to) {
        return new Error('TNK settlement operator target mismatch.');
      }
    }
    const operatorOutputIndex = outputs.findIndex((entry) => entry.role === 'operator_fee');
    const sweptCumAu = this.safeAddAu(fee.swept_cum_au, this.value.operator_fee_au);
    if (sweptCumAu instanceof Error) return sweptCumAu;
    const nextFee = {
      ...fee,
      swept_cum_au: sweptCumAu,
      updated_epoch: Math.max(fee.updated_epoch, this.value.epoch),
      last_settlement_epoch: this.value.epoch,
      last_settlement_msb_tx_hash: operatorOutputIndex >= 0
        ? this.value.msb_tx_hashes[operatorOutputIndex]
        : (fee.last_settlement_msb_tx_hash ?? null),
    };
    const nextFeeError = this.guardianValidateFeeRecord(nextFee, 'tnk');
    if (nextFeeError) return nextFeeError;

    for (const earning of earningUpdates) {
      await this.put(this.earningKey(earning.provider, 'tnk'), earning);
    }
    await this.put(this.feeCumKey('tnk'), nextFee);
    await this.put(key, record);
    console.log('mayhem tnkSettlement', {
      epoch: this.value.epoch,
      provider_au: this.value.provider_au,
      operator_fee_au: this.value.operator_fee_au,
      gross_au: this.value.gross_au,
      msb_tx_hashes: this.value.msb_tx_hashes,
    });
    return {
      ok: true,
      op: 'tnkSettlement',
      epoch: this.value.epoch,
      rail: 'tnk',
      idempotent: false,
      provider_au: this.value.provider_au,
      operator_fee_au: this.value.operator_fee_au,
      gross_au: this.value.gross_au,
      msb_tx_hashes: this.value.msb_tx_hashes,
      transfer_root: this.value.transfer_root,
    };
  }

  validateTnkSettlementValue(value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      [
        'op',
        'epoch',
        'at',
        'rail',
        'network',
        'treasury_from',
        'operator_to',
        'epoch_apply_hash',
        'rate_tnk_usd_au',
        'rate_source',
        'rate_ts',
        'msb_tx_hashes',
        'transfer_root',
        'provider_count',
        'provider_au',
        'operator_fee_au',
        'gross_au',
        'tnk_e18',
        'outputs',
      ],
      'TNK settlement'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'tnk_settlement') return new Error('Invalid TNK settlement op.');
    if (value.rail !== 'tnk') return new Error('TNK settlement rail must be tnk.');
    if (!Number.isSafeInteger(value.epoch) || value.epoch < 1) return new Error('Invalid TNK settlement epoch.');
    if (!Number.isSafeInteger(value.at) || value.at < 0) return new Error('Invalid TNK settlement timestamp.');
    if (!this.isSafeKeyPart(value.network)) return new Error('Invalid TNK settlement network.');
    if (!this.isSafeKeyPart(value.treasury_from)) return new Error('Invalid TNK treasury address.');
    if (!this.isSafeKeyPart(value.operator_to)) return new Error('Invalid TNK operator address.');
    if (!this.isHexBytes(value.epoch_apply_hash, 32)) return new Error('Invalid TNK settlement apply hash.');
    if (!Array.isArray(value.msb_tx_hashes) || value.msb_tx_hashes.length === 0) {
      return new Error('TNK settlement requires one real 64-hex MSB tx hash per output.');
    }
    const seenTxHashes = new Set();
    for (const hash of value.msb_tx_hashes) {
      if (!this.isHexBytes(hash, 32)) {
        return new Error('TNK settlement requires one real 64-hex MSB tx hash per output.');
      }
      if (seenTxHashes.has(hash)) return new Error('Duplicate TNK settlement MSB tx hash.');
      seenTxHashes.add(hash);
    }
    if (!this.isHexBytes(value.transfer_root, 32)) return new Error('Invalid TNK settlement transfer root.');
    const rateTnkUsdAu = this.normalizeAu(value.rate_tnk_usd_au, 'TNK settlement rate', { allowZero: false });
    if (rateTnkUsdAu instanceof Error) {
      return new Error('Invalid TNK settlement rate.');
    }
    if (!RATE_SOURCES.has(value.rate_source)) return new Error('Unsupported TNK settlement rate source.');
    if (!Number.isSafeInteger(value.rate_ts) || value.rate_ts < 0) {
      return new Error('Invalid TNK settlement rate timestamp.');
    }
    if (!Number.isSafeInteger(value.provider_count) || value.provider_count < 0) {
      return new Error('Invalid TNK settlement total.');
    }
    for (const key of ['provider_au', 'operator_fee_au', 'gross_au']) {
      const amount = this.normalizeAu(value[key], `TNK settlement ${key}`, { allowZero: key !== 'gross_au' });
      if (amount instanceof Error) return new Error('Invalid TNK settlement total.');
    }
    const gross = this.safeAddAu(value.provider_au, value.operator_fee_au);
    if (gross instanceof Error) return gross;
    if (this.compareAu(gross, value.gross_au) !== 0) return new Error('TNK settlement gross amount does not balance.');
    const tnkE18 = this.parseTnkE18(value.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;
    if (!Array.isArray(value.outputs) || value.outputs.length === 0) {
      return new Error('TNK settlement outputs are required.');
    }
    if (value.msb_tx_hashes.length !== value.outputs.length) {
      return new Error('TNK settlement MSB tx hash count must match outputs.');
    }
    return null;
  }

  normalizeTnkSettlementOutputs(outputs) {
    if (!Array.isArray(outputs) || outputs.length === 0) {
      return new Error('TNK settlement outputs are required.');
    }
    const providers = new Set();
    const providerOutputs = [];
    const operatorOutputs = [];
    for (const output of outputs) {
      if (!output || typeof output !== 'object' || Array.isArray(output)) {
        return new Error('Invalid TNK settlement output.');
      }
      if (output.role === 'provider') {
        const shapeError = this.validateExactObjectKeys(
          output,
          ['role', 'provider', 'to', 'au', 'tnk_e18'],
          'TNK settlement provider output'
        );
        if (shapeError) return shapeError;
        if (!this.isHexBytes(output.provider, 32) || output.provider !== output.provider.toLowerCase()) {
          return new Error('Invalid TNK settlement provider id.');
        }
        if (providers.has(output.provider)) return new Error('Duplicate TNK settlement provider output.');
        providers.add(output.provider);
        const normalized = this.normalizeTnkSettlementOutput(output);
        if (normalized instanceof Error) return normalized;
        providerOutputs.push(normalized);
      } else if (output.role === 'operator_fee') {
        const shapeError = this.validateExactObjectKeys(
          output,
          ['role', 'to', 'au', 'tnk_e18'],
          'TNK settlement operator output'
        );
        if (shapeError) return shapeError;
        if (operatorOutputs.length > 0) return new Error('Duplicate TNK settlement operator output.');
        const normalized = this.normalizeTnkSettlementOutput(output);
        if (normalized instanceof Error) return normalized;
        operatorOutputs.push(normalized);
      } else {
        return new Error('Invalid TNK settlement output role.');
      }
    }
    providerOutputs.sort((left, right) => compareCodepoint(left.provider, right.provider));
    return [...providerOutputs, ...operatorOutputs];
  }

  normalizeTnkSettlementOutput(output) {
    if (!this.isSafeKeyPart(output.to)) return new Error('Invalid TNK settlement output address.');
    const au = this.normalizeAu(output.au, 'TNK settlement output amount', { allowZero: false });
    if (au instanceof Error) {
      return new Error('Invalid TNK settlement output amount.');
    }
    const tnkE18 = this.parseTnkE18(output.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;
    return output.role === 'provider'
      ? {
          role: 'provider',
          provider: output.provider,
          to: output.to,
          au,
          tnk_e18: output.tnk_e18,
        }
      : {
          role: 'operator_fee',
          to: output.to,
          au,
          tnk_e18: output.tnk_e18,
        };
  }

  tnkSettlementTotals(outputs, rateTnkUsdAu) {
    let providerAu = ZERO_AU;
    let operatorFeeAu = ZERO_AU;
    let tnkE18 = 0n;
    let providerCount = 0;
    for (const output of outputs) {
      const expectedTnkE18 = this.auToTnkE18Ceil(output.au, rateTnkUsdAu);
      if (expectedTnkE18 instanceof Error) return expectedTnkE18;
      if (output.tnk_e18 !== expectedTnkE18.toString()) {
        return new Error('TNK settlement output amount does not match oracle rate.');
      }
      const parsed = this.parseTnkE18(output.tnk_e18);
      if (parsed instanceof Error) return parsed;
      tnkE18 += parsed;
      if (output.role === 'provider') {
        providerCount += 1;
        providerAu = this.safeAddAu(providerAu, output.au);
        if (providerAu instanceof Error) return providerAu;
      } else {
        operatorFeeAu = this.safeAddAu(operatorFeeAu, output.au);
        if (operatorFeeAu instanceof Error) return operatorFeeAu;
      }
    }
    const grossAu = this.safeAddAu(providerAu, operatorFeeAu);
    if (grossAu instanceof Error) return grossAu;
    return {
      provider_count: providerCount,
      provider_au: providerAu,
      operator_fee_au: operatorFeeAu,
      gross_au: grossAu,
      tnk_e18: tnkE18.toString(),
    };
  }

  auToTnkE18Ceil(au, rateTnkUsdAu) {
    const amount = this.parseAu(au, 'TNK settlement amount', { allowZero: false });
    if (amount instanceof Error) return new Error('Invalid TNK settlement amount.');
    const rate = this.parseAu(rateTnkUsdAu, 'TNK/USD atto-rate', { allowZero: false });
    if (rate instanceof Error) return new Error('Invalid TNK/USD atto-rate.');
    const numerator = amount * TNK_E18;
    const denominator = rate;
    return (numerator + denominator - 1n) / denominator;
  }

  async tnkSettlementTransferRoot(outputs) {
    return await this.opaqueHash('mayhem-tnk-settlement-transfer-root-v1', outputs);
  }

  tnkSettlementRecord(outputs) {
    return {
      type: 'tnk_settlement',
      op: 'tnk_settlement',
      idempotency_key: `mayhem:tnk:settle:v1:${this.value.network}:mayhem:${this.value.epoch}:${this.value.epoch_apply_hash}`,
      epoch: this.value.epoch,
      at: this.value.at,
      rail: 'tnk',
      network: this.value.network,
      treasury_from: this.value.treasury_from,
      operator_to: this.value.operator_to,
      epoch_apply_hash: this.value.epoch_apply_hash,
      rate_tnk_usd_au: this.value.rate_tnk_usd_au,
      rate_source: this.value.rate_source,
      rate_ts: this.value.rate_ts,
      msb_tx_hashes: this.value.msb_tx_hashes,
      transfer_root: this.value.transfer_root,
      provider_count: this.value.provider_count,
      provider_au: this.value.provider_au,
      operator_fee_au: this.value.operator_fee_au,
      gross_au: this.value.gross_au,
      tnk_e18: this.value.tnk_e18,
      outputs,
    };
  }

  async fiatSettlement() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateFiatSettlementValue(this.value);
    if (shapeError) return shapeError;
    const settlementParams = await this.activeParamsAt(this.value.at, ['max_fiat_settlement_outputs']);
    if (this.value.outputs.length > settlementParams.max_fiat_settlement_outputs) {
      return new Error('Fiat settlement output count exceeds max_fiat_settlement_outputs.');
    }

    const outputs = this.normalizeFiatSettlementOutputs(this.value.outputs);
    if (outputs instanceof Error) return outputs;
    if (stableJson(outputs) !== stableJson(this.value.outputs)) {
      return new Error('Fiat settlement outputs must be canonical.');
    }
    const totals = this.fiatSettlementTotals(outputs);
    if (totals instanceof Error) return totals;
    if (totals.provider_count !== this.value.provider_count) {
      return new Error('Fiat settlement provider count does not match outputs.');
    }
    if (this.compareAu(totals.provider_au, this.value.provider_au) !== 0) {
      return new Error('Fiat settlement provider amount does not match outputs.');
    }
    if (this.compareAu(totals.operator_fee_au, this.value.operator_fee_au) !== 0) {
      return new Error('Fiat settlement operator fee does not match outputs.');
    }
    if (this.compareAu(totals.gross_au, this.value.gross_au) !== 0) {
      return new Error('Fiat settlement gross amount does not match outputs.');
    }

    const transferRoot = await this.fiatSettlementTransferRoot(outputs);
    if (transferRoot !== this.value.transfer_root) {
      return new Error('Fiat settlement transfer root does not match outputs.');
    }

    const record = this.fiatSettlementRecord(outputs);
    const key = `settle/fiat/${this.value.epoch}`;
    const existing = await this.get(key);
    if (existing) {
      if (stableJson(existing) === stableJson(record)) {
        return {
          ok: true,
          op: 'fiatSettlement',
          epoch: this.value.epoch,
          rail: 'fiat',
          idempotent: true,
          stripe_refs: this.value.stripe_refs,
        };
      }
      return new Error('Fiat settlement already exists for epoch.');
    }

    const applyState = await this.epochApplyStateRecord();
    if (
      applyState.updated_epoch !== this.value.epoch ||
      applyState.last_apply_hash !== this.value.epoch_apply_hash
    ) {
      return new Error('Fiat settlement epoch apply hash does not match current state.');
    }

    const params = await this.activeParamsAt(this.value.at, [
      'holdback_epochs',
      'challenge_epochs',
      'canary_probe_holdback_bps',
      'canary_probe_release_min_passes',
    ]);
    const earningUpdates = [];
    for (const [outputIndex, output] of outputs.entries()) {
      if (output.role !== 'provider') continue;
      const stripeRef = this.value.stripe_refs[outputIndex];
      if (!stripeRef.startsWith('tr_')) {
        return new Error('Fiat provider settlement requires a Stripe transfer id.');
      }
      const provider = await this.get(`prov/${output.provider}`);
      if (!provider) return new Error('Provider not found.');
      if (provider.status !== 'active' && provider.status !== 'banned') {
        return new Error('Fiat settlement provider status is not payable.');
      }
      const payoutError = await this.requireAdminSetPayoutTarget(provider);
      if (payoutError) return payoutError;
      if (provider.payout.method !== 'stripe') {
        return new Error('Fiat settlement provider payout target must be Stripe.');
      }
      if (provider.payout.addr !== output.to) {
        return new Error('Fiat settlement provider payout target mismatch.');
      }
      if (provider.payout.currency !== output.currency) {
        return new Error('Fiat settlement provider payout currency mismatch.');
      }

      const earning = await this.earningRecord(output.provider, 'fiat');
      if (earning instanceof Error) return earning;
      const earningError = this.guardianValidateEarningRecord(earning, output.provider, 'fiat');
      if (earningError) return earningError;
      const probeGate = await this.probeGateForEarning(output.provider, earning, params);
      if (probeGate instanceof Error) return probeGate;
      const lockedEarningEpochs = this.providerLockedEarningEpochs(provider, params);
      if (lockedEarningEpochs instanceof Error) return lockedEarningEpochs;
      const refreshed = this.refreshEarningHoldback(
        earning,
        this.value.epoch,
        lockedEarningEpochs,
        probeGate
      );
      if (refreshed instanceof Error) return refreshed;
      const payable = this.safeSubAu(
        this.safeSubAu(refreshed.total_au, refreshed.held_au),
        refreshed.paid_cum_au
      );
      if (payable instanceof Error) return payable;
      const transferable = this.fiatWholeMinorTransferAu(payable);
      if (transferable instanceof Error) return transferable;
      if (this.isZeroAu(transferable)) return new Error('Fiat settlement provider has no whole-cent payable earnings.');
      if (this.compareAu(output.au, transferable) !== 0) {
        return new Error('Fiat settlement provider amount does not match payable whole-cent earnings.');
      }
      const paidCumAu = this.safeAddAu(refreshed.paid_cum_au, output.au);
      if (paidCumAu instanceof Error) return paidCumAu;
      const nextEarning = {
        ...refreshed,
        paid_cum_au: paidCumAu,
        updated_epoch: Math.max(refreshed.updated_epoch, this.value.epoch),
        last_settlement_epoch: this.value.epoch,
        last_settlement_stripe_ref: stripeRef,
      };
      const nextError = this.guardianValidateEarningRecord(nextEarning, output.provider, 'fiat');
      if (nextError) return nextError;
      earningUpdates.push(nextEarning);
    }

    const fee = await this.feeCumRecord('fiat');
    if (fee instanceof Error) return fee;
    const feeError = this.guardianValidateFeeRecord(fee, 'fiat');
    if (feeError) return feeError;
    const payableFee = this.safeSubAu(fee.cum_au, fee.swept_cum_au);
    if (payableFee instanceof Error) return payableFee;
    const transferableFee = this.fiatWholeMinorTransferAu(payableFee);
    if (transferableFee instanceof Error) return transferableFee;
    if (this.compareAu(transferableFee, this.value.operator_fee_au) !== 0) {
      return new Error('Fiat settlement operator fee does not match fee state.');
    }
    const operatorOutputs = outputs.filter((entry) => entry.role === 'operator_fee');
    if (this.isZeroAu(transferableFee) && operatorOutputs.length !== 0) {
      return new Error('Fiat settlement has unexpected operator fee output.');
    }
    if (this.compareAu(transferableFee, ZERO_AU) > 0) {
      if (operatorOutputs.length !== 1) return new Error('Fiat settlement missing operator fee output.');
      if (operatorOutputs[0].to !== this.value.operator_to) {
        return new Error('Fiat settlement operator target mismatch.');
      }
    }
    const operatorOutputIndex = outputs.findIndex((entry) => entry.role === 'operator_fee');
    const sweptCumAu = this.safeAddAu(fee.swept_cum_au, this.value.operator_fee_au);
    if (sweptCumAu instanceof Error) return sweptCumAu;
    const nextFee = {
      ...fee,
      swept_cum_au: sweptCumAu,
      updated_epoch: Math.max(fee.updated_epoch, this.value.epoch),
      last_settlement_epoch: this.value.epoch,
      last_settlement_stripe_ref: operatorOutputIndex >= 0
        ? this.value.stripe_refs[operatorOutputIndex]
        : (fee.last_settlement_stripe_ref ?? null),
    };
    const nextFeeError = this.guardianValidateFeeRecord(nextFee, 'fiat');
    if (nextFeeError) return nextFeeError;

    for (const earning of earningUpdates) {
      await this.put(this.earningKey(earning.provider, 'fiat'), earning);
    }
    await this.put(this.feeCumKey('fiat'), nextFee);
    await this.put(key, record);
    console.log('mayhem fiatSettlement', {
      epoch: this.value.epoch,
      provider_au: this.value.provider_au,
      operator_fee_au: this.value.operator_fee_au,
      gross_au: this.value.gross_au,
      stripe_refs: this.value.stripe_refs,
    });
    return {
      ok: true,
      op: 'fiatSettlement',
      epoch: this.value.epoch,
      rail: 'fiat',
      processor: 'stripe',
      idempotent: false,
      provider_au: this.value.provider_au,
      operator_fee_au: this.value.operator_fee_au,
      gross_au: this.value.gross_au,
      stripe_refs: this.value.stripe_refs,
      transfer_root: this.value.transfer_root,
    };
  }

  validateFiatSettlementValue(value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      [
        'op',
        'epoch',
        'at',
        'rail',
        'processor',
        'operator_to',
        'epoch_apply_hash',
        'stripe_refs',
        'transfer_root',
        'provider_count',
        'provider_au',
        'operator_fee_au',
        'gross_au',
        'outputs',
      ],
      'Fiat settlement'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'fiat_settlement') return new Error('Invalid fiat settlement op.');
    if (value.rail !== 'fiat') return new Error('Fiat settlement rail must be fiat.');
    if (value.processor !== 'stripe') return new Error('Fiat settlement processor must be stripe.');
    if (!Number.isSafeInteger(value.epoch) || value.epoch < 1) return new Error('Invalid fiat settlement epoch.');
    if (!Number.isSafeInteger(value.at) || value.at < 0) return new Error('Invalid fiat settlement timestamp.');
    if (!this.isSafeKeyPart(value.operator_to)) return new Error('Invalid fiat operator target.');
    if (!this.isHexBytes(value.epoch_apply_hash, 32)) return new Error('Invalid fiat settlement apply hash.');
    if (!Array.isArray(value.stripe_refs) || value.stripe_refs.length === 0) {
      return new Error('Fiat settlement requires one Stripe reference per output.');
    }
    const seenRefs = new Set();
    for (const ref of value.stripe_refs) {
      if (!this.isSafeKeyPart(ref)) return new Error('Invalid fiat settlement Stripe reference.');
      if (seenRefs.has(ref)) return new Error('Duplicate fiat settlement Stripe reference.');
      seenRefs.add(ref);
    }
    if (!this.isHexBytes(value.transfer_root, 32)) return new Error('Invalid fiat settlement transfer root.');
    if (!Number.isSafeInteger(value.provider_count) || value.provider_count < 0) {
      return new Error('Invalid fiat settlement total.');
    }
    for (const key of ['provider_au', 'operator_fee_au', 'gross_au']) {
      const amount = this.normalizeAu(value[key], `Fiat settlement ${key}`, { allowZero: key !== 'gross_au' });
      if (amount instanceof Error) return new Error('Invalid fiat settlement total.');
    }
    const gross = this.safeAddAu(value.provider_au, value.operator_fee_au);
    if (gross instanceof Error) return gross;
    if (this.compareAu(gross, value.gross_au) !== 0) return new Error('Fiat settlement gross amount does not balance.');
    if (!Array.isArray(value.outputs) || value.outputs.length === 0) {
      return new Error('Fiat settlement outputs are required.');
    }
    if (value.stripe_refs.length !== value.outputs.length) {
      return new Error('Fiat settlement Stripe reference count must match outputs.');
    }
    return null;
  }

  normalizeFiatSettlementOutputs(outputs) {
    if (!Array.isArray(outputs) || outputs.length === 0) {
      return new Error('Fiat settlement outputs are required.');
    }
    const providers = new Set();
    const providerOutputs = [];
    const operatorOutputs = [];
    for (const output of outputs) {
      if (!output || typeof output !== 'object' || Array.isArray(output)) {
        return new Error('Invalid fiat settlement output.');
      }
      if (output.role === 'provider') {
        const shapeError = this.validateExactObjectKeys(
          output,
          ['role', 'provider', 'to', 'currency', 'amount_minor', 'au'],
          'Fiat settlement provider output'
        );
        if (shapeError) return shapeError;
        if (!this.isHexBytes(output.provider, 32) || output.provider !== output.provider.toLowerCase()) {
          return new Error('Invalid fiat settlement provider id.');
        }
        if (providers.has(output.provider)) return new Error('Duplicate fiat settlement provider output.');
        providers.add(output.provider);
        const normalized = this.normalizeFiatSettlementOutput(output);
        if (normalized instanceof Error) return normalized;
        providerOutputs.push(normalized);
      } else if (output.role === 'operator_fee') {
        const shapeError = this.validateExactObjectKeys(
          output,
          ['role', 'to', 'currency', 'amount_minor', 'au'],
          'Fiat settlement operator output'
        );
        if (shapeError) return shapeError;
        if (operatorOutputs.length > 0) return new Error('Duplicate fiat settlement operator output.');
        const normalized = this.normalizeFiatSettlementOutput(output);
        if (normalized instanceof Error) return normalized;
        operatorOutputs.push(normalized);
      } else {
        return new Error('Invalid fiat settlement output role.');
      }
    }
    providerOutputs.sort((left, right) => compareCodepoint(left.provider, right.provider));
    return [...providerOutputs, ...operatorOutputs];
  }

  normalizeFiatSettlementOutput(output) {
    if (!this.isSafeKeyPart(output.to)) return new Error('Invalid fiat settlement output target.');
    const currency = this.normalizeFiatCurrency(output.currency);
    if (currency instanceof Error) return currency;
    const amountMinor = this.normalizeFiatMinor(output.amount_minor);
    if (amountMinor instanceof Error) return amountMinor;
    const au = this.normalizeAu(output.au, 'Fiat settlement output amount', { allowZero: false });
    if (au instanceof Error) return new Error('Invalid fiat settlement output amount.');
    const expectedAu = this.fiatMinorToAu(amountMinor);
    if (expectedAu instanceof Error) return expectedAu;
    if (this.compareAu(au, expectedAu) !== 0) {
      return new Error('Fiat settlement output amount must match fiat minor units.');
    }
    return output.role === 'provider'
      ? {
          role: 'provider',
          provider: output.provider,
          to: output.to,
          currency,
          amount_minor: amountMinor,
          au,
        }
      : {
          role: 'operator_fee',
          to: output.to,
          currency,
          amount_minor: amountMinor,
          au,
        };
  }

  fiatSettlementTotals(outputs) {
    let providerAu = ZERO_AU;
    let operatorFeeAu = ZERO_AU;
    let providerCount = 0;
    for (const output of outputs) {
      if (output.role === 'provider') {
        providerCount += 1;
        providerAu = this.safeAddAu(providerAu, output.au);
        if (providerAu instanceof Error) return providerAu;
      } else {
        operatorFeeAu = this.safeAddAu(operatorFeeAu, output.au);
        if (operatorFeeAu instanceof Error) return operatorFeeAu;
      }
    }
    const grossAu = this.safeAddAu(providerAu, operatorFeeAu);
    if (grossAu instanceof Error) return grossAu;
    return {
      provider_count: providerCount,
      provider_au: providerAu,
      operator_fee_au: operatorFeeAu,
      gross_au: grossAu,
    };
  }

  normalizeFiatMinor(value) {
    if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value)) {
      return new Error('Fiat minor amount must be a canonical decimal string.');
    }
    const parsed = BigInt(value);
    if (parsed <= 0n) return new Error('Fiat minor amount must be positive.');
    return parsed.toString();
  }

  fiatMinorToAu(amountMinor) {
    const parsed = this.normalizeFiatMinor(amountMinor);
    if (parsed instanceof Error) return parsed;
    return this.canonicalAu(BigInt(parsed) * FIAT_MINOR_AU);
  }

  fiatWholeMinorTransferAu(au) {
    const parsed = this.parseAu(au, 'Fiat payable amount');
    if (parsed instanceof Error) return parsed;
    return this.canonicalAu((parsed / FIAT_MINOR_AU) * FIAT_MINOR_AU);
  }

  async fiatSettlementTransferRoot(outputs) {
    return await this.opaqueHash('mayhem-fiat-settlement-transfer-root-v1', outputs);
  }

  fiatSettlementRecord(outputs) {
    return {
      type: 'fiat_settlement',
      op: 'fiat_settlement',
      idempotency_key: `mayhem:fiat:settle:v1:stripe:mayhem:${this.value.epoch}:${this.value.epoch_apply_hash}`,
      epoch: this.value.epoch,
      at: this.value.at,
      rail: 'fiat',
      processor: 'stripe',
      operator_to: this.value.operator_to,
      epoch_apply_hash: this.value.epoch_apply_hash,
      stripe_refs: this.value.stripe_refs,
      transfer_root: this.value.transfer_root,
      provider_count: this.value.provider_count,
      provider_au: this.value.provider_au,
      operator_fee_au: this.value.operator_fee_au,
      gross_au: this.value.gross_au,
      outputs,
    };
  }

  async depositTnk() {
    const consentError = await this.requireConsent();
    if (consentError) return consentError;
    if (!this.isSafeKeyPart(this.value.memo_hash)) return new Error('Invalid deposit memo hash.');
    if (!this.isSafeKeyPart(this.value.treasury_address)) return new Error('Invalid TNK treasury address.');
    const tnkE18 = this.parseTnkE18(this.value.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;
    const quotedAu = this.normalizeAu(this.value.quoted_au, 'TNK quoted credit', { allowZero: false });
    if (quotedAu instanceof Error) return quotedAu;
    const quotedRate = this.normalizeAu(this.value.rate_tnk_usd_au, 'TNK quoted rate', { allowZero: false });
    if (quotedRate instanceof Error) return quotedRate;

    const key = `dep/pending/${this.value.memo_hash}`;
    if ((await this.get(key)) !== null) return new Error('TNK deposit memo already pending.');

    const record = {
      memo_hash: this.value.memo_hash,
      user: this.address,
      status: 'pending',
      requested_at: this.tx,
      treasury_address: this.value.treasury_address,
      tnk_e18: this.value.tnk_e18,
      quoted_au: quotedAu,
      rate_tnk_usd_au: quotedRate,
      rate_source: this.value.rate_source,
    };
    await this.put(key, record);
    console.log('mayhem depositTnk', record);
    return {
      ok: true,
      op: 'depositTnk',
      memo_hash: this.value.memo_hash,
      user: this.address,
    };
  }

  async tnkDeposit() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.memo_hash)) return new Error('Invalid deposit memo hash.');

    const rate = await this.guardianRequireFreshRate(this.value.at);
    if (rate instanceof Error) return rate;
    const pendingKey = `dep/pending/${this.value.memo_hash}`;
    const pending = await this.get(pendingKey);
    if (!pending || pending.status !== 'pending') return new Error('Pending TNK deposit intent not found.');

    const tnkE18 = this.parseTnkE18(this.value.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;
    const pendingTnkE18 = this.parseTnkE18(pending.tnk_e18);
    if (pendingTnkE18 instanceof Error) return pendingTnkE18;
    if (pendingTnkE18 !== tnkE18) return new Error('TNK deposit amount does not match pending intent.');
    if (this.compareAu(pending.rate_tnk_usd_au, rate.tnk_usd_au) !== 0) {
      return new Error('TNK deposit rate does not match pending intent.');
    }
    const au = this.tnkE18ToAu(tnkE18, rate.tnk_usd_au);
    if (au instanceof Error) return au;
    if (this.isZeroAu(au)) return new Error('TNK deposit converts to zero au.');
    if (this.compareAu(pending.quoted_au, au) !== 0) {
      return new Error('TNK deposit credit does not match pending intent.');
    }

    const ledgerRail = 'tnk';
    const balance = await this.balanceRecord(pending.user, ledgerRail);
    if (balance instanceof Error) return balance;
    const nextAu = this.safeAddAu(balance.au, au);
    if (nextAu instanceof Error) return nextAu;
    const leaf = await this.depositLeafHash({
      rail: 'tnk',
      memo_hash: this.value.memo_hash,
      user_hash: await this.opaqueHash('deposit-user', pending.user),
      au,
      tnk_e18: this.value.tnk_e18,
      msb_tx_hash: this.value.msb_tx_hash,
      rate_ts: rate.ts,
      treasury_address_hash: await this.opaqueHash('deposit-treasury', pending.treasury_address),
      quoted_au: pending.quoted_au,
    });
    const depositRoot = await this.nextDepositRoot({
      epoch: this.value.epoch,
      leaf,
      au,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const record = {
      ...balance,
      rail: ledgerRail,
      au: nextAu,
      updated_at: this.tx,
      last_deposit_rail: ledgerRail,
      last_deposit_rate_ts: rate.ts,
    };
    await this.put(this.balanceKey(pending.user, ledgerRail), record);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    await this.del(pendingKey);
    console.log('mayhem tnkDeposit', {
      who: pending.user,
      au,
      tnk_e18: this.value.tnk_e18,
      rate_ts: rate.ts,
      epoch: this.value.epoch,
    });
    return {
      ok: true,
      op: 'tnkDeposit',
      who: pending.user,
      au,
      epoch: this.value.epoch,
      deposit_root: depositRoot.merkle_root,
      rate_ts: rate.ts,
    };
  }

  async tapDeposit() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    const shapeError = this.validateTapDepositValue(this.value);
    if (shapeError) return shapeError;

    const tapWei = this.parseTapWei(this.value.tap_wei);
    if (tapWei instanceof Error) return tapWei;
    const rate = await this.guardianRequireFreshTapRate(this.value.at);
    if (rate instanceof Error) return rate;
    const au = this.tapWeiToAu(tapWei, rate.tap_usd_au);
    if (au instanceof Error) return au;
    if (this.isZeroAu(au)) return new Error('TAP deposit converts to zero au.');

    const who = this.value.who.toLowerCase();
    const ethTxHash = this.value.eth_tx_hash.toLowerCase();
    const blockHash = this.value.block_hash.toLowerCase();
    const poolAddress = this.value.pool_address.toLowerCase();
    const eventSignature = this.value.event_signature.toLowerCase();
    const seenKey = `dep/tap/${ethTxHash}/${this.value.log_index}`;
    const existing = await this.get(seenKey);
    if (existing !== null) {
      return {
        ok: true,
        op: 'tapDeposit',
        duplicate: true,
        who: existing.who,
        au: ZERO_AU,
        credited_au: existing.au ?? null,
        epoch: existing.epoch ?? this.value.epoch,
        deposit_root: (await this.get(`ev/dep/${existing.epoch ?? this.value.epoch}`))?.merkle_root ?? null,
      };
    }

    const ledgerRail = 'tap';
    const balance = await this.balanceRecord(who, ledgerRail);
    if (balance instanceof Error) return balance;
    const balanceError = this.guardianValidateBalanceRecord(balance, who, ledgerRail);
    if (balanceError) return balanceError;
    const nextAu = this.safeAddAu(balance.au, au);
    if (nextAu instanceof Error) return nextAu;
    const leaf = await this.depositLeafHash({
      rail: 'tap',
      user_hash: await this.opaqueHash('deposit-user', who),
      au,
      tap_wei: this.value.tap_wei,
      tap_usd_au: rate.tap_usd_au,
      rate_ts: rate.ts,
      rate_source: rate.source,
      eth_tx_hash: ethTxHash,
      log_index: this.value.log_index,
      block_number: this.value.block_number,
      block_hash: blockHash,
      chain_id: this.value.chain_id,
      pool_address_hash: await this.opaqueHash('deposit-pool', poolAddress),
      finalized_block_number: this.value.finalized_block_number,
      confirmation_depth: this.value.confirmation_depth,
      confirmation_policy: this.value.confirmation_policy,
      event_signature: eventSignature,
      watcher_id: this.value.watcher_id,
    });
    const depositRoot = await this.nextDepositRoot({
      epoch: this.value.epoch,
      leaf,
      au,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const seen = {
      rail: 'tap',
      who,
      tap_wei: this.value.tap_wei,
      tap_usd_au: rate.tap_usd_au,
      rate_ts: rate.ts,
      rate_source: rate.source,
      au,
      eth_tx_hash: ethTxHash,
      log_index: this.value.log_index,
      block_number: this.value.block_number,
      block_hash: blockHash,
      pool_address: poolAddress,
      chain_id: this.value.chain_id,
      finalized_block_number: this.value.finalized_block_number,
      confirmation_depth: this.value.confirmation_depth,
      confirmation_policy: this.value.confirmation_policy,
      event_signature: eventSignature,
      watcher_id: this.value.watcher_id,
      epoch: this.value.epoch,
      at: this.value.at,
      credited_at: this.tx,
      credited_by: this.address,
      credited_by_role: 'admin',
    };
    const record = {
      ...balance,
      user: who,
      rail: ledgerRail,
      au: nextAu,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch),
      updated_at: this.tx,
      last_deposit_rail: 'tap',
      last_deposit_rate_ts: rate.ts,
      last_deposit_rate_source: rate.source,
      last_deposit_tap_usd_au: rate.tap_usd_au,
    };
    await this.put(seenKey, seen);
    await this.put(this.balanceKey(who, ledgerRail), record);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    console.log('mayhem tapDeposit', {
      who,
      au,
      tap_wei: this.value.tap_wei,
      tap_usd_au: rate.tap_usd_au,
      rate_ts: rate.ts,
      eth_tx_hash: ethTxHash,
      log_index: this.value.log_index,
      epoch: this.value.epoch,
    });
    return {
      ok: true,
      op: 'tapDeposit',
      duplicate: false,
      who,
      au,
      epoch: this.value.epoch,
      deposit_root: depositRoot.merkle_root,
      eth_tx_hash: ethTxHash,
      log_index: this.value.log_index,
      rate_ts: rate.ts,
    };
  }

  validateTapDepositValue(value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      [
        'op',
        'who',
        'tap_wei',
        'eth_tx_hash',
        'log_index',
        'block_number',
        'block_hash',
        'pool_address',
        'chain_id',
        'finalized_block_number',
        'confirmation_depth',
        'confirmation_policy',
        'event_signature',
        'watcher_id',
        'epoch',
        'at',
      ],
      'TAP deposit'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'tap_deposit') return new Error('Invalid TAP deposit op.');
    if (!this.isSafeKeyPart(value.who)) return new Error('Invalid TAP deposit recipient.');
    if (!this.isEthHexBytes(value.eth_tx_hash, 32)) return new Error('Invalid Ethereum tx hash.');
    if (!Number.isSafeInteger(value.log_index) || value.log_index < 0) {
      return new Error('Invalid TAP deposit log index.');
    }
    if (!Number.isSafeInteger(value.block_number) || value.block_number < 0) {
      return new Error('Invalid TAP deposit block number.');
    }
    if (!this.isEthHexBytes(value.block_hash, 32)) return new Error('Invalid TAP deposit block hash.');
    if (!this.isSafeKeyPart(value.pool_address)) return new Error('Invalid TAP pool address.');
    if (!Number.isSafeInteger(value.chain_id) || value.chain_id < 1) return new Error('Invalid TAP chain id.');
    if (!Number.isSafeInteger(value.finalized_block_number) || value.finalized_block_number < value.block_number) {
      return new Error('Invalid TAP finalized block number.');
    }
    if (!Number.isSafeInteger(value.confirmation_depth) || value.confirmation_depth < 0) {
      return new Error('Invalid TAP confirmation depth.');
    }
    if (value.confirmation_depth !== value.finalized_block_number - value.block_number) {
      return new Error('TAP confirmation depth does not match finalized block.');
    }
    if (!this.isSafeKeyPart(value.confirmation_policy)) return new Error('Invalid TAP confirmation policy.');
    if (value.confirmation_policy !== 'finalized-tag') {
      if (value.confirmation_depth < MIN_TAP_CONFIRMATION_DEPTH) {
        return new Error(`TAP confirmation depth below minimum ${MIN_TAP_CONFIRMATION_DEPTH}.`);
      }
      if (value.confirmation_policy !== `depth-${value.confirmation_depth}`) {
        return new Error('TAP confirmation policy must match the confirmed depth or use finalized-tag.');
      }
    }
    if (!this.isEthHexBytes(value.event_signature, 32)) return new Error('Invalid TAP event signature.');
    if (value.event_signature.toLowerCase() !== TAP_DEPOSIT_EVENT_SIGNATURE) {
      return new Error('TAP deposit event signature mismatch.');
    }
    if (value.watcher_id !== TAP_DEPOSIT_WATCHER_ID) return new Error('Invalid TAP deposit watcher id.');
    if (!Number.isSafeInteger(value.epoch) || value.epoch < 1) return new Error('Invalid TAP deposit epoch.');
    if (!Number.isSafeInteger(value.at) || value.at < 0) return new Error('Invalid TAP deposit timestamp.');
    const tapWei = this.parseTapWei(value.tap_wei);
    if (tapWei instanceof Error) return tapWei;
    return null;
  }

  async fiatDeposit() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!FIAT_DEPOSIT_RAILS.has(this.value.rail)) return new Error('Unsupported fiat deposit rail.');
    if (!this.isSafeKeyPart(this.value.who)) return new Error('Invalid deposit recipient.');
    if (!this.isSafeKeyPart(this.value.ext_ref_hash)) return new Error('Invalid external reference hash.');
    const fiat = this.fiatEvidenceFields();
    if (fiat instanceof Error) return fiat;
    const au = this.normalizeAu(this.value.au, 'fiat deposit amount', { allowZero: false });
    if (au instanceof Error) return au;

    const seenKey = `dep/fiat/${this.value.ext_ref_hash}`;
    const existing = await this.get(seenKey);
    if (existing !== null) {
      return {
        ok: true,
        op: 'fiatDeposit',
        duplicate: true,
        rail: existing.rail ?? 'fiat',
        processor_rail: existing.processor_rail ?? this.value.rail,
        who: existing.who,
        au: ZERO_AU,
        credited_au: existing.au ?? null,
        epoch: existing.epoch ?? this.value.epoch,
        deposit_root: (await this.get(`ev/dep/${existing.epoch ?? this.value.epoch}`))?.merkle_root ?? null,
      };
    }

    const ledgerRail = 'fiat';
    const balance = await this.balanceRecord(this.value.who, ledgerRail);
    if (balance instanceof Error) return balance;
    const balanceError = this.guardianValidateBalanceRecord(balance, this.value.who, ledgerRail);
    if (balanceError) return balanceError;
    const nextAu = this.safeAddAu(balance.au, au);
    if (nextAu instanceof Error) return nextAu;
    const leaf = await this.depositLeafHash({
      rail: ledgerRail,
      processor_rail: this.value.rail,
      user_hash: await this.opaqueHash('deposit-user', this.value.who),
      au,
      ext_ref_hash: this.value.ext_ref_hash,
      ...fiat,
    });
    const depositRoot = await this.nextDepositRoot({
      epoch: this.value.epoch,
      leaf,
      au,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const record = {
      ...balance,
      rail: ledgerRail,
      au: nextAu,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch),
      updated_at: this.tx,
      last_deposit_rail: ledgerRail,
      last_deposit_processor_rail: this.value.rail,
      ...(fiat.fiat_currency ? { last_deposit_fiat_currency: fiat.fiat_currency } : {}),
    };
    const seen = {
      rail: ledgerRail,
      processor_rail: this.value.rail,
      who: this.value.who,
      au,
      ext_ref_hash: this.value.ext_ref_hash,
      ...fiat,
      epoch: this.value.epoch,
      at: this.value.at,
      credited_at: this.tx,
      credited_by: this.address,
      credited_by_role: 'admin',
      chargeback_au_cum: ZERO_AU,
      network_absorbed_au_cum: ZERO_AU,
    };
    await this.put(seenKey, seen);
    await this.put(this.balanceKey(this.value.who, ledgerRail), record);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    console.log('mayhem fiatDeposit', {
      rail: ledgerRail,
      processor_rail: this.value.rail,
      who: this.value.who,
      au,
      ...fiat,
      epoch: this.value.epoch,
    });
    return {
      ok: true,
      op: 'fiatDeposit',
      rail: ledgerRail,
      processor_rail: this.value.rail,
      who: this.value.who,
      au,
      duplicate: false,
      epoch: this.value.epoch,
      deposit_root: depositRoot.merkle_root,
      ...fiat,
    };
  }

  async fiatChargeback() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!FIAT_DEPOSIT_RAILS.has(this.value.rail)) return new Error('Unsupported fiat chargeback rail.');
    if (!this.isSafeKeyPart(this.value.who)) return new Error('Invalid chargeback account.');
    if (!this.isSafeKeyPart(this.value.ext_ref_hash)) return new Error('Invalid external reference hash.');
    if (!this.isSafeKeyPart(this.value.dispute_ref_hash)) return new Error('Invalid dispute reference hash.');
    const fiat = this.fiatEvidenceFields();
    if (fiat instanceof Error) return fiat;
    const au = this.normalizeAu(this.value.au, 'fiat chargeback amount', { allowZero: false });
    if (au instanceof Error) return au;

    const depositSeenKey = `dep/fiat/${this.value.ext_ref_hash}`;
    const depositSeen = await this.get(depositSeenKey);
    if (depositSeen === null) return new Error('Fiat deposit reference not found.');
    if (depositSeen.who !== this.value.who) return new Error('Fiat chargeback recipient mismatch.');
    if (depositSeen.processor_rail !== this.value.rail) return new Error('Fiat chargeback processor rail mismatch.');
    const chargebackSeenKey = `${depositSeenKey}/chargeback/${this.value.dispute_ref_hash}`;
    const existingChargeback = await this.get(chargebackSeenKey);
    if (existingChargeback !== null) {
      return {
        ok: true,
        op: 'fiatChargeback',
        duplicate: true,
        rail: existingChargeback.rail ?? 'fiat',
        processor_rail: existingChargeback.processor_rail ?? this.value.rail,
        who: existingChargeback.who,
        au: ZERO_AU,
        disputed_au: existingChargeback.au ?? null,
        clawback_au: ZERO_AU,
        credited_clawback_au: existingChargeback.clawback_au ?? null,
        network_absorbed_au: ZERO_AU,
        frozen: true,
        epoch: existingChargeback.epoch ?? this.value.epoch,
        deposit_root: (await this.get(`ev/dep/${existingChargeback.epoch ?? this.value.epoch}`))?.merkle_root ?? null,
        ...fiat,
      };
    }
    const depositDisputedAuCum = this.safeAddAu(depositSeen.disputed_au_cum ?? ZERO_AU, au);
    if (depositDisputedAuCum instanceof Error) return depositDisputedAuCum;
    if (this.compareAu(depositDisputedAuCum, depositSeen.au) > 0) {
      return new Error('Fiat chargeback exceeds original deposit.');
    }

    const ledgerRail = 'fiat';
    const balance = await this.balanceRecord(this.value.who, ledgerRail);
    if (balance instanceof Error) return balance;
    const balanceError = this.guardianValidateBalanceRecord(balance, this.value.who, ledgerRail);
    if (balanceError) return balanceError;
    const clawbackAu = this.compareAu(balance.au, au) < 0 ? balance.au : au;
    const networkAbsorbedAu = this.safeSubAu(au, clawbackAu);
    if (networkAbsorbedAu instanceof Error) return networkAbsorbedAu;
    const nextAu = this.safeSubAu(balance.au, clawbackAu);
    if (nextAu instanceof Error) return nextAu;

    const leaf = await this.depositLeafHash({
      rail: ledgerRail,
      processor_rail: this.value.rail,
      user_hash: await this.opaqueHash('deposit-user', this.value.who),
      au,
      clawback_au: clawbackAu,
      network_absorbed_au: networkAbsorbedAu,
      ext_ref_hash: this.value.ext_ref_hash,
      dispute_ref_hash: this.value.dispute_ref_hash,
      reversed: true,
      ...fiat,
    });
    const depositRoot = await this.nextDepositReversalRoot({
      epoch: this.value.epoch,
      leaf,
      disputedAu: au,
      clawbackAu,
      absorbedAu: networkAbsorbedAu,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const frozen = await this.get(`frozen/${this.value.who}`);
    const frozenDisputedAuCum = this.safeAddAu(frozen?.disputed_au_cum ?? ZERO_AU, au);
    if (frozenDisputedAuCum instanceof Error) return frozenDisputedAuCum;
    const clawbackAuCum = this.safeAddAu(frozen?.clawback_au_cum ?? ZERO_AU, clawbackAu);
    if (clawbackAuCum instanceof Error) return clawbackAuCum;
    const absorbedAuCum = this.safeAddAu(frozen?.network_absorbed_au_cum ?? ZERO_AU, networkAbsorbedAu);
    if (absorbedAuCum instanceof Error) return absorbedAuCum;
    const freezeRecord = {
      user: this.value.who,
      status: 'frozen',
      reason: 'fiat_chargeback',
      rail: ledgerRail,
      processor_rail: this.value.rail,
      first_frozen_at: frozen?.first_frozen_at ?? this.tx,
      first_frozen_at_seconds: frozen?.first_frozen_at_seconds ?? this.value.at,
      updated_at: this.tx,
      updated_at_seconds: this.value.at,
      updated_epoch: Math.max(frozen?.updated_epoch ?? 0, this.value.epoch),
      dispute_count: (frozen?.dispute_count ?? 0) + 1,
      disputed_au_cum: frozenDisputedAuCum,
      clawback_au_cum: clawbackAuCum,
      network_absorbed_au_cum: absorbedAuCum,
      last_ext_ref_hash: this.value.ext_ref_hash,
      last_dispute_ref_hash: this.value.dispute_ref_hash,
      last_fiat_currency: fiat.fiat_currency,
    };
    const depositChargebackAuCum = this.safeAddAu(depositSeen.chargeback_au_cum ?? ZERO_AU, clawbackAu);
    if (depositChargebackAuCum instanceof Error) return depositChargebackAuCum;
    const depositAbsorbedAuCum = this.safeAddAu(depositSeen.network_absorbed_au_cum ?? ZERO_AU, networkAbsorbedAu);
    if (depositAbsorbedAuCum instanceof Error) return depositAbsorbedAuCum;
    const chargebackSeen = {
      rail: ledgerRail,
      processor_rail: this.value.rail,
      who: this.value.who,
      au,
      clawback_au: clawbackAu,
      network_absorbed_au: networkAbsorbedAu,
      ext_ref_hash: this.value.ext_ref_hash,
      dispute_ref_hash: this.value.dispute_ref_hash,
      ...fiat,
      epoch: this.value.epoch,
      at: this.value.at,
      credited_at: this.tx,
      credited_by: this.address,
      credited_by_role: 'admin',
    };

    await this.put(this.balanceKey(this.value.who, ledgerRail), {
      ...balance,
      rail: ledgerRail,
      au: nextAu,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch),
      updated_at: this.tx,
      last_chargeback_rail: ledgerRail,
      last_chargeback_processor_rail: this.value.rail,
      last_chargeback_fiat_currency: fiat.fiat_currency,
    });
    await this.put(depositSeenKey, {
      ...depositSeen,
      disputed_au_cum: depositDisputedAuCum,
      chargeback_au_cum: depositChargebackAuCum,
      network_absorbed_au_cum: depositAbsorbedAuCum,
      last_dispute_ref_hash: this.value.dispute_ref_hash,
      last_chargeback_at: this.tx,
      last_chargeback_at_seconds: this.value.at,
      last_chargeback_epoch: this.value.epoch,
    });
    await this.put(chargebackSeenKey, chargebackSeen);
    await this.put(`frozen/${this.value.who}`, freezeRecord);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    console.log('mayhem fiatChargeback', {
      rail: ledgerRail,
      processor_rail: this.value.rail,
      who: this.value.who,
      au,
      duplicate: false,
      clawback_au: clawbackAu,
      network_absorbed_au: networkAbsorbedAu,
      ...fiat,
      epoch: this.value.epoch,
    });
    return {
      ok: true,
      op: 'fiatChargeback',
      rail: ledgerRail,
      processor_rail: this.value.rail,
      who: this.value.who,
      au: this.value.au,
      clawback_au: clawbackAu,
      network_absorbed_au: networkAbsorbedAu,
      frozen: true,
      epoch: this.value.epoch,
      deposit_root: depositRoot.merkle_root,
      ...fiat,
    };
  }

  async currentRules() {
    return await this.get(CURRENT_RULES_KEY);
  }

  async isAdmin(sender = this.address) {
    const admin = await this.get('admin');
    return typeof admin === 'string' && admin === sender;
  }

  async requireAdmin(sender = this.address) {
    if (await this.isAdmin(sender)) return null;
    return new Error('Admin required.');
  }

  async requireConsent(sender = this.address) {
    if (!sender) return new Error('Consent required.');

    const rules = await this.currentRules();
    if (!rules) return new Error('Rules are not set.');

    const consent = await this.get(`consent/${sender}`);
    if (!consent || consent.ver !== rules.ver || consent.hash !== rules.hash) {
      return new Error(`Consent required for rules version ${rules.ver}.`);
    }
    const frozen = await this.get(`frozen/${sender}`);
    if (frozen?.status === 'frozen') return new Error('Account frozen.');
    return null;
  }

  async requireProvider(sender = this.address) {
    const provider = await this.get(`prov/${sender}`);
    if (!provider || provider.status !== 'active') return new Error('Provider registration required.');
    return null;
  }

  async requireAdminSetPayoutTarget(provider) {
    const payout = provider?.payout;
    if (!payout || typeof payout !== 'object') {
      return new Error('Provider payout target is not set.');
    }
    if (!payout.set_by || typeof payout.set_by !== 'string') {
      return new Error('Provider payout target must be admin-set.');
    }
    const admin = await this.get('admin');
    if (admin === null) {
      return new Error('Provider payout target requires a current admin key.');
    }
    if (payout.set_by !== admin) {
      return new Error('Provider payout target was not set by the current admin.');
    }
    if (payout.set_by_role !== 'admin') {
      return new Error('Provider payout target must be admin-set.');
    }
    return null;
  }

  async requireCurrentAdminPrice(enclaveId, ctxBracket = null) {
    const enclave = await this.get(`enclave/${enclaveId}`);
    const resolvedCtxBracket =
      ctxBracket ??
      (enclave && this.enclaveUsesCtxPrice(enclave)
        ? await this.defaultPriceCtxBracketForEnclave(enclave, 0)
        : null);
    if (resolvedCtxBracket instanceof Error) return resolvedCtxBracket;
    const schedule = await this.get(this.priceScheduleKey(enclaveId, resolvedCtxBracket));
    const current = schedule?.current;
    if (!current) {
      return new Error('Current admin price required before provider serving.');
    }
    if (schedule.denom !== PRICE_DENOMINATION || current.denom !== PRICE_DENOMINATION) {
      return new Error('Provider serving requires a current au_usd admin price.');
    }
    if (current.enclave_id !== enclaveId) {
      return new Error('Provider serving requires a current admin-set enclave price.');
    }
    if ((current.ctx_bracket ?? null) !== (resolvedCtxBracket ?? null)) {
      return new Error('Provider serving requires a current admin-set enclave price for the context bracket.');
    }
    if (!current.set_by || typeof current.set_by !== 'string') {
      return new Error('Provider serving requires a current admin-set enclave price.');
    }
    const admin = await this.get('admin');
    if (admin === null) {
      return new Error('Provider serving requires a current admin key.');
    }
    if (current.set_by !== admin) {
      return new Error('Provider serving requires a current price set by the current admin.');
    }
    if (current.set_by_role !== 'admin') {
      return new Error('Provider serving requires a current admin-set enclave price.');
    }
    return null;
  }

  enclaveUsesCtxPrice(enclave) {
    return this.modelClassFor(enclave) === DEFAULT_MODEL_CLASS;
  }

  enclaveCtxCapacity(enclave) {
    const caps = enclave?.caps && typeof enclave.caps === 'object' && !Array.isArray(enclave.caps)
      ? enclave.caps
      : {};
    const value = caps.ctx_max ?? caps.ctx ?? 0;
    if (!Number.isSafeInteger(value) || value < 0) {
      return new Error('Invalid enclave context capacity.');
    }
    return value;
  }

  async defaultPriceCtxBracketForEnclave(enclave, at) {
    const table = await this.ctxBracketTableAt(at);
    if (table instanceof Error) return table;
    const ctx = this.enclaveCtxCapacity(enclave);
    if (ctx instanceof Error) return ctx;
    const bracket = ctxBracketForTokens(ctx, table.brackets);
    if (!bracket) return new Error('No context bracket covers enclave context capacity.');
    return bracket;
  }

  async priceCtxMetaForEnclave(enclave, ctxBracket, at, label = 'Price') {
    if (!this.enclaveUsesCtxPrice(enclave)) {
      if (ctxBracket !== undefined && ctxBracket !== null) {
        return new Error(`${label} context bracket is only valid for text-generation enclaves.`);
      }
      return null;
    }
    const table = await this.ctxBracketTableAt(at);
    if (table instanceof Error) return table;
    const bracket = ctxBracket ?? await this.defaultPriceCtxBracketForEnclave(enclave, at);
    if (bracket instanceof Error) return bracket;
    if (!this.isSafeKeyPart(bracket)) return new Error(`Invalid ${label} context bracket.`);
    if (!Array.isArray(table.brackets) || !table.brackets.some((entry) => entry.id === bracket)) {
      return new Error(`${label} context bracket is not in the active admin table.`);
    }
    return { ctx_bracket: bracket, ctx_bracket_table_ver: table.ver };
  }

  priceScheduleKey(enclaveId, ctxBracket = null) {
    return ctxBracket ? `price/${enclaveId}/${ctxBracket}` : `price/${enclaveId}`;
  }

  priceRecordKey(enclaveId, ver, ctxBracket = null) {
    return ctxBracket ? `price/${enclaveId}/${ctxBracket}/v/${ver}` : `price/${enclaveId}/v/${ver}`;
  }

  priceMarketKey(enclaveId, ctxBracket = null) {
    return stableJson([enclaveId, ctxBracket ?? null]);
  }

  requireAdminCreatedEnclave(enclave) {
    if (enclave?.created_by_role !== 'admin') {
      return new Error('Canonical serving requires an admin-created enclave.');
    }
    return null;
  }

  requireAdminCreatedRoom(room) {
    if (room?.creator_role !== 'admin') {
      return new Error('Provider room serving requires an admin-created room.');
    }
    return null;
  }

  validateExactCommandValue(allowedKeys, opName, optionalKeys = []) {
    if (!this.value || typeof this.value !== 'object' || Array.isArray(this.value)) {
      return new Error(`${opName} value must be an object.`);
    }
    const allowed = new Set([...allowedKeys, ...optionalKeys]);
    const unknown = Object.keys(this.value).filter((key) => !allowed.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`${opName} does not accept provider-authored fields: ${unknown.join(', ')}.`);
    }
    for (const key of allowedKeys) {
      if (!hasOwn(this.value, key)) return new Error(`${opName} is missing ${key}.`);
    }
    if (hasOwn(this.value, 'op') && this.value.op !== opName) {
      return new Error(`Invalid ${opName} op.`);
    }
    return null;
  }

  validateExactObjectKeys(value, allowedKeys, label) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error(`${label} value must be an object.`);
    }
    const allowed = new Set(allowedKeys);
    const unknown = Object.keys(value).filter((key) => !allowed.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`${label} does not accept fields: ${unknown.join(', ')}.`);
    }
    for (const key of allowedKeys) {
      if (!hasOwn(value, key)) return new Error(`${label} is missing ${key}.`);
    }
    return null;
  }

  normalizeCtxBracketTable(brackets) {
    if (!Array.isArray(brackets) || brackets.length === 0 || brackets.length > 32) {
      return new Error('Context bracket table must be a non-empty array.');
    }
    const ids = new Set();
    let previousMax = 0;
    const normalized = [];
    for (let idx = 0; idx < brackets.length; idx += 1) {
      const entry = brackets[idx];
      const shapeError = this.validateExactObjectKeys(entry, ['id', 'max_ctx'], 'context bracket');
      if (shapeError) return shapeError;
      if (!this.isSafeKeyPart(entry.id)) return new Error('Invalid context bracket id.');
      if (ids.has(entry.id)) return new Error('Duplicate context bracket id.');
      ids.add(entry.id);

      const isLast = idx === brackets.length - 1;
      if (isLast) {
        if (entry.max_ctx !== null) return new Error('Last context bracket max_ctx must be null.');
        normalized.push({ id: entry.id, max_ctx: null });
        continue;
      }
      if (!Number.isSafeInteger(entry.max_ctx) || entry.max_ctx <= previousMax) {
        return new Error('Context bracket max_ctx values must increase.');
      }
      previousMax = entry.max_ctx;
      normalized.push({ id: entry.id, max_ctx: entry.max_ctx });
    }
    return normalized;
  }

  defaultCtxBracketTableRecord() {
    return {
      ver: CTX_BRACKET_TABLE_VERSION,
      brackets: cloneValue(CTX_BRACKETS),
      submitted_at: 0,
      effective_at: 0,
      effective_from: null,
      updated_at: null,
      set_by: null,
      set_by_role: 'genesis',
    };
  }

  async ctxBracketSchedule() {
    if (!this.storage) return { current: this.defaultCtxBracketTableRecord(), pending: null };
    const stored = await this.get('ctx_brackets');
    const fallback = this.defaultCtxBracketTableRecord();
    if (!stored) return { current: fallback, pending: null };
    return {
      current: stored.current ?? fallback,
      pending: stored.pending ?? null,
    };
  }

  ctxBracketLatestEntry(schedule) {
    if (schedule.pending && schedule.pending.ver > schedule.current.ver) return schedule.pending;
    return schedule.current;
  }

  ctxBracketActiveEntry(schedule, at) {
    if (schedule.pending && schedule.pending.effective_at <= at) return schedule.pending;
    return schedule.current;
  }

  async ctxBracketTableAt(at) {
    if (!Number.isSafeInteger(at) || at < 0) return new Error('Invalid context bracket timestamp.');
    return cloneValue(this.ctxBracketActiveEntry(await this.ctxBracketSchedule(), at));
  }

  async ctxBracketTableByVersion(ver) {
    if (!Number.isSafeInteger(ver) || ver < 1) return new Error('Invalid context bracket table version.');
    if (ver === CTX_BRACKET_TABLE_VERSION) return this.defaultCtxBracketTableRecord();
    const record = await this.get(`ctx_brackets/v/${ver}`);
    if (!record) return new Error('Unknown context bracket table version.');
    return cloneValue(record);
  }

  validateCtxBracketEvidence(tokens, bracket, tableVer, table, label) {
    if (typeof bracket !== 'string') return new Error(`Invalid ${label} context bracket.`);
    if (!Number.isSafeInteger(tableVer) || tableVer < 1) {
      return new Error(`Invalid ${label} context bracket table version.`);
    }
    if (!table || table.ver !== tableVer) {
      return new Error(`${label} context bracket table version mismatch.`);
    }
    if (!Array.isArray(table.brackets) || !table.brackets.some((entry) => entry.id === bracket)) {
      return new Error(`Invalid ${label} context bracket.`);
    }
    if (ctxBracketForTokens(tokens, table.brackets) !== bracket) {
      return new Error(`${label} context bracket does not match served context.`);
    }
    return null;
  }

  async normalizeCtxBracketEvidenceForEnclave(enclaveId, tokens, bracket, tableVer, table, label) {
    const enclave = await this.get(`enclave/${enclaveId}`);
    if (!enclave) return new Error(`${label} enclave not found.`);
    if (!this.enclaveUsesCtxPrice(enclave)) {
      if (bracket !== undefined && bracket !== null) {
        return new Error(`${label} context bracket is only valid for text-generation enclaves.`);
      }
      if (tableVer !== undefined && tableVer !== null) {
        return new Error(`${label} context bracket table version is only valid for text-generation enclaves.`);
      }
      return {
        enclave,
        ctx_bracket: null,
        ctx_bracket_table_ver: null,
      };
    }
    const ctxError = this.validateCtxBracketEvidence(tokens, bracket, tableVer, table, label);
    if (ctxError) return ctxError;
    return {
      enclave,
      ctx_bracket: bracket,
      ctx_bracket_table_ver: tableVer,
    };
  }

  validateProviderLifecycleIntent(intent) {
    if (!PROVIDER_LIFECYCLE_OPS.has(intent.op)) return new Error('Unsupported provider lifecycle op.');
    const allowed = intent.op === 'register_provider'
      ? ['op', 'provider', 'nonce']
      : intent.op === 'set_provider_rails'
        ? ['op', 'provider', 'rails', 'nonce']
        : intent.op === 'join_room' || intent.op === 'leave_room'
          ? ['op', 'provider', 'enclave_id', 'room_id', 'nonce']
          : intent.op === 'join_enclave'
            ? [
                'op',
                'provider',
                'enclave_id',
                'nonce',
                'served_ctx',
                'ctx_bracket',
                'ctx_bracket_table_ver',
                'hardware_fingerprint',
                'device_key',
              ]
            : ['op', 'provider', 'enclave_id', 'nonce'];
    const required = intent.op === 'join_enclave'
      ? ['op', 'provider', 'enclave_id', 'nonce', 'served_ctx', 'ctx_bracket', 'ctx_bracket_table_ver']
      : allowed;
    const allowedSet = new Set(allowed);
    const unknown = Object.keys(intent).filter((key) => !allowedSet.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`provider lifecycle intent does not accept fields: ${unknown.join(', ')}.`);
    }
    for (const key of required) {
      if (!hasOwn(intent, key)) return new Error(`provider lifecycle intent is missing ${key}.`);
    }
    if (!this.isHexBytes(intent.provider, 32)) return new Error('Invalid provider id.');
    if (!this.isHexBytes(intent.nonce, 32)) return new Error('Invalid lifecycle nonce.');
    if (hasOwn(intent, 'enclave_id') && !this.isSafeKeyPart(intent.enclave_id)) {
      return new Error('Invalid enclave id.');
    }
    if (hasOwn(intent, 'room_id') && !this.isSafeKeyPart(intent.room_id)) {
      return new Error('Invalid room id.');
    }
    if (
      hasOwn(intent, 'served_ctx') &&
      (!Number.isSafeInteger(intent.served_ctx) || intent.served_ctx < 0)
    ) {
      return new Error('Invalid provider served context.');
    }
    if (
      hasOwn(intent, 'ctx_bracket') &&
      intent.ctx_bracket !== null &&
      !this.isSafeKeyPart(intent.ctx_bracket)
    ) {
      return new Error('Invalid provider context bracket.');
    }
    if (
      hasOwn(intent, 'ctx_bracket_table_ver') &&
      intent.ctx_bracket_table_ver !== null &&
      (!Number.isSafeInteger(intent.ctx_bracket_table_ver) || intent.ctx_bracket_table_ver < 1)
    ) {
      return new Error('Invalid provider context bracket table version.');
    }
    if (hasOwn(intent, 'hardware_fingerprint') && !this.isHexBytes(intent.hardware_fingerprint, 32)) {
      return new Error('Invalid provider hardware fingerprint.');
    }
    if (hasOwn(intent, 'device_key') && !this.isHexBytes(intent.device_key, 32)) {
      return new Error('Invalid provider device key.');
    }
    if (hasOwn(intent, 'rails')) {
      const rails = this.normalizeProviderAcceptedRails(intent.rails);
      if (rails instanceof Error) return rails;
    }
    return null;
  }

  async normalizeProviderServeTerms(enclaveId, terms, label) {
    if (terms === null || terms === undefined) return null;
    if (!terms || typeof terms !== 'object' || Array.isArray(terms)) {
      return new Error(`${label} terms must be an object.`);
    }
    for (const field of ['served_ctx', 'ctx_bracket', 'ctx_bracket_table_ver']) {
      if (!hasOwn(terms, field)) return new Error(`${label} terms are missing ${field}.`);
    }
    if (!Number.isSafeInteger(terms.served_ctx) || terms.served_ctx < 0) {
      return new Error(`Invalid ${label} served context.`);
    }
    const table = terms.ctx_bracket_table_ver === null || terms.ctx_bracket_table_ver === undefined
      ? null
      : await this.ctxBracketTableByVersion(terms.ctx_bracket_table_ver);
    if (table instanceof Error) return table;
    const ctxMeta = await this.normalizeCtxBracketEvidenceForEnclave(
      enclaveId,
      terms.served_ctx,
      terms.ctx_bracket,
      terms.ctx_bracket_table_ver,
      table,
      label
    );
    if (ctxMeta instanceof Error) return ctxMeta;
    return {
      served_ctx: terms.served_ctx,
      ctx_bracket: ctxMeta.ctx_bracket,
      ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
    };
  }

  validateCommittedServeTerms(serve, normalized) {
    if (!Number.isSafeInteger(serve.served_ctx) || serve.served_ctx < 0) {
      return new Error('Provider serve record is missing committed context terms.');
    }
    if (serve.served_ctx !== normalized.served_ctx) {
      return new Error('Spend reservation served context does not match provider committed context.');
    }
    if ((serve.ctx_bracket ?? null) !== (normalized.ctx_bracket ?? null)) {
      return new Error('Spend reservation context bracket does not match provider committed context.');
    }
    if ((serve.ctx_bracket_table_ver ?? null) !== (normalized.ctx_bracket_table_ver ?? null)) {
      return new Error('Spend reservation context bracket table does not match provider committed context.');
    }
    return null;
  }

  banRecordKey(targetType, target) {
    switch (targetType) {
      case 'provider':
        return `ban/provider/${target}`;
      case 'device':
        return `ban/device/${target}`;
      case 'fingerprint':
        return `ban/fingerprint/${target}`;
      case 'committer':
        return `committer/ban/${target}`;
      default:
        return `ban/unknown/${target}`;
    }
  }

  normalizedKybIdentityValues(kyb) {
    if (!kyb || typeof kyb !== 'object') return new Error('Invalid KYB identity.');
    const legalName = typeof kyb.legal_name === 'string'
      ? kyb.legal_name.trim().replace(/\s+/g, ' ').toLowerCase()
      : '';
    const kybRef = typeof kyb.kyb_ref === 'string' ? kyb.kyb_ref.trim() : '';
    const proofHash = typeof kyb.proof_hash === 'string' ? kyb.proof_hash.toLowerCase() : '';
    if (!legalName) return new Error('Invalid KYB legal name.');
    if (!this.isSafeExternalRef(kybRef)) return new Error('Invalid provider KYB reference.');
    if (!this.isHexBytes(proofHash, 32)) return new Error('Invalid provider KYB proof hash.');
    return {
      legal_name: legalName,
      kyb_ref: kybRef,
      proof_hash: proofHash,
    };
  }

  async kybBanIndexKey(kind, value) {
    return `ban/kyb/${kind}/${await this.opaqueHash('mayhem-kyb-ban-index-v1', { kind, value })}`;
  }

  async kybBanIndexKeys(kyb) {
    const values = this.normalizedKybIdentityValues(kyb);
    if (values instanceof Error) return values;
    return [
      await this.kybBanIndexKey('legal_name', values.legal_name),
      await this.kybBanIndexKey('kyb_ref', values.kyb_ref),
      await this.kybBanIndexKey('proof_hash', values.proof_hash),
    ];
  }

  async rejectBannedProviderKyb(kyb) {
    const keys = await this.kybBanIndexKeys(kyb);
    if (keys instanceof Error) return keys;
    for (const key of keys) {
      const ban = await this.get(key);
      if (ban?.status === 'banned' || ban?.status === 'revoked') {
        return new Error('Provider KYB identity is banned or revoked.');
      }
    }
    return null;
  }

  async writeProviderKybBanIndexes(kyb, meta) {
    const keys = await this.kybBanIndexKeys(kyb);
    if (keys instanceof Error) return keys;
    const values = this.normalizedKybIdentityValues(kyb);
    if (values instanceof Error) return values;
    for (const key of keys) {
      const current = await this.get(key);
      await this.put(key, {
        ...(current ?? {}),
        target_type: 'kyb',
        target: key.split('/').at(-1),
        status: meta.status,
        provider: meta.provider,
        source: meta.source,
        reason_hash: meta.reason_hash,
        recorded_at: meta.recorded_at,
        recorded_by: meta.recorded_by,
        recorded_by_role: meta.recorded_by_role,
        legal_name_hash: await this.kybBanIndexKey('legal_name', values.legal_name).then((k) => k.split('/').at(-1)),
        kyb_ref_hash: await this.kybBanIndexKey('kyb_ref', values.kyb_ref).then((k) => k.split('/').at(-1)),
        proof_hash: values.proof_hash,
        providers: {
          ...(current?.providers ?? {}),
          [meta.provider]: {
            provider: meta.provider,
            source: meta.source,
            reason_hash: meta.reason_hash,
            recorded_at: meta.recorded_at,
          },
        },
        reversible: true,
      });
    }
    return null;
  }

  async providerLifecycleFeatureKey(intent) {
    const digest = await blake3(b4a.from(providerLifecycleIntentMessage(intent)));
    return `intent/provider/${intent.provider}/${intent.op}/${b4a.toString(digest, 'hex')}`;
  }

  async providerLifecycleFeatureKeys(intent) {
    return [await this.providerLifecycleFeatureKey(intent)];
  }

  async spendReservationFeatureKey(value) {
    const normalized = value.voucher_body ? value : await this.normalizeSpendReserveValue(value);
    if (normalized instanceof Error) return normalized;
    const digest = b4a.toString(
      await blake3(b4a.from(stableJson({
        domain: 'mayhem-spend-reservation-feature-v1',
        value: spendReservationEvidence(normalized),
      }))),
      'hex'
    );
    return `hold/reserve/${normalized.rail}/${normalized.user}/${normalized.epoch}/${normalized.session_id}/${digest}`;
  }

  async activeParamsAt(at, keys = Object.keys(PARAM_DEFINITIONS)) {
    const params = {};
    for (const key of keys) {
      params[key] = this.paramActiveEntry(await this.paramRecord(key), at).value;
    }
    return params;
  }

  async paramRecord(key) {
    const existing = await this.get(`params/${key}`);
    if (existing) return existing;
    return {
      key,
      current: {
        value: PARAM_DEFINITIONS[key].default,
        ver: 0,
        submitted_at: 0,
        effective_at: 0,
        set_at: null,
      },
      pending: null,
    };
  }

  paramActiveEntry(record, at) {
    if (record.pending && record.pending.effective_at <= at) return cloneValue(record.pending);
    return cloneValue(record.current);
  }

  validateParamValues(values) {
    if (!values || typeof values !== 'object' || Array.isArray(values)) {
      return new Error('Parameter values must be an object.');
    }
    const keys = Object.keys(values);
    if (keys.length === 0) return new Error('At least one parameter is required.');
    const keyError = this.validateParamKeys(keys);
    if (keyError) return keyError;

    for (const key of keys) {
      const value = values[key];
      const def = PARAM_DEFINITIONS[key];
      if (def.money) {
        const au = this.normalizeAu(value, `Parameter ${key}`, { allowZero: def.min === ZERO_AU });
        if (au instanceof Error) return au;
        if (this.compareAu(au, def.min) < 0) return new Error(`Parameter ${key} is out of range.`);
        continue;
      }
      if (!Number.isInteger(value)) return new Error(`Parameter ${key} must be an integer.`);
      if (value < def.min || value > def.max) return new Error(`Parameter ${key} is out of range.`);
    }
    return null;
  }

  normalizeParamValues(values) {
    const normalized = {};
    for (const [key, value] of Object.entries(values)) {
      const def = PARAM_DEFINITIONS[key];
      normalized[key] = def.money
        ? this.normalizeAu(value, `Parameter ${key}`, { allowZero: def.min === ZERO_AU })
        : value;
      if (normalized[key] instanceof Error) return normalized[key];
    }
    return normalized;
  }

  validateParamKeys(keys) {
    if (!Array.isArray(keys) || keys.length === 0 || keys.length > 64) {
      return new Error('Invalid parameter keys.');
    }
    for (const key of keys) {
      if (!hasOwn(PARAM_DEFINITIONS, key)) return new Error(`Unknown parameter ${key}.`);
    }
    return null;
  }

  validateParamBounds(params) {
    if (params.price_min_bps > params.price_max_bps) {
      return new Error('price_min_bps must not exceed price_max_bps.');
    }
    if (params.market_max_utilization_bps < params.market_target_utilization_bps) {
      return new Error('market_max_utilization_bps must be >= market_target_utilization_bps.');
    }
    return null;
  }

  validateModelRef(value) {
    if (!this.isSafeModelId(value.model_id)) return new Error('Invalid model id.');
    const classError = this.validateModelClass(this.modelClassFor(value), 'Model reference model_class');
    if (classError) return classError;
    const rateError = this.validateRateMap(value.rate_map, this.modelClassFor(value), 'Model reference rate_map');
    if (rateError) return rateError;
    if (value.source_hash !== undefined && !this.isSafeKeyPart(value.source_hash)) {
      return new Error('Invalid model reference source hash.');
    }
    return null;
  }

  validateCatalogRelease(value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      [
        'op',
        'catalog_id',
        'source_kind',
        'catalog_url',
        'signature_url',
        'catalog_hash',
        'signature_hash',
        'key_id',
        'public_key',
        'model_count',
        'artifact_count',
        'canaries',
      ],
      'catalog release'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'publish_catalog') return new Error('Invalid publish_catalog op.');
    if (!this.isSafeKeyPart(value.catalog_id)) return new Error('Invalid catalog id.');
    if (!CATALOG_SOURCE_KINDS.has(value.source_kind)) {
      return new Error('Unsupported catalog source kind.');
    }
    if (!this.isHttpsUrl(value.catalog_url) || !this.isHttpsUrl(value.signature_url)) {
      return new Error('Catalog release URLs must be HTTPS.');
    }
    if (value.source_kind === 'huggingface') {
      if (!this.isPinnedHuggingFaceResolveUrl(value.catalog_url)) {
        return new Error('Hugging Face catalog URL must use huggingface.co/resolve/<40-hex-revision>/.');
      }
      if (!this.isPinnedHuggingFaceResolveUrl(value.signature_url)) {
        return new Error('Hugging Face catalog signature URL must use huggingface.co/resolve/<40-hex-revision>/.');
      }
    }
    if (!this.isHexBytes(value.catalog_hash, 32)) {
      return new Error('Catalog hash must be a 32-byte hex BLAKE3 hash.');
    }
    if (!this.isHexBytes(value.signature_hash, 32)) {
      return new Error('Catalog signature hash must be a 32-byte hex BLAKE3 hash.');
    }
    if (!this.isSafeKeyPart(value.key_id)) return new Error('Invalid catalog key id.');
    if (!this.isHexBytes(value.public_key, 32)) {
      return new Error('Catalog public key must be 32-byte hex.');
    }
    const seen = new Set();
    for (const entry of value.canaries) {
      const entryError = this.validateCatalogCanaryRef(entry);
      if (entryError) return entryError;
      if (value.source_kind === 'huggingface' && !this.isPinnedHuggingFaceResolveUrl(entry.url)) {
        return new Error('Hugging Face catalog canary URL must use huggingface.co/resolve/<40-hex-revision>/.');
      }
      if (seen.has(entry.set_id)) return new Error('Duplicate catalog canary set.');
      seen.add(entry.set_id);
    }
    return null;
  }

  validateCatalogCanaryRef(entry) {
    const shapeError = this.validateExactObjectKeys(
      entry,
      ['set_id', 'url', 'hash'],
      'catalog canary ref'
    );
    if (shapeError) return shapeError;
    if (!this.isSafeKeyPart(entry.set_id)) return new Error('Invalid catalog canary set id.');
    if (!this.isHttpsUrl(entry.url)) return new Error('Catalog canary URL must be HTTPS.');
    if (!this.isHexBytes(entry.hash, 32)) {
      return new Error('Catalog canary hash must be a 32-byte hex BLAKE3 hash.');
    }
    return null;
  }

  validateEnclaveCaps(caps, modelClass = DEFAULT_MODEL_CLASS) {
    if (!caps || typeof caps !== 'object' || Array.isArray(caps)) {
      return new Error('Enclave caps must be an object.');
    }
    const classError = this.validateModelClass(modelClass, 'Enclave caps model_class');
    if (classError) return classError;
    const unknown = Object.keys(caps).filter((key) => !ENCLAVE_CAP_FIELDS.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`Unsupported enclave caps field: ${unknown.join(', ')}.`);
    }
    for (const key of ENCLAVE_CAP_BOOLEAN_FIELDS) {
      if (hasOwn(caps, key) && typeof caps[key] !== 'boolean') {
        return new Error(`Enclave caps ${key} must be a boolean.`);
      }
    }
    const hasCtx = hasOwn(caps, 'ctx');
    const hasCtxMax = hasOwn(caps, 'ctx_max');
    for (const key of ENCLAVE_CAP_INTEGER_FIELDS) {
      if (hasOwn(caps, key) && (!Number.isSafeInteger(caps[key]) || caps[key] <= 0)) {
        return new Error(`Enclave caps ${key} must be a positive integer.`);
      }
    }
    if (hasOwn(caps, 'vllm_gpu_memory_utilization_pct') && caps.vllm_gpu_memory_utilization_pct > 100) {
      return new Error('Enclave caps vllm_gpu_memory_utilization_pct must be between 1 and 100.');
    }
    for (const key of ENCLAVE_CAP_STRING_FIELDS) {
      if (hasOwn(caps, key) && (typeof caps[key] !== 'string' || caps[key].length === 0 || caps[key].length > 64)) {
        return new Error(`Enclave caps ${key} must be a non-empty string with at most 64 characters.`);
      }
    }
    if (hasCtx && hasCtxMax && caps.ctx !== caps.ctx_max) {
      return new Error('Enclave caps ctx and ctx_max must match when both are set.');
    }
    const allowedOutputModalities = MODEL_CLASS_OUTPUT_MODALITIES[modelClass] ?? new Set();
    const validateOutputModality = (modality, label) => {
      if (typeof modality !== 'string' || modality.length === 0 || modality.length > 32) {
        return new Error(`Enclave caps ${label} must be a non-empty string.`);
      }
      if (!CAP_OUTPUT_MODALITIES.has(modality)) {
        return new Error(`Unsupported enclave caps ${label}: ${modality}.`);
      }
      if (!allowedOutputModalities.has(modality)) {
        return new Error(`Enclave caps ${label} ${modality} is not allowed for model_class ${modelClass}.`);
      }
      return null;
    };
    const outputModalities = new Set();
    if (hasOwn(caps, 'output_modality')) {
      const error = validateOutputModality(caps.output_modality, 'output_modality');
      if (error) return error;
      outputModalities.add(caps.output_modality);
    }
    if (hasOwn(caps, 'output_modalities')) {
      if (!Array.isArray(caps.output_modalities) || caps.output_modalities.length === 0 || caps.output_modalities.length > 8) {
        return new Error('Enclave caps output_modalities must be a non-empty array with at most 8 entries.');
      }
      const seenOutputModalities = new Set();
      for (const modality of caps.output_modalities) {
        const error = validateOutputModality(modality, 'output_modalities entry');
        if (error) return error;
        if (seenOutputModalities.has(modality)) {
          return new Error(`Enclave caps output_modalities has duplicate modality ${modality}.`);
        }
        seenOutputModalities.add(modality);
        outputModalities.add(modality);
      }
      if (hasOwn(caps, 'output_modality') && !caps.output_modalities.includes(caps.output_modality)) {
        return new Error('Enclave caps output_modalities must include output_modality.');
      }
    }
    for (const [flag, modality] of [['image', 'image'], ['video', 'video']]) {
      if (caps[flag] === true && !allowedOutputModalities.has(modality)) {
        return new Error(`Enclave caps ${flag} output is not allowed for model_class ${modelClass}.`);
      }
    }
    return null;
  }

  validateEnclaveArtifactBinding(value) {
    const classError = this.validateModelClass(this.modelClassFor(value), 'Enclave model_class');
    if (classError) return classError;
    if (!ENCLAVE_BACKENDS.has(value.backend)) return new Error('Unsupported enclave backend.');
    if (!this.isHexBytes(value.artifact_root, 32)) {
      return new Error('Enclave artifact_root must be a 32-byte hex Merkle root.');
    }
    if (value.artifact_root_kind !== ENCLAVE_ARTIFACT_ROOT_KIND) {
      return new Error(`Enclave artifact_root_kind must be ${ENCLAVE_ARTIFACT_ROOT_KIND}.`);
    }
    if (!this.isHexBytes(value.manifest_hash, 32)) {
      return new Error('Enclave manifest_hash must be 32-byte hex.');
    }
    if (!this.isHexBytes(value.binary_hash, 32)) {
      return new Error('Enclave binary_hash must be 32-byte hex.');
    }
    if (
      value.source_sha256 !== undefined &&
      value.source_sha256 !== null &&
      !this.isHexBytes(value.source_sha256, 32)
    ) {
      return new Error('Enclave source_sha256 must be 32-byte hex.');
    }
    const sourceError = this.validateHuggingFaceArtifactSource(value.artifact_source, 'enclave artifact_source');
    if (sourceError) return sourceError;
    return this.validateEnclaveArtifactSidecars(value.artifact_sidecars ?? {});
  }

  normalizeEnclaveLaunchMeasurements(value) {
    if (value === undefined || value === null) return null;
    if (!value || typeof value !== 'object' || Array.isArray(value)) return cloneValue(value);
    if (hasOwn(value, 'layers')) {
      return {
        schema_version: value.schema_version ?? 1,
        effective_epoch: value.effective_epoch ?? 0,
        ...(hasOwn(value, 'platform') ? { platform: value.platform } : {}),
        layers: cloneValue(value.layers),
      };
    }
    if (hasOwn(value, 'measurements')) {
      return {
        schema_version: value.schema_version ?? 1,
        effective_epoch: value.effective_epoch ?? 0,
        ...(hasOwn(value, 'platform') ? { platform: value.platform } : {}),
        layers: { workload: cloneValue(value.measurements) },
      };
    }
    const measurements = cloneValue(value);
    delete measurements.schema_version;
    delete measurements.effective_epoch;
    delete measurements.platform;
    delete measurements.layers;
    return {
      schema_version: 1,
      effective_epoch: value.effective_epoch ?? 0,
      ...(hasOwn(value, 'platform') ? { platform: value.platform } : {}),
      layers: { workload: measurements },
    };
  }

  validateEnclaveLaunchMeasurements(value, attTier) {
    if (attTier < 3 && (value === undefined || value === null)) return null;
    if (attTier >= 3 && (value === undefined || value === null)) {
      return new Error('Tier-3 enclaves require admin-published launch_measurements.');
    }
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Enclave launch_measurements must be an object.');
    }
    const allowed = new Set(['schema_version', 'effective_epoch', 'platform', 'layers']);
    const unknown = Object.keys(value).filter((key) => !allowed.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`enclave launch measurements does not accept fields: ${unknown.join(', ')}.`);
    }
    if (value.schema_version !== 1) {
      return new Error('Enclave launch_measurements schema_version must be 1.');
    }
    if (!Number.isSafeInteger(value.effective_epoch) || value.effective_epoch < 0) {
      return new Error('Enclave launch_measurements effective_epoch must be a non-negative integer.');
    }
    if (attTier >= 3 && !hasOwn(value, 'platform')) {
      return new Error('Tier-3 launch_measurements must include a platform label.');
    }
    if (hasOwn(value, 'platform') && !this.isSafeKeyPart(value.platform)) {
      return new Error('Enclave launch_measurements platform must be a safe label.');
    }
    const layers = value.layers;
    if (!layers || typeof layers !== 'object' || Array.isArray(layers)) {
      return new Error('Enclave launch_measurements layers must be an object.');
    }
    let count = 0;
    let workloadCount = 0;
    let names = 0;
    for (const [layer, measurements] of Object.entries(layers)) {
      if (!this.isTier3MeasurementLayer(layer)) {
        return new Error('Enclave launch_measurements layers must be vendor or workload.');
      }
      if (!measurements || typeof measurements !== 'object' || Array.isArray(measurements)) {
        return new Error('Enclave launch_measurements layer values must be objects.');
      }
      const layerNames = Object.keys(measurements);
      const layerCount = this.countLaunchMeasurementValues(measurements);
      names += layerNames.length;
      count += layerCount;
      if (layer === 'workload') workloadCount += layerCount;
      for (const [name, measurement] of Object.entries(measurements)) {
        if (!this.isSafeLaunchMeasurementName(name)) {
          return new Error('Enclave launch_measurements names must be non-empty safe labels.');
        }
        const measurementError = this.validateLaunchMeasurementValueList(name, measurement, `Enclave launch_measurements ${layer}`);
        if (measurementError) return measurementError;
      }
    }
    if (attTier >= 3 && count === 0) {
      return new Error('Tier-3 enclaves require at least one launch measurement.');
    }
    if (attTier >= 3 && workloadCount === 0) {
      return new Error('Tier-3 enclaves require workload PCR/stack measurements.');
    }
    if (names > TIER3_MEASUREMENT_MAX_NAMES) {
      return new Error('Enclave launch_measurements may contain at most 32 measurements.');
    }
    if (count > TIER3_MEASUREMENT_MAX_VALUES) {
      return new Error('Enclave launch_measurements may contain at most 128 measurement values.');
    }
    return null;
  }

  countLaunchMeasurementValues(measurements) {
    let count = 0;
    for (const value of Object.values(measurements ?? {})) {
      if (typeof value === 'string') count += 1;
      else if (Array.isArray(value)) count += value.length;
      else if (value && typeof value === 'object' && Array.isArray(value.values)) count += value.values.length;
      else if (value && typeof value === 'object' && typeof value.measurement === 'string') count += 1;
    }
    return count;
  }

  isSafeLaunchMeasurementName(value) {
    return typeof value === 'string' && value.length > 0 && value.length <= 64 && /^[A-Za-z0-9_.:-]+$/.test(value);
  }

  isTier3MeasurementLayer(value) {
    return value === 'vendor' || value === 'workload';
  }

  validateLaunchMeasurementHex(label, measurement) {
    if (
      typeof measurement !== 'string' ||
      !/^[0-9a-fA-F]+$/.test(measurement) ||
      measurement.length < 64 ||
      measurement.length > 256 ||
      measurement.length % 2 !== 0
    ) {
      return new Error(`${label} must be a 32-128 byte hex value.`);
    }
    return null;
  }

  validateLaunchMeasurementValueList(name, value, label) {
    if (typeof value === 'string') {
      return this.validateLaunchMeasurementHex(`${label} ${name}`, value);
    }
    if (Array.isArray(value)) {
      if (value.length === 0) return new Error(`${label} ${name} must not be empty.`);
      for (const item of value) {
        const measurement = typeof item === 'string' ? item : item?.measurement;
        const error = this.validateLaunchMeasurementHex(`${label} ${name}`, measurement);
        if (error) return error;
      }
      return null;
    }
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      if (Array.isArray(value.values)) return this.validateLaunchMeasurementValueList(name, value.values, label);
      return this.validateLaunchMeasurementHex(`${label} ${name}`, value.measurement);
    }
    return new Error(`${label} ${name} must be a 32-128 byte hex value or array of values.`);
  }

  validateTier3MeasurementBlessValue(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Tier-3 measurement blessing value must be an object.');
    }
    const required = ['op', 'platform', 'layer', 'measurement_name', 'measurement', 'effective_epoch', 'at'];
    const allowed = new Set([...required, 'region', 'derivation_hash', 'source']);
    const unknown = Object.keys(value).filter((key) => !allowed.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`tier3 measurement blessing does not accept fields: ${unknown.join(', ')}.`);
    }
    for (const key of required) {
      if (!hasOwn(value, key)) return new Error(`Tier-3 measurement blessing is missing ${key}.`);
    }
    if (value.op !== 'tier3_bless_measurement') return new Error('Invalid Tier-3 measurement blessing op.');
    if (!this.isSafeKeyPart(value.platform)) return new Error('Invalid Tier-3 platform.');
    if (!this.isTier3MeasurementLayer(value.layer)) return new Error('Invalid Tier-3 measurement layer.');
    if (!this.isSafeLaunchMeasurementName(value.measurement_name)) return new Error('Invalid Tier-3 measurement name.');
    const measurementError = this.validateLaunchMeasurementHex('Tier-3 measurement', value.measurement);
    if (measurementError) return measurementError;
    if (!Number.isSafeInteger(value.effective_epoch) || value.effective_epoch < 0) {
      return new Error('Invalid Tier-3 measurement effective epoch.');
    }
    if (!Number.isSafeInteger(value.at) || value.at < 0) {
      return new Error('Invalid Tier-3 measurement timestamp.');
    }
    if (hasOwn(value, 'region') && value.region !== null && !this.isSafeKeyPart(value.region)) {
      return new Error('Invalid Tier-3 measurement region.');
    }
    if (hasOwn(value, 'derivation_hash') && value.derivation_hash !== null && !this.isHexBytes(value.derivation_hash, 32)) {
      return new Error('Invalid Tier-3 derivation hash.');
    }
    if (hasOwn(value, 'source') && (typeof value.source !== 'string' || value.source.length < 1 || value.source.length > 64)) {
      return new Error('Invalid Tier-3 measurement source.');
    }
    return null;
  }

  normalizeEnclaveQuant(value) {
    const normalized = String(value ?? 'unknown').trim().toLowerCase().replace(/_/g, '-');
    if (ENCLAVE_QUANT_BUCKETS.has(normalized)) return normalized;
    return this.quantBucketFromDescriptor(normalized);
  }

  quantBucketFromDescriptor(descriptor) {
    if (descriptor.includes('nvfp4')) return 'nvfp4';
    if (descriptor.includes('fp8')) return 'fp8';
    if (descriptor.includes('bf16')) return 'bf16';
    if (descriptor.includes('fp16') || descriptor.includes('f16')) return 'fp16';
    if (descriptor.includes('int8') || descriptor.includes('8bit') || descriptor.includes('q8')) return 'int8';
    if (descriptor.includes('int4') || descriptor.includes('4bit') || descriptor.includes('q4')) return 'int4';
    return descriptor;
  }

  validateEnclaveQuant(value) {
    if (typeof value !== 'string') return new Error('Enclave quant must be a string.');
    const quant = this.normalizeEnclaveQuant(value);
    if (!ENCLAVE_QUANT_BUCKETS.has(quant)) {
      return new Error('Enclave quant must be one of unknown, fp32, fp16, bf16, fp8, nvfp4, int8, int4.');
    }
    return null;
  }

  validateEnclaveArtifactSidecars(sidecars) {
    if (!sidecars || typeof sidecars !== 'object' || Array.isArray(sidecars)) {
      return new Error('Enclave artifact_sidecars must be an object.');
    }
    const names = Object.keys(sidecars).sort();
    if (names.length > 16) return new Error('Enclave artifact_sidecars has too many entries.');
    for (const name of names) {
      if (!this.isSafeHuggingFacePathSegment(name)) {
        return new Error('Enclave artifact_sidecars keys must be safe names.');
      }
      const sidecar = sidecars[name];
      const shapeError = this.validateExactObjectKeys(
        sidecar,
        ['source', 'path', 'artifact_root', 'artifact_root_kind', 'weights_bytes', 'source_sha256'],
        `enclave artifact_sidecars.${name}`
      );
      if (shapeError) return shapeError;
      const sourceError = this.validateHuggingFaceArtifactSource(
        sidecar.source,
        `enclave artifact_sidecars.${name}.source`
      );
      if (sourceError) return sourceError;
      if (!this.isSafeHuggingFacePath(sidecar.path)) {
        return new Error(`Enclave artifact_sidecars.${name}.path must be a safe relative Hugging Face artifact path.`);
      }
      if (sidecar.source.path !== sidecar.path) {
        return new Error(`Enclave artifact_sidecars.${name}.source.path must match path.`);
      }
      if (!this.isHexBytes(sidecar.artifact_root, 32)) {
        return new Error(`Enclave artifact_sidecars.${name}.artifact_root must be a 32-byte hex Merkle root.`);
      }
      if (sidecar.artifact_root_kind !== ENCLAVE_ARTIFACT_ROOT_KIND) {
        return new Error(`Enclave artifact_sidecars.${name}.artifact_root_kind must be ${ENCLAVE_ARTIFACT_ROOT_KIND}.`);
      }
      if (!Number.isSafeInteger(sidecar.weights_bytes) || sidecar.weights_bytes <= 0) {
        return new Error(`Enclave artifact_sidecars.${name}.weights_bytes must be a positive integer.`);
      }
      if (!this.isHexBytes(sidecar.source_sha256, 32)) {
        return new Error(`Enclave artifact_sidecars.${name}.source_sha256 must be 32-byte hex.`);
      }
    }
    return null;
  }

  validateHuggingFaceArtifactSource(source, label = 'enclave artifact_source') {
    const shapeError = this.validateExactObjectKeys(
      source,
      ['kind', 'repo', 'revision', 'path'],
      label
    );
    if (shapeError) return shapeError;
    if (source.kind !== 'huggingface') {
      return new Error(`${label}.kind must be huggingface.`);
    }
    if (!this.isSafeHuggingFaceRepo(source.repo)) {
      return new Error(`${label}.repo must be a safe namespace/name repo id.`);
    }
    if (!this.isHexBytes(source.revision, 20)) {
      return new Error(`${label}.revision must be a 20-byte git commit hex.`);
    }
    if (!this.isSafeHuggingFacePath(source.path)) {
      return new Error(`${label}.path must be a safe relative Hugging Face artifact path.`);
    }
    return null;
  }

  validateRoomPolicy(policy) {
    if (!policy || typeof policy !== 'object' || Array.isArray(policy)) {
      return new Error('Room policy must be an object.');
    }
    const unknown = Object.keys(policy).filter((key) => !ROOM_POLICY_FIELDS.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`Unsupported room policy field: ${unknown.join(', ')}.`);
    }
    for (const key of ['region_hint', 'canary_set']) {
      if (
        hasOwn(policy, key) &&
        (
          typeof policy[key] !== 'string' ||
          policy[key].length === 0 ||
          policy[key].length > 128
        )
      ) {
        return new Error(`Room policy ${key} must be a non-empty string.`);
      }
    }
    if (
      hasOwn(policy, 'min_reputation') &&
      (
        typeof policy.min_reputation !== 'number' ||
        !Number.isFinite(policy.min_reputation) ||
        policy.min_reputation < 0 ||
        policy.min_reputation > 1
      )
    ) {
      return new Error('Room policy min_reputation must be between 0 and 1.');
    }
    if (
      hasOwn(policy, 'max_price_mult') &&
      (
        typeof policy.max_price_mult !== 'number' ||
        !Number.isFinite(policy.max_price_mult) ||
        policy.max_price_mult <= 0
      )
    ) {
      return new Error('Room policy max_price_mult must be positive.');
    }
    return null;
  }

  async priceSchedule(key, enclave, ctxMeta = null) {
    const existing = await this.get(key);
    if (existing) return existing;
    return {
      enclave_id: enclave.enclave_id,
      model_id: enclave.model_id,
      denom: PRICE_DENOMINATION,
      ...(ctxMeta ? {
        ctx_bracket: ctxMeta.ctx_bracket,
        ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
      } : {}),
      current: null,
      pending: null,
    };
  }

  priceScheduleAt(schedule, at) {
    const updated = cloneValue(schedule);
    if (updated.pending && updated.pending.effective_at <= at) {
      updated.current = updated.pending;
      updated.pending = null;
    }
    return updated;
  }

  priceActiveEntry(schedule, at) {
    if (schedule.pending && schedule.pending.effective_at <= at) return cloneValue(schedule.pending);
    return schedule.current ? cloneValue(schedule.current) : null;
  }

  priceLatestEntry(schedule) {
    return schedule.pending ?? schedule.current;
  }

  priceSeedSnapshot(record) {
    if (!record) return null;
    return {
      enclave_id: record.enclave_id,
      model_id: record.model_id,
      denom: record.denom,
      ver: record.seed_ver ?? record.ver,
      ...(record.ctx_bracket ? { ctx_bracket: record.ctx_bracket } : {}),
      ...(record.ctx_bracket_table_ver ? { ctx_bracket_table_ver: record.ctx_bracket_table_ver } : {}),
      rate_map: cloneValue(record.seed_rate_map ?? record.rate_map),
      per_req_au: record.seed_per_req_au ?? record.per_req_au,
      min_session_au: record.seed_min_session_au ?? record.min_session_au,
      effective_at: record.seed_effective_at ?? record.effective_at,
      effective_from: record.seed_effective_from ?? record.effective_from,
      updated_at: record.seed_updated_at ?? record.updated_at,
      set_by: record.seed_by ?? record.set_by,
      set_by_role: record.seed_by_role ?? record.set_by_role,
    };
  }

  priceSeedEntry(record) {
    return record?.seed ? cloneValue(record.seed) : this.priceSeedSnapshot(record);
  }

  priceLatestSeedEntry(schedule) {
    if (schedule.pending) return this.priceSeedEntry(schedule.pending);
    if (schedule.current) return this.priceSeedEntry(schedule.current);
    return null;
  }

  marketPriceParamKeys() {
    return [
      'market_target_utilization_bps',
      'market_ema_alpha_bps',
      'market_gain_bps',
      'market_max_step_bps',
      'market_cold_start_min_providers',
      'market_provider_epoch_target_au',
      'market_max_utilization_bps',
      'market_below_target_discount_bps',
      'market_above_target_slope_bps',
    ];
  }

  marketPriceConstants(params) {
    return {
      target_utilization_bps: params.market_target_utilization_bps,
      ema_alpha_bps: params.market_ema_alpha_bps,
      gain_bps: params.market_gain_bps,
      max_step_bps: params.market_max_step_bps,
      cold_start_min_providers: params.market_cold_start_min_providers,
      provider_epoch_target_au: params.market_provider_epoch_target_au,
      max_utilization_bps: params.market_max_utilization_bps,
      below_target_discount_bps: params.market_below_target_discount_bps,
      above_target_slope_bps: params.market_above_target_slope_bps,
    };
  }

  marketUtilizationBps(demandAu, activeSupply, constants) {
    const demand = this.parseAu(demandAu, 'market demand');
    if (demand instanceof Error) return new Error('Invalid market demand.');
    if (!Number.isSafeInteger(activeSupply) || activeSupply < 0) {
      return new Error('Invalid market active supply.');
    }
    if (activeSupply === 0) return 0;
    const providerEpochTarget = this.parseAu(
      constants.provider_epoch_target_au,
      'market provider epoch target',
      { allowZero: false }
    );
    if (providerEpochTarget instanceof Error) {
      return new Error('Invalid market supply capacity.');
    }
    const capacityAu = BigInt(activeSupply) * providerEpochTarget;
    if (capacityAu <= 0n) return new Error('Invalid market supply capacity.');
    const util = (demand * 10_000n) / capacityAu;
    const capped = util > BigInt(constants.max_utilization_bps)
      ? BigInt(constants.max_utilization_bps)
      : util;
    return Number(capped);
  }

  marketEmaUtilizationBps(previousEmaBps, utilizationBps, constants) {
    const previous = Number.isSafeInteger(previousEmaBps)
      ? previousEmaBps
      : constants.target_utilization_bps;
    return Math.floor(
      (
        previous * (10_000 - constants.ema_alpha_bps) +
        utilizationBps * constants.ema_alpha_bps
      ) / 10_000
    );
  }

  marketCurveMultiplierBps(utilizationBps, constants) {
    if (utilizationBps <= constants.target_utilization_bps) {
      const shortfall = constants.target_utilization_bps - utilizationBps;
      return 10_000 - Math.floor(
        (shortfall * constants.below_target_discount_bps) /
        constants.target_utilization_bps
      );
    }
    const excess = utilizationBps - constants.target_utilization_bps;
    const denominator = 10_000 - constants.target_utilization_bps;
    return 10_000 + Math.floor((excess * constants.above_target_slope_bps) / denominator);
  }

  scalePriceTerm(term, multiplierBps) {
    const amount = this.parseAu(term, 'price term');
    if (amount instanceof Error || !Number.isSafeInteger(multiplierBps) || multiplierBps < 0) {
      return new Error('Invalid price term.');
    }
    if (amount === 0n) return ZERO_AU;
    const scaled = (amount * BigInt(multiplierBps) + 5_000n) / 10_000n;
    return this.canonicalAu(scaled > 0n ? scaled : 1n);
  }

  stepPriceTerm(current, desired, constants) {
    const currentAu = this.parseAu(current, 'current price term');
    const desiredAu = this.parseAu(desired, 'desired price term');
    if (currentAu instanceof Error || desiredAu instanceof Error) return new Error('Invalid price term.');
    if (currentAu === desiredAu) return this.canonicalAu(currentAu);
    if (currentAu === 0n) return this.canonicalAu(desiredAu);
    const delta = currentAu > desiredAu ? currentAu - desiredAu : desiredAu - currentAu;
    const gainedRaw = (delta * BigInt(constants.gain_bps)) / 10_000n;
    const maxStepRaw = (currentAu * BigInt(constants.max_step_bps)) / 10_000n;
    const gained = gainedRaw > 0n ? gainedRaw : 1n;
    const maxStep = maxStepRaw > 0n ? maxStepRaw : 1n;
    const step = gained < maxStep ? gained : maxStep;
    return this.canonicalAu(
      desiredAu > currentAu
        ? currentAu + step
        : (step > currentAu ? 0n : currentAu - step)
    );
  }

  scaleRateMap(rateMap, multiplierBps) {
    const scaled = [];
    for (const entry of rateMap) {
      const perUnitAu = this.scalePriceTerm(entry.per_unit_au, multiplierBps);
      if (perUnitAu instanceof Error) return perUnitAu;
      scaled.push({ ...entry, per_unit_au: perUnitAu });
    }
    return this.normalizeRateMap(scaled);
  }

  stepRateMap(currentRateMap, desiredRateMap, constants) {
    const desiredByUnit = this.rateMapByUnit(desiredRateMap);
    const stepped = [];
    for (const entry of currentRateMap) {
      const desired = desiredByUnit.get(entry.unit);
      if (!desired || desired.granularity !== entry.granularity) {
        return new Error('Market price rate_map shape changed.');
      }
      const perUnitAu = this.stepPriceTerm(entry.per_unit_au, desired.per_unit_au, constants);
      if (perUnitAu instanceof Error) return perUnitAu;
      stepped.push({ ...entry, per_unit_au: perUnitAu });
    }
    return this.normalizeRateMap(stepped);
  }

  priceTermsEqual(left, right) {
    return (
      stableJson(left.rate_map) === stableJson(right.rate_map) &&
      this.compareAu(left.per_req_au, right.per_req_au) === 0 &&
      this.compareAu(left.min_session_au, right.min_session_au) === 0
    );
  }

  async computeMarketPriceUpdates(marketUsageMap, context = {}) {
    const at = context.at ?? this.value.at;
    const epoch = context.epoch ?? this.value.epoch;
    const epochSeconds = context.epochSeconds ?? null;
    const tx = context.tx ?? this.tx;
    const marketParams = await this.activeParamsAt(at, this.marketPriceParamKeys());
    const constants = this.marketPriceConstants(marketParams);
    const updates = [];
    for (const usage of this.mapMarketUsageEntriesForHash(marketUsageMap)) {
      const enclave = await this.get(`enclave/${usage.enclave_id}`);
      if (!enclave || enclave.status !== 'active') return new Error('Market usage enclave is not active.');
      const ctxMeta = await this.priceCtxMetaForEnclave(enclave, usage.ctx_bracket, at, 'Market usage');
      if (ctxMeta instanceof Error) return ctxMeta;
      if (ctxMeta && usage.ctx_bracket_table_ver !== undefined && usage.ctx_bracket_table_ver !== ctxMeta.ctx_bracket_table_ver) {
        return new Error('Market usage context bracket table version is not active for the epoch.');
      }
      const scheduleKey = this.priceScheduleKey(usage.enclave_id, ctxMeta?.ctx_bracket ?? null);
      const storedSchedule = await this.priceSchedule(scheduleKey, enclave, ctxMeta);
      const schedule = this.priceScheduleAt(storedSchedule, at);
      const current = this.priceActiveEntry(schedule, at);
      if (!current) return new Error('Market usage enclave has no admin price seed.');
      const seed = this.priceSeedEntry(current);
      if (!seed || seed.set_by_role !== 'admin') {
        return new Error('Market price requires an admin seed.');
      }

      const activeSupply = usage.provider_count;
      const previousMarket = current.market && typeof current.market === 'object'
        ? current.market
        : {};
      const utilizationBps = this.marketUtilizationBps(usage.demand_au, activeSupply, constants);
      if (utilizationBps instanceof Error) return utilizationBps;
      const frozen = activeSupply < constants.cold_start_min_providers;
      const emaUtilizationBps = frozen
        ? constants.target_utilization_bps
        : this.marketEmaUtilizationBps(previousMarket.ema_utilization_bps, utilizationBps, constants);
      const multiplierBps = frozen
        ? 10_000
        : this.marketCurveMultiplierBps(emaUtilizationBps, constants);
      const desiredRateMap = this.scaleRateMap(seed.rate_map, multiplierBps);
      if (desiredRateMap instanceof Error) return desiredRateMap;
      const desiredPerReqAu = this.scalePriceTerm(seed.per_req_au, multiplierBps);
      if (desiredPerReqAu instanceof Error) return desiredPerReqAu;
      const desiredMinSessionAu = this.scalePriceTerm(seed.min_session_au, multiplierBps);
      if (desiredMinSessionAu instanceof Error) return desiredMinSessionAu;

      const nextTerms = frozen
        ? {
            rate_map: cloneValue(seed.rate_map),
            per_req_au: seed.per_req_au,
            min_session_au: seed.min_session_au,
          }
        : {
            rate_map: this.stepRateMap(current.rate_map, desiredRateMap, constants),
            per_req_au: this.stepPriceTerm(current.per_req_au, desiredPerReqAu, constants),
            min_session_au: this.stepPriceTerm(current.min_session_au, desiredMinSessionAu, constants),
          };
      if (nextTerms.rate_map instanceof Error) return nextTerms.rate_map;
      if (nextTerms.per_req_au instanceof Error) return nextTerms.per_req_au;
      if (nextTerms.min_session_au instanceof Error) return nextTerms.min_session_au;
      const latest = this.priceLatestEntry(schedule);
      const record = {
        enclave_id: current.enclave_id,
        model_id: current.model_id,
        denom: PRICE_DENOMINATION,
        ver: latest ? latest.ver + 1 : 1,
        rate_map: nextTerms.rate_map,
        per_req_au: nextTerms.per_req_au,
        min_session_au: nextTerms.min_session_au,
        effective_at: at,
        effective_from: tx,
        updated_at: tx,
        set_by: seed.set_by,
        set_by_role: 'admin',
        ...(ctxMeta ? {
          ctx_bracket: ctxMeta.ctx_bracket,
          ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
        } : {}),
        price_source: frozen ? 'admin_seed_cold_start' : 'market_float',
        seed,
        market: {
          schema_version: 1,
          source: 'settled_epoch_usage',
          epoch,
          epoch_seconds: epochSeconds,
          active_supply: activeSupply,
          active_demand_au: usage.demand_au,
          session_count: usage.session_count,
          ...(ctxMeta ? {
            ctx_bracket: ctxMeta.ctx_bracket,
            ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
          } : {}),
          utilization_bps: utilizationBps,
          ema_utilization_bps: emaUtilizationBps,
          multiplier_bps: multiplierBps,
          desired_rate_map: desiredRateMap,
          desired_per_req_au: desiredPerReqAu,
          desired_min_session_au: desiredMinSessionAu,
          frozen,
          frozen_reason: frozen ? 'cold_start_min_providers' : null,
          constants,
          previous_price_ver: current.ver,
          previous_rate_map: cloneValue(current.rate_map),
          previous_per_req_au: current.per_req_au,
          previous_min_session_au: current.min_session_au,
          seed_price_ver: seed.ver,
        },
      };
      const updatedSchedule = {
        ...schedule,
        current: record,
      };
      updates.push({
        enclave_id: usage.enclave_id,
        ...(ctxMeta ? {
          ctx_bracket: ctxMeta.ctx_bracket,
          ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
        } : {}),
        market_key: this.priceMarketKey(usage.enclave_id, ctxMeta?.ctx_bracket ?? null),
        ver: record.ver,
        rate_map: record.rate_map,
        utilization_bps: utilizationBps,
        ema_utilization_bps: emaUtilizationBps,
        active_supply: activeSupply,
        active_demand_au: usage.demand_au,
        frozen,
        schedule_key: scheduleKey,
        schedule: updatedSchedule,
        record_key: this.priceRecordKey(usage.enclave_id, record.ver, ctxMeta?.ctx_bracket ?? null),
        record,
      });
    }
    return updates;
  }

  modelClassFor(value) {
    if (!value || !hasOwn(value, 'model_class') || value.model_class === null || value.model_class === undefined) {
      return undefined;
    }
    return value.model_class;
  }

  validateModelClass(modelClass, label) {
    if (typeof modelClass !== 'string' || modelClass.length === 0 || modelClass.length > 64) {
      return new Error(`${label} must be a non-empty string.`);
    }
    if (!MODEL_CLASSES.has(modelClass)) return new Error(`Unsupported ${label}.`);
    return null;
  }

  validateLaunchEnclaveAttestationTier(attTier) {
    if (attTier <= MAX_LAUNCH_ENCLAVE_ATTESTATION_TIER) return null;
    return new Error(
      `Enclave attestation tiers above ${MAX_LAUNCH_ENCLAVE_ATTESTATION_TIER} are not launch-advertisable; Tier 4 is provider KYB, not enclave hardware.`
    );
  }

  async currentAppliedEpoch() {
    const state = await this.get('epoch/apply/state');
    const epoch = state?.epoch ?? 0;
    return Number.isSafeInteger(epoch) && epoch >= 0 ? epoch : 0;
  }

  async enclaveMinTierPolicy(enclave, currentEpoch = null) {
    const epoch = currentEpoch ?? await this.currentAppliedEpoch();
    const policy = await this.get(`tierpolicy/enclave/${enclave.enclave_id}`);
    const baseMinTier = policy?.current_min_att_tier ?? enclave.min_att_tier ?? enclave.att_tier ?? 1;
    const pending = policy?.pending ?? enclave.pending_min_att_tier ?? null;
    if (pending && pending.effective_epoch <= epoch) {
      return {
        min_att_tier: pending.min_att_tier,
        pending: null,
        effective_epoch: pending.effective_epoch,
        current_epoch: epoch,
      };
    }
    return {
      min_att_tier: baseMinTier,
      pending,
      current_epoch: epoch,
    };
  }

  validateRateMap(rateMap, modelClass, label, { allowZeroPrice = false } = {}) {
    if (!Array.isArray(rateMap) || rateMap.length === 0 || rateMap.length > RATE_MAP_MAX_ENTRIES) {
      return new Error(`${label} must be a non-empty array with at most ${RATE_MAP_MAX_ENTRIES} entries.`);
    }
    const validUnits = MODEL_CLASS_RATE_UNITS[modelClass];
    if (!validUnits) return new Error(`No rate units configured for model_class ${modelClass}.`);
    const seen = new Set();
    for (const entry of rateMap) {
      const shapeError = this.validateExactObjectKeys(entry, ['unit', 'per_unit_au', 'granularity'], `${label} entry`);
      if (shapeError) return shapeError;
      if (typeof entry.unit !== 'string' || entry.unit.length === 0 || entry.unit.length > 64) {
        return new Error(`${label} unit must be a non-empty string.`);
      }
      if (!validUnits.has(entry.unit)) return new Error(`${label} unit ${entry.unit} is not allowed for model_class ${modelClass}.`);
      if (seen.has(entry.unit)) return new Error(`${label} has duplicate unit ${entry.unit}.`);
      seen.add(entry.unit);
      const perUnitAu = this.normalizeAu(entry.per_unit_au, `${label} per_unit_au`, { allowZero: allowZeroPrice });
      if (perUnitAu instanceof Error || (!allowZeroPrice && this.isZeroAu(perUnitAu))) {
        return new Error(`${label} per_unit_au must be ${allowZeroPrice ? 'a non-negative' : 'a positive'} integer.`);
      }
      if (!Number.isSafeInteger(entry.granularity) || entry.granularity <= 0) {
        return new Error(`${label} granularity must be a positive integer.`);
      }
    }
    return null;
  }

  normalizeRateMap(rateMap) {
    return rateMap
      .map((entry) => ({
        unit: entry.unit,
        per_unit_au: this.normalizeAu(entry.per_unit_au),
        granularity: entry.granularity,
      }))
      .sort((left, right) => compareCodepoint(left.unit, right.unit));
  }

  normalizeLockedRateMap(rateMap, label) {
    if (!Array.isArray(rateMap) || rateMap.length === 0 || rateMap.length > RATE_MAP_MAX_ENTRIES) {
      return new Error(`${label} must be a non-empty array with at most ${RATE_MAP_MAX_ENTRIES} entries.`);
    }
    const seen = new Set();
    const normalized = [];
    for (const entry of rateMap) {
      const shapeError = this.validateExactObjectKeys(entry, ['unit', 'per_unit_au', 'granularity'], `${label} entry`);
      if (shapeError) return shapeError;
      if (typeof entry.unit !== 'string' || entry.unit.length === 0 || entry.unit.length > 64) {
        return new Error(`${label} unit must be a non-empty string.`);
      }
      const unit = this.canonicalUsageUnit(entry.unit);
      if (!this.isSafeKeyPart(unit)) return new Error(`${label} unit is invalid.`);
      if (seen.has(unit)) return new Error(`${label} has duplicate unit ${unit}.`);
      seen.add(unit);
      const perUnitAu = this.normalizeAu(entry.per_unit_au, `${label} per_unit_au`, { allowZero: false });
      if (perUnitAu instanceof Error) {
        return new Error(`${label} per_unit_au must be a positive integer.`);
      }
      if (!Number.isSafeInteger(entry.granularity) || entry.granularity <= 0) {
        return new Error(`${label} granularity must be a positive integer.`);
      }
      normalized.push({
        unit,
        per_unit_au: perUnitAu,
        granularity: entry.granularity,
      });
    }
    return normalized.sort((left, right) => compareCodepoint(left.unit, right.unit));
  }

  rateMapByUnit(rateMap) {
    const byUnit = new Map();
    for (const entry of rateMap ?? []) byUnit.set(entry.unit, entry);
    return byUnit;
  }

  validateRateMapBounds(priceRateMap, referenceRateMap, params) {
    const priceByUnit = this.rateMapByUnit(priceRateMap);
    const referenceByUnit = this.rateMapByUnit(referenceRateMap);
    if (priceByUnit.size !== referenceByUnit.size) {
      return new Error('Price rate_map units must match model reference rate_map units.');
    }
    for (const [unit, ref] of referenceByUnit.entries()) {
      const price = priceByUnit.get(unit);
      if (!price) return new Error(`Price rate_map is missing model reference unit ${unit}.`);
      if (!this.rateWithinBounds(price, ref, params)) {
        return new Error(`Price rate_map unit ${unit} outside model reference bounds.`);
      }
    }
    return null;
  }

  rateWithinBounds(price, ref, params = {
    price_min_bps: PARAM_DEFINITIONS.price_min_bps.default,
    price_max_bps: PARAM_DEFINITIONS.price_max_bps.default,
  }) {
    const refPerUnit = this.parseAu(ref?.per_unit_au, 'reference rate per_unit_au', { allowZero: false });
    const pricePerUnit = this.parseAu(price?.per_unit_au, 'price rate per_unit_au');
    if (
      refPerUnit instanceof Error ||
      !Number.isSafeInteger(ref?.granularity) ||
      ref.granularity <= 0 ||
      pricePerUnit instanceof Error ||
      !Number.isSafeInteger(price?.granularity) ||
      price.granularity <= 0
    ) {
      return false;
    }
    const priceScaled = pricePerUnit * BigInt(ref.granularity) * 10_000n;
    const refScaled = refPerUnit * BigInt(price.granularity);
    return (
      priceScaled >= refScaled * BigInt(params.price_min_bps) &&
      priceScaled <= refScaled * BigInt(params.price_max_bps)
    );
  }

  validateReputationEvent(value) {
    if (!this.isSafeKeyPart(value.event_id)) return new Error('Invalid reputation event id.');
    if (!REPUTATION_EVENT_KINDS.has(value.kind)) return new Error('Unsupported reputation event kind.');
    if (
      (value.kind === 'session_ok' || value.kind === 'session_partial') &&
      this.normalizeAu(value.paid_au, 'reputation paid amount') instanceof Error
    ) {
      return new Error('Reputation event requires paid_au.');
    }
    if (
      value.kind === 'session_fail' &&
      this.normalizeAu(value.max_spend_au, 'reputation max spend') instanceof Error
    ) {
      return new Error('Reputation event requires max_spend_au.');
    }
    return null;
  }

  validateReputationAnchor(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Invalid reputation anchor payload.');
    }
    if (value.op !== 'anchor_reputation') return new Error('Invalid reputation anchor op.');
    if (!this.isSafeKeyPart(value.provider)) return new Error('Invalid reputation provider.');
    if (!Number.isSafeInteger(value.epoch) || value.epoch < 0) {
      return new Error('Invalid reputation anchor epoch.');
    }
    if (!Number.isSafeInteger(value.folded_at) || value.folded_at < 0) {
      return new Error('Invalid reputation anchor folded_at.');
    }
    if (!this.isHexBytes(value.events_head, 32)) return new Error('Invalid reputation events head.');
    if (!Number.isSafeInteger(value.r_bps) || value.r_bps < 0 || value.r_bps > 10_000) {
      return new Error('Invalid reputation r_bps.');
    }
    if (!Number.isSafeInteger(value.raw_milli)) return new Error('Invalid reputation raw_milli.');
    if (!Number.isSafeInteger(value.successful_sessions) || value.successful_sessions < 0) {
      return new Error('Invalid reputation successful_sessions.');
    }
    if (
      value.provenance_violation !== undefined &&
      typeof value.provenance_violation !== 'boolean'
    ) {
      return new Error('Invalid reputation provenance_violation.');
    }
    return null;
  }

  validateProbeResult(value) {
    if (!this.isSafeKeyPart(value.probe_id)) return new Error('Invalid probe id.');
    if (!PROBE_KINDS.has(value.probe_kind)) return new Error('Unsupported probe kind.');
    if (value.probe_kind === 'canary') {
      if (!value.enclave_id) return new Error('Canary probe requires enclave_id.');
      if (!value.binary_hash) return new Error('Canary probe requires binary_hash.');
      if (!Number.isInteger(value.match_bps)) return new Error('Canary probe requires match_bps.');
      if (typeof value.pass !== 'boolean') return new Error('Canary probe requires pass.');
      if (!value.canary_set) return new Error('Canary probe requires canary_set.');
      if (!value.verification_method) return new Error('Canary probe requires verification_method.');
      if (!PROBE_VERIFICATION_METHODS.has(value.verification_method)) {
        return new Error('Unsupported canary verification_method.');
      }
      if (!value.session_receipt_hash) return new Error('Canary probe requires session_receipt_hash.');
      if (!value.evidence_hash) return new Error('Canary probe requires evidence_hash.');
      if (!value.auditor_sig) return new Error('Canary probe requires auditor_sig.');
      if (!this.isSafeKeyPart(value.enclave_id)) return new Error('Invalid canary enclave id.');
      if (!this.isHexBytes(value.binary_hash, 32)) return new Error('Invalid canary binary hash.');
      if (!this.isHexBytes(value.session_receipt_hash, 32)) {
        return new Error('Invalid canary session receipt hash.');
      }
      if (!this.isHexBytes(value.evidence_hash, 32)) return new Error('Invalid canary evidence hash.');
      if (!this.isHexBytes(value.auditor_sig, 64)) return new Error('Invalid canary auditor signature.');
    }
    return null;
  }

  async normalizeSpendVoucherForReserve(voucher) {
    const shapeError = this.validateExactObjectKeys(
      voucher,
      [
        'session_id',
        'rail',
        'enclave_id',
        'price_ver',
        'locked_rate_map',
        'locked_per_req_au',
        'locked_min_session_au',
        'served_ctx',
        'ctx_bracket',
        'ctx_bracket_table_ver',
        'max_spend_au',
        'checkpoint_every',
        'user_sig',
      ],
      'spend voucher'
    );
    if (shapeError) return shapeError;
    const checkpointError = this.validateExactObjectKeys(
      voucher.checkpoint_every,
      ['tokens', 'ms'],
      'spend voucher checkpoint policy'
    );
    if (checkpointError) return checkpointError;
    const rail = this.normalizeLedgerRail(voucher.rail, 'spend voucher rail');
    if (rail instanceof Error) return rail;
    if (!this.isHexBytes(voucher.session_id, 32)) return new Error('Invalid spend voucher session id.');
    if (!this.isHexBytes(voucher.enclave_id, 32)) return new Error('Invalid spend voucher enclave id.');
    if (!Number.isSafeInteger(voucher.price_ver) || voucher.price_ver < 1) {
      return new Error('Invalid spend voucher price version.');
    }
    const lockedRateMap = this.normalizeLockedRateMap(voucher.locked_rate_map, 'spend voucher locked_rate_map');
    if (lockedRateMap instanceof Error) return lockedRateMap;
    const lockedPerReqAu = this.normalizeAu(voucher.locked_per_req_au, 'spend voucher locked per-request price');
    if (lockedPerReqAu instanceof Error) {
      return new Error('Invalid spend voucher locked per-request price.');
    }
    const lockedMinSessionAu = this.normalizeAu(voucher.locked_min_session_au, 'spend voucher locked minimum session price');
    if (lockedMinSessionAu instanceof Error) {
      return new Error('Invalid spend voucher locked minimum session price.');
    }
    if (!Number.isSafeInteger(voucher.served_ctx) || voucher.served_ctx < 0) {
      return new Error('Invalid spend voucher served context.');
    }
    const table = voucher.ctx_bracket_table_ver === null || voucher.ctx_bracket_table_ver === undefined
      ? null
      : await this.ctxBracketTableByVersion(voucher.ctx_bracket_table_ver);
    if (table instanceof Error) return table;
    const ctxMeta = await this.normalizeCtxBracketEvidenceForEnclave(
      voucher.enclave_id.toLowerCase(),
      voucher.served_ctx,
      voucher.ctx_bracket,
      voucher.ctx_bracket_table_ver,
      table,
      'spend voucher'
    );
    if (ctxMeta instanceof Error) return ctxMeta;
    const maxSpendAu = this.normalizeAu(voucher.max_spend_au, 'spend voucher max spend', { allowZero: false });
    if (maxSpendAu instanceof Error) {
      return new Error('Invalid spend voucher max spend.');
    }
    if (!Number.isSafeInteger(voucher.checkpoint_every.tokens) || voucher.checkpoint_every.tokens < 1) {
      return new Error('Invalid spend voucher checkpoint tokens.');
    }
    if (!Number.isSafeInteger(voucher.checkpoint_every.ms) || voucher.checkpoint_every.ms < 1) {
      return new Error('Invalid spend voucher checkpoint milliseconds.');
    }
    if (!this.isHexBytes(voucher.user_sig, 64)) return new Error('Invalid spend voucher user signature.');
    const body = {
      session_id: voucher.session_id.toLowerCase(),
      rail,
      enclave_id: voucher.enclave_id.toLowerCase(),
      price_ver: voucher.price_ver,
      locked_rate_map: lockedRateMap,
      locked_per_req_au: lockedPerReqAu,
      locked_min_session_au: lockedMinSessionAu,
      served_ctx: voucher.served_ctx,
      ctx_bracket: ctxMeta.ctx_bracket,
      ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
      max_spend_au: maxSpendAu,
      checkpoint_every: {
        tokens: voucher.checkpoint_every.tokens,
        ms: voucher.checkpoint_every.ms,
      },
    };
    return {
      ...body,
      user_sig: voucher.user_sig.toLowerCase(),
      body,
    };
  }

  async normalizeSpendReserveValue(value) {
    const shapeError = this.validateExactObjectKeys(
      value,
      [
        'op',
        'contract_version',
        'session_id',
        'epoch',
        'at',
        'rail',
        'user',
        'provider',
        'enclave_id',
        'price_ver',
        'rules_ver',
        'served_ctx',
        'ctx_bracket',
        'ctx_bracket_table_ver',
        'max_spend_au',
        'voucher',
        'provider_sig',
      ],
      'spend reservation feature'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'spend_reserve') return new Error('Invalid spend reservation op.');
    if (value.contract_version !== CONTRACT_VERSION) return new Error('Invalid spend reservation contract version.');
    if (!this.isHexBytes(value.session_id, 32)) return new Error('Invalid spend reservation session id.');
    if (!Number.isSafeInteger(value.epoch) || value.epoch < 1) return new Error('Invalid spend reservation epoch.');
    if (!Number.isSafeInteger(value.at) || value.at < 0) return new Error('Invalid spend reservation timestamp.');
    const rail = this.normalizeLedgerRail(value.rail, 'spend reservation rail');
    if (rail instanceof Error) return rail;
    if (!this.isHexBytes(value.user, 32)) return new Error('Invalid spend reservation user.');
    if (!this.isHexBytes(value.provider, 32)) return new Error('Invalid spend reservation provider.');
    if (!this.isHexBytes(value.enclave_id, 32)) return new Error('Invalid spend reservation enclave.');
    if (!Number.isSafeInteger(value.price_ver) || value.price_ver < 1) {
      return new Error('Invalid spend reservation price version.');
    }
    if (!Number.isSafeInteger(value.rules_ver) || value.rules_ver < 1) {
      return new Error('Invalid spend reservation rules version.');
    }
    if (!Number.isSafeInteger(value.served_ctx) || value.served_ctx < 0) {
      return new Error('Invalid spend reservation served context.');
    }
    const activeTable = value.ctx_bracket_table_ver === null || value.ctx_bracket_table_ver === undefined
      ? null
      : await this.ctxBracketTableAt(value.at);
    if (activeTable instanceof Error) return activeTable;
    if (activeTable && value.ctx_bracket_table_ver !== activeTable.ver) {
      return new Error('Spend reservation context bracket table is not active.');
    }
    const ctxMeta = await this.normalizeCtxBracketEvidenceForEnclave(
      value.enclave_id.toLowerCase(),
      value.served_ctx,
      value.ctx_bracket,
      value.ctx_bracket_table_ver,
      activeTable,
      'spend reservation'
    );
    if (ctxMeta instanceof Error) return ctxMeta;
    const maxSpendAu = this.normalizeAu(value.max_spend_au, 'spend reservation max spend', { allowZero: false });
    if (maxSpendAu instanceof Error) {
      return new Error('Invalid spend reservation max spend.');
    }
    if (!this.isHexBytes(value.provider_sig, 64)) {
      return new Error('Invalid spend reservation provider signature.');
    }
    const voucher = await this.normalizeSpendVoucherForReserve(value.voucher);
    if (voucher instanceof Error) return voucher;
    if (voucher.session_id !== value.session_id.toLowerCase()) {
      return new Error('Spend reservation voucher session mismatch.');
    }
    if (voucher.rail !== rail) return new Error('Spend reservation voucher rail mismatch.');
    if (voucher.enclave_id !== value.enclave_id.toLowerCase()) {
      return new Error('Spend reservation voucher enclave mismatch.');
    }
    if (voucher.price_ver !== value.price_ver) {
      return new Error('Spend reservation voucher price mismatch.');
    }
    if (voucher.body.served_ctx !== value.served_ctx) {
      return new Error('Spend reservation voucher served context mismatch.');
    }
    if (voucher.body.ctx_bracket !== value.ctx_bracket) {
      return new Error('Spend reservation voucher context bracket mismatch.');
    }
    if (voucher.body.ctx_bracket_table_ver !== value.ctx_bracket_table_ver) {
      return new Error('Spend reservation voucher context bracket table version mismatch.');
    }
    if (this.compareAu(voucher.max_spend_au, maxSpendAu) !== 0) {
      return new Error('Spend reservation voucher max spend mismatch.');
    }
    return {
      op: 'spend_reserve',
      contract_version: CONTRACT_VERSION,
      session_id: value.session_id.toLowerCase(),
      epoch: value.epoch,
      at: value.at,
      rail,
      user: value.user.toLowerCase(),
      provider: value.provider.toLowerCase(),
      enclave_id: value.enclave_id.toLowerCase(),
      price_ver: value.price_ver,
      locked_rate_map: voucher.body.locked_rate_map,
      locked_per_req_au: voucher.body.locked_per_req_au,
      locked_min_session_au: voucher.body.locked_min_session_au,
      served_ctx: value.served_ctx,
      ctx_bracket: ctxMeta.ctx_bracket,
      ctx_bracket_table_ver: ctxMeta.ctx_bracket_table_ver,
      rules_ver: value.rules_ver,
      max_spend_au: maxSpendAu,
      voucher: {
        session_id: voucher.body.session_id,
        rail: voucher.body.rail,
        enclave_id: voucher.body.enclave_id,
        price_ver: voucher.body.price_ver,
        locked_rate_map: voucher.body.locked_rate_map,
        locked_per_req_au: voucher.body.locked_per_req_au,
        locked_min_session_au: voucher.body.locked_min_session_au,
        served_ctx: voucher.body.served_ctx,
        ctx_bracket: voucher.body.ctx_bracket,
        ctx_bracket_table_ver: voucher.body.ctx_bracket_table_ver,
        max_spend_au: voucher.body.max_spend_au,
        checkpoint_every: voucher.body.checkpoint_every,
        user_sig: voucher.user_sig,
      },
      voucher_body: voucher.body,
      provider_sig: value.provider_sig.toLowerCase(),
    };
  }

  async normalizeSpendHoldRecord(record, user, rail, epoch) {
    if (!record) {
      return {
        user,
        rail,
        denom: PRICE_DENOMINATION,
        epoch,
        reserved_au: ZERO_AU,
        balance_au_at_last_reserve: null,
        sessions: [],
        updated_at: null,
      };
    }
    if (typeof record !== 'object' || Array.isArray(record)) {
      return new Error('Spend hold record must be an object.');
    }
    if (record.user !== user || record.rail !== rail || record.epoch !== epoch) {
      return new Error('Spend hold record key mismatch.');
    }
    if (record.denom !== PRICE_DENOMINATION) return new Error('Spend hold denomination mismatch.');
    const reservedAu = this.normalizeAu(record.reserved_au, 'spend hold reserved amount');
    if (reservedAu instanceof Error) {
      return new Error('Invalid spend hold reserved amount.');
    }
    if (!Array.isArray(record.sessions)) return new Error('Spend hold sessions must be an array.');
    for (const session of record.sessions) {
      if (!session || typeof session !== 'object' || Array.isArray(session)) {
        return new Error('Invalid spend hold session.');
      }
      if (!this.isHexBytes(session.session_id, 32)) return new Error('Invalid spend hold session id.');
      if (!this.isHexBytes(session.provider, 32)) return new Error('Invalid spend hold provider.');
      if (!this.isHexBytes(session.enclave_id, 32)) return new Error('Invalid spend hold enclave.');
      if (!Number.isSafeInteger(session.price_ver) || session.price_ver < 1) {
        return new Error('Invalid spend hold price version.');
      }
      const lockedRateMap = this.normalizeLockedRateMap(
        session.locked_rate_map,
        'spend hold locked_rate_map'
      );
      if (lockedRateMap instanceof Error) return lockedRateMap;
      if (stableJson(lockedRateMap) !== stableJson(session.locked_rate_map)) {
        return new Error('Spend hold locked_rate_map must be canonical.');
      }
      const lockedPerReqAu = this.normalizeAu(session.locked_per_req_au, 'spend hold locked per-request price');
      if (lockedPerReqAu instanceof Error) {
        return new Error('Invalid spend hold locked per-request price.');
      }
      const lockedMinSessionAu = this.normalizeAu(session.locked_min_session_au, 'spend hold locked minimum session price');
      if (lockedMinSessionAu instanceof Error) {
        return new Error('Invalid spend hold locked minimum session price.');
      }
      if (!Number.isSafeInteger(session.served_ctx) || session.served_ctx < 0) {
        return new Error('Invalid spend hold served context.');
      }
      const table = session.ctx_bracket_table_ver === null || session.ctx_bracket_table_ver === undefined
        ? null
        : await this.ctxBracketTableByVersion(session.ctx_bracket_table_ver);
      if (table instanceof Error) return table;
      const ctxMeta = await this.normalizeCtxBracketEvidenceForEnclave(
        session.enclave_id,
        session.served_ctx,
        session.ctx_bracket,
        session.ctx_bracket_table_ver,
        table,
        'spend hold'
      );
      if (ctxMeta instanceof Error) return ctxMeta;
      const maxSpendAu = this.normalizeAu(session.max_spend_au, 'spend hold max spend', { allowZero: false });
      if (maxSpendAu instanceof Error) {
        return new Error('Invalid spend hold max spend.');
      }
      if (!this.isHexBytes(session.voucher_hash, 32)) return new Error('Invalid spend hold voucher hash.');
    }
    return {
      ...record,
      reserved_au: reservedAu,
      sessions: record.sessions.map((session) => ({
        ...session,
        locked_per_req_au: this.normalizeAu(session.locked_per_req_au, 'spend hold locked per-request price'),
        locked_min_session_au: this.normalizeAu(session.locked_min_session_au, 'spend hold locked minimum session price'),
        max_spend_au: this.normalizeAu(session.max_spend_au, 'spend hold max spend', { allowZero: false }),
      })),
    };
  }

  normalizePendingReservationDebitTotals(entries) {
    const out = new Map();
    if (entries === null || entries === undefined) return out;
    if (!Array.isArray(entries)) return new Error('Pending reservation debits must be an array.');
    for (const entry of entries) {
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
        return new Error('Invalid pending reservation debit entry.');
      }
      const rail = this.normalizeLedgerRail(entry.rail, 'pending reservation debit rail');
      if (rail instanceof Error) return rail;
      if (!this.isSafeKeyPart(entry.user)) return new Error('Invalid pending reservation debit user.');
      const au = this.normalizeAu(entry.au, 'pending reservation debit amount', { allowZero: false });
      if (au instanceof Error) return new Error('Invalid pending reservation debit amount.');
      const key = stableJson([rail, entry.user]);
      if (out.has(key)) return new Error('Duplicate pending reservation debit entry.');
      out.set(key, { rail, user: entry.user, au });
    }
    return out;
  }

  reservationDebitTotalEntries(map) {
    if (!map) return [];
    return this.sortedRailRecords(map, 'user').map((entry) => ({
      rail: entry.rail,
      user: entry.user,
      au: entry.au,
    }));
  }

  nextReservationDebitTotals(applyState, epoch, page, debitMap) {
    const base = page === 0
      ? new Map()
      : this.normalizePendingReservationDebitTotals(applyState.pending_reserved_debits);
    if (base instanceof Error) return base;
    if (page > 0 && applyState.pending_epoch === epoch && !Array.isArray(applyState.pending_reserved_debits)) {
      return new Error('Pending reservation debit state missing for paged epoch apply.');
    }
    const out = new Map(base);
    for (const debit of debitMap.values()) {
      const key = stableJson([debit.rail, debit.user]);
      const current = out.get(key) ?? { rail: debit.rail, user: debit.user, au: ZERO_AU };
      const nextAu = this.safeAddAu(current.au, debit.au);
      if (nextAu instanceof Error) return nextAu;
      out.set(key, { ...current, au: nextAu });
    }
    return out;
  }

  async validateEpochDebitReservations(epoch, debitTotals) {
    for (const debit of debitTotals.values()) {
      const hold = await this.normalizeSpendHoldRecord(
        (await this.get(this.spendHoldKey(debit.user, debit.rail, epoch))) ?? null,
        debit.user,
        debit.rail,
        epoch
      );
      if (hold instanceof Error) return hold;
      if (this.compareAu(hold.reserved_au, debit.au) < 0) {
        return new Error('Epoch debit exceeds reserved spend hold.');
      }
    }
    return null;
  }

  async requireBoundCanaryProbe(value, auditor) {
    const catalogError = await this.requirePublishedCanarySet(value.canary_set);
    if (catalogError) return catalogError;

    const enclave = await this.get(`enclave/${value.enclave_id}`);
    if (!enclave) return new Error('Canary probe enclave not found.');
    if (enclave.binary_hash !== value.binary_hash) {
      return new Error('Canary probe binary_hash does not match enclave.');
    }

    if (!this.verifyProbeResultSignature(auditor, value)) {
      return new Error('Invalid canary auditor signature.');
    }
    return null;
  }

  async requirePublishedCanarySet(canarySet) {
    const catalog = await this.get('catalog/current');
    if (!catalog || catalog.status !== 'active') {
      return new Error('Published catalog required for canary probe.');
    }
    if (!Array.isArray(catalog.canaries) || !catalog.canaries.some((entry) => entry.set_id === canarySet)) {
      return new Error('Canary set is not published in the active catalog.');
    }
    return null;
  }

  validateDisputeOpen(value) {
    const rail = this.normalizeLedgerRail(value.rail, 'dispute rail');
    if (rail instanceof Error) return rail;
    if (!this.isSafeKeyPart(value.session_id)) return new Error('Invalid dispute session id.');
    if (!this.isSafeKeyPart(value.reason)) return new Error('Invalid dispute reason.');
    for (const key of ['provider', 'counterparty', 'enclave_id']) {
      if (value[key] !== undefined && !this.isSafeKeyPart(value[key])) {
        return new Error(`Invalid dispute ${key}.`);
      }
    }
    if (value.evidence !== undefined) {
      const bytes = b4a.from(stableJson(value.evidence)).byteLength;
      if (bytes > DISPUTE_EVIDENCE_MAX_BYTES) {
        return new Error('Dispute evidence bundle is too large.');
      }
    }
    return null;
  }

  validateEpochApplyShape(value) {
    const arrays = [
      ['debits', value.debits],
      ['earnings', value.earnings],
    ];
    if (value.market_usage !== undefined) arrays.push(['market_usage', value.market_usage]);
    for (const [name, entries] of arrays) {
      if (!Array.isArray(entries)) return new Error(`Epoch apply ${name} must be an array.`);
    }
    return null;
  }

  epochApplyPage(value) {
    if (!hasOwn(value, 'page')) return 0;
    if (!Number.isSafeInteger(value.page) || value.page < 0) {
      return new Error('Invalid epochApply page.');
    }
    return value.page;
  }

  epochApplyLastPage(value) {
    if (!hasOwn(value, 'last_page')) return !hasOwn(value, 'page');
    if (typeof value.last_page !== 'boolean') {
      return new Error('Invalid epochApply last_page.');
    }
    return value.last_page;
  }

  isIdempotentEpochApplyPage(applyState, epoch, page, lastPage, applyHash) {
    if (applyState.last_apply_hash !== applyHash) return false;
    if (lastPage) {
      return applyState.updated_epoch === epoch && (applyState.pending_epoch ?? null) === null;
    }
    return (
      applyState.pending_epoch === epoch &&
      applyState.pending_next_page === page + 1
    );
  }

  validateEpochApplyPageOrder(applyState, epoch, page) {
    const pendingEpoch = applyState.pending_epoch ?? null;
    if (page === 0) {
      if (pendingEpoch !== null) return new Error('Guardian monotonic epoch invariant failed: pending epoch apply page exists.');
      if (epoch <= applyState.updated_epoch) return new Error('Guardian monotonic epoch invariant failed.');
      if (epoch !== applyState.updated_epoch + 1) {
        return new Error('Guardian monotonic epoch invariant failed: epoch apply must be contiguous.');
      }
      return null;
    }
    if (pendingEpoch !== epoch) {
      return new Error('Guardian monotonic epoch invariant failed: epoch apply page has no pending predecessor.');
    }
    if (epoch !== applyState.updated_epoch + 1) {
      return new Error('Guardian monotonic epoch invariant failed: epoch apply must be contiguous.');
    }
    if (applyState.pending_next_page !== page) {
      return new Error('Guardian monotonic epoch invariant failed: epoch apply page is not contiguous.');
    }
    if (!this.isHexBytes(applyState.last_apply_hash, 32)) {
      return new Error('Guardian monotonic epoch invariant failed: pending epoch apply hash missing.');
    }
    return null;
  }

  nextEpochApplyState({ applyState, epoch, page, lastPage, applyHash, epochSeconds, reservationDebitTotals = null }) {
    const base = {
      ...applyState,
      updated_at: this.tx,
      last_apply_hash: applyHash,
      last_epoch_seconds: epochSeconds,
    };
    if (lastPage) {
      return {
        ...base,
        updated_epoch: epoch,
        pending_epoch: null,
        pending_next_page: 0,
        pending_reserved_debits: null,
        last_page: page,
      };
    }
    return {
      ...base,
      pending_epoch: epoch,
      pending_next_page: page + 1,
      pending_reserved_debits: this.reservationDebitTotalEntries(reservationDebitTotals),
      updated_epoch: applyState.updated_epoch,
      last_page: page,
    };
  }

  validateEpochApplyFeatureValue(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('epochApply feature value must be an object.');
    }
    const required = ['op', 'epoch', 'at', 'debits', 'earnings'];
    const allowed = new Set([...required, 'market_usage', 'roots', 'totals', 'page', 'last_page']);
    const unknown = Object.keys(value).filter((key) => !allowed.has(key)).sort();
    if (unknown.length > 0) {
      return new Error(`epochApply feature does not accept fields: ${unknown.join(', ')}.`);
    }
    for (const key of required) {
      if (!hasOwn(value, key)) return new Error(`epochApply feature is missing ${key}.`);
    }
    if (value.op !== 'epoch_apply') return new Error('Invalid epochApply feature op.');
    if (!Number.isSafeInteger(value.epoch) || value.epoch < 1) {
      return new Error('Invalid epochApply feature epoch.');
    }
    if (!Number.isSafeInteger(value.at) || value.at < 0) {
      return new Error('Invalid epochApply feature timestamp.');
    }
    const page = this.epochApplyPage(value);
    if (page instanceof Error) return page;
    const lastPage = this.epochApplyLastPage(value);
    if (lastPage instanceof Error) return lastPage;
    return this.validateEpochApplyShape(value);
  }

  validateDepositTnkIntent(intent) {
    const shapeError = this.validateExactObjectKeys(
      intent,
      [
        'op',
        'memo_hash',
        'treasury_address',
        'tnk_e18',
        'quoted_au',
        'rate_tnk_usd_au',
        'rate_source',
      ],
      'deposit TNK intent'
    );
    if (shapeError) return shapeError;
    if (intent.op !== 'deposit_tnk') return new Error('Invalid deposit TNK intent op.');
    if (!this.isSafeKeyPart(intent.memo_hash)) return new Error('Invalid deposit memo hash.');
    if (!this.isSafeKeyPart(intent.treasury_address)) return new Error('Invalid TNK treasury address.');
    const quotedAu = this.normalizeAu(intent.quoted_au, 'TNK quoted credit', { allowZero: false });
    if (quotedAu instanceof Error) {
      return new Error('Invalid TNK quoted credit.');
    }
    const rate = this.normalizeAu(intent.rate_tnk_usd_au, 'TNK quoted rate', { allowZero: false });
    if (rate instanceof Error) {
      return new Error('Invalid TNK quoted rate.');
    }
    if (!this.isSafeKeyPart(intent.rate_source)) return new Error('Invalid TNK rate source.');
    const tnkE18 = this.parseTnkE18(intent.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;
    return null;
  }

  guardianValidateBalanceRecord(record, user = null, rail = null) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) {
      return new Error('Guardian non-negative balance invariant failed.');
    }
    if (rail !== null && record.rail !== rail) {
      return new Error('Guardian balance rail invariant failed.');
    }
    if (record.denom !== PRICE_DENOMINATION) {
      return new Error('Guardian balance denomination invariant failed.');
    }
    if (user !== null && record.user !== user) {
      return new Error('Guardian balance owner invariant failed.');
    }
    if (this.normalizeAu(record.au, 'balance au') instanceof Error) {
      return new Error('Guardian non-negative balance invariant failed.');
    }
    if (!Number.isSafeInteger(record.updated_epoch) || record.updated_epoch < 0) {
      return new Error('Guardian balance epoch invariant failed.');
    }
    return null;
  }

  guardianValidateEarningRecord(record, provider = null, rail = null) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    if (rail !== null && record.rail !== rail) {
      return new Error('Guardian earnings rail invariant failed.');
    }
    if (record.denom !== PRICE_DENOMINATION) {
      return new Error('Guardian earnings denomination invariant failed.');
    }
    if (provider !== null && record.provider !== provider) {
      return new Error('Guardian earnings owner invariant failed.');
    }
    for (const key of ['total_au', 'held_au', 'paid_cum_au']) {
      if (this.normalizeAu(record[key], `earning ${key}`) instanceof Error) {
        return new Error('Guardian non-negative earnings invariant failed.');
      }
    }
    if (!Number.isSafeInteger(record.updated_epoch) || record.updated_epoch < 0) {
      return new Error('Guardian earnings epoch invariant failed.');
    }
    const payableAu = this.safeSubAu(record.total_au, record.held_au);
    if (payableAu instanceof Error) return payableAu;
    if (this.compareAu(record.held_au, record.total_au) > 0 || this.compareAu(record.paid_cum_au, payableAu) > 0) {
      return new Error('Guardian earnings conservation invariant failed.');
    }
    if (record.holdbacks !== undefined) {
      const holdbacks = this.normalizeHoldbackBuckets(record);
      if (holdbacks instanceof Error) return holdbacks;
      const heldAu = this.holdbackBucketTotal(holdbacks);
      if (heldAu instanceof Error) return heldAu;
      if (this.compareAu(heldAu, record.held_au) !== 0) {
        return new Error('Guardian earnings conservation invariant failed.');
      }
    }
    return null;
  }

  guardianValidateFeeRecord(record, rail = null) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) {
      return new Error('Guardian fee conservation invariant failed.');
    }
    if (rail !== null && record.rail !== rail) {
      return new Error('Guardian fee rail invariant failed.');
    }
    if (record.denom !== PRICE_DENOMINATION) {
      return new Error('Guardian fee denomination invariant failed.');
    }
    for (const key of ['cum_au', 'swept_cum_au']) {
      if (this.normalizeAu(record[key], `fee ${key}`) instanceof Error) {
        return new Error('Guardian fee conservation invariant failed.');
      }
    }
    if (!Number.isSafeInteger(record.updated_epoch) || record.updated_epoch < 0) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    if (this.compareAu(record.swept_cum_au, record.cum_au) > 0) {
      return new Error('Guardian fee conservation invariant failed.');
    }
    const settledCumAu = record.settled_cum_au ?? record.cum_au;
    if (
      this.normalizeAu(settledCumAu, 'fee settled cumulative amount') instanceof Error ||
      this.compareAu(settledCumAu, record.cum_au) < 0
    ) {
      return new Error('Guardian conservation invariant failed.');
    }
    return null;
  }

  guardianCheckEpochApply({
    epoch,
    applyState,
    feeRecords,
    debitTotal,
    debitRailTotals,
    feeDeltaByRail,
    providerDeltaTotal,
    nextFeeRecords,
    balances,
    earnings,
  }) {
    if (!applyState || !Number.isSafeInteger(applyState.updated_epoch) || applyState.updated_epoch < 0) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    if (epoch <= applyState.updated_epoch || epoch !== applyState.updated_epoch + 1) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    let feeDeltaAu = ZERO_AU;
    for (const rail of PROVIDER_ACCEPTED_RAIL_ORDER) {
      const fee = feeRecords.get(rail);
      const feeError = this.guardianValidateFeeRecord(fee, rail);
      if (feeError) return feeError;
      const nextFee = nextFeeRecords.get(rail);
      const nextFeeError = this.guardianValidateFeeRecord(nextFee, rail);
      if (nextFeeError) return nextFeeError;
      const railFeeDelta = feeDeltaByRail.get(rail) ?? ZERO_AU;
      feeDeltaAu = this.safeAddAu(feeDeltaAu, railFeeDelta);
      if (feeDeltaAu instanceof Error) return feeDeltaAu;
      const expectedFeeCumAu = this.safeAddAu(fee.cum_au, railFeeDelta);
      if (expectedFeeCumAu instanceof Error) return expectedFeeCumAu;
      if (this.compareAu(nextFee.cum_au, expectedFeeCumAu) !== 0) {
        return new Error('Guardian fee conservation invariant failed.');
      }
    }
    const grossDeltaTotal = this.safeAddAu(providerDeltaTotal, feeDeltaAu);
    if (grossDeltaTotal instanceof Error) return grossDeltaTotal;
    if (this.compareAu(grossDeltaTotal, debitTotal) !== 0) {
      return new Error('Guardian conservation invariant failed.');
    }

    for (const balance of balances.values()) {
      const balanceError = this.guardianValidateBalanceRecord(balance, balance.user, balance.rail);
      if (balanceError) return balanceError;
    }
    for (const earning of earnings.values()) {
      const earningError = this.guardianValidateEarningRecord(earning, earning.provider, earning.rail);
      if (earningError) return earningError;
    }

    const nextSettledCumByRail = new Map();
    for (const rail of PROVIDER_ACCEPTED_RAIL_ORDER) {
      const fee = feeRecords.get(rail);
      const nextFee = nextFeeRecords.get(rail);
      const priorSettledCumAu = fee.settled_cum_au ?? fee.cum_au;
      const nextSettledCumAu = this.safeAddAu(priorSettledCumAu, debitRailTotals.get(rail) ?? ZERO_AU);
      if (nextSettledCumAu instanceof Error) return nextSettledCumAu;
      if (this.compareAu(nextFee.cum_au, nextSettledCumAu) > 0) {
        return new Error('Guardian conservation invariant failed.');
      }
      nextSettledCumByRail.set(rail, nextSettledCumAu);
    }
    return { ok: true, next_settled_cum_by_rail: nextSettledCumByRail };
  }

  lockedEarningEpochs(params) {
    return Math.max(params.holdback_epochs ?? 0, params.challenge_epochs ?? 0);
  }

  providerLockedEarningEpochs(provider, params) {
    const normalHoldback = params.holdback_epochs ?? 0;
    const newProviderHoldback = params.new_provider_holdback_epochs ?? normalHoldback;
    const threshold = params.probation_successful_sessions ?? 0;
    const successfulSessions = provider?.probation?.successful_sessions ?? 0;
    if (
      !Number.isSafeInteger(normalHoldback) ||
      !Number.isSafeInteger(newProviderHoldback) ||
      !Number.isSafeInteger(threshold) ||
      !Number.isSafeInteger(successfulSessions) ||
      normalHoldback < 0 ||
      newProviderHoldback < 0 ||
      threshold < 0 ||
      successfulSessions < 0
    ) {
      return new Error('Invalid provider holdback probation state.');
    }
    const providerHoldback = successfulSessions < threshold
      ? Math.max(normalHoldback, newProviderHoldback)
      : normalHoldback;
    return Math.max(providerHoldback, params.challenge_epochs ?? 0);
  }

  async recordCanaryProbePass(value) {
    const key = `probe/pass/${value.provider}/${value.epoch}`;
    const current = await this.get(key);
    const passCount = this.safeAddCount(current?.pass_count ?? 0, 1, 'canary pass count');
    if (passCount instanceof Error) return passCount;
    const record = {
      provider: value.provider,
      epoch: value.epoch,
      pass_count: passCount,
      last_probe_id: value.probe_id,
      last_evidence_hash: value.evidence_hash ?? null,
      updated_at: this.tx,
    };
    await this.put(key, record);
    return record;
  }

  async probeGateForEarning(provider, earning, params) {
    const holdbackBps = params.canary_probe_holdback_bps ?? 0;
    const requiredPasses = params.canary_probe_release_min_passes ?? 0;
    if (holdbackBps <= 0 || requiredPasses <= 0) return null;
    if (!Number.isSafeInteger(holdbackBps) || holdbackBps < 0 || holdbackBps > 10_000) {
      return new Error('Invalid canary probe holdback bps.');
    }
    if (!Number.isSafeInteger(requiredPasses) || requiredPasses < 0) {
      return new Error('Invalid canary probe release threshold.');
    }
    const holdbacks = this.normalizeHoldbackBuckets(earning);
    if (holdbacks instanceof Error) return holdbacks;
    const passedEpochs = new Set();
    for (const epoch of [...new Set(holdbacks.map((bucket) => bucket.epoch))]) {
      const passRecord = await this.get(`probe/pass/${provider}/${epoch}`);
      if ((passRecord?.pass_count ?? 0) >= requiredPasses) {
        passedEpochs.add(epoch);
      }
    }
    return {
      holdback_bps: holdbackBps,
      required_passes: requiredPasses,
      passed_epochs: passedEpochs,
    };
  }

  normalizeHoldbackBuckets(record) {
    if (!Array.isArray(record.holdbacks)) {
      const heldAu = this.normalizeAu(record.held_au, 'earning held amount');
      if (heldAu instanceof Error) {
        return new Error('Guardian non-negative earnings invariant failed.');
      }
      if (this.isZeroAu(heldAu)) return [];
      return [{
        epoch: Number.isSafeInteger(record.updated_epoch) ? record.updated_epoch : 0,
        au: heldAu,
      }];
    }

    const byEpoch = new Map();
    for (const bucket of record.holdbacks) {
      if (!bucket || typeof bucket !== 'object' || Array.isArray(bucket)) {
        return new Error('Guardian earnings conservation invariant failed.');
      }
      if (!Number.isSafeInteger(bucket.epoch) || bucket.epoch < 0) {
        return new Error('Guardian monotonic epoch invariant failed.');
      }
      const bucketAu = this.normalizeAu(bucket.au, 'holdback bucket amount', { allowZero: false });
      if (bucketAu instanceof Error) {
        return new Error('Guardian non-negative earnings invariant failed.');
      }
      const lockedEpochs = hasOwn(bucket, 'locked_epochs') ? bucket.locked_epochs : null;
      if (lockedEpochs !== null && (!Number.isSafeInteger(lockedEpochs) || lockedEpochs < 0)) {
        return new Error('Guardian monotonic epoch invariant failed.');
      }
      const key = `${bucket.epoch}:${bucket.probe_gate === true ? 'probe' : 'time'}:${lockedEpochs ?? 'default'}`;
      const next = this.safeAddAu(byEpoch.get(key)?.au ?? ZERO_AU, bucketAu);
      if (next instanceof Error) return next;
      byEpoch.set(key, {
        epoch: bucket.epoch,
        au: next,
        probe_gate: bucket.probe_gate === true,
        ...(lockedEpochs !== null ? { locked_epochs: lockedEpochs } : {}),
      });
    }
    return Array.from(byEpoch.values())
      .sort((a, b) => (
        a.epoch - b.epoch ||
        (a.locked_epochs ?? -1) - (b.locked_epochs ?? -1) ||
        Number(a.probe_gate) - Number(b.probe_gate)
      ))
      .map((bucket) => (
        bucket.probe_gate
          ? {
              epoch: bucket.epoch,
              au: bucket.au,
              probe_gate: true,
              ...(hasOwn(bucket, 'locked_epochs') ? { locked_epochs: bucket.locked_epochs } : {}),
            }
          : {
              epoch: bucket.epoch,
              au: bucket.au,
              ...(hasOwn(bucket, 'locked_epochs') ? { locked_epochs: bucket.locked_epochs } : {}),
            }
      ));
  }

  holdbackBucketTotal(holdbacks) {
    let total = ZERO_AU;
    for (const bucket of holdbacks) {
      const next = this.safeAddAu(total, bucket.au);
      if (next instanceof Error) return next;
      total = next;
    }
    return total;
  }

  refreshEarningHoldback(record, currentEpoch, lockedEpochs, probeGate = null) {
    if (!Number.isSafeInteger(currentEpoch) || currentEpoch < 0) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    if (!Number.isSafeInteger(lockedEpochs) || lockedEpochs < 0) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    const holdbacks = this.normalizeHoldbackBuckets(record);
    if (holdbacks instanceof Error) return holdbacks;
    const kept = [];
    for (const bucket of holdbacks) {
      const bucketLockedEpochs = hasOwn(bucket, 'locked_epochs') ? bucket.locked_epochs : lockedEpochs;
      if (!Number.isSafeInteger(bucketLockedEpochs) || bucketLockedEpochs < 0) {
        return new Error('Guardian monotonic epoch invariant failed.');
      }
      if (bucket.epoch + bucketLockedEpochs > currentEpoch) {
        kept.push(bucket);
        continue;
      }
      if (!probeGate) continue;
      if (probeGate.passed_epochs.has(bucket.epoch)) continue;
      if (bucket.probe_gate === true) {
        kept.push(bucket);
        continue;
      }
      const gatedAu = this.safeMulDivAu(bucket.au, probeGate.holdback_bps, 10_000);
      if (gatedAu instanceof Error) return gatedAu;
      if (this.compareAu(gatedAu, ZERO_AU) > 0) {
        kept.push({
          epoch: bucket.epoch,
          au: gatedAu,
          probe_gate: true,
          ...(hasOwn(bucket, 'locked_epochs') ? { locked_epochs: bucket.locked_epochs } : {}),
        });
      }
    }
    const heldAu = this.holdbackBucketTotal(kept);
    if (heldAu instanceof Error) return heldAu;
    return {
      ...record,
      held_au: heldAu,
      holdbacks: kept,
      last_holdback_release_epoch: currentEpoch,
    };
  }

  slashHoldbackBuckets(holdbacks, slashAu) {
    const slashAmountAu = this.normalizeAu(slashAu, 'slash amount');
    if (slashAmountAu instanceof Error) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    if (this.isZeroAu(slashAmountAu)) return holdbacks;

    let remaining = slashAmountAu;
    const kept = [];
    for (let idx = holdbacks.length - 1; idx >= 0; idx -= 1) {
      const bucket = holdbacks[idx];
      if (this.isZeroAu(remaining)) {
        kept.push(bucket);
        continue;
      }
      if (this.compareAu(bucket.au, remaining) <= 0) {
        remaining = this.safeSubAu(remaining, bucket.au);
        if (remaining instanceof Error) return remaining;
        continue;
      }
      const reducedAu = this.safeSubAu(bucket.au, remaining);
      if (reducedAu instanceof Error) return reducedAu;
      kept.push({
        ...bucket,
        epoch: bucket.epoch,
        au: reducedAu,
      });
      remaining = ZERO_AU;
    }
    if (!this.isZeroAu(remaining)) return new Error('Guardian earnings conservation invariant failed.');
    return kept.reverse();
  }

  slashAmount(heldAu, slashBps) {
    if (this.normalizeAu(heldAu, 'held amount') instanceof Error) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    if (!Number.isSafeInteger(slashBps) || slashBps < 0 || slashBps > 10_000) {
      return new Error('Invalid slash bps.');
    }
    return this.safeMulDivAu(heldAu, slashBps, 10_000);
  }

  providerActiveEnclaves(provider) {
    if (!provider || !Array.isArray(provider.enclaves)) return [];
    return [...new Set(provider.enclaves.filter((enclaveId) => this.isSafeKeyPart(enclaveId)))].sort();
  }

  providerEnclavesWith(provider, enclaveId) {
    const enclaves = this.providerActiveEnclaves(provider);
    if (!enclaves.includes(enclaveId)) enclaves.push(enclaveId);
    return enclaves.sort();
  }

  providerEnclavesWithout(provider, enclaveId) {
    return this.providerActiveEnclaves(provider).filter((activeEnclaveId) => activeEnclaveId !== enclaveId);
  }

  enclaveActiveProviders(enclave) {
    if (!enclave || !Array.isArray(enclave.providers)) return [];
    return [...new Set(enclave.providers.filter((providerId) => this.isSafeKeyPart(providerId)))].sort();
  }

  enclaveProvidersWith(enclave, providerId) {
    const providers = this.enclaveActiveProviders(enclave);
    if (!providers.includes(providerId)) providers.push(providerId);
    return providers.sort();
  }

  enclaveProvidersWithout(enclave, providerId) {
    return this.enclaveActiveProviders(enclave).filter((activeProviderId) => activeProviderId !== providerId);
  }

  roomServingEntries(room) {
    if (!room || !Array.isArray(room.serves)) return [];
    const entries = new Map();
    for (const entry of room.serves) {
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) continue;
      if (!this.isSafeKeyPart(entry.provider) || !this.isSafeKeyPart(entry.enclave_id)) continue;
      entries.set(JSON.stringify([entry.provider, entry.enclave_id]), {
        provider: entry.provider,
        enclave_id: entry.enclave_id,
      });
    }
    return Array.from(entries.values()).sort((a, b) => (
      compareCodepoint(a.provider, b.provider) || compareCodepoint(a.enclave_id, b.enclave_id)
    ));
  }

  roomServesWith(room, providerId, enclaveId) {
    const entries = this.roomServingEntries(room);
    if (!entries.some((entry) => entry.provider === providerId && entry.enclave_id === enclaveId)) {
      entries.push({ provider: providerId, enclave_id: enclaveId });
    }
    return entries.sort((a, b) => (
      compareCodepoint(a.provider, b.provider) || compareCodepoint(a.enclave_id, b.enclave_id)
    ));
  }

  roomServesWithout(room, providerId, enclaveId = null) {
    return this.roomServingEntries(room).filter((entry) => (
      entry.provider !== providerId || (enclaveId !== null && entry.enclave_id !== enclaveId)
    ));
  }

  async tombstoneRoomServes(roomId, entries, evidenceHash) {
    if (!this.isSafeKeyPart(roomId)) return new Error('Invalid room id.');
    const tombstones = [];
    for (const entry of entries) {
      const tombstone = await this.tombstoneRoomServing(
        roomId,
        entry.provider,
        entry.enclave_id,
        evidenceHash
      );
      if (tombstone instanceof Error) return tombstone;
      tombstones.push(tombstone);
    }
    return tombstones;
  }

  async tombstoneRoomServing(roomId, providerId, enclaveId, evidenceHash) {
    if (!this.isSafeKeyPart(roomId)) return new Error('Invalid room id.');
    if (!this.isSafeKeyPart(providerId)) return new Error('Invalid provider id.');
    if (!this.isSafeKeyPart(enclaveId)) return new Error('Invalid enclave id.');

    const roomServeKey = `roomserve/${roomId}/${providerId}/${enclaveId}`;
    const roomServing = await this.get(roomServeKey);
    const roomKey = `room/${roomId}`;
    const room = await this.get(roomKey);
    const servingKey = `serve/${providerId}/${enclaveId}`;
    const serving = await this.get(servingKey);

    if (room) {
      await this.put(roomKey, {
        ...room,
        serves: this.roomServesWithout(room, providerId, enclaveId),
        serves_updated_at: this.tx,
      });
    }
    if (serving) {
      await this.put(servingKey, {
        ...serving,
        rooms: Array.isArray(serving.rooms)
          ? serving.rooms.filter((activeRoomId) => activeRoomId !== roomId)
          : [],
        updated_at: this.tx,
      });
    }
    if (!roomServing || roomServing.status !== 'active') {
      return {
        room_id: roomId,
        provider: providerId,
        enclave_id: enclaveId,
        roomserve_tombstoned: false,
      };
    }

    await this.put(roomServeKey, {
      ...roomServing,
      status: 'tombstoned',
      updated_at: this.tx,
      tombstoned_at: this.tx,
      tombstone_reason_hash: evidenceHash,
    });
    return {
      room_id: roomId,
      provider: providerId,
      enclave_id: enclaveId,
      roomserve_tombstoned: true,
    };
  }

  async tombstoneEnclaveProviders(enclaveId, providerIds, evidenceHash) {
    if (!this.isSafeKeyPart(enclaveId)) return new Error('Invalid enclave id.');
    const tombstones = [];
    for (const providerId of [...new Set(providerIds)].sort()) {
      const tombstone = await this.tombstoneProviderEnclave(providerId, enclaveId, evidenceHash);
      if (tombstone instanceof Error) return tombstone;
      tombstones.push(tombstone);
    }
    return tombstones;
  }

  async tombstoneProviderEnclaves(providerId, enclaveIds, evidenceHash) {
    if (!this.isSafeKeyPart(providerId)) return new Error('Invalid provider id.');
    const tombstones = [];
    for (const enclaveId of [...new Set(enclaveIds)].sort()) {
      const tombstone = await this.tombstoneProviderEnclave(providerId, enclaveId, evidenceHash);
      if (tombstone instanceof Error) return tombstone;
      tombstones.push(tombstone);
    }
    return tombstones;
  }

  async tombstoneProviderEnclave(providerId, enclaveId, evidenceHash) {
    if (!this.isSafeKeyPart(providerId)) return new Error('Invalid provider id.');
    if (!enclaveId) {
      return {
        provider: providerId,
        enclave_id: null,
        serve_tombstoned: false,
        rooms_tombstoned: [],
      };
    }
    if (!this.isSafeKeyPart(enclaveId)) return new Error('Invalid enclave id.');

    const serveKey = `serve/${providerId}/${enclaveId}`;
    const serving = await this.get(serveKey);
    if (!serving) {
      return {
        provider: providerId,
        enclave_id: enclaveId,
        serve_tombstoned: false,
        rooms_tombstoned: [],
      };
    }

    const rooms = Array.isArray(serving.rooms) ? serving.rooms.slice().sort() : [];
    const tombstonedRooms = [];
    for (const roomId of rooms) {
      const tombstone = await this.tombstoneRoomServing(roomId, providerId, enclaveId, evidenceHash);
      if (tombstone instanceof Error) return tombstone;
      if (tombstone.roomserve_tombstoned) tombstonedRooms.push(roomId);
    }

    await this.put(serveKey, {
      ...serving,
      status: 'tombstoned',
      rooms: [],
      updated_at: this.tx,
      tombstoned_at: this.tx,
      tombstone_reason_hash: evidenceHash,
    });
    const provider = await this.get(`prov/${providerId}`);
    if (provider) {
      await this.put(`prov/${providerId}`, {
        ...provider,
        enclaves: this.providerEnclavesWithout(provider, enclaveId),
        updated_at: this.tx,
      });
    }
    const enclave = await this.get(`enclave/${enclaveId}`);
    if (enclave) {
      await this.put(`enclave/${enclaveId}`, {
        ...enclave,
        providers: this.enclaveProvidersWithout(enclave, providerId),
        updated_at: this.tx,
      });
    }
    return {
      provider: providerId,
      enclave_id: enclaveId,
      serve_tombstoned: true,
      rooms_tombstoned: tombstonedRooms,
    };
  }

  async applyProviderSlash({
    providerId,
    source,
    reason,
    evidenceHash,
    epoch,
    at,
    slashBps,
    beneficiary = null,
    enclaveId = null,
    probeId = null,
    eventId = null,
    banProvider = false,
    tombstoneEnclave = false,
  }) {
    if (!this.isSafeKeyPart(providerId)) return new Error('Invalid provider id.');
    if (beneficiary !== null && !this.isSafeKeyPart(beneficiary)) {
      return new Error('Invalid slash beneficiary.');
    }

    const providerKey = `prov/${providerId}`;
    const provider = await this.get(providerKey);
    if (!provider) return new Error('Provider not found.');

    const updatedEarnings = new Map();
    const beneficiaryBalances = new Map();
    const updatedFees = new Map();
    const railSlashRecords = [];
    let heldBeforeAu = ZERO_AU;
    let heldAfterAu = ZERO_AU;
    let forfeitedAu = ZERO_AU;
    let reporterAu = ZERO_AU;
    let treasuryAu = ZERO_AU;

    for (const rail of PROVIDER_ACCEPTED_RAIL_ORDER) {
      const earning = await this.earningRecord(providerId, rail);
      if (earning instanceof Error) return earning;
      const earningError = this.guardianValidateEarningRecord(earning, providerId, rail);
      if (earningError) return earningError;
      const holdbacks = this.normalizeHoldbackBuckets(earning);
      if (holdbacks instanceof Error) return holdbacks;

      const railForfeitedAu = this.slashAmount(earning.held_au, slashBps);
      if (railForfeitedAu instanceof Error) return railForfeitedAu;
      const railReporterAu = beneficiary === null ? ZERO_AU : this.safeMulDivAu(railForfeitedAu, 1, 2);
      if (railReporterAu instanceof Error) return railReporterAu;
      const railTreasuryAu = this.safeSubAu(railForfeitedAu, railReporterAu);
      if (railTreasuryAu instanceof Error) return railTreasuryAu;
      const remainingHoldbacks = this.slashHoldbackBuckets(holdbacks, railForfeitedAu);
      if (remainingHoldbacks instanceof Error) return remainingHoldbacks;
      const railHeldAfterAu = this.safeSubAu(earning.held_au, railForfeitedAu);
      if (railHeldAfterAu instanceof Error) return railHeldAfterAu;
      const railTotalAu = this.safeSubAu(earning.total_au, railForfeitedAu);
      if (railTotalAu instanceof Error) return railTotalAu;
      const slashedCumAu = this.safeAddAu(earning.slashed_cum_au ?? ZERO_AU, railForfeitedAu);
      if (slashedCumAu instanceof Error) return slashedCumAu;

      heldBeforeAu = this.safeAddAu(heldBeforeAu, earning.held_au);
      if (heldBeforeAu instanceof Error) return heldBeforeAu;
      heldAfterAu = this.safeAddAu(heldAfterAu, railHeldAfterAu);
      if (heldAfterAu instanceof Error) return heldAfterAu;
      forfeitedAu = this.safeAddAu(forfeitedAu, railForfeitedAu);
      if (forfeitedAu instanceof Error) return forfeitedAu;
      reporterAu = this.safeAddAu(reporterAu, railReporterAu);
      if (reporterAu instanceof Error) return reporterAu;
      treasuryAu = this.safeAddAu(treasuryAu, railTreasuryAu);
      if (treasuryAu instanceof Error) return treasuryAu;

      if (this.compareAu(earning.total_au, ZERO_AU) > 0 || this.compareAu(railForfeitedAu, ZERO_AU) > 0) {
        const updatedEarning = {
          ...earning,
          rail,
          total_au: railTotalAu,
          held_au: railHeldAfterAu,
          holdbacks: remainingHoldbacks,
          slashed_cum_au: slashedCumAu,
          last_slash_at: this.tx,
          updated_at: this.tx,
        };
        const updatedEarningError = this.guardianValidateEarningRecord(updatedEarning, providerId, rail);
        if (updatedEarningError) return updatedEarningError;
        updatedEarnings.set(rail, updatedEarning);
      }

      if (this.compareAu(railReporterAu, ZERO_AU) > 0) {
        const currentBalance = await this.balanceRecord(beneficiary, rail);
        if (currentBalance instanceof Error) return currentBalance;
        const balanceError = this.guardianValidateBalanceRecord(currentBalance, beneficiary, rail);
        if (balanceError) return balanceError;
        const nextAu = this.safeAddAu(currentBalance.au, railReporterAu);
        if (nextAu instanceof Error) return nextAu;
        beneficiaryBalances.set(rail, {
          ...currentBalance,
          rail,
          au: nextAu,
          updated_epoch: Math.max(currentBalance.updated_epoch, epoch),
          updated_at: this.tx,
        });
      }

      if (this.compareAu(railTreasuryAu, ZERO_AU) > 0) {
        const fee = await this.feeCumRecord(rail);
        if (fee instanceof Error) return fee;
        const feeError = this.guardianValidateFeeRecord(fee, rail);
        if (feeError) return feeError;
        const cumAu = this.safeAddAu(fee.cum_au, railTreasuryAu);
        if (cumAu instanceof Error) return cumAu;
        const settledCumAu = this.safeAddAu(fee.settled_cum_au ?? fee.cum_au, railTreasuryAu);
        if (settledCumAu instanceof Error) return settledCumAu;
        const updatedFee = {
          ...fee,
          rail,
          cum_au: cumAu,
          settled_cum_au: settledCumAu,
          updated_epoch: Math.max(fee.updated_epoch, epoch),
          updated_at: this.tx,
          last_slash_at: this.tx,
        };
        const updatedFeeError = this.guardianValidateFeeRecord(updatedFee, rail);
        if (updatedFeeError) return updatedFeeError;
        updatedFees.set(rail, updatedFee);
      }

      if (this.compareAu(earning.held_au, ZERO_AU) > 0 || this.compareAu(railForfeitedAu, ZERO_AU) > 0) {
        railSlashRecords.push({
          rail,
          held_before_au: earning.held_au,
          held_after_au: railHeldAfterAu,
          forfeited_au: railForfeitedAu,
          beneficiary_au: railReporterAu,
          treasury_au: railTreasuryAu,
        });
      }
    }

    const tombstone = tombstoneEnclave
      ? await this.tombstoneProviderEnclave(providerId, enclaveId, evidenceHash)
      : {
          enclave_id: enclaveId,
          serve_tombstoned: false,
          rooms_tombstoned: [],
        };
    if (tombstone instanceof Error) return tombstone;
    const banTombstones = banProvider
      ? await this.tombstoneProviderEnclaves(
          providerId,
          this.providerActiveEnclaves(provider).filter((activeEnclaveId) => activeEnclaveId !== enclaveId),
          evidenceHash
        )
      : [];
    if (banTombstones instanceof Error) return banTombstones;

    const slash = {
      type: 'slash',
      provider: providerId,
      source,
      reason,
      evidence_hash: evidenceHash,
      epoch,
      at,
      tx: this.tx,
      slashed_by: this.address,
      beneficiary,
      enclave_id: enclaveId,
      probe_id: probeId,
      event_id: eventId,
      slash_bps: slashBps,
      held_before_au: heldBeforeAu,
      held_after_au: heldAfterAu,
      forfeited_au: forfeitedAu,
      beneficiary_au: reporterAu,
      treasury_au: treasuryAu,
      rails: railSlashRecords,
      tombstone,
      ban_tombstones: banTombstones,
      provider_banned: banProvider,
    };
    slash.slash_hash = await this.opaqueHash('mayhem-slash-v1', slash);

    const updatedProvider = banProvider
      ? {
          ...provider,
          status: 'banned',
          enclaves: [],
          tombstoned_enclaves: [tombstone, ...banTombstones]
            .filter((entry) => entry.enclave_id)
            .map((entry) => entry.enclave_id),
          banned_at: provider.banned_at ?? this.tx,
          banned_by: provider.banned_by ?? this.address,
          ban_reason_hash: provider.ban_reason_hash ?? evidenceHash,
          updated_at: this.tx,
        }
      : {
          ...provider,
          enclaves: tombstone.serve_tombstoned
            ? this.providerEnclavesWithout(provider, tombstone.enclave_id)
            : this.providerActiveEnclaves(provider),
          updated_at: this.tx,
        };

    for (const [rail, updatedEarning] of updatedEarnings) {
      await this.put(this.earningKey(providerId, rail), updatedEarning);
    }
    for (const [rail, beneficiaryBalance] of beneficiaryBalances) {
      await this.put(this.balanceKey(beneficiary, rail), beneficiaryBalance);
    }
    for (const [rail, updatedFee] of updatedFees) {
      await this.put(this.feeCumKey(rail), updatedFee);
    }
    await this.put(providerKey, updatedProvider);
    await this.put(`ev/slash/${providerId}/${this.tx}`, slash);
    return slash;
  }

  appendHoldbackBucket(holdbacks, epoch, au, lockedEpochs = null) {
    if (!Number.isSafeInteger(epoch) || epoch < 0) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    const amount = this.normalizeAu(au, 'holdback amount', { allowZero: false });
    if (amount instanceof Error) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    if (lockedEpochs !== null && (!Number.isSafeInteger(lockedEpochs) || lockedEpochs < 0)) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    const normalized = this.normalizeHoldbackBuckets({ holdbacks });
    if (normalized instanceof Error) return normalized;
    const gated = normalized.filter((bucket) => bucket.probe_gate === true);
    const byEpoch = new Map(
      normalized
        .filter((bucket) => bucket.probe_gate !== true)
        .map((bucket) => [
          `${bucket.epoch}:${hasOwn(bucket, 'locked_epochs') ? bucket.locked_epochs : 'default'}`,
          bucket,
        ])
    );
    const key = `${epoch}:${lockedEpochs ?? 'default'}`;
    const current = byEpoch.get(key);
    const next = this.safeAddAu(current?.au ?? ZERO_AU, amount);
    if (next instanceof Error) return next;
    byEpoch.set(key, {
      epoch,
      au: next,
      ...(lockedEpochs !== null ? { locked_epochs: lockedEpochs } : {}),
    });
    return [
      ...Array.from(byEpoch.values())
        .map((bucket) => ({
          epoch: bucket.epoch,
          au: bucket.au,
          ...(hasOwn(bucket, 'locked_epochs') ? { locked_epochs: bucket.locked_epochs } : {}),
        })),
      ...gated,
    ]
      .sort((a, b) => (
        a.epoch - b.epoch ||
        (a.locked_epochs ?? -1) - (b.locked_epochs ?? -1) ||
        Number(a.probe_gate === true) - Number(b.probe_gate === true)
      ));
  }

  normalizeEpochRoots(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Epoch roots must be an object.');
    }
    const keys = Object.keys(value).sort();
    if (
      keys.length !== EPOCH_ROOT_KEYS.length ||
      keys.some((key, idx) => key !== EPOCH_ROOT_KEYS.slice().sort()[idx])
    ) {
      return new Error('Epoch roots must include dep, use, earn, fee, and price.');
    }
    const roots = {};
    for (const key of EPOCH_ROOT_KEYS) {
      const root = value[key];
      if (typeof root !== 'string' || !/^[0-9a-fA-F]{64}$/.test(root)) {
        return new Error(`Invalid epoch ${key} root.`);
      }
      roots[key] = root.toLowerCase();
    }
    return roots;
  }

  normalizeEpochTotals(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Epoch totals must be an object.');
    }
    const expected = EPOCH_TOTAL_KEYS.slice().sort();
    const keys = Object.keys(value).sort();
    if (keys.length !== expected.length || keys.some((key, idx) => key !== expected[idx])) {
      return new Error('Epoch totals have an invalid shape.');
    }
    const totals = {};
    for (const key of EPOCH_TOTAL_KEYS) {
      const total = value[key];
      if (EPOCH_TOTAL_MONEY_KEYS.has(key)) {
        const au = this.normalizeAu(total, `epoch total ${key}`);
        if (au instanceof Error) {
          return new Error(`Invalid epoch total ${key}.`);
        }
        totals[key] = au;
        continue;
      }
      if (!Number.isSafeInteger(total) || total < 0) {
        return new Error(`Invalid epoch total ${key}.`);
      }
      totals[key] = total;
    }
    return totals;
  }

  canonicalUsageUnit(unit) {
    switch (unit) {
      case 'in':
      case 'in_tokens':
      case 'input':
      case 'input_tokens':
      case 'prompt_tokens':
      case 'input_token':
        return 'input_token';
      case 'cached_input':
      case 'cached_inputs':
      case 'cached_input_tokens':
      case 'cached_prompt_tokens':
      case 'cached_tokens':
      case 'cached_input_token':
        return 'cached_input_token';
      case 'out':
      case 'out_tokens':
      case 'output':
      case 'output_tokens':
      case 'completion_tokens':
      case 'output_token':
        return 'output_token';
      case 'images':
      case 'image':
        return 'image';
      case 'steps':
      case 'step':
        return 'step';
      default:
        return unit;
    }
  }

  normalizeReceiptUsage(usageSource) {
    if (!usageSource || typeof usageSource !== 'object' || Array.isArray(usageSource)) {
      return new Error('Fraud proof receipt usage must be an object.');
    }
    const usage = {};
    for (const [rawUnit, count] of Object.entries(usageSource)) {
      if (typeof rawUnit !== 'string' || rawUnit.length === 0 || rawUnit.length > 64) {
        return new Error('Invalid receipt usage unit.');
      }
      if (!Number.isSafeInteger(count) || count < 0) {
        return new Error('Invalid receipt usage count.');
      }
      if (count === 0) continue;
      const unit = this.canonicalUsageUnit(rawUnit);
      if (!this.isSafeKeyPart(unit)) return new Error('Invalid receipt usage unit.');
      const next = this.safeAddCount(usage[unit] ?? 0, count, 'receipt usage count');
      if (next instanceof Error) return next;
      usage[unit] = next;
    }
    return Object.fromEntries(Object.entries(usage).sort(([left], [right]) => compareCodepoint(left, right)));
  }

  async normalizeReceiptEnvelope(value, options = {}) {
    const targetSchemaVersion = options.targetSchemaVersion ?? SESSION_RECEIPT_SCHEMA_VERSION;
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Fraud proof receipt must be an object.');
    }
    const receipt = value.receipt ?? value;
    if (!receipt || typeof receipt !== 'object' || Array.isArray(receipt)) {
      return new Error('Fraud proof receipt must be an object.');
    }
    const bodySource = receipt.body ?? receipt;
    if (!bodySource || typeof bodySource !== 'object' || Array.isArray(bodySource)) {
      return new Error('Fraud proof receipt body must be an object.');
    }
    const body = {
      schema_version: bodySource.schema_version,
      session_id: bodySource.session_id,
      seq: bodySource.seq,
      final: bodySource.final,
      rail: bodySource.rail,
      user: bodySource.user,
      provider: bodySource.provider,
      enclave_id: bodySource.enclave_id,
      model_id: bodySource.model_id,
      price_ver: bodySource.price_ver,
      locked_rate_map: cloneValue(bodySource.locked_rate_map),
      rules_ver: bodySource.rules_ver,
      usage: cloneValue(bodySource.usage),
      au_owed_cum: bodySource.au_owed_cum,
      prompt_hash: bodySource.prompt_hash,
      ts: bodySource.ts,
    };
    if (hasOwn(bodySource, 'locked_per_req_au')) body.locked_per_req_au = bodySource.locked_per_req_au;
    if (hasOwn(bodySource, 'locked_min_session_au')) body.locked_min_session_au = bodySource.locked_min_session_au;
    if (hasOwn(bodySource, 'served_ctx')) body.served_ctx = bodySource.served_ctx;
    if (hasOwn(bodySource, 'ctx_bracket')) body.ctx_bracket = bodySource.ctx_bracket;
    if (hasOwn(bodySource, 'ctx_bracket_table_ver')) {
      body.ctx_bracket_table_ver = bodySource.ctx_bracket_table_ver;
    }

    if (body.schema_version !== targetSchemaVersion) {
      return new Error('Unsupported receipt schema version.');
    }

    const bodyError = await this.validateReceiptBody(body, targetSchemaVersion);
    if (bodyError) return bodyError;

    const envelope = {
      body,
      enclave_sig: receipt.enclave_sig ?? value.enclave_sig,
      user_sig: receipt.user_sig ?? value.user_sig,
      enclave_pubkey: receipt.enclave_pubkey ?? value.enclave_pubkey ?? bodySource.enclave_pubkey ?? null,
    };
    if (!this.isHexBytes(envelope.enclave_sig, 64)) return new Error('Invalid enclave receipt signature.');
    if (!this.isHexBytes(envelope.user_sig, 64)) return new Error('Invalid user receipt signature.');
    if (envelope.enclave_pubkey !== null && !this.isHexBytes(envelope.enclave_pubkey, 32)) {
      return new Error('Invalid enclave receipt public key.');
    }
    return envelope;
  }

  async validateReceiptBody(body, expectedSchemaVersion = SESSION_RECEIPT_SCHEMA_VERSION) {
    if (body.schema_version !== expectedSchemaVersion) {
      return new Error('Unsupported receipt schema version.');
    }
    for (const field of ['session_id', 'user', 'provider', 'enclave_id', 'model_id', 'prompt_hash']) {
      if (typeof body[field] !== 'string' || body[field].length === 0 || body[field].length > 256) {
        return new Error(`Invalid receipt ${field}.`);
      }
    }
    if (!this.isHexBytes(body.user, 32)) return new Error('Invalid receipt user public key.');
    if (!this.isHexBytes(body.provider, 32)) return new Error('Invalid receipt provider public key.');
    const rail = this.normalizeLedgerRail(body.rail, 'receipt rail');
    if (rail instanceof Error) return rail;
    if (body.rail !== rail) return new Error('Receipt rail must be canonical.');
    if (!Number.isSafeInteger(body.seq) || body.seq < 0) return new Error('Invalid receipt sequence.');
    if (typeof body.final !== 'boolean') return new Error('Invalid receipt final flag.');
    if (!Number.isSafeInteger(body.price_ver) || body.price_ver < 1) {
      return new Error('Invalid receipt price version.');
    }
    const lockedRateMap = this.normalizeLockedRateMap(body.locked_rate_map, 'receipt locked_rate_map');
    if (lockedRateMap instanceof Error) return lockedRateMap;
    if (stableJson(lockedRateMap) !== stableJson(body.locked_rate_map)) {
      return new Error('Receipt locked_rate_map must be canonical.');
    }
    const lockedPerReqAu = this.normalizeAu(body.locked_per_req_au, 'receipt locked per-request price');
    if (lockedPerReqAu instanceof Error) {
      return new Error('Invalid receipt locked per-request price.');
    }
    const lockedMinSessionAu = this.normalizeAu(body.locked_min_session_au, 'receipt locked minimum session price');
    if (lockedMinSessionAu instanceof Error) {
      return new Error('Invalid receipt locked minimum session price.');
    }
    if (!Number.isSafeInteger(body.served_ctx) || body.served_ctx < 0) {
      return new Error('Invalid receipt served context.');
    }
    const table = body.ctx_bracket_table_ver === null || body.ctx_bracket_table_ver === undefined
      ? null
      : await this.ctxBracketTableByVersion(body.ctx_bracket_table_ver);
    if (table instanceof Error) return table;
    const ctxMeta = await this.normalizeCtxBracketEvidenceForEnclave(
      body.enclave_id,
      body.served_ctx,
      body.ctx_bracket,
      body.ctx_bracket_table_ver,
      table,
      'receipt'
    );
    if (ctxMeta instanceof Error) return ctxMeta;
    if (!Number.isSafeInteger(body.rules_ver) || body.rules_ver < 1) {
      return new Error('Invalid receipt rules version.');
    }
    const usage = this.normalizeReceiptUsage(body.usage);
    if (usage instanceof Error) return usage;
    if (stableJson(usage) !== stableJson(body.usage)) {
      return new Error('Receipt usage must be canonical.');
    }
    const auOwedCum = this.normalizeAu(body.au_owed_cum, 'receipt cumulative amount');
    if (auOwedCum instanceof Error) {
      return new Error('Invalid receipt cumulative amount.');
    }
    const lockedAu = this.usageAuForLockedTerms(
      body.locked_rate_map,
      lockedPerReqAu,
      lockedMinSessionAu,
      body.usage
    );
    if (lockedAu instanceof Error) return lockedAu;
    if (this.compareAu(auOwedCum, lockedAu) !== 0) {
      return new Error('Receipt cumulative amount does not match locked price terms.');
    }
    if (!Number.isSafeInteger(body.ts) || body.ts < 0) return new Error('Invalid receipt timestamp.');
    return null;
  }

  verifyReceiptEnvelope(envelope) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    const signedBody = envelope.body;
    if (!signedBody || typeof signedBody !== 'object' || Array.isArray(signedBody)) return false;
    const enclaveKey = envelope.enclave_pubkey ?? (
      this.isHexBytes(signedBody.enclave_id, 32) ? signedBody.enclave_id : null
    );
    if (!enclaveKey) return false;
    const message = receiptMessage(signedBody);
    return (
      verify.call(this.protocol.peer.wallet, envelope.enclave_sig, message, enclaveKey) === true &&
      verify.call(this.protocol.peer.wallet, envelope.user_sig, message, signedBody.user) === true
    );
  }

  receiptLeafEnvelope(envelope) {
    return {
      body: cloneValue(envelope.body),
      enclave_sig: envelope.enclave_sig,
      user_sig: envelope.user_sig,
    };
  }

  async usageLeafHash(envelope) {
    return await this.opaqueHash('mayhem-usage-leaf-v1', this.receiptLeafEnvelope(envelope));
  }

  async fraudProofHash(value) {
    return await this.opaqueHash('mayhem-fraud-proof-v1', value);
  }

  async validateOverCreditFraudProof(commit, receipt) {
    if (commit.status === 'void') return new Error('Epoch commit is already void.');
    if (this.value.proof_epoch > commit.provisional_until_epoch) {
      return new Error('Epoch commit challenge window has closed.');
    }
    if (commit.totals.use_count !== 1) {
      return new Error('Over-credit proof requires a single committed receipt.');
    }

    const previousAu = this.normalizeAu(this.value.previous_au_owed_cum ?? ZERO_AU, 'previous receipt amount');
    if (previousAu instanceof Error) {
      return new Error('Invalid previous receipt amount.');
    }
    const claimedCum = this.normalizeAu(this.value.claimed_au_owed_cum, 'claimed receipt amount');
    if (claimedCum instanceof Error) return new Error('Invalid claimed receipt amount.');
    if (this.compareAu(previousAu, receipt.body.au_owed_cum) > 0 || this.compareAu(previousAu, claimedCum) > 0) {
      return new Error('Previous receipt amount exceeds cumulative amount.');
    }
    const actualAu = this.safeSubAu(receipt.body.au_owed_cum, previousAu);
    if (actualAu instanceof Error) return actualAu;
    const claimedAu = this.safeSubAu(claimedCum, previousAu);
    if (claimedAu instanceof Error) return claimedAu;
    if (this.compareAu(claimedAu, actualAu) <= 0) return new Error('Receipt does not contradict committed usage.');
    if (this.compareAu(commit.totals.use_au, claimedAu) !== 0) {
      return new Error('Fraud proof claimed amount does not match committed usage total.');
    }

    const claimedReceipt = {
      ...receipt,
      body: {
        ...receipt.body,
        au_owed_cum: claimedCum,
      },
    };
    const claimedUseRoot = await this.usageLeafHash(claimedReceipt);
    if (commit.roots.use !== claimedUseRoot) {
      return new Error('Fraud proof does not match committed usage root.');
    }

    return {
      actual_au: actualAu,
      claimed_au: claimedAu,
      receipt_hash: await this.usageLeafHash(receipt),
    };
  }

  priceTermsSnapshot(record) {
    return {
      ver: record.ver,
      ...(record.ctx_bracket ? { ctx_bracket: record.ctx_bracket } : {}),
      ...(record.ctx_bracket_table_ver ? { ctx_bracket_table_ver: record.ctx_bracket_table_ver } : {}),
      rate_map: cloneValue(record.rate_map),
      per_req_au: record.per_req_au,
      min_session_au: record.min_session_au,
    };
  }

  priceDerivationLeafValue(derivation) {
    const {
      derivation_hash: _derivationHash,
      price_root: _priceRoot,
      updated_at: _updatedAt,
      ...leaf
    } = derivation;
    return leaf;
  }

  priceDerivationFromMarketUpdate(update, { epoch, at, epochSeconds = null, usageRoot = null } = {}) {
    const record = update.record;
    const market = record.market;
    if (!record || !market || typeof market !== 'object') {
      return new Error('Market price update is missing derivation data.');
    }
    return {
      type: 'price_derivation',
      schema_version: 1,
      epoch,
      at,
      epoch_seconds: epochSeconds,
      enclave_id: record.enclave_id,
      ...(record.ctx_bracket ? {
        ctx_bracket: record.ctx_bracket,
        ctx_bracket_table_ver: record.ctx_bracket_table_ver,
      } : {}),
      model_id: record.model_id,
      denom: record.denom,
      price_ver: record.ver,
      price_source: record.price_source,
      usage: {
        usage_root: usageRoot,
        active_demand_au: market.active_demand_au,
        session_count: market.session_count,
        ...(record.ctx_bracket ? {
          ctx_bracket: record.ctx_bracket,
          ctx_bracket_table_ver: record.ctx_bracket_table_ver,
        } : {}),
      },
      controller: {
        source: market.source,
        active_supply: market.active_supply,
        utilization_bps: market.utilization_bps,
        ema_utilization_bps: market.ema_utilization_bps,
        multiplier_bps: market.multiplier_bps,
        frozen: market.frozen,
        frozen_reason: market.frozen_reason,
        constants: cloneValue(market.constants),
      },
      seed_price: this.priceTermsSnapshot(record.seed),
      previous_price: {
        ver: market.previous_price_ver,
        rate_map: cloneValue(market.previous_rate_map),
        per_req_au: market.previous_per_req_au,
        min_session_au: market.previous_min_session_au,
      },
      desired_price: {
        rate_map: cloneValue(market.desired_rate_map),
        per_req_au: market.desired_per_req_au,
        min_session_au: market.desired_min_session_au,
      },
      result_price: this.priceTermsSnapshot(record),
    };
  }

  async priceDerivationsFromMarketUpdates(updates, context = {}) {
    const derivations = [];
    const sorted = updates.slice().sort((left, right) => (
      compareCodepoint(left.enclave_id, right.enclave_id) ||
      compareCodepoint(left.ctx_bracket ?? '', right.ctx_bracket ?? '')
    ));
    for (const update of sorted) {
      const derivation = this.priceDerivationFromMarketUpdate(update, context);
      if (derivation instanceof Error) return derivation;
      const derivationHash = await this.priceDerivationLeafHash(derivation);
      update.derivation_hash = derivationHash;
      derivations.push({
        ...derivation,
        derivation_hash: derivationHash,
      });
    }
    const priceRoot = await this.priceDerivationRoot(derivations);
    for (const derivation of derivations) {
      derivation.price_root = priceRoot;
    }
    return derivations;
  }

  async priceDerivationLeafHash(derivation) {
    return await this.opaqueHash('mayhem-price-derivation-leaf-v1', this.priceDerivationLeafValue(derivation));
  }

  async merkleRoot(kind, leaves) {
    if (leaves.length === 0) return await this.opaqueHash(`mayhem-${kind}-empty-root-v1`, {});
    let level = leaves.slice().sort();
    while (level.length > 1) {
      const next = [];
      for (let idx = 0; idx < level.length; idx += 2) {
        const left = level[idx];
        const right = idx + 1 < level.length ? level[idx + 1] : left;
        next.push(await this.opaqueHash(`mayhem-${kind}-node-v1`, { left, right }));
      }
      level = next;
    }
    return level[0];
  }

  async priceDerivationRoot(derivations) {
    const leaves = [];
    for (const derivation of derivations) {
      leaves.push(await this.priceDerivationLeafHash(derivation));
    }
    return await this.merkleRoot('price', leaves);
  }

  normalizePriceProofUsage(value) {
    const usageMap = this.aggregateMarketUsageEntries([value]);
    if (usageMap instanceof Error) return usageMap;
    const entries = this.mapMarketUsageEntriesForHash(usageMap);
    if (entries.length !== 1) return new Error('Price derivation proof requires one market usage entry.');
    return entries[0];
  }

  async validatePriceDerivationFraudProof(commit) {
    if (commit.status === 'void') return new Error('Epoch commit is already void.');
    if (this.value.proof_epoch > commit.provisional_until_epoch) {
      return new Error('Epoch commit challenge window has closed.');
    }
    if (commit.totals.price_count !== 1) {
      return new Error('Price derivation proof requires a single committed price derivation.');
    }
    const priceUsage = this.normalizePriceProofUsage(this.value.price_usage);
    if (priceUsage instanceof Error) return priceUsage;
    if (this.compareAu(commit.totals.use_au, priceUsage.demand_au) !== 0) {
      return new Error('Price derivation proof usage does not match committed usage total.');
    }
    if (commit.totals.use_count !== priceUsage.session_count) {
      return new Error('Price derivation proof session count does not match committed usage count.');
    }

    const usageMap = new Map([[this.priceMarketKey(priceUsage.enclave_id, priceUsage.ctx_bracket ?? null), priceUsage]]);
    const updates = await this.computeMarketPriceUpdates(usageMap, {
      epoch: commit.epoch,
      at: commit.at,
      epochSeconds: commit.epoch_seconds ?? null,
      tx: commit.submitted_at,
    });
    if (updates instanceof Error) return updates;
    const derivations = await this.priceDerivationsFromMarketUpdates(updates, {
      epoch: commit.epoch,
      at: commit.at,
      epochSeconds: commit.epoch_seconds ?? null,
      usageRoot: commit.roots.use,
    });
    if (derivations instanceof Error) return derivations;
    if (derivations.length !== 1) {
      return new Error('Price derivation proof did not produce one expected derivation.');
    }
    const expectedPriceRoot = await this.priceDerivationRoot(derivations);
    if (expectedPriceRoot === commit.roots.price) {
      return new Error('Price derivation proof does not contradict committed price root.');
    }
    return {
      price_usage: priceUsage,
      enclave_id: priceUsage.enclave_id,
      ...(priceUsage.ctx_bracket ? {
        ctx_bracket: priceUsage.ctx_bracket,
        ctx_bracket_table_ver: priceUsage.ctx_bracket_table_ver,
      } : {}),
      expected_price_root: expectedPriceRoot,
      committed_price_root: commit.roots.price,
      price_derivation_hash: derivations[0].derivation_hash,
      price_derivation: derivations[0],
    };
  }

  async validateEpochApplyTotals({
    epoch,
    roots,
    totals,
    debitTotal,
    feeDeltaAu,
    nextFeeCum,
    providerCount,
    earnCumTotal,
    epochSeconds,
    priceDerivations,
  }) {
    const commit = await this.get(`epoch/commit/${epoch}`);
    if (!commit) return new Error('Epoch commit required before applying evidence roots.');
    if (commit.status === 'void') return new Error('Epoch commit is void.');
    if (commit.epoch_seconds !== epochSeconds) {
      return new Error('Epoch apply epoch_seconds does not match committed epoch timing.');
    }
    if (
      stableJson(commit.roots) !== stableJson(roots) ||
      stableJson(commit.totals) !== stableJson(totals)
    ) {
      return new Error('Epoch apply roots do not match committed roots.');
    }
    if (this.compareAu(totals.use_au, debitTotal) !== 0) return new Error('Epoch usage total does not match debits.');
    if (this.compareAu(totals.earn_au, earnCumTotal) !== 0) {
      return new Error('Epoch earn total does not match cumulative provider earnings.');
    }
    if (this.compareAu(totals.fee_au, feeDeltaAu) !== 0) return new Error('Epoch fee total does not match computed fee.');
    if (this.compareAu(totals.fee_cum_au, nextFeeCum) !== 0) {
      return new Error('Epoch cumulative fee total does not match fee state.');
    }
    if (totals.provider_count !== providerCount) {
      return new Error('Epoch provider count does not match earnings.');
    }
    if (totals.price_count !== priceDerivations.length) {
      return new Error('Epoch price derivation count does not match market updates.');
    }
    const priceRoot = await this.priceDerivationRoot(priceDerivations);
    if (roots.price !== priceRoot) {
      return new Error('Epoch price root does not match recomputed price derivations.');
    }

    const depositRoot = await this.get(`ev/dep/${epoch}`);
    if (depositRoot) {
      if (depositRoot.type !== 'deposit_root') return new Error('Invalid deposit evidence root.');
      if (
        depositRoot.merkle_root !== roots.dep ||
        depositRoot.count !== totals.dep_count ||
        this.compareAu(depositRoot.au_total, totals.dep_au) !== 0
      ) {
        return new Error('Committed deposit root does not match deposit evidence.');
      }
    }
    for (const key of ['use', 'earn', 'fee', 'price']) {
      if ((await this.get(`ev/${key}/${epoch}`)) !== null) {
        return new Error(`Epoch ${key} evidence root already exists.`);
      }
    }
    return null;
  }

  async writePriceDerivationEvidence({ epoch, at, epoch_seconds, root, count, derivations }) {
    await this.put(`ev/price/${epoch}`, {
      type: 'price_root',
      epoch,
      epoch_seconds,
      merkle_root: root,
      price_count: count,
      ts: at,
      updated_at: this.tx,
    });
    for (const derivation of derivations) {
      const key = derivation.ctx_bracket
        ? `ev/price/${epoch}/${derivation.enclave_id}/${derivation.ctx_bracket}`
        : `ev/price/${epoch}/${derivation.enclave_id}`;
      await this.put(key, {
        ...derivation,
        price_root: root,
        updated_at: this.tx,
      });
    }
  }

  async writeEpochEvidenceRoots({ epoch, at, epoch_seconds, roots, totals, feeDeltaAu, feeCumAu, priceDerivations }) {
    if ((await this.get(`ev/dep/${epoch}`)) === null) {
      await this.put(`ev/dep/${epoch}`, {
        type: 'deposit_root',
        epoch,
        epoch_seconds,
        merkle_root: roots.dep,
        count: totals.dep_count,
        au_total: totals.dep_au,
        ts: at,
        updated_at: this.tx,
      });
    }
    await this.put(`ev/use/${epoch}`, {
      type: 'usage_root',
      epoch,
      epoch_seconds,
      merkle_root: roots.use,
      sessions: totals.use_count,
      au_total: totals.use_au,
      providers: totals.provider_count,
      ts: at,
      updated_at: this.tx,
    });
    await this.put(`ev/earn/${epoch}`, {
      type: 'earn_root',
      epoch,
      epoch_seconds,
      merkle_root: roots.earn,
      provider_count: totals.provider_count,
      au_cum_total: totals.earn_au,
      ts: at,
      updated_at: this.tx,
    });
    await this.put(`ev/fee/${epoch}`, {
      type: 'fee_root',
      epoch,
      epoch_seconds,
      merkle_root: roots.fee,
      au_fee_epoch: feeDeltaAu,
      au_fee_cum: feeCumAu,
      sweep_msb_tx_hash: null,
      ts: at,
      updated_at: this.tx,
    });
    await this.writePriceDerivationEvidence({
      epoch,
      at,
      epoch_seconds,
      root: roots.price,
      count: totals.price_count,
      derivations: priceDerivations,
    });
  }

  aggregateLedgerEntries(entries, idKey, amountKey, label) {
    const out = new Map();
    for (const entry of entries) {
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
        return new Error(`Invalid ${label} entry.`);
      }
      const id = entry[idKey];
      const au = this.normalizeAu(entry[amountKey], `${label} amount`, { allowZero: false });
      if (au instanceof Error) return new Error(`Invalid ${label} amount.`);
      if (!this.isSafeKeyPart(id)) return new Error(`Invalid ${label} ${idKey}.`);
      const next = this.safeAddAu(out.get(id) ?? ZERO_AU, au);
      if (next instanceof Error) return next;
      out.set(id, next);
    }
    return out;
  }

  aggregateRailLedgerEntries(entries, idKey, amountKey, label) {
    const out = new Map();
    for (const entry of entries) {
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
        return new Error(`Invalid ${label} entry.`);
      }
      const rail = this.normalizeLedgerRail(entry.rail, `${label} rail`);
      if (rail instanceof Error) return rail;
      const id = entry[idKey];
      const au = this.normalizeAu(entry[amountKey], `${label} amount`, { allowZero: false });
      if (au instanceof Error) return new Error(`Invalid ${label} amount.`);
      if (!this.isSafeKeyPart(id)) return new Error(`Invalid ${label} ${idKey}.`);
      const key = stableJson([rail, id]);
      const current = out.get(key) ?? { rail, [idKey]: id, [amountKey]: ZERO_AU };
      const next = this.safeAddAu(current[amountKey], au);
      if (next instanceof Error) return next;
      out.set(key, { ...current, [amountKey]: next });
    }
    return out;
  }

  aggregateMarketUsageEntries(entries) {
    const out = new Map();
    for (const entry of entries) {
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
        return new Error('Invalid market usage entry.');
      }
      const allowed = new Set([
        'enclave_id',
        'ctx_bracket',
        'ctx_bracket_table_ver',
        'demand_au',
        'session_count',
        'provider_count',
      ]);
      const unknown = Object.keys(entry).filter((key) => !allowed.has(key)).sort();
      if (unknown.length > 0) {
        return new Error(`market usage entry does not accept fields: ${unknown.join(', ')}.`);
      }
      for (const key of ['enclave_id', 'demand_au', 'session_count', 'provider_count']) {
        if (!hasOwn(entry, key)) return new Error(`market usage entry is missing ${key}.`);
      }
      const enclaveId = entry.enclave_id;
      if (!this.isSafeKeyPart(enclaveId)) return new Error('Invalid market usage enclave_id.');
      const ctxBracket = entry.ctx_bracket ?? null;
      if (ctxBracket !== null && !this.isSafeKeyPart(ctxBracket)) {
        return new Error('Invalid market usage context bracket.');
      }
      if (
        hasOwn(entry, 'ctx_bracket_table_ver') &&
        (!Number.isSafeInteger(entry.ctx_bracket_table_ver) || entry.ctx_bracket_table_ver < 1)
      ) {
        return new Error('Invalid market usage context bracket table version.');
      }
      const demandEntryAu = this.normalizeAu(entry.demand_au, 'market usage demand', { allowZero: false });
      if (demandEntryAu instanceof Error) {
        return new Error('Invalid market usage demand.');
      }
      if (!Number.isSafeInteger(entry.session_count) || entry.session_count <= 0) {
        return new Error('Invalid market usage session_count.');
      }
      if (!Number.isSafeInteger(entry.provider_count) || entry.provider_count <= 0) {
        return new Error('Invalid market usage provider_count.');
      }
      const key = this.priceMarketKey(enclaveId, ctxBracket);
      const current = out.get(key) ?? {
        enclave_id: enclaveId,
        ...(ctxBracket ? { ctx_bracket: ctxBracket } : {}),
        ...(entry.ctx_bracket_table_ver ? { ctx_bracket_table_ver: entry.ctx_bracket_table_ver } : {}),
        demand_au: ZERO_AU,
        session_count: 0,
        provider_count: 0,
      };
      if ((current.ctx_bracket_table_ver ?? null) !== (entry.ctx_bracket_table_ver ?? current.ctx_bracket_table_ver ?? null)) {
        return new Error('Market usage context bracket table version mismatch.');
      }
      const demandAu = this.safeAddAu(current.demand_au, demandEntryAu);
      if (demandAu instanceof Error) return demandAu;
      const sessionCount = this.safeAddCount(current.session_count, entry.session_count, 'market usage session_count');
      if (sessionCount instanceof Error) return sessionCount;
      const providerCount = this.safeAddCount(current.provider_count, entry.provider_count, 'market usage provider_count');
      if (providerCount instanceof Error) return providerCount;
      out.set(key, {
        enclave_id: enclaveId,
        ...(ctxBracket ? { ctx_bracket: ctxBracket } : {}),
        ...(entry.ctx_bracket_table_ver ? { ctx_bracket_table_ver: entry.ctx_bracket_table_ver } : {}),
        demand_au: demandAu,
        session_count: sessionCount,
        provider_count: providerCount,
      });
    }
    return out;
  }

  sumAu(entries) {
    let sum = ZERO_AU;
    for (const [, au] of entries) {
      const next = this.safeAddAu(sum, au);
      if (next instanceof Error) return next;
      sum = next;
    }
    return sum;
  }

  sumRailAu(entries, amountKey) {
    let sum = ZERO_AU;
    for (const entry of entries.values()) {
      const next = this.safeAddAu(sum, entry[amountKey]);
      if (next instanceof Error) return next;
      sum = next;
    }
    return sum;
  }

  sumMarketDemandAu(entries) {
    let sum = ZERO_AU;
    for (const entry of entries.values()) {
      const next = this.safeAddAu(sum, entry.demand_au);
      if (next instanceof Error) return next;
      sum = next;
    }
    return sum;
  }

  usageAuForRateMap(rateMap, usage) {
    const rates = this.rateMapByUnit(rateMap);
    const priced = [];
    for (const [unit, count] of Object.entries(usage ?? {})) {
      if (!Number.isSafeInteger(count) || count < 0) return new Error('Invalid receipt usage count.');
      if (count === 0) continue;
      const rate = rates.get(unit);
      if (!rate) return new Error(`Receipt locked_rate_map missing usage unit ${unit}.`);
      const perUnitAu = this.parseAu(rate.per_unit_au, 'locked rate per_unit_au', { allowZero: false });
      if (perUnitAu instanceof Error) {
        return new Error('Invalid locked rate per_unit_au.');
      }
      if (!Number.isSafeInteger(rate.granularity) || rate.granularity <= 0) {
        return new Error('Invalid locked rate granularity.');
      }
      priced.push({
        count: BigInt(count),
        perUnitAu,
        granularity: BigInt(rate.granularity),
      });
    }
    if (priced.length === 0) return ZERO_AU;
    const sameGranularity = priced.every((entry) => entry.granularity === priced[0].granularity);
    if (sameGranularity) {
      const raw = priced.reduce((sum, entry) => sum + entry.count * entry.perUnitAu, 0n);
      return this.canonicalAu(this.ceilDivBigInt(raw, priced[0].granularity));
    }
    let total = 0n;
    for (const entry of priced) {
      total += this.ceilDivBigInt(entry.count * entry.perUnitAu, entry.granularity);
    }
    return this.canonicalAu(total);
  }

  usageAuForLockedTerms(rateMap, perReqAu, minSessionAu, usage) {
    const normalizedPerReqAu = this.normalizeAu(perReqAu, 'locked per-request price');
    if (normalizedPerReqAu instanceof Error) return new Error('Invalid locked per-request price.');
    const normalizedMinSessionAu = this.normalizeAu(minSessionAu, 'locked minimum session price');
    if (normalizedMinSessionAu instanceof Error) return new Error('Invalid locked minimum session price.');
    const usageAu = this.usageAuForRateMap(rateMap, usage);
    if (usageAu instanceof Error) return usageAu;
    const subtotal = this.safeAddAu(usageAu, normalizedPerReqAu);
    if (subtotal instanceof Error) return subtotal;
    return this.maxAu(subtotal, normalizedMinSessionAu);
  }

  ceilDivBigInt(value, divisor) {
    if (value <= 0n) return 0n;
    return (value + divisor - 1n) / divisor;
  }

  railTotals(entries, amountKey) {
    const out = new Map(PROVIDER_ACCEPTED_RAIL_ORDER.map((rail) => [rail, ZERO_AU]));
    for (const entry of entries.values()) {
      const next = this.safeAddAu(out.get(entry.rail) ?? ZERO_AU, entry[amountKey]);
      if (next instanceof Error) return next;
      out.set(entry.rail, next);
    }
    return out;
  }

  assertMatchingRailTotals(left, right) {
    for (const rail of PROVIDER_ACCEPTED_RAIL_ORDER) {
      if (this.compareAu(left.get(rail) ?? ZERO_AU, right.get(rail) ?? ZERO_AU) !== 0) {
        return new Error('Epoch debits must equal gross provider earnings per rail.');
      }
    }
    return null;
  }

  safeAddAu(a, b) {
    const left = this.parseAu(a, 'au value');
    if (left instanceof Error) return left;
    const right = this.parseAu(b, 'au value');
    if (right instanceof Error) return right;
    return this.canonicalAu(left + right);
  }

  safeSubAu(a, b) {
    const left = this.parseAu(a, 'au value');
    if (left instanceof Error) return left;
    const right = this.parseAu(b, 'au value');
    if (right instanceof Error) return right;
    if (right > left) return new Error('au value underflow.');
    return this.canonicalAu(left - right);
  }

  safeMulDivAu(a, b, divisor) {
    const amount = this.parseAu(a, 'au value');
    if (amount instanceof Error) return amount;
    if (
      !Number.isSafeInteger(b) ||
      !Number.isSafeInteger(divisor) ||
      b < 0 ||
      divisor <= 0
    ) {
      return new Error('Invalid au multiplier.');
    }
    return this.canonicalAu((amount * BigInt(b)) / BigInt(divisor));
  }

  safeAddCount(a, b, label = 'count') {
    if (!Number.isSafeInteger(a) || !Number.isSafeInteger(b) || a < 0 || b < 0) {
      return new Error(`Invalid ${label}.`);
    }
    const next = a + b;
    if (!Number.isSafeInteger(next)) return new Error(`${label} overflow.`);
    return next;
  }

  parseAu(value, label = 'au value', { allowZero = true } = {}) {
    if (typeof value === 'bigint') {
      if (value < 0n || (!allowZero && value === 0n)) return new Error(`${label} must be positive.`);
      return value;
    }
    if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value)) {
      return new Error(`${label} must be a canonical decimal string.`);
    }
    const parsed = BigInt(value);
    if (!allowZero && parsed === 0n) return new Error(`${label} must be positive.`);
    return parsed;
  }

  normalizeAu(value, label = 'au value', options = {}) {
    const parsed = this.parseAu(value, label, options);
    if (parsed instanceof Error) return parsed;
    return this.canonicalAu(parsed);
  }

  canonicalAu(value) {
    if (typeof value !== 'bigint' || value < 0n) return new Error('Invalid au value.');
    return value.toString();
  }

  compareAu(a, b) {
    const left = this.parseAu(a, 'au value');
    if (left instanceof Error) return NaN;
    const right = this.parseAu(b, 'au value');
    if (right instanceof Error) return NaN;
    return left === right ? 0 : (left < right ? -1 : 1);
  }

  maxAu(a, b) {
    return this.compareAu(a, b) >= 0 ? this.normalizeAu(a) : this.normalizeAu(b);
  }

  isZeroAu(value) {
    return this.compareAu(value, ZERO_AU) === 0;
  }

  sortedMapEntries(map) {
    return Array.from(map.entries()).sort(([a], [b]) => compareCodepoint(a, b));
  }

  sortedRailRecords(map, idKey) {
    return Array.from(map.values()).sort((a, b) => {
      const railOrder =
        PROVIDER_ACCEPTED_RAIL_ORDER.indexOf(a.rail) - PROVIDER_ACCEPTED_RAIL_ORDER.indexOf(b.rail);
      if (railOrder !== 0) return railOrder;
      return compareCodepoint(a[idKey], b[idKey]);
    });
  }

  mapEntriesForHash(map, idKey, amountKey) {
    return this.sortedMapEntries(map).map(([id, au]) => ({
      [idKey]: id,
      [amountKey]: au,
    }));
  }

  mapRailEntriesForHash(map, idKey, amountKey) {
    return this.sortedRailRecords(map, idKey).map((entry) => ({
      rail: entry.rail,
      [idKey]: entry[idKey],
      [amountKey]: entry[amountKey],
    }));
  }

  mapMarketUsageEntriesForHash(map) {
    return Array.from(map.values())
      .sort((left, right) => (
        compareCodepoint(left.enclave_id, right.enclave_id) ||
        compareCodepoint(left.ctx_bracket ?? '', right.ctx_bracket ?? '')
      ))
      .map((entry) => ({
        enclave_id: entry.enclave_id,
        ...(entry.ctx_bracket ? { ctx_bracket: entry.ctx_bracket } : {}),
        ...(entry.ctx_bracket_table_ver ? { ctx_bracket_table_ver: entry.ctx_bracket_table_ver } : {}),
        demand_au: entry.demand_au,
        session_count: entry.session_count,
        provider_count: entry.provider_count,
      }));
  }

  async balanceRecord(user, rail) {
    const normalizedRail = this.normalizeLedgerRail(rail, 'balance rail');
    if (normalizedRail instanceof Error) return normalizedRail;
    return (await this.get(this.balanceKey(user, normalizedRail))) ?? {
      user,
      rail: normalizedRail,
      denom: PRICE_DENOMINATION,
      au: ZERO_AU,
      updated_epoch: 0,
      updated_at: null,
    };
  }

  async earningRecord(provider, rail) {
    const normalizedRail = this.normalizeLedgerRail(rail, 'earning rail');
    if (normalizedRail instanceof Error) return normalizedRail;
    return (await this.get(this.earningKey(provider, normalizedRail))) ?? {
      provider,
      rail: normalizedRail,
      denom: PRICE_DENOMINATION,
      total_au: ZERO_AU,
      held_au: ZERO_AU,
      paid_cum_au: ZERO_AU,
      updated_epoch: 0,
      updated_at: null,
    };
  }

  async feeCumRecord(rail) {
    const normalizedRail = this.normalizeLedgerRail(rail, 'fee rail');
    if (normalizedRail instanceof Error) return normalizedRail;
    return (await this.get(this.feeCumKey(normalizedRail))) ?? {
      rail: normalizedRail,
      denom: PRICE_DENOMINATION,
      cum_au: ZERO_AU,
      swept_cum_au: ZERO_AU,
      updated_epoch: 0,
      updated_at: null,
      last_apply_hash: null,
      last_fee_bps: null,
    };
  }

  async epochApplyStateRecord() {
    return (await this.get('epoch/apply/state')) ?? {
      updated_epoch: 0,
      updated_at: null,
      last_apply_hash: null,
    };
  }

  parseTnkE18(value) {
    if (typeof value !== 'string' || !/^[0-9]+$/.test(value)) {
      return new Error('tnk_e18 must be a decimal integer string.');
    }
    const parsed = BigInt(value);
    if (parsed <= 0n) return new Error('tnk_e18 must be positive.');
    return parsed;
  }

  parseTapWei(value) {
    if (typeof value !== 'string' || !/^[0-9]+$/.test(value)) {
      return new Error('tap_wei must be a decimal integer string.');
    }
    const parsed = BigInt(value);
    if (parsed <= 0n) return new Error('tap_wei must be positive.');
    return parsed;
  }

  normalizeFiatCurrency(value) {
    if (typeof value !== 'string') return new Error('Invalid fiat currency.');
    const currency = value.trim().toLowerCase();
    if (!FIAT_CURRENCIES.has(currency)) return new Error('Unsupported fiat currency.');
    return currency;
  }

  fiatEvidenceFields() {
    if (this.value.fiat_currency === undefined || this.value.fiat_amount_minor === undefined) {
      return new Error('Fiat evidence requires fiat_currency and fiat_amount_minor.');
    }
    const currency = this.normalizeFiatCurrency(this.value.fiat_currency);
    if (currency instanceof Error) return currency;
    if (
      !Number.isSafeInteger(this.value.fiat_amount_minor) ||
      this.value.fiat_amount_minor <= 0
    ) {
      return new Error('Invalid fiat amount.');
    }
    return {
      fiat_currency: currency,
      fiat_amount_minor: this.value.fiat_amount_minor,
    };
  }

  tnkE18ToAu(tnkE18, tnkUsdAu) {
    const rate = this.parseAu(tnkUsdAu, 'TNK/USD atto-rate', { allowZero: false });
    if (rate instanceof Error) return new Error('Invalid TNK/USD atto-rate.');
    return this.canonicalAu((tnkE18 * rate) / TNK_E18);
  }

  tapWeiToAu(tapWei, tapUsdAu) {
    const rate = this.parseAu(tapUsdAu, 'TAP/USD policy rate', { allowZero: false });
    if (rate instanceof Error) return new Error('Invalid TAP/USD policy rate.');
    return this.canonicalAu((tapWei * rate) / TAP_WEI);
  }

  async requireFreshRate(at) {
    const rate = await this.get('rate/latest');
    if (!rate) return new Error('Fresh rate oracle required.');
    const admin = await this.get('admin');
    if (admin === null) return new Error('Fresh rate oracle requires a current admin key.');
    if (rate.posted_by !== admin || rate.posted_by_role !== 'admin') {
      return new Error('Fresh rate oracle must be admin-posted.');
    }
    if (rate.ts > at) return new Error('Rate oracle timestamp is in the future.');
    const params = await this.activeParamsAt(at, ['rate_staleness_seconds']);
    if (at - rate.ts > params.rate_staleness_seconds) {
      return new Error('Rate oracle is stale.');
    }
    return rate;
  }

  async requireFreshTapRate(at) {
    const rate = await this.get('tap/rate/latest');
    if (!rate) return new Error('Fresh TAP rate oracle required.');
    const admin = await this.get('admin');
    if (admin === null) return new Error('Fresh TAP rate oracle requires a current admin key.');
    if (rate.posted_by !== admin || rate.posted_by_role !== 'admin') {
      return new Error('Fresh TAP rate oracle must be admin-posted.');
    }
    if (rate.ts > at) return new Error('TAP rate oracle timestamp is in the future.');
    const params = await this.activeParamsAt(at, ['rate_staleness_seconds']);
    if (at - rate.ts > params.rate_staleness_seconds) {
      return new Error('TAP rate oracle is stale.');
    }
    return rate;
  }

  async guardianRequireFreshRate(at) {
    const rate = await this.requireFreshRate(at);
    if (rate instanceof Error) {
      return new Error(`Guardian rate freshness invariant failed: ${rate.message}`);
    }
    return rate;
  }

  async guardianRequireFreshTapRate(at) {
    const rate = await this.requireFreshTapRate(at);
    if (rate instanceof Error) {
      return new Error(`Guardian TAP rate freshness invariant failed: ${rate.message}`);
    }
    return rate;
  }

  async opaqueHash(domain, value) {
    const digest = await blake3(b4a.from(stableJson({ domain, value })));
    return b4a.toString(digest, 'hex');
  }

  async depositLeafHash(value) {
    return await this.opaqueHash('mayhem-deposit-leaf-v1', value);
  }

  async nextDepositRoot({ epoch, leaf, au, at }) {
    const current = await this.get(`ev/dep/${epoch}`);
    if (current && current.type !== 'deposit_root') {
      return new Error('Invalid deposit evidence root.');
    }
    const count = (current?.count ?? 0) + 1;
    const auTotal = this.safeAddAu(current?.au_total ?? ZERO_AU, au);
    if (auTotal instanceof Error) return auTotal;
    const merkleRoot = current
      ? await this.opaqueHash('mayhem-deposit-root-v1', {
        previous_root: current.merkle_root,
        leaf,
        count,
      })
      : leaf;
    return {
      type: 'deposit_root',
      epoch,
      merkle_root: merkleRoot,
      count,
      au_total: auTotal,
      ts: at,
      updated_at: this.tx,
    };
  }

  async nextDepositReversalRoot({ epoch, leaf, disputedAu, clawbackAu, absorbedAu, at }) {
    const current = await this.get(`ev/dep/${epoch}`);
    if (current && current.type !== 'deposit_root') {
      return new Error('Invalid deposit evidence root.');
    }
    const count = (current?.count ?? 0) + 1;
    const reversedAuTotal = this.safeAddAu(current?.reversed_au_total ?? ZERO_AU, disputedAu);
    if (reversedAuTotal instanceof Error) return reversedAuTotal;
    const clawbackAuTotal = this.safeAddAu(current?.clawback_au_total ?? ZERO_AU, clawbackAu);
    if (clawbackAuTotal instanceof Error) return clawbackAuTotal;
    const networkAbsorbedAuTotal = this.safeAddAu(current?.network_absorbed_au_total ?? ZERO_AU, absorbedAu);
    if (networkAbsorbedAuTotal instanceof Error) return networkAbsorbedAuTotal;
    const merkleRoot = current
      ? await this.opaqueHash('mayhem-deposit-root-v1', {
        previous_root: current.merkle_root,
        leaf,
        count,
      })
      : leaf;
    return {
      ...(current ?? {
        type: 'deposit_root',
        epoch,
        au_total: ZERO_AU,
      }),
      type: 'deposit_root',
      epoch,
      merkle_root: merkleRoot,
      count,
      reversed: true,
      reversal_count: (current?.reversal_count ?? 0) + 1,
      reversed_au_total: reversedAuTotal,
      clawback_au_total: clawbackAuTotal,
      network_absorbed_au_total: networkAbsorbedAuTotal,
      ts: at,
      updated_at: this.tx,
    };
  }

  async epochApplyHash(value) {
    const digest = await blake3(b4a.from(stableJson({
      domain: 'mayhem-epoch-apply-v1',
      value,
    })));
    return b4a.toString(digest, 'hex');
  }

  epochEmptySealHashValue(value) {
    return {
      type: value.type,
      epoch: value.epoch,
      at: value.at,
      epoch_seconds: value.epoch_seconds,
      previous_apply_hash: value.previous_apply_hash ?? null,
      reason_hash: value.reason_hash,
      sealed_by: value.sealed_by,
      sealed_by_role: value.sealed_by_role,
      totals: value.totals,
    };
  }

  async epochEmptySealHash(value) {
    return await this.opaqueHash('mayhem-epoch-empty-seal-v1', this.epochEmptySealHashValue(value));
  }

  async epochApplyFeatureKey(value) {
    const shapeError = this.validateEpochApplyFeatureValue(value);
    if (shapeError) return shapeError;
    const digest = await blake3(b4a.from(stableJson({
      domain: 'mayhem-epoch-apply-feature-v1',
      value,
    })));
    return `epoch/apply/${value.epoch}/${b4a.toString(digest, 'hex')}`;
  }

  async depositFeatureKey(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Deposit feature value must be an object.');
    }
    const digest = b4a.toString(
      await blake3(b4a.from(stableJson({
        domain: 'mayhem-deposit-feature-v1',
        value,
      }))),
      'hex'
    );
    if (value.op === 'deposit_tnk' && value.intent) {
      const intentError = this.validateDepositTnkIntent(value.intent);
      if (intentError) return intentError;
      return `dep/tnk-intent/${value.intent.memo_hash}/${digest}`;
    }
    if (value.op === 'tnk_deposit') {
      if (!this.isSafeKeyPart(value.memo_hash)) return new Error('Invalid deposit memo hash.');
      return `dep/tnk/${value.memo_hash}/${digest}`;
    }
    if (value.op === 'tap_deposit') {
      if (!this.isSafeKeyPart(value.eth_tx_hash)) return new Error('Invalid Ethereum tx hash.');
      if (!Number.isSafeInteger(value.log_index) || value.log_index < 0) {
        return new Error('Invalid TAP deposit log index.');
      }
      return `dep/tap/${value.eth_tx_hash.toLowerCase()}/${value.log_index}/${digest}`;
    }
    if (value.op === 'fiat_deposit') {
      if (!this.isSafeKeyPart(value.ext_ref_hash)) return new Error('Invalid external reference hash.');
      return `dep/fiat/${value.ext_ref_hash}`;
    }
    return new Error('Unsupported deposit feature op.');
  }

  async rateFeatureKey(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('Rate feature value must be an object.');
    }
    let kind;
    if (value.op === 'rate_oracle') {
      const shapeError = this.validateRateOracleValue(value);
      if (shapeError) return shapeError;
      kind = 'tnk';
    } else if (value.op === 'tap_rate_oracle') {
      const shapeError = this.validateTapRateOracleValue(value);
      if (shapeError) return shapeError;
      kind = 'tap';
    } else {
      return new Error('Unsupported rate feature op.');
    }
    const digest = b4a.toString(
      await blake3(b4a.from(stableJson({
        domain: 'mayhem-rate-feature-v1',
        value,
      }))),
      'hex'
    );
    return `rate/${kind}/${value.ts}/${digest}`;
  }

  async tnkSettlementFeatureKey(value) {
    const shapeError = this.validateTnkSettlementValue(value);
    if (shapeError) return shapeError;
    const digest = b4a.toString(
      await blake3(b4a.from(stableJson({
        domain: 'mayhem-tnk-settlement-feature-v1',
        value,
      }))),
      'hex'
    );
    return `settle/tnk/${value.epoch}/${digest}`;
  }

  async fiatSettlementFeatureKey(value) {
    const shapeError = this.validateFiatSettlementValue(value);
    if (shapeError) return shapeError;
    const digest = b4a.toString(
      await blake3(b4a.from(stableJson({
        domain: 'mayhem-fiat-settlement-feature-v1',
        value,
      }))),
      'hex'
    );
    return `settle/fiat/${value.epoch}/${digest}`;
  }

  async reputationAnchorFeatureKey(value) {
    const validationError = this.validateReputationAnchor(value);
    if (validationError) return validationError;
    const digest = b4a.toString(
      await blake3(b4a.from(stableJson({
        domain: 'mayhem-reputation-anchor-feature-v1',
        value,
      }))),
      'hex'
    );
    return `rep/${value.provider}/${value.epoch}/${digest}`;
  }

  async tier3MeasurementFeatureKey(value) {
    const validationError = this.validateTier3MeasurementBlessValue(value);
    if (validationError) return validationError;
    const digest = b4a.toString(
      await blake3(b4a.from(stableJson({
        domain: 'mayhem-tier3-measurement-feature-v1',
        value,
      }))),
      'hex'
    );
    return `tier3/measurement/${value.platform}/${digest}`;
  }

  async epochCommitHash(value) {
    const digest = await blake3(b4a.from(stableJson({
      domain: 'mayhem-epoch-commit-v1',
      value,
    })));
    return b4a.toString(digest, 'hex');
  }

  probePass(value, params) {
    if (value.probe_kind === 'uptime_tick') return true;
    return value.match_bps >= params.canary_match_min_bps;
  }

  async requireAuditorEligibility(auditor, atSeconds) {
    const rep = await this.get(`rep/${auditor}`);
    if (!rep) return new Error('Auditor reputation snapshot required.');
    const params = await this.activeParamsAt(atSeconds, [
      'auditor_min_reputation_bps',
      'auditor_min_age_seconds',
    ]);
    if (rep.provenance_violation === true) return new Error('Auditor has a provenance violation.');
    if ((rep.r_bps ?? 0) < params.auditor_min_reputation_bps) {
      return new Error('Auditor reputation too low.');
    }
    const sinceSeconds = rep.probation?.since_seconds ?? 0;
    if (atSeconds - sinceSeconds < params.auditor_min_age_seconds) {
      return new Error('Auditor account age too low.');
    }
    return null;
  }

  async appendReputationEvent(event) {
    if (!this.isSafeKeyPart(event.event_id)) return new Error('Invalid reputation event id.');
    const key = `ev/rep/${event.provider}/${event.event_id}`;
    if ((await this.get(key)) !== null) return new Error('Reputation event already recorded.');

    const headKey = `ev/rep/head/${event.provider}`;
    const currentHead = await this.get(headKey);
    const body = {
      ...event,
      paid_au: event.paid_au !== null && event.paid_au !== undefined
        ? this.normalizeAu(event.paid_au, 'reputation paid amount')
        : null,
      max_spend_au: event.max_spend_au !== null && event.max_spend_au !== undefined
        ? this.normalizeAu(event.max_spend_au, 'reputation max spend')
        : null,
      evidence_hash: event.evidence_hash ?? null,
      recorded_at: this.tx,
      recorded_by: this.address,
    };
    const head = await this.reputationEventHead(currentHead?.head ?? null, body);
    const record = {
      ...body,
      head,
    };
    const headRecord = {
      provider: event.provider,
      head,
      count: (currentHead?.count ?? 0) + 1,
      updated_at: this.tx,
    };

    await this.put(key, record);
    await this.put(headKey, headRecord);
    return record;
  }

  isSafeKeyPart(value) {
    return typeof value === 'string' && /^[a-zA-Z0-9._:-]{1,128}$/.test(value);
  }

  isSafeModelId(value) {
    return typeof value === 'string' &&
      /^[a-zA-Z0-9._:@/+~-]{1,256}$/.test(value) &&
      !value.startsWith('/') &&
      !value.endsWith('/') &&
      !value.includes('//');
  }

  isSafeHuggingFaceRepo(value) {
    if (typeof value !== 'string') return false;
    const parts = value.split('/');
    return parts.length === 2 &&
      parts.every((part) => this.isSafeHuggingFaceComponent(part));
  }

  isSafeHuggingFaceComponent(value) {
    return typeof value === 'string' &&
      /^[A-Za-z0-9][A-Za-z0-9._-]{0,95}$/.test(value) &&
      !value.endsWith('.') &&
      !value.endsWith('-') &&
      !value.includes('..') &&
      !value.includes('--');
  }

  isSafeHuggingFacePath(value) {
    return typeof value === 'string' &&
      value.length > 0 &&
      !value.startsWith('/') &&
      !value.startsWith('\\') &&
      !value.includes('\\') &&
      !value.includes('?') &&
      !value.includes('#') &&
      !value.includes('%') &&
      !/[\x00-\x1f\x7f]/.test(value) &&
      value.split('/').every((part) => this.isSafeHuggingFacePathSegment(part));
  }

  isSafeHuggingFacePathSegment(value) {
    return typeof value === 'string' &&
      value.length > 0 &&
      value !== '.' &&
      value !== '..' &&
      /^[A-Za-z0-9._+-]+$/.test(value);
  }

  isHttpsUrl(value) {
    if (typeof value !== 'string' || value.length === 0 || value.length > 512) return false;
    try {
      const parsed = new URL(value);
      return parsed.protocol === 'https:' && !!parsed.hostname;
    } catch {
      return false;
    }
  }

  isPinnedHuggingFaceResolveUrl(value) {
    if (!this.isHttpsUrl(value)) return false;
    const parsed = new URL(value);
    if (parsed.hostname !== 'huggingface.co') return false;
    const parts = parsed.pathname.split('/').filter(Boolean);
    const resolveIndex = parts.indexOf('resolve');
    if (resolveIndex < 0 || resolveIndex + 2 >= parts.length) return false;
    return this.isHexBytes(parts[resolveIndex + 1], 20);
  }

  isSafeExternalRef(value) {
    return typeof value === 'string' && /^[a-zA-Z0-9._:-]{1,256}$/.test(value);
  }

  normalizeProviderKybValue(value) {
    if (!this.isHexBytes(value.provider, 32)) return new Error('Invalid provider id.');
    const legalName = value.legal_name.trim();
    if (!legalName || /[\x00-\x1f\x7f]/.test(legalName)) {
      return new Error('Invalid provider KYB legal name.');
    }
    const jurisdiction = value.jurisdiction.trim().toUpperCase();
    if (!/^[A-Z0-9._:-]{1,64}$/.test(jurisdiction)) {
      return new Error('Invalid provider KYB jurisdiction.');
    }
    const proofHash = value.proof_hash.toLowerCase();
    if (!this.isHexBytes(proofHash, 32)) return new Error('Invalid provider KYB proof hash.');
    const kybRef = value.kyb_ref.trim();
    if (!this.isSafeExternalRef(kybRef)) return new Error('Invalid provider KYB reference.');
    const schemaVersion = value.schema_version ?? 1;
    if (!Number.isInteger(schemaVersion) || schemaVersion < 1) {
      return new Error('Invalid provider KYB schema version.');
    }
    const adminSig = value.admin_sig.toLowerCase();
    if (!this.isHexBytes(adminSig, 64)) return new Error('Invalid provider KYB admin signature.');
    return {
      provider: value.provider.toLowerCase(),
      legal_name: legalName,
      jurisdiction,
      proof_hash: proofHash,
      kyb_ref: kybRef,
      verified_at: value.verified_at,
      schema_version: schemaVersion,
      admin_sig: adminSig,
    };
  }

  isHexBytes(value, bytes) {
    return typeof value === 'string' &&
      value.length === bytes * 2 &&
      /^[0-9a-fA-F]+$/.test(value);
  }

  isEthHexBytes(value, bytes) {
    return typeof value === 'string' &&
      value.length === bytes * 2 + 2 &&
      /^0x[0-9a-fA-F]+$/.test(value);
  }

  async reputationEventHead(previousHead, event) {
    const payload = stableJson({
      domain: 'mayhem-reputation-event-v1',
      previous_head: previousHead,
      event,
    });
    const digest = await blake3(b4a.from(payload));
    return b4a.toString(digest, 'hex');
  }

  verifyConsentSignature(sender, ver, hash, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, sig, consentMessage(ver, hash), sender) === true;
  }

  verifyProviderLifecycleSignature(provider, intent, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, sig, providerLifecycleIntentMessage(intent), provider) === true;
  }

  verifyDepositTnkSignature(sender, intent, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, sig, depositTnkIntentMessage(intent), sender) === true;
  }

  verifySpendVoucherSignature(user, body, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, sig, spendVoucherMessage(body), user) === true;
  }

  verifySpendReservationSignature(provider, value) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, value.provider_sig, spendReservationMessage(value), provider) === true;
  }

  verifyProbeResultSignature(auditor, value) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, value.auditor_sig, probeResultMessage(value, auditor), auditor) === true;
  }

  async verifyProviderKybSignature(value) {
    const admin = await this.get('admin');
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof admin !== 'string' || typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, value.admin_sig, providerKybMessage(value), admin) === true;
  }
}

export default MayhemContract;
