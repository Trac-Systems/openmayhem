import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { Contract } from 'trac-peer';

export const CONTRACT_VERSION = 1;
const SIGNING_MESSAGE_VERSION = 2;
const SUPPORTED_SIGNING_MESSAGE_VERSIONS = Object.freeze([2, 1]);
const CURRENT_RULES_KEY = 'rules/current';
const PAYOUT_METHODS = new Set(['tnk', 'stripe', 'coinbase']);
const FIAT_DEPOSIT_RAILS = new Set(['stripe', 'coinbase']);
const FIAT_CURRENCIES = new Set(['usd', 'eur']);
const PRICE_DENOMINATION = 'mu_usd';
const RATE_SOURCES = new Set(['gate-spot', 'mexc-spot']);
const TAP_RATE_SOURCES = new Set(['uniswap-v2', 'config', 'stale']);
const CATALOG_SOURCE_KINDS = new Set(['https', 'huggingface']);
const PROVIDER_LIFECYCLE_OPS = new Set([
  'register_provider',
  'join_enclave',
  'leave_enclave',
  'join_room',
  'leave_room',
]);
const PRICE_RATE_LIMIT_SECONDS = 6 * 60 * 60;
const PARAM_ACTIVATION_DELAY_SECONDS = 24 * 60 * 60;
const DAY_SECONDS = 24 * 60 * 60;
const PROBATION_SECONDS = 7 * DAY_SECONDS;
const FULL_SLASH_BPS = 10_000;
const DISPUTE_LOST_SLASH_BPS = 2_000;
const DISPUTE_DEPOSIT_MU = 5_000;
const DISPUTE_EVIDENCE_MAX_BYTES = 4_096;
const LEDGER_BATCH_SCHEMA_MAX = 5_000;
const FRAUD_PROOF_MAX_BYTES = 4_096;
export const SESSION_RECEIPT_SCHEMA_VERSION = 2;
export const NEXT_SESSION_RECEIPT_SCHEMA_VERSION = 3;
const TNK_E18 = 1_000_000_000_000_000_000n;
const TAP_WEI = 1_000_000_000_000_000_000n;
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
  probe_reward_mu: { default: 5_000, min: 0, max: Number.MAX_SAFE_INTEGER },
  uptime_tick_seconds: { default: 6 * 60 * 60, min: 60, max: 30 * DAY_SECONDS },
  holdback_epochs: { default: 168, min: 0, max: 1_000_000 },
  fee_bps: { default: 1_500, min: 0, max: 10_000 },
  payout_min_mu: { default: 1_000_000, min: 0, max: Number.MAX_SAFE_INTEGER },
  price_min_bps: { default: 2_500, min: 1, max: 1_000_000 },
  price_max_bps: { default: 40_000, min: 1, max: 1_000_000 },
  epoch_seconds: { default: 3_600, min: 60, max: 86_400 },
  rate_staleness_seconds: { default: 45 * 60, min: 60, max: 86_400 },
  rules_grace_seconds: { default: 14 * 24 * 60 * 60, min: 0, max: 365 * 24 * 60 * 60 },
  challenge_epochs: { default: 6, min: 0, max: 1_000_000 },
  max_apply_batch: { default: 500, min: 1, max: 5_000 },
});
const REPUTATION_EVENT_KINDS = new Set([
  'session_ok',
  'session_partial',
  'session_fail',
  'probe_ok',
  'probe_fail',
  'uptime_tick',
  'dispute_lost',
  'provenance_violation',
]);
const PROBE_KINDS = new Set(['canary', 'uptime_tick']);
const FRAUD_PROOF_REASONS = new Set(['over_credit']);
const DISPUTE_OUTCOMES = new Set(['provider_fault', 'opener_fault', 'no_fault']);
const DISPUTE_DEPOSIT_ACTIONS = new Set(['refund', 'forfeit']);
const EPOCH_ROOT_KEYS = ['dep', 'use', 'earn', 'fee'];
const EPOCH_TOTAL_KEYS = [
  'dep_count',
  'dep_mu',
  'use_count',
  'use_mu',
  'provider_count',
  'earn_mu',
  'fee_mu',
  'fee_cum_mu',
];
const ENCLAVE_UPDATE_FIELDS = [
  'model_class',
  'backend',
  'artifact_root',
  'artifact_root_kind',
  'artifact_source',
  'source_sha256',
  'manifest_hash',
  'att_tier',
  'binary_hash',
  'caps',
];
const ENCLAVE_BACKENDS = new Set(['llama.cpp', 'mlx', 'trt-llm']);
const DEFAULT_MODEL_CLASS = 'text-generation';
const MODEL_CLASSES = new Set([
  DEFAULT_MODEL_CLASS,
  'embedding',
  'image-generation',
  'video-generation',
  'tts',
  'stt',
]);
const RATE_MAP_MAX_ENTRIES = 16;
const MODEL_CLASS_RATE_UNITS = Object.freeze({
  [DEFAULT_MODEL_CLASS]: new Set(['input_token', 'output_token']),
  embedding: new Set(['input_token', 'embedding']),
  'image-generation': new Set(['image', 'step']),
  'video-generation': new Set(['video_second', 'frame']),
  tts: new Set(['input_character', 'audio_second']),
  stt: new Set(['audio_second']),
});
const ENCLAVE_ARTIFACT_ROOT_KIND = 'blake3_merkle_v1';
const ENCLAVE_CAP_BOOLEAN_FIELDS = [
  'chat',
  'tools',
  'json',
  'embeddings',
  'vision',
  'audio',
];
const ENCLAVE_CAP_FIELDS = new Set([
  ...ENCLAVE_CAP_BOOLEAN_FIELDS,
  'ctx',
  'ctx_max',
  'tp_degree',
  'max_batch_size',
  'max_num_tokens',
]);
const ROOM_POLICY_FIELDS = new Set([
  'region_hint',
  'canary_set',
  'min_reputation',
  'max_price_mult',
]);

export const signingMessageVersions = () => [...SUPPORTED_SIGNING_MESSAGE_VERSIONS];
export const consentMessage = (ver, hash, signingVersion = SIGNING_MESSAGE_VERSION) => {
  if (signingVersion === 1) return `mayhem-consent${ver}${hash}`;
  if (signingVersion === 2) {
    return JSON.stringify({
      domain: 'mayhem-consent',
      signing_version: 2,
      rules_ver: ver,
      rules_hash: hash,
    });
  }
  throw new Error(`Unsupported signing message version: ${signingVersion}`);
};
export const providerLifecycleIntentMessage = (intent, signingVersion = SIGNING_MESSAGE_VERSION) => {
  if (signingVersion === 1) return `mayhem-provider-lifecycle-v1${stableJson(intent)}`;
  if (signingVersion === 2) {
    return JSON.stringify({
      domain: 'mayhem-provider-lifecycle',
      signing_version: 2,
      intent: stableValue(intent),
    });
  }
  throw new Error(`Unsupported signing message version: ${signingVersion}`);
};
export const depositTnkIntentMessage = (intent) =>
  `mayhem-deposit-tnk-intent-v1${stableJson(intent)}`;
export const receiptMessage = (body, signingVersion = SIGNING_MESSAGE_VERSION) => {
  if (signingVersion === 1) {
    return JSON.stringify({
      domain: 'mayhem-session-receipt-v1',
      body,
    });
  }
  if (signingVersion === 2) {
    return JSON.stringify({
      domain: 'mayhem-session-receipt',
      signing_version: 2,
      body,
    });
  }
  throw new Error(`Unsupported signing message version: ${signingVersion}`);
};
export const probeResultEvidence = (value, auditor) => ({
  auditor,
  probe_id: value.probe_id,
  probe_kind: value.probe_kind,
  provider: value.provider,
  enclave_id: value.enclave_id,
  binary_hash: value.binary_hash,
  canary_set: value.canary_set,
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

class MayhemContract extends Contract {
  constructor(protocol, options = {}) {
    super(protocol, options);
    const self = this;

    this.addFeature('mayhem_feature', async function () {
      await self.mayhemFeature();
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
      },
    });

    this.addSchema('setModelRef', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        model_id: { type: 'string', min: 1, max: 256 },
        model_class: { type: 'string', min: 1, max: 64, optional: true },
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
        model_class: { type: 'string', min: 1, max: 64, optional: true },
        backend: { type: 'string', min: 1, max: 64 },
        artifact_root: { type: 'string', min: 1, max: 256 },
        artifact_root_kind: { type: 'string', min: 1, max: 64 },
        artifact_source: { type: 'any' },
        source_sha256: { type: 'string', min: 1, max: 128, optional: true },
        manifest_hash: { type: 'string', min: 1, max: 128 },
        att_tier: { type: 'number', integer: true, min: 1, max: 4 },
        binary_hash: { type: 'string', min: 1, max: 128 },
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
        source_sha256: { type: 'string', min: 1, max: 128, optional: true },
        manifest_hash: { type: 'string', min: 1, max: 128, optional: true },
        att_tier: { type: 'number', integer: true, min: 1, max: 4, optional: true },
        binary_hash: { type: 'string', min: 1, max: 128, optional: true },
        caps: { type: 'any', optional: true },
      },
    });

    this.addSchema('joinEnclave', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
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
        per_req_mu: { type: 'number', integer: true, min: 0 },
        min_session_mu: { type: 'number', integer: true, min: 0 },
        effective_at: { type: 'number', integer: true, min: 0 },
      },
    });

    this.addSchema('readPrice', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        at: { type: 'number', integer: true, min: 0 },
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
        paid_mu: { type: 'number', integer: true, min: 0, optional: true },
        max_spend_mu: { type: 'number', integer: true, min: 0, optional: true },
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

    this.addSchema('fraudProof', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        epoch: { type: 'number', integer: true, min: 1 },
        proof_epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
        reason: { type: 'string', min: 1, max: 64 },
        receipt: { type: 'any' },
        claimed_mu_owed_cum: { type: 'number', integer: true, min: 0 },
        previous_mu_owed_cum: { type: 'number', integer: true, min: 0, optional: true },
      },
    });

    this.addSchema('dispute', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
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

    this.addSchema('fiatChargeback', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        rail: { type: 'string', min: 1, max: 64 },
        who: { type: 'string', min: 1, max: 128 },
        mu: { type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER },
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
        await this.applyRegisterProvider(intent.provider, key);
        return;
      case 'join_enclave':
        await this.applyJoinEnclave(intent.provider, intent.enclave_id, key);
        return;
      case 'leave_enclave':
        await this.applyLeaveEnclave(intent.provider, intent.enclave_id, key);
        return;
      case 'join_room':
        await this.applyJoinRoom(intent.provider, intent.room_id, intent.enclave_id, key);
        return;
      case 'leave_room':
        await this.applyLeaveRoom(intent.provider, intent.room_id, intent.enclave_id, key);
        return;
      default:
        return;
    }
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

    if (this.value.effective_at - this.value.submitted_at < PARAM_ACTIVATION_DELAY_SECONDS) {
      return new Error('Parameter changes require at least 24h activation delay.');
    }

    const valuesError = this.validateParamValues(this.value.values);
    if (valuesError) return valuesError;

    const existingAtEffective = await this.activeParamsAt(this.value.effective_at);
    const mergedAtEffective = { ...existingAtEffective, ...this.value.values };
    const boundsError = this.validateParamBounds(mergedAtEffective);
    if (boundsError) return boundsError;

    const meta = await this.get('params/current');
    const ver = meta ? meta.ver + 1 : 1;
    const keys = Object.keys(this.value.values).sort();
    const update = {
      ver,
      values: cloneValue(this.value.values),
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
          value: this.value.values[key],
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

  async setProviderPayout() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.provider)) return new Error('Invalid provider id.');
    if (!PAYOUT_METHODS.has(this.value.payout_method)) {
      return new Error('Unsupported payout method.');
    }
    let payoutCurrency = null;
    if (this.value.payout_method === 'tnk') {
      if (this.value.payout_currency !== undefined) {
        return new Error('TNK payout target must not include fiat currency.');
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
    await this.put(key, updated);
    console.log('mayhem banProvider', updated);
    return {
      ok: true,
      op: 'banProvider',
      provider: this.value.provider,
      tombstoned_enclaves: updated.tombstoned_enclaves,
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

    const capsError = this.validateEnclaveCaps(this.value.caps);
    if (capsError) return capsError;
    const artifactError = this.validateEnclaveArtifactBinding(this.value);
    if (artifactError) return artifactError;

    const key = `enclave/${this.value.enclave_id}`;
    if ((await this.get(key)) !== null) return new Error('Enclave already registered.');
    const modelClass = this.modelClassFor(this.value);

    const record = {
      enclave_id: this.value.enclave_id,
      model_id: this.value.model_id,
      model_class: modelClass,
      backend: this.value.backend,
      artifact_root: this.value.artifact_root,
      artifact_root_kind: this.value.artifact_root_kind,
      artifact_source: cloneValue(this.value.artifact_source),
      source_sha256: this.value.source_sha256 ?? null,
      manifest_hash: this.value.manifest_hash,
      att_tier: this.value.att_tier,
      binary_hash: this.value.binary_hash,
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
      if (field === 'caps') {
        const capsError = this.validateEnclaveCaps(this.value.caps);
        if (capsError) return capsError;
      }
      updated[field] = cloneValue(this.value[field]);
      changed = true;
    }
    if (!changed) return new Error('No enclave fields to update.');
    const artifactError = this.validateEnclaveArtifactBinding(updated);
    if (artifactError) return artifactError;

    updated.updated_by = this.address;
    updated.updated_by_role = 'admin';
    updated.updated_at = this.tx;
    await this.put(key, updated);
    console.log('mayhem updateEnclave', updated);
    return { ok: true, op: 'updateEnclave', enclave_id: updated.enclave_id };
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
    const shapeError = this.validateExactCommandValue(['op', 'enclave_id'], 'join_enclave');
    if (shapeError) return shapeError;
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');

    return this.applyJoinEnclave(this.address, this.value.enclave_id, this.tx);
  }

  async applyJoinEnclave(providerId, enclaveId, stamp) {
    const consentError = await this.requireConsent(providerId);
    if (consentError) return consentError;
    const providerError = await this.requireProvider(providerId);
    if (providerError) return providerError;

    const enclave = await this.get(`enclave/${enclaveId}`);
    if (!enclave) return new Error('Enclave not found.');
    if (enclave.status !== 'active') return new Error('Enclave is not active.');
    const enclaveRoleError = this.requireAdminCreatedEnclave(enclave);
    if (enclaveRoleError) return enclaveRoleError;
    const priceError = await this.requireCurrentAdminPrice(enclaveId);
    if (priceError) return priceError;

    const key = `serve/${providerId}/${enclaveId}`;
    const existing = await this.get(key);
    if (existing && existing.status === 'active') return new Error('Provider already serving enclave.');
    const provider = await this.get(`prov/${providerId}`);
    if (!provider || provider.status !== 'active') return new Error('Provider registration required.');

    const record = {
      provider: providerId,
      enclave_id: enclaveId,
      model_id: enclave.model_id,
      status: 'active',
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
    await this.put(`prov/${providerId}`, {
      ...provider,
      enclaves: this.providerEnclavesWith(provider, enclaveId),
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
    const priceError = await this.requireCurrentAdminPrice(enclaveId);
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
    if (this.modelClassFor(modelRef) !== this.modelClassFor(enclave)) {
      return new Error('Model reference model_class must match enclave model_class.');
    }

    const rateError = this.validateRateMap(this.value.rate_map, this.modelClassFor(enclave), 'Enclave price rate_map', {
      allowZeroPrice: true,
    });
    if (rateError) return rateError;
    const priceRateMap = this.normalizeRateMap(this.value.rate_map);
    const params = await this.activeParamsAt(this.value.effective_at, ['price_min_bps', 'price_max_bps']);
    const boundsError = this.validateRateMapBounds(priceRateMap, modelRef.rate_map, params);
    if (boundsError) return boundsError;

    const key = `price/${this.value.enclave_id}`;
    const schedule = await this.priceSchedule(key, enclave);
    const latest = this.priceLatestEntry(schedule);
    if (
      latest &&
      this.value.effective_at - latest.effective_at < PRICE_RATE_LIMIT_SECONDS
    ) {
      return new Error('Price changes are limited to once per 6h.');
    }

    const record = {
      enclave_id: this.value.enclave_id,
      model_id: enclave.model_id,
      denom: PRICE_DENOMINATION,
      ver: latest ? latest.ver + 1 : 1,
      rate_map: priceRateMap,
      per_req_mu: this.value.per_req_mu,
      min_session_mu: this.value.min_session_mu,
      effective_at: this.value.effective_at,
      effective_from: this.tx,
      updated_at: this.tx,
      set_by: this.address,
      set_by_role: 'admin',
    };

    const updated = {
      enclave_id: this.value.enclave_id,
      model_id: enclave.model_id,
      denom: PRICE_DENOMINATION,
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
    await this.put(`price/${this.value.enclave_id}/v/${record.ver}`, record);
    console.log('mayhem setPrice', { schedule: updated, record });
    return { ok: true, op: 'setPrice', enclave_id: record.enclave_id, ver: record.ver };
  }

  async readPrice() {
    if (!this.isSafeKeyPart(this.value.enclave_id)) return new Error('Invalid enclave id.');
    const schedule = await this.get(`price/${this.value.enclave_id}`);
    if (!schedule) return { ok: true, op: 'readPrice', enclave_id: this.value.enclave_id, at: this.value.at, price: null };

    const price = this.priceActiveEntry(schedule, this.value.at);
    console.log('mayhem readPrice', { enclave_id: this.value.enclave_id, at: this.value.at, price });
    return { ok: true, op: 'readPrice', enclave_id: this.value.enclave_id, at: this.value.at, price };
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
      paid_mu: this.value.paid_mu ?? null,
      max_spend_mu: this.value.max_spend_mu ?? null,
      evidence_hash: this.value.evidence_hash ?? null,
    });
    if (record instanceof Error) return record;

    let slash = null;
    if (this.value.kind === 'dispute_lost') {
      slash = await this.applyProviderSlash({
        providerId: this.value.provider,
        source: 'dispute',
        reason: 'dispute_lost',
        evidenceHash: this.value.evidence_hash ?? null,
        epoch: this.value.epoch,
        at: this.value.at,
        slashBps: DISPUTE_LOST_SLASH_BPS,
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
      'probe_reward_mu',
      'uptime_tick_seconds',
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
      paid_mu: null,
      max_spend_mu: null,
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
        paid_mu: null,
        max_spend_mu: null,
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
        slashBps: FULL_SLASH_BPS,
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
      binary_hash: this.value.binary_hash ?? null,
      match_bps: this.value.match_bps ?? null,
      pass,
      session_receipt_hash: this.value.session_receipt_hash ?? null,
      evidence_hash: this.value.evidence_hash ?? null,
      auditor_sig: this.value.auditor_sig ?? null,
      reputation_head: (await this.get(`ev/rep/head/${this.value.provider}`))?.head ?? null,
      provenance_violation: provenanceViolation,
      probe_reward_mu: params.probe_reward_mu,
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

    const params = await this.activeParamsAt(this.value.at, [
      'fee_bps',
      'max_apply_batch',
      'holdback_epochs',
      'challenge_epochs',
      'canary_probe_holdback_bps',
      'canary_probe_release_min_passes',
    ]);
    if (this.value.debits.length + this.value.earnings.length > params.max_apply_batch) {
      return new Error('Epoch apply batch exceeds max_apply_batch.');
    }

    const debitMap = this.aggregateLedgerEntries(this.value.debits, 'user', 'mu', 'debit');
    if (debitMap instanceof Error) return debitMap;
    const grossEarningMap = this.aggregateLedgerEntries(
      this.value.earnings,
      'provider',
      'gross_mu',
      'earning'
    );
    if (grossEarningMap instanceof Error) return grossEarningMap;

    const debitTotal = this.sumMu(debitMap);
    if (debitTotal instanceof Error) return debitTotal;
    const grossTotal = this.sumMu(grossEarningMap);
    if (grossTotal instanceof Error) return grossTotal;
    if (debitTotal !== grossTotal) {
      return new Error('Epoch debits must equal gross provider earnings.');
    }

    const fee = await this.feeCumRecord();
    const normalized = {
      epoch: this.value.epoch,
      at: this.value.at,
      fee_bps: params.fee_bps,
      debits: this.mapEntriesForHash(debitMap, 'user', 'mu'),
      earnings: this.mapEntriesForHash(grossEarningMap, 'provider', 'gross_mu'),
      roots,
      totals,
    };
    const applyHash = await this.epochApplyHash(normalized);
    if (fee.updated_epoch === this.value.epoch && fee.last_apply_hash === applyHash) {
      return {
        ok: true,
        op: 'epochApply',
        epoch: this.value.epoch,
        idempotent: true,
        debited_mu: 0,
        earned_mu: 0,
        fee_mu: 0,
      };
    }
    if (this.value.epoch <= fee.updated_epoch) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    if (this.value.epoch !== fee.updated_epoch + 1) {
      return new Error('Guardian monotonic epoch invariant failed: epoch apply must be contiguous.');
    }

    const balances = new Map();
    for (const [user, debitMu] of debitMap) {
      const balance = await this.balanceRecord(user);
      const balanceError = this.guardianValidateBalanceRecord(balance, user);
      if (balanceError) return balanceError;
      if (balance.mu < debitMu) return new Error('Insufficient credit balance.');
      balances.set(user, {
        ...balance,
        mu: balance.mu - debitMu,
        updated_epoch: this.value.epoch,
        updated_at: this.tx,
      });
    }

    const earningDeltas = new Map();
    let feeDeltaMu = 0;
    for (const [provider, grossMu] of grossEarningMap) {
      const providerRecord = await this.get(`prov/${provider}`);
      if (!providerRecord) return new Error('Provider not found.');

      const feeMu = Math.floor((grossMu * params.fee_bps) / 10_000);
      const providerMu = grossMu - feeMu;
      feeDeltaMu = this.safeAddMu(feeDeltaMu, feeMu);
      if (feeDeltaMu instanceof Error) return feeDeltaMu;
      const current = earningDeltas.get(provider) ?? 0;
      const next = this.safeAddMu(current, providerMu);
      if (next instanceof Error) return next;
      earningDeltas.set(provider, next);
    }

    const earnings = new Map();
    let earnCumTotal = 0;
    for (const [provider, deltaMu] of earningDeltas) {
      const current = await this.earningRecord(provider);
      const currentError = this.guardianValidateEarningRecord(current, provider);
      if (currentError) return currentError;
      const probeGate = await this.probeGateForEarning(provider, current, params);
      if (probeGate instanceof Error) return probeGate;
      const refreshed = this.refreshEarningHoldback(
        current,
        this.value.epoch,
        this.lockedEarningEpochs(params),
        probeGate
      );
      if (refreshed instanceof Error) return refreshed;
      const totalMu = this.safeAddMu(refreshed.total_mu, deltaMu);
      if (totalMu instanceof Error) return totalMu;
      const heldMu = this.safeAddMu(refreshed.held_mu, deltaMu);
      if (heldMu instanceof Error) return heldMu;
      const holdbacks = this.appendHoldbackBucket(refreshed.holdbacks, this.value.epoch, deltaMu);
      if (holdbacks instanceof Error) return holdbacks;
      earnings.set(provider, {
        ...refreshed,
        total_mu: totalMu,
        held_mu: heldMu,
        holdbacks,
        updated_epoch: this.value.epoch,
        updated_at: this.tx,
      });
      earnCumTotal = this.safeAddMu(earnCumTotal, totalMu);
      if (earnCumTotal instanceof Error) return earnCumTotal;
    }

    const nextFeeCum = this.safeAddMu(fee.cum_mu, feeDeltaMu);
    if (nextFeeCum instanceof Error) return nextFeeCum;
    const guardian = this.guardianCheckEpochApply({
      epoch: this.value.epoch,
      fee,
      debitTotal,
      feeDeltaMu,
      providerDeltaTotal: grossTotal - feeDeltaMu,
      nextFeeCum,
      balances,
      earnings,
    });
    if (guardian instanceof Error) return guardian;

    if (totals) {
      const totalsError = await this.validateEpochApplyTotals({
        epoch: this.value.epoch,
        roots,
        totals,
        debitTotal,
        feeDeltaMu,
        nextFeeCum,
        providerCount: grossEarningMap.size,
        earnCumTotal,
      });
      if (totalsError) return totalsError;
    }

    for (const [user, balance] of this.sortedMapEntries(balances)) {
      await this.put(`bal/${user}`, balance);
    }
    for (const [provider, earning] of this.sortedMapEntries(earnings)) {
      await this.put(`earn/${provider}`, earning);
    }
    const feeRecord = {
      ...fee,
      cum_mu: nextFeeCum,
      settled_cum_mu: guardian.next_settled_cum_mu,
      updated_epoch: this.value.epoch,
      updated_at: this.tx,
      last_apply_hash: applyHash,
      last_fee_bps: params.fee_bps,
    };
    await this.put('fee/cum', feeRecord);
    if (totals) {
      await this.writeEpochEvidenceRoots({
        epoch: this.value.epoch,
        at: this.value.at,
        roots,
        totals,
        feeDeltaMu,
        feeCumMu: nextFeeCum,
      });
    }

    const result = {
      ok: true,
      op: 'epochApply',
      epoch: this.value.epoch,
      idempotent: false,
      debited_mu: debitTotal,
      earned_mu: grossTotal - feeDeltaMu,
      fee_mu: feeDeltaMu,
    };
    console.log('mayhem epochApply', result);
    return result;
  }

  async epochCommit() {
    const banned = await this.get(`committer/ban/${this.address}`);
    if (banned) return new Error('Epoch committer is banned.');

    const roots = this.normalizeEpochRoots(this.value.roots);
    if (roots instanceof Error) return roots;
    const totals = this.normalizeEpochTotals(this.value.totals);
    if (totals instanceof Error) return totals;
    const params = await this.activeParamsAt(this.value.at, ['challenge_epochs']);
    const normalized = {
      epoch: this.value.epoch,
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

    const receipt = this.normalizeReceiptEnvelope(this.value.receipt);
    if (receipt instanceof Error) return receipt;
    if (!this.verifyReceiptEnvelope(receipt)) {
      return new Error('Invalid receipt signature.');
    }

    const proofHash = await this.fraudProofHash({
      epoch: this.value.epoch,
      proof_epoch: this.value.proof_epoch,
      reason: this.value.reason,
      receipt,
      claimed_mu_owed_cum: this.value.claimed_mu_owed_cum,
      previous_mu_owed_cum: this.value.previous_mu_owed_cum ?? 0,
    });
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

    const proof = await this.validateOverCreditFraudProof(commit, receipt);
    if (proof instanceof Error) return proof;

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
      receipt_hash: proof.receipt_hash,
      actual_mu: proof.actual_mu,
      claimed_mu: proof.claimed_mu,
      committed_use_root: commit.roots.use,
      submitted_by: this.address,
      submitted_at: this.tx,
      at: this.value.at,
      voided_commit: commit.commit_hash,
      banned_submitter: commit.submitted_by,
    };
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
    const ban = existingBan ?? {
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
      slash = await this.applyProviderSlash({
        providerId: commit.submitted_by,
        source: 'fraud_proof',
        reason: 'receipt_forgery',
        evidenceHash: proofHash,
        epoch: this.value.epoch,
        at: this.value.at,
        slashBps: FULL_SLASH_BPS,
        beneficiary: this.address,
        enclaveId: receipt.body.enclave_id,
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

    const balance = await this.balanceRecord(this.address);
    const balanceError = this.guardianValidateBalanceRecord(balance, this.address);
    if (balanceError) return balanceError;
    if (balance.mu < DISPUTE_DEPOSIT_MU) return new Error('Insufficient balance for dispute deposit.');

    const nextBalance = {
      ...balance,
      mu: balance.mu - DISPUTE_DEPOSIT_MU,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch ?? 0),
      updated_at: this.tx,
    };
    const nextBalanceError = this.guardianValidateBalanceRecord(nextBalance, this.address);
    if (nextBalanceError) return nextBalanceError;

    const next = await this.get('disp/next');
    const disputeId = next?.next ?? 1;
    const record = {
      type: 'dispute',
      dispute_id: disputeId,
      status: 'open',
      opened_by: this.address,
      session_id: this.value.session_id,
      reason: this.value.reason,
      provider: this.value.provider ?? null,
      counterparty: this.value.counterparty ?? null,
      enclave_id: this.value.enclave_id ?? null,
      epoch: this.value.epoch ?? null,
      at: this.value.at,
      evidence_hash: this.value.evidence_hash ?? null,
      evidence: cloneValue(this.value.evidence ?? null),
      deposit_mu: DISPUTE_DEPOSIT_MU,
      deposit_holder: this.address,
      opened_at: this.tx,
      updated_at: this.tx,
    };
    record.dispute_hash = await this.opaqueHash('mayhem-dispute-v1', record);

    await this.put(`bal/${this.address}`, nextBalance);
    await this.put(`disp/${disputeId}`, record);
    await this.put('disp/next', { next: disputeId + 1, updated_at: this.tx });
    console.log('mayhem dispute', record);
    return {
      ok: true,
      op: 'dispute',
      dispute_id: disputeId,
      deposit_mu: DISPUTE_DEPOSIT_MU,
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

    let depositRefundedMu = 0;
    let depositForfeitedMu = 0;
    if (this.value.deposit_action === 'refund') {
      depositRefundedMu = dispute.deposit_mu;
      const balance = await this.balanceRecord(dispute.opened_by);
      const balanceError = this.guardianValidateBalanceRecord(balance, dispute.opened_by);
      if (balanceError) return balanceError;
      const nextMu = this.safeAddMu(balance.mu, depositRefundedMu);
      if (nextMu instanceof Error) return nextMu;
      await this.put(`bal/${dispute.opened_by}`, {
        ...balance,
        mu: nextMu,
        updated_epoch: Math.max(balance.updated_epoch, dispute.epoch ?? 0),
        updated_at: this.tx,
      });
    } else {
      depositForfeitedMu = dispute.deposit_mu;
      const fee = await this.feeCumRecord();
      const feeError = this.guardianValidateFeeRecord(fee);
      if (feeError) return feeError;
      const cumMu = this.safeAddMu(fee.cum_mu, depositForfeitedMu);
      if (cumMu instanceof Error) return cumMu;
      const settledCumMu = this.safeAddMu(fee.settled_cum_mu ?? fee.cum_mu, depositForfeitedMu);
      if (settledCumMu instanceof Error) return settledCumMu;
      const updatedFee = {
        ...fee,
        cum_mu: cumMu,
        settled_cum_mu: settledCumMu,
        updated_epoch: Math.max(fee.updated_epoch, dispute.epoch ?? 0),
        updated_at: this.tx,
        last_dispute_forfeit_at: this.tx,
      };
      const updatedFeeError = this.guardianValidateFeeRecord(updatedFee);
      if (updatedFeeError) return updatedFeeError;
      await this.put('fee/cum', updatedFee);
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
        paid_mu: null,
        max_spend_mu: null,
        evidence_hash: this.value.rationale_hash,
      });
      if (reputationEvent instanceof Error) return reputationEvent;

      if (this.value.slash === true) {
        slash = await this.applyProviderSlash({
          providerId: dispute.provider,
          source: 'dispute',
          reason: 'dispute_lost',
          evidenceHash: this.value.rationale_hash,
          epoch: dispute.epoch ?? 0,
          at: this.value.at,
          slashBps: DISPUTE_LOST_SLASH_BPS,
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
      deposit_refunded_mu: depositRefundedMu,
      deposit_forfeited_mu: depositForfeitedMu,
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
      deposit_refunded_mu: depositRefundedMu,
      deposit_forfeited_mu: depositForfeitedMu,
      slash,
      resolution_hash: resolved.resolution_hash,
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
      denom: 'tnk_usd_e6',
      tnk_usd_e6: this.value.tnk_usd_e6,
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
      denom: 'tap_usd_e6',
      tap_usd_e6: this.value.tap_usd_e6,
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
      ['op', 'tnk_usd_e6', 'source', 'ts'],
      'rate oracle'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'rate_oracle') return new Error('Invalid rate oracle op.');
    if (!Number.isSafeInteger(value.tnk_usd_e6) || value.tnk_usd_e6 < 1) {
      return new Error('Invalid TNK/USD rate.');
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
      ['op', 'tap_usd_e6', 'source', 'ts'],
      'TAP rate oracle'
    );
    if (shapeError) return shapeError;
    if (value.op !== 'tap_rate_oracle') return new Error('Invalid TAP rate oracle op.');
    if (!Number.isSafeInteger(value.tap_usd_e6) || value.tap_usd_e6 < 1) {
      return new Error('Invalid TAP/USD rate.');
    }
    if (typeof value.source !== 'string' || value.source.length < 1 || value.source.length > 64) {
      return new Error('Invalid TAP rate source.');
    }
    if (!Number.isSafeInteger(value.ts) || value.ts < 0) {
      return new Error('Invalid TAP rate timestamp.');
    }
    return null;
  }

  async depositTnk() {
    const consentError = await this.requireConsent();
    if (consentError) return consentError;
    if (!this.isSafeKeyPart(this.value.memo_hash)) return new Error('Invalid deposit memo hash.');
    if (!this.isSafeKeyPart(this.value.treasury_address)) return new Error('Invalid TNK treasury address.');
    const tnkE18 = this.parseTnkE18(this.value.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;

    const key = `dep/pending/${this.value.memo_hash}`;
    if ((await this.get(key)) !== null) return new Error('TNK deposit memo already pending.');

    const record = {
      memo_hash: this.value.memo_hash,
      user: this.address,
      status: 'pending',
      requested_at: this.tx,
      treasury_address: this.value.treasury_address,
      tnk_e18: this.value.tnk_e18,
      quoted_mu: this.value.quoted_mu,
      rate_tnk_usd_e6: this.value.rate_tnk_usd_e6,
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
    if (pending.rate_tnk_usd_e6 !== rate.tnk_usd_e6) return new Error('TNK deposit rate does not match pending intent.');
    const mu = this.tnkE18ToMu(tnkE18, rate.tnk_usd_e6);
    if (mu instanceof Error) return mu;
    if (mu <= 0) return new Error('TNK deposit converts to zero mu.');
    if (pending.quoted_mu !== mu) return new Error('TNK deposit credit does not match pending intent.');

    const balance = await this.balanceRecord(pending.user);
    const nextMu = this.safeAddMu(balance.mu, mu);
    if (nextMu instanceof Error) return nextMu;
    const leaf = await this.depositLeafHash({
      rail: 'tnk',
      memo_hash: this.value.memo_hash,
      user_hash: await this.opaqueHash('deposit-user', pending.user),
      mu,
      tnk_e18: this.value.tnk_e18,
      msb_tx_hash: this.value.msb_tx_hash,
      rate_ts: rate.ts,
      treasury_address_hash: await this.opaqueHash('deposit-treasury', pending.treasury_address),
      quoted_mu: pending.quoted_mu,
    });
    const depositRoot = await this.nextDepositRoot({
      epoch: this.value.epoch,
      leaf,
      mu,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const record = {
      ...balance,
      mu: nextMu,
      updated_at: this.tx,
      last_deposit_rate_ts: rate.ts,
    };
    await this.put(`bal/${pending.user}`, record);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    await this.del(pendingKey);
    console.log('mayhem tnkDeposit', {
      who: pending.user,
      mu,
      tnk_e18: this.value.tnk_e18,
      rate_ts: rate.ts,
      epoch: this.value.epoch,
    });
    return {
      ok: true,
      op: 'tnkDeposit',
      who: pending.user,
      mu,
      epoch: this.value.epoch,
      deposit_root: depositRoot.merkle_root,
      rate_ts: rate.ts,
    };
  }

  async tapDeposit() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.who)) return new Error('Invalid TAP deposit recipient.');
    if (!this.isSafeKeyPart(this.value.eth_tx_hash)) return new Error('Invalid Ethereum tx hash.');
    if (!this.isSafeKeyPart(this.value.pool_address)) return new Error('Invalid TAP pool address.');

    const tapWei = this.parseTapWei(this.value.tap_wei);
    if (tapWei instanceof Error) return tapWei;
    const rate = await this.guardianRequireFreshTapRate(this.value.at);
    if (rate instanceof Error) return rate;
    const mu = this.tapWeiToMu(tapWei, rate.tap_usd_e6);
    if (mu instanceof Error) return mu;
    if (mu <= 0) return new Error('TAP deposit converts to zero mu.');

    const who = this.value.who.toLowerCase();
    const ethTxHash = this.value.eth_tx_hash.toLowerCase();
    const poolAddress = this.value.pool_address.toLowerCase();
    const seenKey = `dep/tap/${ethTxHash}/${this.value.log_index}`;
    const existing = await this.get(seenKey);
    if (existing !== null) {
      return {
        ok: true,
        op: 'tapDeposit',
        duplicate: true,
        who: existing.who,
        mu: 0,
        credited_mu: existing.mu ?? null,
        epoch: existing.epoch ?? this.value.epoch,
        deposit_root: (await this.get(`ev/dep/${existing.epoch ?? this.value.epoch}`))?.merkle_root ?? null,
      };
    }

    const balance = await this.balanceRecord(who);
    const balanceError = this.guardianValidateBalanceRecord(balance, who);
    if (balanceError) return balanceError;
    const nextMu = this.safeAddMu(balance.mu, mu);
    if (nextMu instanceof Error) return nextMu;
    const leaf = await this.depositLeafHash({
      rail: 'tap',
      user_hash: await this.opaqueHash('deposit-user', who),
      mu,
      tap_wei: this.value.tap_wei,
      tap_usd_e6: rate.tap_usd_e6,
      rate_ts: rate.ts,
      rate_source: rate.source,
      eth_tx_hash: ethTxHash,
      log_index: this.value.log_index,
      block_number: this.value.block_number,
      chain_id: this.value.chain_id,
      pool_address_hash: await this.opaqueHash('deposit-pool', poolAddress),
    });
    const depositRoot = await this.nextDepositRoot({
      epoch: this.value.epoch,
      leaf,
      mu,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const seen = {
      rail: 'tap',
      who,
      tap_wei: this.value.tap_wei,
      tap_usd_e6: rate.tap_usd_e6,
      rate_ts: rate.ts,
      rate_source: rate.source,
      mu,
      eth_tx_hash: ethTxHash,
      log_index: this.value.log_index,
      block_number: this.value.block_number,
      pool_address: poolAddress,
      chain_id: this.value.chain_id,
      epoch: this.value.epoch,
      at: this.value.at,
      credited_at: this.tx,
      credited_by: this.address,
      credited_by_role: 'admin',
    };
    const record = {
      ...balance,
      user: who,
      mu: nextMu,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch),
      updated_at: this.tx,
      last_deposit_rail: 'tap',
      last_deposit_rate_ts: rate.ts,
      last_deposit_rate_source: rate.source,
      last_deposit_tap_usd_e6: rate.tap_usd_e6,
    };
    await this.put(seenKey, seen);
    await this.put(`bal/${who}`, record);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    console.log('mayhem tapDeposit', {
      who,
      mu,
      tap_wei: this.value.tap_wei,
      tap_usd_e6: rate.tap_usd_e6,
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
      mu,
      epoch: this.value.epoch,
      deposit_root: depositRoot.merkle_root,
      eth_tx_hash: ethTxHash,
      log_index: this.value.log_index,
      rate_ts: rate.ts,
    };
  }

  async fiatDeposit() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!FIAT_DEPOSIT_RAILS.has(this.value.rail)) return new Error('Unsupported fiat deposit rail.');
    if (!this.isSafeKeyPart(this.value.who)) return new Error('Invalid deposit recipient.');
    if (!this.isSafeKeyPart(this.value.ext_ref_hash)) return new Error('Invalid external reference hash.');
    const fiat = this.fiatEvidenceFields();
    if (fiat instanceof Error) return fiat;

    const balance = await this.balanceRecord(this.value.who);
    const balanceError = this.guardianValidateBalanceRecord(balance, this.value.who);
    if (balanceError) return balanceError;
    const nextMu = this.safeAddMu(balance.mu, this.value.mu);
    if (nextMu instanceof Error) return nextMu;
    const leaf = await this.depositLeafHash({
      rail: this.value.rail,
      user_hash: await this.opaqueHash('deposit-user', this.value.who),
      mu: this.value.mu,
      ext_ref_hash: this.value.ext_ref_hash,
      ...fiat,
    });
    const depositRoot = await this.nextDepositRoot({
      epoch: this.value.epoch,
      leaf,
      mu: this.value.mu,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const record = {
      ...balance,
      mu: nextMu,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch),
      updated_at: this.tx,
      last_deposit_rail: this.value.rail,
      ...(fiat.fiat_currency ? { last_deposit_fiat_currency: fiat.fiat_currency } : {}),
    };
    await this.put(`bal/${this.value.who}`, record);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    console.log('mayhem fiatDeposit', {
      rail: this.value.rail,
      who: this.value.who,
      mu: this.value.mu,
      ...fiat,
      epoch: this.value.epoch,
    });
    return {
      ok: true,
      op: 'fiatDeposit',
      rail: this.value.rail,
      who: this.value.who,
      mu: this.value.mu,
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

    const balance = await this.balanceRecord(this.value.who);
    const balanceError = this.guardianValidateBalanceRecord(balance, this.value.who);
    if (balanceError) return balanceError;
    const clawbackMu = Math.min(balance.mu, this.value.mu);
    const networkAbsorbedMu = this.value.mu - clawbackMu;
    const nextMu = this.safeSubMu(balance.mu, clawbackMu);
    if (nextMu instanceof Error) return nextMu;

    const leaf = await this.depositLeafHash({
      rail: this.value.rail,
      user_hash: await this.opaqueHash('deposit-user', this.value.who),
      mu: this.value.mu,
      clawback_mu: clawbackMu,
      network_absorbed_mu: networkAbsorbedMu,
      ext_ref_hash: this.value.ext_ref_hash,
      dispute_ref_hash: this.value.dispute_ref_hash,
      reversed: true,
      ...fiat,
    });
    const depositRoot = await this.nextDepositReversalRoot({
      epoch: this.value.epoch,
      leaf,
      disputedMu: this.value.mu,
      clawbackMu,
      absorbedMu: networkAbsorbedMu,
      at: this.value.at,
    });
    if (depositRoot instanceof Error) return depositRoot;

    const frozen = await this.get(`frozen/${this.value.who}`);
    const disputedMuCum = this.safeAddMu(frozen?.disputed_mu_cum ?? 0, this.value.mu);
    if (disputedMuCum instanceof Error) return disputedMuCum;
    const clawbackMuCum = this.safeAddMu(frozen?.clawback_mu_cum ?? 0, clawbackMu);
    if (clawbackMuCum instanceof Error) return clawbackMuCum;
    const absorbedMuCum = this.safeAddMu(frozen?.network_absorbed_mu_cum ?? 0, networkAbsorbedMu);
    if (absorbedMuCum instanceof Error) return absorbedMuCum;
    const freezeRecord = {
      user: this.value.who,
      status: 'frozen',
      reason: 'fiat_chargeback',
      rail: this.value.rail,
      first_frozen_at: frozen?.first_frozen_at ?? this.tx,
      first_frozen_at_seconds: frozen?.first_frozen_at_seconds ?? this.value.at,
      updated_at: this.tx,
      updated_at_seconds: this.value.at,
      updated_epoch: Math.max(frozen?.updated_epoch ?? 0, this.value.epoch),
      dispute_count: (frozen?.dispute_count ?? 0) + 1,
      disputed_mu_cum: disputedMuCum,
      clawback_mu_cum: clawbackMuCum,
      network_absorbed_mu_cum: absorbedMuCum,
      last_ext_ref_hash: this.value.ext_ref_hash,
      last_dispute_ref_hash: this.value.dispute_ref_hash,
      last_fiat_currency: fiat.fiat_currency,
    };

    await this.put(`bal/${this.value.who}`, {
      ...balance,
      mu: nextMu,
      updated_epoch: Math.max(balance.updated_epoch, this.value.epoch),
      updated_at: this.tx,
      last_chargeback_rail: this.value.rail,
      last_chargeback_fiat_currency: fiat.fiat_currency,
    });
    await this.put(`frozen/${this.value.who}`, freezeRecord);
    await this.put(`ev/dep/${this.value.epoch}`, depositRoot);
    console.log('mayhem fiatChargeback', {
      rail: this.value.rail,
      who: this.value.who,
      mu: this.value.mu,
      clawback_mu: clawbackMu,
      network_absorbed_mu: networkAbsorbedMu,
      ...fiat,
      epoch: this.value.epoch,
    });
    return {
      ok: true,
      op: 'fiatChargeback',
      rail: this.value.rail,
      who: this.value.who,
      mu: this.value.mu,
      clawback_mu: clawbackMu,
      network_absorbed_mu: networkAbsorbedMu,
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

  async requireCurrentAdminPrice(enclaveId) {
    const schedule = await this.get(`price/${enclaveId}`);
    const current = schedule?.current;
    if (!current) {
      return new Error('Current admin price required before provider serving.');
    }
    if (schedule.denom !== PRICE_DENOMINATION || current.denom !== PRICE_DENOMINATION) {
      return new Error('Provider serving requires a current mu_usd admin price.');
    }
    if (current.enclave_id !== enclaveId) {
      return new Error('Provider serving requires a current admin-set enclave price.');
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

  validateProviderLifecycleIntent(intent) {
    if (!PROVIDER_LIFECYCLE_OPS.has(intent.op)) return new Error('Unsupported provider lifecycle op.');
    const allowed = intent.op === 'register_provider'
      ? ['op', 'provider', 'nonce']
      : intent.op === 'join_room' || intent.op === 'leave_room'
        ? ['op', 'provider', 'enclave_id', 'room_id', 'nonce']
        : ['op', 'provider', 'enclave_id', 'nonce'];
    const shapeError = this.validateExactObjectKeys(intent, allowed, 'provider lifecycle intent');
    if (shapeError) return shapeError;
    if (!this.isHexBytes(intent.provider, 32)) return new Error('Invalid provider id.');
    if (!this.isHexBytes(intent.nonce, 32)) return new Error('Invalid lifecycle nonce.');
    if (hasOwn(intent, 'enclave_id') && !this.isSafeKeyPart(intent.enclave_id)) {
      return new Error('Invalid enclave id.');
    }
    if (hasOwn(intent, 'room_id') && !this.isSafeKeyPart(intent.room_id)) {
      return new Error('Invalid room id.');
    }
    return null;
  }

  async providerLifecycleFeatureKey(intent) {
    const digest = await blake3(b4a.from(providerLifecycleIntentMessage(intent)));
    return `intent/provider/${intent.provider}/${intent.op}/${b4a.toString(digest, 'hex')}`;
  }

  async providerLifecycleFeatureKeys(intent) {
    const keys = [];
    for (const signingVersion of SUPPORTED_SIGNING_MESSAGE_VERSIONS) {
      const digest = await blake3(b4a.from(providerLifecycleIntentMessage(intent, signingVersion)));
      keys.push(`intent/provider/${intent.provider}/${intent.op}/${b4a.toString(digest, 'hex')}`);
    }
    return keys;
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
      if (!Number.isInteger(value)) return new Error(`Parameter ${key} must be an integer.`);
      if (value < def.min || value > def.max) return new Error(`Parameter ${key} is out of range.`);
    }
    return null;
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

  validateEnclaveCaps(caps) {
    if (!caps || typeof caps !== 'object' || Array.isArray(caps)) {
      return new Error('Enclave caps must be an object.');
    }
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
    for (const key of ['ctx', 'ctx_max', 'tp_degree', 'max_batch_size', 'max_num_tokens']) {
      if (hasOwn(caps, key) && (!Number.isSafeInteger(caps[key]) || caps[key] <= 0)) {
        return new Error(`Enclave caps ${key} must be a positive integer.`);
      }
    }
    if (hasCtx && hasCtxMax && caps.ctx !== caps.ctx_max) {
      return new Error('Enclave caps ctx and ctx_max must match when both are set.');
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
    return this.validateHuggingFaceArtifactSource(value.artifact_source);
  }

  validateHuggingFaceArtifactSource(source) {
    const shapeError = this.validateExactObjectKeys(
      source,
      ['kind', 'repo', 'revision', 'path'],
      'enclave artifact_source'
    );
    if (shapeError) return shapeError;
    if (source.kind !== 'huggingface') {
      return new Error('Enclave artifact_source.kind must be huggingface.');
    }
    if (!this.isSafeHuggingFaceRepo(source.repo)) {
      return new Error('Enclave artifact_source.repo must be a safe namespace/name repo id.');
    }
    if (!this.isHexBytes(source.revision, 20)) {
      return new Error('Enclave artifact_source.revision must be a 20-byte git commit hex.');
    }
    if (!this.isSafeHuggingFacePath(source.path)) {
      return new Error('Enclave artifact_source.path must be a safe relative Hugging Face artifact path.');
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

  async priceSchedule(key, enclave) {
    const existing = await this.get(key);
    if (existing) return existing;
    return {
      enclave_id: enclave.enclave_id,
      model_id: enclave.model_id,
      denom: PRICE_DENOMINATION,
      current: null,
      pending: null,
    };
  }

  priceActiveEntry(schedule, at) {
    if (schedule.pending && schedule.pending.effective_at <= at) return cloneValue(schedule.pending);
    return schedule.current ? cloneValue(schedule.current) : null;
  }

  priceLatestEntry(schedule) {
    return schedule.pending ?? schedule.current;
  }

  modelClassFor(value) {
    if (!value || !hasOwn(value, 'model_class') || value.model_class === null || value.model_class === undefined) {
      return DEFAULT_MODEL_CLASS;
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

  validateRateMap(rateMap, modelClass, label, { allowZeroPrice = false } = {}) {
    if (!Array.isArray(rateMap) || rateMap.length === 0 || rateMap.length > RATE_MAP_MAX_ENTRIES) {
      return new Error(`${label} must be a non-empty array with at most ${RATE_MAP_MAX_ENTRIES} entries.`);
    }
    const validUnits = MODEL_CLASS_RATE_UNITS[modelClass];
    if (!validUnits) return new Error(`No rate units configured for model_class ${modelClass}.`);
    const seen = new Set();
    for (const entry of rateMap) {
      const shapeError = this.validateExactObjectKeys(entry, ['unit', 'per_unit_mu', 'granularity'], `${label} entry`);
      if (shapeError) return shapeError;
      if (typeof entry.unit !== 'string' || entry.unit.length === 0 || entry.unit.length > 64) {
        return new Error(`${label} unit must be a non-empty string.`);
      }
      if (!validUnits.has(entry.unit)) return new Error(`${label} unit ${entry.unit} is not allowed for model_class ${modelClass}.`);
      if (seen.has(entry.unit)) return new Error(`${label} has duplicate unit ${entry.unit}.`);
      seen.add(entry.unit);
      if (!Number.isSafeInteger(entry.per_unit_mu) || entry.per_unit_mu < 0 || (!allowZeroPrice && entry.per_unit_mu === 0)) {
        return new Error(`${label} per_unit_mu must be ${allowZeroPrice ? 'a non-negative' : 'a positive'} integer.`);
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
        per_unit_mu: entry.per_unit_mu,
        granularity: entry.granularity,
      }))
      .sort((left, right) => left.unit.localeCompare(right.unit));
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
    if (
      !Number.isSafeInteger(ref?.per_unit_mu) ||
      ref.per_unit_mu <= 0 ||
      !Number.isSafeInteger(ref?.granularity) ||
      ref.granularity <= 0 ||
      !Number.isSafeInteger(price?.per_unit_mu) ||
      price.per_unit_mu < 0 ||
      !Number.isSafeInteger(price?.granularity) ||
      price.granularity <= 0
    ) {
      return false;
    }
    const priceScaled = BigInt(price.per_unit_mu) * BigInt(ref.granularity) * 10_000n;
    const refScaled = BigInt(ref.per_unit_mu) * BigInt(price.granularity);
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
      !Number.isInteger(value.paid_mu)
    ) {
      return new Error('Reputation event requires paid_mu.');
    }
    if (value.kind === 'session_fail' && !Number.isInteger(value.max_spend_mu)) {
      return new Error('Reputation event requires max_spend_mu.');
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
    for (const [name, entries] of arrays) {
      if (!Array.isArray(entries)) return new Error(`Epoch apply ${name} must be an array.`);
      if (entries.length > LEDGER_BATCH_SCHEMA_MAX) {
        return new Error(`Epoch apply ${name} batch is too large.`);
      }
    }
    return null;
  }

  validateEpochApplyFeatureValue(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return new Error('epochApply feature value must be an object.');
    }
    const required = ['op', 'epoch', 'at', 'debits', 'earnings'];
    const allowed = new Set([...required, 'roots', 'totals']);
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
        'quoted_mu',
        'rate_tnk_usd_e6',
        'rate_source',
      ],
      'deposit TNK intent'
    );
    if (shapeError) return shapeError;
    if (intent.op !== 'deposit_tnk') return new Error('Invalid deposit TNK intent op.');
    if (!this.isSafeKeyPart(intent.memo_hash)) return new Error('Invalid deposit memo hash.');
    if (!this.isSafeKeyPart(intent.treasury_address)) return new Error('Invalid TNK treasury address.');
    if (!Number.isSafeInteger(intent.quoted_mu) || intent.quoted_mu < 1) {
      return new Error('Invalid TNK quoted credit.');
    }
    if (!Number.isSafeInteger(intent.rate_tnk_usd_e6) || intent.rate_tnk_usd_e6 < 1) {
      return new Error('Invalid TNK quoted rate.');
    }
    if (!this.isSafeKeyPart(intent.rate_source)) return new Error('Invalid TNK rate source.');
    const tnkE18 = this.parseTnkE18(intent.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;
    return null;
  }

  guardianValidateBalanceRecord(record, user = null) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) {
      return new Error('Guardian non-negative balance invariant failed.');
    }
    if (record.denom !== PRICE_DENOMINATION) {
      return new Error('Guardian balance denomination invariant failed.');
    }
    if (user !== null && record.user !== user) {
      return new Error('Guardian balance owner invariant failed.');
    }
    if (!Number.isSafeInteger(record.mu) || record.mu < 0) {
      return new Error('Guardian non-negative balance invariant failed.');
    }
    if (!Number.isSafeInteger(record.updated_epoch) || record.updated_epoch < 0) {
      return new Error('Guardian balance epoch invariant failed.');
    }
    return null;
  }

  guardianValidateEarningRecord(record, provider = null) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    if (record.denom !== PRICE_DENOMINATION) {
      return new Error('Guardian earnings denomination invariant failed.');
    }
    if (provider !== null && record.provider !== provider) {
      return new Error('Guardian earnings owner invariant failed.');
    }
    for (const key of ['total_mu', 'held_mu', 'paid_cum_mu']) {
      if (!Number.isSafeInteger(record[key]) || record[key] < 0) {
        return new Error('Guardian non-negative earnings invariant failed.');
      }
    }
    if (!Number.isSafeInteger(record.updated_epoch) || record.updated_epoch < 0) {
      return new Error('Guardian earnings epoch invariant failed.');
    }
    if (record.held_mu > record.total_mu || record.paid_cum_mu > record.total_mu - record.held_mu) {
      return new Error('Guardian earnings conservation invariant failed.');
    }
    if (record.holdbacks !== undefined) {
      const holdbacks = this.normalizeHoldbackBuckets(record);
      if (holdbacks instanceof Error) return holdbacks;
      const heldMu = this.holdbackBucketTotal(holdbacks);
      if (heldMu instanceof Error) return heldMu;
      if (heldMu !== record.held_mu) {
        return new Error('Guardian earnings conservation invariant failed.');
      }
    }
    return null;
  }

  guardianValidateFeeRecord(record) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) {
      return new Error('Guardian fee conservation invariant failed.');
    }
    if (record.denom !== PRICE_DENOMINATION) {
      return new Error('Guardian fee denomination invariant failed.');
    }
    for (const key of ['cum_mu', 'swept_cum_mu']) {
      if (!Number.isSafeInteger(record[key]) || record[key] < 0) {
        return new Error('Guardian fee conservation invariant failed.');
      }
    }
    if (!Number.isSafeInteger(record.updated_epoch) || record.updated_epoch < 0) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    if (record.swept_cum_mu > record.cum_mu) {
      return new Error('Guardian fee conservation invariant failed.');
    }
    const settledCumMu = record.settled_cum_mu ?? record.cum_mu;
    if (!Number.isSafeInteger(settledCumMu) || settledCumMu < 0 || settledCumMu < record.cum_mu) {
      return new Error('Guardian conservation invariant failed.');
    }
    return null;
  }

  guardianCheckEpochApply({
    epoch,
    fee,
    debitTotal,
    feeDeltaMu,
    providerDeltaTotal,
    nextFeeCum,
    balances,
    earnings,
  }) {
    const feeError = this.guardianValidateFeeRecord(fee);
    if (feeError) return feeError;
    if (epoch <= fee.updated_epoch || epoch !== fee.updated_epoch + 1) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    if (providerDeltaTotal + feeDeltaMu !== debitTotal) {
      return new Error('Guardian conservation invariant failed.');
    }

    for (const [user, balance] of balances) {
      const balanceError = this.guardianValidateBalanceRecord(balance, user);
      if (balanceError) return balanceError;
    }
    for (const [provider, earning] of earnings) {
      const earningError = this.guardianValidateEarningRecord(earning, provider);
      if (earningError) return earningError;
    }

    const priorSettledCumMu = fee.settled_cum_mu ?? fee.cum_mu;
    const nextSettledCumMu = this.safeAddMu(priorSettledCumMu, debitTotal);
    if (nextSettledCumMu instanceof Error) return nextSettledCumMu;
    if (nextFeeCum > nextSettledCumMu) {
      return new Error('Guardian conservation invariant failed.');
    }
    return { ok: true, next_settled_cum_mu: nextSettledCumMu };
  }

  lockedEarningEpochs(params) {
    return Math.max(params.holdback_epochs ?? 0, params.challenge_epochs ?? 0);
  }

  async recordCanaryProbePass(value) {
    const key = `probe/pass/${value.provider}/${value.epoch}`;
    const current = await this.get(key);
    const passCount = this.safeAddMu(current?.pass_count ?? 0, 1);
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
      if (!Number.isSafeInteger(record.held_mu) || record.held_mu < 0) {
        return new Error('Guardian non-negative earnings invariant failed.');
      }
      if (record.held_mu === 0) return [];
      return [{
        epoch: Number.isSafeInteger(record.updated_epoch) ? record.updated_epoch : 0,
        mu: record.held_mu,
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
      if (!Number.isSafeInteger(bucket.mu) || bucket.mu <= 0) {
        return new Error('Guardian non-negative earnings invariant failed.');
      }
      const key = `${bucket.epoch}:${bucket.probe_gate === true ? 'probe' : 'time'}`;
      const next = this.safeAddMu(byEpoch.get(key)?.mu ?? 0, bucket.mu);
      if (next instanceof Error) return next;
      byEpoch.set(key, {
        epoch: bucket.epoch,
        mu: next,
        probe_gate: bucket.probe_gate === true,
      });
    }
    return Array.from(byEpoch.values())
      .sort((a, b) => a.epoch - b.epoch || Number(a.probe_gate) - Number(b.probe_gate))
      .map((bucket) => (
        bucket.probe_gate
          ? { epoch: bucket.epoch, mu: bucket.mu, probe_gate: true }
          : { epoch: bucket.epoch, mu: bucket.mu }
      ));
  }

  holdbackBucketTotal(holdbacks) {
    let total = 0;
    for (const bucket of holdbacks) {
      const next = this.safeAddMu(total, bucket.mu);
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
      if (bucket.epoch + lockedEpochs > currentEpoch) {
        kept.push(bucket);
        continue;
      }
      if (!probeGate) continue;
      if (probeGate.passed_epochs.has(bucket.epoch)) continue;
      if (bucket.probe_gate === true) {
        kept.push(bucket);
        continue;
      }
      const gatedMu = Math.floor((bucket.mu * probeGate.holdback_bps) / 10_000);
      if (gatedMu > 0) {
        kept.push({ epoch: bucket.epoch, mu: gatedMu, probe_gate: true });
      }
    }
    const heldMu = this.holdbackBucketTotal(kept);
    if (heldMu instanceof Error) return heldMu;
    return {
      ...record,
      held_mu: heldMu,
      holdbacks: kept,
      last_holdback_release_epoch: currentEpoch,
    };
  }

  slashHoldbackBuckets(holdbacks, slashMu) {
    if (!Number.isSafeInteger(slashMu) || slashMu < 0) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    if (slashMu === 0) return holdbacks;

    let remaining = slashMu;
    const kept = [];
    for (let idx = holdbacks.length - 1; idx >= 0; idx -= 1) {
      const bucket = holdbacks[idx];
      if (remaining === 0) {
        kept.push(bucket);
        continue;
      }
      if (bucket.mu <= remaining) {
        remaining -= bucket.mu;
        continue;
      }
      kept.push({
        ...bucket,
        epoch: bucket.epoch,
        mu: bucket.mu - remaining,
      });
      remaining = 0;
    }
    if (remaining !== 0) return new Error('Guardian earnings conservation invariant failed.');
    return kept.reverse();
  }

  slashAmount(heldMu, slashBps) {
    if (!Number.isSafeInteger(heldMu) || heldMu < 0) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    if (!Number.isSafeInteger(slashBps) || slashBps < 0 || slashBps > 10_000) {
      return new Error('Invalid slash bps.');
    }
    return Number((BigInt(heldMu) * BigInt(slashBps)) / 10_000n);
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
      a.provider.localeCompare(b.provider) || a.enclave_id.localeCompare(b.enclave_id)
    ));
  }

  roomServesWith(room, providerId, enclaveId) {
    const entries = this.roomServingEntries(room);
    if (!entries.some((entry) => entry.provider === providerId && entry.enclave_id === enclaveId)) {
      entries.push({ provider: providerId, enclave_id: enclaveId });
    }
    return entries.sort((a, b) => (
      a.provider.localeCompare(b.provider) || a.enclave_id.localeCompare(b.enclave_id)
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

    const earning = await this.earningRecord(providerId);
    const earningError = this.guardianValidateEarningRecord(earning, providerId);
    if (earningError) return earningError;
    const holdbacks = this.normalizeHoldbackBuckets(earning);
    if (holdbacks instanceof Error) return holdbacks;

    const forfeitedMu = this.slashAmount(earning.held_mu, slashBps);
    if (forfeitedMu instanceof Error) return forfeitedMu;
    const reporterMu = beneficiary === null ? 0 : Math.floor(forfeitedMu / 2);
    const treasuryMu = forfeitedMu - reporterMu;
    const remainingHoldbacks = this.slashHoldbackBuckets(holdbacks, forfeitedMu);
    if (remainingHoldbacks instanceof Error) return remainingHoldbacks;
    const heldMu = earning.held_mu - forfeitedMu;
    const totalMu = earning.total_mu - forfeitedMu;
    const slashedCumMu = this.safeAddMu(earning.slashed_cum_mu ?? 0, forfeitedMu);
    if (slashedCumMu instanceof Error) return slashedCumMu;
    const updatedEarning = {
      ...earning,
      total_mu: totalMu,
      held_mu: heldMu,
      holdbacks: remainingHoldbacks,
      slashed_cum_mu: slashedCumMu,
      last_slash_at: this.tx,
      updated_at: this.tx,
    };
    const updatedEarningError = this.guardianValidateEarningRecord(updatedEarning, providerId);
    if (updatedEarningError) return updatedEarningError;

    let beneficiaryBalance = null;
    if (reporterMu > 0) {
      const currentBalance = await this.balanceRecord(beneficiary);
      const balanceError = this.guardianValidateBalanceRecord(currentBalance, beneficiary);
      if (balanceError) return balanceError;
      const nextMu = this.safeAddMu(currentBalance.mu, reporterMu);
      if (nextMu instanceof Error) return nextMu;
      beneficiaryBalance = {
        ...currentBalance,
        mu: nextMu,
        updated_epoch: Math.max(currentBalance.updated_epoch, epoch),
        updated_at: this.tx,
      };
    }

    let fee = null;
    let updatedFee = null;
    if (treasuryMu > 0) {
      fee = await this.feeCumRecord();
      const feeError = this.guardianValidateFeeRecord(fee);
      if (feeError) return feeError;
      const cumMu = this.safeAddMu(fee.cum_mu, treasuryMu);
      if (cumMu instanceof Error) return cumMu;
      const settledCumMu = this.safeAddMu(fee.settled_cum_mu ?? fee.cum_mu, treasuryMu);
      if (settledCumMu instanceof Error) return settledCumMu;
      updatedFee = {
        ...fee,
        cum_mu: cumMu,
        settled_cum_mu: settledCumMu,
        updated_epoch: Math.max(fee.updated_epoch, epoch),
        updated_at: this.tx,
        last_slash_at: this.tx,
      };
      const updatedFeeError = this.guardianValidateFeeRecord(updatedFee);
      if (updatedFeeError) return updatedFeeError;
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
      held_before_mu: earning.held_mu,
      held_after_mu: heldMu,
      forfeited_mu: forfeitedMu,
      beneficiary_mu: reporterMu,
      treasury_mu: treasuryMu,
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

    await this.put(`earn/${providerId}`, updatedEarning);
    if (beneficiaryBalance) await this.put(`bal/${beneficiary}`, beneficiaryBalance);
    if (updatedFee) await this.put('fee/cum', updatedFee);
    await this.put(providerKey, updatedProvider);
    await this.put(`ev/slash/${providerId}/${this.tx}`, slash);
    return slash;
  }

  appendHoldbackBucket(holdbacks, epoch, mu) {
    if (!Number.isSafeInteger(epoch) || epoch < 0) {
      return new Error('Guardian monotonic epoch invariant failed.');
    }
    if (!Number.isSafeInteger(mu) || mu <= 0) {
      return new Error('Guardian non-negative earnings invariant failed.');
    }
    const normalized = this.normalizeHoldbackBuckets({ holdbacks });
    if (normalized instanceof Error) return normalized;
    const gated = normalized.filter((bucket) => bucket.probe_gate === true);
    const byEpoch = new Map(
      normalized
        .filter((bucket) => bucket.probe_gate !== true)
        .map((bucket) => [bucket.epoch, bucket.mu])
    );
    const next = this.safeAddMu(byEpoch.get(epoch) ?? 0, mu);
    if (next instanceof Error) return next;
    byEpoch.set(epoch, next);
    return [
      ...Array.from(byEpoch.entries())
        .map(([bucketEpoch, bucketMu]) => ({ epoch: bucketEpoch, mu: bucketMu })),
      ...gated,
    ]
      .sort((a, b) => a.epoch - b.epoch || Number(a.probe_gate === true) - Number(b.probe_gate === true));
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
      return new Error('Epoch roots must include dep, use, earn, and fee.');
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
      const next = this.safeAddMu(usage[unit] ?? 0, count);
      if (next instanceof Error) return next;
      usage[unit] = next;
    }
    return Object.fromEntries(Object.entries(usage).sort(([left], [right]) => left.localeCompare(right)));
  }

  normalizeReceiptEnvelope(value, options = {}) {
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
    const finalReceipt = hasOwn(bodySource, 'final')
      ? bodySource.final
      : bodySource.final_receipt;
    const body = {
      schema_version: bodySource.schema_version,
      session_id: bodySource.session_id,
      seq: bodySource.seq,
      final: finalReceipt,
      user: bodySource.user,
      provider: bodySource.provider,
      enclave_id: bodySource.enclave_id,
      model_id: bodySource.model_id,
      price_ver: bodySource.price_ver,
      rules_ver: bodySource.rules_ver,
      usage: cloneValue(bodySource.usage),
      mu_owed_cum: bodySource.mu_owed_cum,
      prompt_hash: bodySource.prompt_hash,
      ts: bodySource.ts,
    };

    const migratedBody = this.migrateReceiptBody(body, targetSchemaVersion);
    if (migratedBody instanceof Error) return migratedBody;

    const bodyError = this.validateReceiptBody(migratedBody, targetSchemaVersion);
    if (bodyError) return bodyError;

    const envelope = {
      body: migratedBody,
      enclave_sig: receipt.enclave_sig ?? value.enclave_sig,
      user_sig: receipt.user_sig ?? value.user_sig,
      enclave_pubkey: receipt.enclave_pubkey ?? value.enclave_pubkey ?? bodySource.enclave_pubkey ?? null,
    };
    if (stableJson(body) !== stableJson(migratedBody)) envelope.signed_body = body;
    if (!this.isHexBytes(envelope.enclave_sig, 64)) return new Error('Invalid enclave receipt signature.');
    if (!this.isHexBytes(envelope.user_sig, 64)) return new Error('Invalid user receipt signature.');
    if (envelope.enclave_pubkey !== null && !this.isHexBytes(envelope.enclave_pubkey, 32)) {
      return new Error('Invalid enclave receipt public key.');
    }
    return envelope;
  }

  migrateReceiptBody(body, targetSchemaVersion = SESSION_RECEIPT_SCHEMA_VERSION) {
    if (!Number.isSafeInteger(targetSchemaVersion) || targetSchemaVersion < 1) {
      return new Error('Invalid target receipt schema version.');
    }
    if (!body || typeof body !== 'object' || Array.isArray(body)) {
      return new Error('Invalid receipt body.');
    }
    if (!Number.isSafeInteger(body.schema_version) || body.schema_version < 1) {
      return new Error('Unsupported receipt schema version.');
    }
    if (body.schema_version > targetSchemaVersion) {
      return new Error(
        `Unsupported receipt schema migration ${body.schema_version} -> ${targetSchemaVersion}.`
      );
    }

    const migrated = cloneValue(body);
    const usage = this.normalizeReceiptUsage(migrated.usage);
    if (usage instanceof Error) return usage;
    migrated.usage = usage;
    while (migrated.schema_version < targetSchemaVersion) {
      if (migrated.schema_version === 1) {
        migrated.schema_version = 2;
      } else if (migrated.schema_version === 2) {
        migrated.schema_version = 3;
      } else {
        return new Error(
          `Unsupported receipt schema migration ${migrated.schema_version} -> ${targetSchemaVersion}.`
        );
      }
    }
    return migrated;
  }

  validateReceiptBody(body, expectedSchemaVersion = SESSION_RECEIPT_SCHEMA_VERSION) {
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
    if (!Number.isSafeInteger(body.seq) || body.seq < 0) return new Error('Invalid receipt sequence.');
    if (typeof body.final !== 'boolean') return new Error('Invalid receipt final flag.');
    if (!Number.isSafeInteger(body.price_ver) || body.price_ver < 1) {
      return new Error('Invalid receipt price version.');
    }
    if (!Number.isSafeInteger(body.rules_ver) || body.rules_ver < 1) {
      return new Error('Invalid receipt rules version.');
    }
    const usage = this.normalizeReceiptUsage(body.usage);
    if (usage instanceof Error) return usage;
    if (stableJson(usage) !== stableJson(body.usage)) {
      return new Error('Receipt usage must be canonical.');
    }
    if (!Number.isSafeInteger(body.mu_owed_cum) || body.mu_owed_cum < 0) {
      return new Error('Invalid receipt cumulative amount.');
    }
    if (!Number.isSafeInteger(body.ts) || body.ts < 0) return new Error('Invalid receipt timestamp.');
    return null;
  }

  verifyReceiptEnvelope(envelope) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    const signedBody = envelope.signed_body ?? envelope.body;
    const enclaveKey = envelope.enclave_pubkey ?? (
      this.isHexBytes(signedBody.enclave_id, 32) ? signedBody.enclave_id : null
    );
    if (!enclaveKey) return false;
    return SUPPORTED_SIGNING_MESSAGE_VERSIONS.some((signingVersion) => {
      const message = receiptMessage(signedBody, signingVersion);
      return (
        verify.call(this.protocol.peer.wallet, envelope.enclave_sig, message, enclaveKey) === true &&
        verify.call(this.protocol.peer.wallet, envelope.user_sig, message, signedBody.user) === true
      );
    });
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

    const previousMu = this.value.previous_mu_owed_cum ?? 0;
    const claimedCum = this.value.claimed_mu_owed_cum;
    if (!Number.isSafeInteger(previousMu) || previousMu < 0) {
      return new Error('Invalid previous receipt amount.');
    }
    if (previousMu > receipt.body.mu_owed_cum || previousMu > claimedCum) {
      return new Error('Previous receipt amount exceeds cumulative amount.');
    }
    const actualMu = receipt.body.mu_owed_cum - previousMu;
    const claimedMu = claimedCum - previousMu;
    if (claimedMu <= actualMu) return new Error('Receipt does not contradict committed usage.');
    if (commit.totals.use_mu !== claimedMu) {
      return new Error('Fraud proof claimed amount does not match committed usage total.');
    }

    const claimedReceipt = {
      ...receipt,
      body: {
        ...receipt.body,
        mu_owed_cum: claimedCum,
      },
    };
    const claimedUseRoot = await this.usageLeafHash(claimedReceipt);
    if (commit.roots.use !== claimedUseRoot) {
      return new Error('Fraud proof does not match committed usage root.');
    }

    return {
      actual_mu: actualMu,
      claimed_mu: claimedMu,
      receipt_hash: await this.usageLeafHash(receipt),
    };
  }

  async validateEpochApplyTotals({
    epoch,
    roots,
    totals,
    debitTotal,
    feeDeltaMu,
    nextFeeCum,
    providerCount,
    earnCumTotal,
  }) {
    const commit = await this.get(`epoch/commit/${epoch}`);
    if (!commit) return new Error('Epoch commit required before applying evidence roots.');
    if (commit.status === 'void') return new Error('Epoch commit is void.');
    if (
      stableJson(commit.roots) !== stableJson(roots) ||
      stableJson(commit.totals) !== stableJson(totals)
    ) {
      return new Error('Epoch apply roots do not match committed roots.');
    }
    if (totals.use_mu !== debitTotal) return new Error('Epoch usage total does not match debits.');
    if (totals.earn_mu !== earnCumTotal) {
      return new Error('Epoch earn total does not match cumulative provider earnings.');
    }
    if (totals.fee_mu !== feeDeltaMu) return new Error('Epoch fee total does not match computed fee.');
    if (totals.fee_cum_mu !== nextFeeCum) {
      return new Error('Epoch cumulative fee total does not match fee state.');
    }
    if (totals.provider_count !== providerCount) {
      return new Error('Epoch provider count does not match earnings.');
    }

    const depositRoot = await this.get(`ev/dep/${epoch}`);
    if (depositRoot) {
      if (depositRoot.type !== 'deposit_root') return new Error('Invalid deposit evidence root.');
      if (
        depositRoot.merkle_root !== roots.dep ||
        depositRoot.count !== totals.dep_count ||
        depositRoot.mu_total !== totals.dep_mu
      ) {
        return new Error('Committed deposit root does not match deposit evidence.');
      }
    }
    for (const key of ['use', 'earn', 'fee']) {
      if ((await this.get(`ev/${key}/${epoch}`)) !== null) {
        return new Error(`Epoch ${key} evidence root already exists.`);
      }
    }
    return null;
  }

  async writeEpochEvidenceRoots({ epoch, at, roots, totals, feeDeltaMu, feeCumMu }) {
    if ((await this.get(`ev/dep/${epoch}`)) === null) {
      await this.put(`ev/dep/${epoch}`, {
        type: 'deposit_root',
        epoch,
        merkle_root: roots.dep,
        count: totals.dep_count,
        mu_total: totals.dep_mu,
        ts: at,
        updated_at: this.tx,
      });
    }
    await this.put(`ev/use/${epoch}`, {
      type: 'usage_root',
      epoch,
      merkle_root: roots.use,
      sessions: totals.use_count,
      mu_total: totals.use_mu,
      providers: totals.provider_count,
      ts: at,
      updated_at: this.tx,
    });
    await this.put(`ev/earn/${epoch}`, {
      type: 'earn_root',
      epoch,
      merkle_root: roots.earn,
      provider_count: totals.provider_count,
      mu_cum_total: totals.earn_mu,
      ts: at,
      updated_at: this.tx,
    });
    await this.put(`ev/fee/${epoch}`, {
      type: 'fee_root',
      epoch,
      merkle_root: roots.fee,
      mu_fee_epoch: feeDeltaMu,
      mu_fee_cum: feeCumMu,
      sweep_msb_tx_hash: null,
      ts: at,
      updated_at: this.tx,
    });
  }

  aggregateLedgerEntries(entries, idKey, amountKey, label) {
    const out = new Map();
    for (const entry of entries) {
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
        return new Error(`Invalid ${label} entry.`);
      }
      const id = entry[idKey];
      const mu = entry[amountKey];
      if (!this.isSafeKeyPart(id)) return new Error(`Invalid ${label} ${idKey}.`);
      if (!Number.isSafeInteger(mu) || mu <= 0) {
        return new Error(`Invalid ${label} amount.`);
      }
      const next = this.safeAddMu(out.get(id) ?? 0, mu);
      if (next instanceof Error) return next;
      out.set(id, next);
    }
    return out;
  }

  sumMu(entries) {
    let sum = 0;
    for (const [, mu] of entries) {
      const next = this.safeAddMu(sum, mu);
      if (next instanceof Error) return next;
      sum = next;
    }
    return sum;
  }

  safeAddMu(a, b) {
    if (!Number.isSafeInteger(a) || !Number.isSafeInteger(b)) {
      return new Error('mu values must be safe integers.');
    }
    const sum = a + b;
    if (!Number.isSafeInteger(sum) || sum > Number.MAX_SAFE_INTEGER) {
      return new Error('mu value overflow.');
    }
    return sum;
  }

  safeSubMu(a, b) {
    if (!Number.isSafeInteger(a) || !Number.isSafeInteger(b) || b < 0) {
      return new Error('mu values must be safe integers.');
    }
    if (b > a) return new Error('mu value underflow.');
    return a - b;
  }

  sortedMapEntries(map) {
    return Array.from(map.entries()).sort(([a], [b]) => a.localeCompare(b));
  }

  mapEntriesForHash(map, idKey, amountKey) {
    return this.sortedMapEntries(map).map(([id, mu]) => ({
      [idKey]: id,
      [amountKey]: mu,
    }));
  }

  async balanceRecord(user) {
    return (await this.get(`bal/${user}`)) ?? {
      user,
      denom: PRICE_DENOMINATION,
      mu: 0,
      updated_epoch: 0,
      updated_at: null,
    };
  }

  async earningRecord(provider) {
    return (await this.get(`earn/${provider}`)) ?? {
      provider,
      denom: PRICE_DENOMINATION,
      total_mu: 0,
      held_mu: 0,
      paid_cum_mu: 0,
      updated_epoch: 0,
      updated_at: null,
    };
  }

  async feeCumRecord() {
    return (await this.get('fee/cum')) ?? {
      denom: PRICE_DENOMINATION,
      cum_mu: 0,
      swept_cum_mu: 0,
      updated_epoch: 0,
      updated_at: null,
      last_apply_hash: null,
      last_fee_bps: null,
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

  tnkE18ToMu(tnkE18, tnkUsdE6) {
    if (!Number.isSafeInteger(tnkUsdE6) || tnkUsdE6 <= 0) {
      return new Error('Invalid TNK/USD rate.');
    }
    const mu = (tnkE18 * BigInt(tnkUsdE6)) / TNK_E18;
    if (mu > BigInt(Number.MAX_SAFE_INTEGER)) return new Error('mu value overflow.');
    return Number(mu);
  }

  tapWeiToMu(tapWei, tapUsdE6) {
    if (!Number.isSafeInteger(tapUsdE6) || tapUsdE6 <= 0) {
      return new Error('Invalid TAP/USD policy rate.');
    }
    const mu = (tapWei * BigInt(tapUsdE6)) / TAP_WEI;
    if (mu > BigInt(Number.MAX_SAFE_INTEGER)) return new Error('mu value overflow.');
    return Number(mu);
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

  async nextDepositRoot({ epoch, leaf, mu, at }) {
    const current = await this.get(`ev/dep/${epoch}`);
    if (current && current.type !== 'deposit_root') {
      return new Error('Invalid deposit evidence root.');
    }
    const count = (current?.count ?? 0) + 1;
    const muTotal = this.safeAddMu(current?.mu_total ?? 0, mu);
    if (muTotal instanceof Error) return muTotal;
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
      mu_total: muTotal,
      ts: at,
      updated_at: this.tx,
    };
  }

  async nextDepositReversalRoot({ epoch, leaf, disputedMu, clawbackMu, absorbedMu, at }) {
    const current = await this.get(`ev/dep/${epoch}`);
    if (current && current.type !== 'deposit_root') {
      return new Error('Invalid deposit evidence root.');
    }
    const count = (current?.count ?? 0) + 1;
    const reversedMuTotal = this.safeAddMu(current?.reversed_mu_total ?? 0, disputedMu);
    if (reversedMuTotal instanceof Error) return reversedMuTotal;
    const clawbackMuTotal = this.safeAddMu(current?.clawback_mu_total ?? 0, clawbackMu);
    if (clawbackMuTotal instanceof Error) return clawbackMuTotal;
    const networkAbsorbedMuTotal = this.safeAddMu(current?.network_absorbed_mu_total ?? 0, absorbedMu);
    if (networkAbsorbedMuTotal instanceof Error) return networkAbsorbedMuTotal;
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
        mu_total: 0,
      }),
      type: 'deposit_root',
      epoch,
      merkle_root: merkleRoot,
      count,
      reversed: true,
      reversal_count: (current?.reversal_count ?? 0) + 1,
      reversed_mu_total: reversedMuTotal,
      clawback_mu_total: clawbackMuTotal,
      network_absorbed_mu_total: networkAbsorbedMuTotal,
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
      if (!this.isSafeKeyPart(value.rail)) return new Error('Invalid fiat deposit rail.');
      if (!this.isSafeKeyPart(value.ext_ref_hash)) return new Error('Invalid external reference hash.');
      return `dep/fiat/${value.rail}/${value.ext_ref_hash}/${digest}`;
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
      paid_mu: event.paid_mu ?? null,
      max_spend_mu: event.max_spend_mu ?? null,
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
      value.split('/').every((part) => (
        part.length > 0 &&
        part !== '.' &&
        part !== '..' &&
        /^[A-Za-z0-9._+-]+$/.test(part)
      ));
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

  async reputationEventHead(previousHead, event) {
    const payload = JSON.stringify({
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
    return SUPPORTED_SIGNING_MESSAGE_VERSIONS.some((signingVersion) =>
      verify.call(this.protocol.peer.wallet, sig, consentMessage(ver, hash, signingVersion), sender) === true
    );
  }

  verifyProviderLifecycleSignature(provider, intent, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return SUPPORTED_SIGNING_MESSAGE_VERSIONS.some((signingVersion) =>
      verify.call(this.protocol.peer.wallet, sig, providerLifecycleIntentMessage(intent, signingVersion), provider) === true
    );
  }

  verifyDepositTnkSignature(sender, intent, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, sig, depositTnkIntentMessage(intent), sender) === true;
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
