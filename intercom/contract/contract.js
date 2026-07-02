import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { Contract } from 'trac-peer';

const CONTRACT_VERSION = 1;
const CURRENT_RULES_KEY = 'rules/current';
const PAYOUT_METHODS = new Set(['tnk', 'stripe', 'coinbase']);
const PRICE_DENOMINATION = 'mu_usd';
const RATE_SOURCES = new Set(['coinbase-spot', 'kraken']);
const PRICE_RATE_LIMIT_SECONDS = 6 * 60 * 60;
const PARAM_ACTIVATION_DELAY_SECONDS = 24 * 60 * 60;
const DAY_SECONDS = 24 * 60 * 60;
const PROBATION_SECONDS = 7 * DAY_SECONDS;
const LEDGER_BATCH_SCHEMA_MAX = 5_000;
const FRAUD_PROOF_MAX_BYTES = 4_096;
const SESSION_RECEIPT_SCHEMA_VERSION = 1;
const TNK_E18 = 1_000_000_000_000_000_000n;
const PARAM_DEFINITIONS = Object.freeze({
  probation_successful_sessions: { default: 50, min: 0, max: 1_000_000 },
  probation_seconds: { default: PROBATION_SECONDS, min: 0, max: 365 * 24 * 60 * 60 },
  probation_max_concurrent_sessions_per_user: { default: 2, min: 1, max: 1_000_000 },
  probation_price_max_bps: { default: 10_000, min: 0, max: 1_000_000 },
  probation_weight_bps: { default: 5_000, min: 0, max: 10_000 },
  auditor_min_reputation_bps: { default: 8_000, min: 0, max: 10_000 },
  auditor_min_age_seconds: { default: 30 * DAY_SECONDS, min: 0, max: 10 * 365 * DAY_SECONDS },
  canary_match_min_bps: { default: 9_000, min: 0, max: 10_000 },
  probe_reward_mu: { default: 5_000, min: 0, max: Number.MAX_SAFE_INTEGER },
  uptime_tick_seconds: { default: 6 * 60 * 60, min: 60, max: 30 * DAY_SECONDS },
  holdback_epochs: { default: 168, min: 0, max: 1_000_000 },
  fee_bps: { default: 1_500, min: 0, max: 10_000 },
  payout_min_mu: { default: 1_000_000, min: 0, max: Number.MAX_SAFE_INTEGER },
  price_min_bps: { default: 2_500, min: 1, max: 1_000_000 },
  price_max_bps: { default: 40_000, min: 1, max: 1_000_000 },
  epoch_seconds: { default: 3_600, min: 60, max: 86_400 },
  rate_staleness_seconds: { default: 900, min: 60, max: 86_400 },
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
const EPOCH_ROOT_KEYS = ['dep', 'use', 'earn', 'fee', 'pay'];
const EPOCH_TOTAL_KEYS = [
  'dep_count',
  'dep_mu',
  'use_count',
  'use_mu',
  'provider_count',
  'earn_mu',
  'fee_mu',
  'fee_cum_mu',
  'pay_count',
  'pay_mu',
];
const ENCLAVE_UPDATE_FIELDS = [
  'backend',
  'artifact_root',
  'manifest_hash',
  'att_tier',
  'binary_hash',
  'caps',
];

export const consentMessage = (ver, hash) => `mayhem-consent${ver}${hash}`;
export const receiptMessage = (body) => JSON.stringify({
  domain: 'mayhem-session-receipt-v1',
  body,
});
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

    this.addSchema('registerProvider', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        payout_addr: { type: 'string', min: 1, max: 256, optional: true },
        payout_method: { type: 'string', min: 1, max: 32, optional: true },
        registered_at_seconds: { type: 'number', integer: true, min: 0, optional: true },
      },
    });

    this.addSchema('setProviderPayout', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        provider: { type: 'string', min: 1, max: 128 },
        payout_addr: { type: 'string', min: 1, max: 256 },
        payout_method: { type: 'string', min: 1, max: 32 },
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

    this.addSchema('registerEnclave', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        enclave_id: { type: 'string', min: 1, max: 128 },
        model_id: { type: 'string', min: 1, max: 256 },
        backend: { type: 'string', min: 1, max: 64 },
        artifact_root: { type: 'string', min: 1, max: 256 },
        manifest_hash: { type: 'string', min: 1, max: 128 },
        att_tier: { type: 'number', integer: true, min: 1, max: 2 },
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
        backend: { type: 'string', min: 1, max: 64, optional: true },
        artifact_root: { type: 'string', min: 1, max: 256, optional: true },
        manifest_hash: { type: 'string', min: 1, max: 128, optional: true },
        att_tier: { type: 'number', integer: true, min: 1, max: 2, optional: true },
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
        model_id: { type: 'string', min: 1, max: 256 },
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
        in_per_1k_mu: { type: 'number', integer: true, min: 0 },
        out_per_1k_mu: { type: 'number', integer: true, min: 0 },
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
        epoch: { type: 'number', integer: true, min: 0 },
        at: { type: 'number', integer: true, min: 0 },
        canary_set: { type: 'string', min: 1, max: 128, optional: true },
        match_bps: { type: 'number', integer: true, min: 0, max: 10_000, optional: true },
        pass: { type: 'boolean', optional: true },
        session_receipt_hash: { type: 'string', min: 1, max: 128, optional: true },
        evidence_hash: { type: 'string', min: 1, max: 128, optional: true },
      },
    });

    this.addSchema('epochApply', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
        debits: {
          type: 'array',
          max: LEDGER_BATCH_SCHEMA_MAX,
          items: { type: 'any' },
        },
        earnings: {
          type: 'array',
          max: LEDGER_BATCH_SCHEMA_MAX,
          items: { type: 'any' },
        },
        roots: { type: 'any', optional: true },
        totals: { type: 'any', optional: true },
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

    this.addSchema('rateOracle', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        tnk_usd_e6: { type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER },
        source: { type: 'string', min: 1, max: 64 },
        ts: { type: 'number', integer: true, min: 0 },
      },
    });

    this.addSchema('depositTnk', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        memo_hash: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('tnkDeposit', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        memo_hash: { type: 'string', min: 1, max: 128 },
        tnk_e18: { type: 'string', min: 1, max: 80 },
        msb_tx_hash: { type: 'string', min: 1, max: 128 },
        epoch: { type: 'number', integer: true, min: 1 },
        at: { type: 'number', integer: true, min: 0 },
      },
    });

    this.addSchema('payoutConfirm', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        who: { type: 'string', min: 1, max: 128 },
        mu: { type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER },
        tnk_e18: { type: 'string', min: 1, max: 80 },
        msb_tx_hash: { type: 'string', min: 1, max: 128 },
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
          set_at: this.tx,
        },
      };
      await this.put(`params/${key}`, updated);
    }

    await this.put(`params/update/${ver}`, update);
    await this.put('params/current', {
      ver,
      keys,
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
    const consentError = await this.requireConsent();
    if (consentError) return consentError;

    const key = `prov/${this.address}`;
    if ((await this.get(key)) !== null) return new Error('Provider already registered.');

    const record = {
      provider: this.address,
      payout: null,
      status: 'active',
      probation: {
        since: this.tx,
        since_seconds: 0,
        successful_sessions: 0,
      },
      registered_at: this.tx,
      updated_at: this.tx,
    };
    await this.put(key, record);
    console.log('mayhem registerProvider', record);
    return { ok: true, op: 'registerProvider', provider: this.address };
  }

  async setProviderPayout() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.provider)) return new Error('Invalid provider id.');
    if (!PAYOUT_METHODS.has(this.value.payout_method)) {
      return new Error('Unsupported payout method.');
    }

    const key = `prov/${this.value.provider}`;
    const record = await this.get(key);
    if (!record) return new Error('Provider not found.');

    const updated = {
      ...record,
      payout: {
        addr: this.value.payout_addr,
        method: this.value.payout_method,
        set_by: this.address,
        set_at: this.tx,
      },
      updated_at: this.tx,
    };
    await this.put(key, updated);
    console.log('mayhem setProviderPayout', updated);
    return { ok: true, op: 'setProviderPayout', provider: this.value.provider };
  }

  async banProvider() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const key = `prov/${this.value.provider}`;
    const record = await this.get(key);
    if (!record) return new Error('Provider not found.');
    if (record.status === 'banned') return new Error('Provider already banned.');

    const updated = {
      ...record,
      status: 'banned',
      banned_at: this.tx,
      banned_by: this.address,
      ban_reason_hash: this.value.reason_hash ?? null,
      updated_at: this.tx,
    };
    await this.put(key, updated);
    console.log('mayhem banProvider', updated);
    return { ok: true, op: 'banProvider', provider: this.value.provider };
  }

  async registerEnclave() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const key = `enclave/${this.value.enclave_id}`;
    if ((await this.get(key)) !== null) return new Error('Enclave already registered.');

    const record = {
      enclave_id: this.value.enclave_id,
      model_id: this.value.model_id,
      backend: this.value.backend,
      artifact_root: this.value.artifact_root,
      manifest_hash: this.value.manifest_hash,
      att_tier: this.value.att_tier,
      binary_hash: this.value.binary_hash,
      caps: cloneValue(this.value.caps),
      status: 'active',
      created_by: this.address,
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

    updated.updated_at = this.tx;
    await this.put(key, updated);
    console.log('mayhem updateEnclave', updated);
    return { ok: true, op: 'updateEnclave', enclave_id: updated.enclave_id };
  }

  async retireEnclave() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const key = `enclave/${this.value.enclave_id}`;
    const record = await this.get(key);
    if (!record) return new Error('Enclave not found.');
    if (record.status === 'retired') return new Error('Enclave already retired.');

    const updated = {
      ...record,
      status: 'retired',
      retired_at: this.tx,
      updated_at: this.tx,
    };
    await this.put(key, updated);
    console.log('mayhem retireEnclave', updated);
    return { ok: true, op: 'retireEnclave', enclave_id: updated.enclave_id };
  }

  async joinEnclave() {
    const consentError = await this.requireConsent();
    if (consentError) return consentError;
    const providerError = await this.requireProvider();
    if (providerError) return providerError;

    const enclave = await this.get(`enclave/${this.value.enclave_id}`);
    if (!enclave) return new Error('Enclave not found.');
    if (enclave.status !== 'active') return new Error('Enclave is not active.');

    const key = `serve/${this.address}/${this.value.enclave_id}`;
    const existing = await this.get(key);
    if (existing && existing.status === 'active') return new Error('Provider already serving enclave.');

    const record = {
      provider: this.address,
      enclave_id: this.value.enclave_id,
      model_id: enclave.model_id,
      status: 'active',
      joined_at: existing?.joined_at ?? this.tx,
      updated_at: this.tx,
      left_at: null,
      rooms: Array.isArray(existing?.rooms) ? existing.rooms.slice() : [],
    };
    await this.put(key, record);
    console.log('mayhem joinEnclave', record);
    return { ok: true, op: 'joinEnclave', provider: this.address, enclave_id: this.value.enclave_id };
  }

  async leaveEnclave() {
    const key = `serve/${this.address}/${this.value.enclave_id}`;
    const record = await this.get(key);
    if (!record || record.status !== 'active') return new Error('Provider is not serving enclave.');
    if (Array.isArray(record.rooms) && record.rooms.length > 0) {
      return new Error('Provider must leave rooms before leaving enclave.');
    }

    const updated = {
      ...record,
      status: 'inactive',
      updated_at: this.tx,
      left_at: this.tx,
    };
    await this.put(key, updated);
    console.log('mayhem leaveEnclave', updated);
    return { ok: true, op: 'leaveEnclave', provider: this.address, enclave_id: this.value.enclave_id };
  }

  async joinRoom() {
    const consentError = await this.requireConsent();
    if (consentError) return consentError;
    const providerError = await this.requireProvider();
    if (providerError) return providerError;

    const room = await this.get(`room/${this.value.room_id}`);
    if (!room) return new Error('Room not found.');
    if (room.status !== 'open') return new Error('Room is not open.');

    const serving = await this.get(`serve/${this.address}/${this.value.enclave_id}`);
    if (!serving || serving.status !== 'active') return new Error('Provider is not serving enclave.');
    const enclave = await this.get(`enclave/${this.value.enclave_id}`);
    if (!enclave || enclave.status !== 'active') return new Error('Enclave is not active.');
    if (serving.model_id !== enclave.model_id || enclave.model_id !== room.model_id) {
      return new Error('Enclave model does not match room model.');
    }

    const key = `roomserve/${this.value.room_id}/${this.address}/${this.value.enclave_id}`;
    const existing = await this.get(key);
    if (existing && existing.status === 'active') return new Error('Provider already joined room with enclave.');

    const rooms = Array.isArray(serving.rooms) ? serving.rooms.slice() : [];
    if (!rooms.includes(this.value.room_id)) rooms.push(this.value.room_id);
    rooms.sort();
    const record = {
      room_id: this.value.room_id,
      sidechannel: room.sidechannel,
      provider: this.address,
      enclave_id: this.value.enclave_id,
      model_id: enclave.model_id,
      status: 'active',
      joined_at: existing?.joined_at ?? this.tx,
      updated_at: this.tx,
      left_at: null,
    };
    await this.put(key, record);
    await this.put(`serve/${this.address}/${this.value.enclave_id}`, {
      ...serving,
      rooms,
      updated_at: this.tx,
    });
    console.log('mayhem joinRoom', record);
    return {
      ok: true,
      op: 'joinRoom',
      room_id: this.value.room_id,
      provider: this.address,
      enclave_id: this.value.enclave_id,
      sidechannel: room.sidechannel,
    };
  }

  async leaveRoom() {
    const key = `roomserve/${this.value.room_id}/${this.address}/${this.value.enclave_id}`;
    const record = await this.get(key);
    if (!record || record.status !== 'active') return new Error('Provider has not joined room with enclave.');

    const servingKey = `serve/${this.address}/${this.value.enclave_id}`;
    const serving = await this.get(servingKey);
    const rooms = Array.isArray(serving?.rooms)
      ? serving.rooms.filter((roomId) => roomId !== this.value.room_id)
      : [];
    const updated = {
      ...record,
      status: 'inactive',
      updated_at: this.tx,
      left_at: this.tx,
    };
    await this.put(key, updated);
    if (serving) {
      await this.put(servingKey, {
        ...serving,
        rooms,
        updated_at: this.tx,
      });
    }
    console.log('mayhem leaveRoom', updated);
    return {
      ok: true,
      op: 'leaveRoom',
      room_id: this.value.room_id,
      provider: this.address,
      enclave_id: this.value.enclave_id,
      sidechannel: updated.sidechannel,
    };
  }

  async openRoom() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const roomId = await deriveRoomId(this.value.model_id, this.address, this.value.nonce);
    const key = `room/${roomId}`;
    const existing = await this.get(key);
    if (existing && existing.status !== 'closed') return new Error('Room already open.');

    const record = {
      room_id: roomId,
      sidechannel: roomSidechannelName(roomId),
      model_id: this.value.model_id,
      label: this.value.label,
      creator: this.address,
      policy: cloneValue(this.value.policy),
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

    const key = `room/${this.value.room_id}`;
    const record = await this.get(key);
    if (!record) return new Error('Room not found.');
    if (record.status === 'closed') return new Error('Room already closed.');

    const updated = {
      ...record,
      status: 'closed',
      updated_at: this.tx,
      closed_at: this.tx,
    };
    await this.put(key, updated);
    console.log('mayhem closeRoom', updated);
    return { ok: true, op: 'closeRoom', room_id: updated.room_id, sidechannel: updated.sidechannel };
  }

  async setPrice() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const enclave = await this.get(`enclave/${this.value.enclave_id}`);
    if (!enclave) return new Error('Enclave not found.');
    if (enclave.status === 'retired') return new Error('Enclave is retired.');

    const modelRef = await this.get(`modelref/${enclave.model_id}`);
    if (!modelRef) return new Error('Model reference not found.');

    const inRef = this.modelRefPrice(modelRef, 'in_per_1k_mu', 'in_per_1k');
    const outRef = this.modelRefPrice(modelRef, 'out_per_1k_mu', 'out_per_1k');
    const params = await this.activeParamsAt(this.value.effective_at, ['price_min_bps', 'price_max_bps']);
    if (!this.priceWithinBounds(this.value.in_per_1k_mu, inRef, params)) {
      return new Error('Input price outside model reference bounds.');
    }
    if (!this.priceWithinBounds(this.value.out_per_1k_mu, outRef, params)) {
      return new Error('Output price outside model reference bounds.');
    }

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
      in_per_1k_mu: this.value.in_per_1k_mu,
      out_per_1k_mu: this.value.out_per_1k_mu,
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

    console.log('mayhem recordReputationEvent', record);
    return {
      ok: true,
      op: 'recordReputationEvent',
      provider: this.value.provider,
      event_id: this.value.event_id,
      head: record.head,
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

    const validationError = this.validateProbeResult(this.value);
    if (validationError) return validationError;

    const providerKey = `prov/${this.value.provider}`;
    const provider = await this.get(providerKey);
    if (!provider) return new Error('Provider not found.');

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

      slash = {
        provider: this.value.provider,
        auditor: this.address,
        reason: 'canary_mismatch',
        evidence_hash: this.value.evidence_hash ?? null,
        probe_id: this.value.probe_id,
        at: this.value.at,
        tx: this.tx,
      };
      await this.put(`ev/slash/${this.value.provider}/${this.tx}`, slash);
      await this.put(providerKey, {
        ...provider,
        status: 'banned',
        banned_at: this.tx,
        banned_by: this.address,
        ban_reason_hash: this.value.evidence_hash ?? null,
        updated_at: this.tx,
      });
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
      match_bps: this.value.match_bps ?? null,
      pass,
      session_receipt_hash: this.value.session_receipt_hash ?? null,
      evidence_hash: this.value.evidence_hash ?? null,
      reputation_head: (await this.get(`ev/rep/head/${this.value.provider}`))?.head ?? null,
      provenance_violation: provenanceViolation,
      probe_reward_mu: params.probe_reward_mu,
      slash,
      recorded_at: this.tx,
    };
    await this.put(`ev/probe/${this.value.probe_id}`, record);
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

    const params = await this.activeParamsAt(this.value.at, ['fee_bps', 'max_apply_batch']);
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
      const totalMu = this.safeAddMu(current.total_mu, deltaMu);
      if (totalMu instanceof Error) return totalMu;
      const heldMu = this.safeAddMu(current.held_mu, deltaMu);
      if (heldMu instanceof Error) return heldMu;
      earnings.set(provider, {
        ...current,
        total_mu: totalMu,
        held_mu: heldMu,
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
    };
  }

  async rateOracle() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
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
    };
    await this.put('rate/latest', record);
    console.log('mayhem rateOracle', record);
    return { ok: true, op: 'rateOracle', ts: record.ts, source: record.source };
  }

  async depositTnk() {
    const consentError = await this.requireConsent();
    if (consentError) return consentError;
    if (!this.isSafeKeyPart(this.value.memo_hash)) return new Error('Invalid deposit memo hash.');

    const key = `dep/pending/${this.value.memo_hash}`;
    if ((await this.get(key)) !== null) return new Error('TNK deposit memo already pending.');

    const record = {
      memo_hash: this.value.memo_hash,
      user: this.address,
      status: 'pending',
      requested_at: this.tx,
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
    const mu = this.tnkE18ToMu(tnkE18, rate.tnk_usd_e6);
    if (mu instanceof Error) return mu;
    if (mu <= 0) return new Error('TNK deposit converts to zero mu.');

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

  async payoutConfirm() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;
    if (!this.isSafeKeyPart(this.value.who)) return new Error('Invalid payout recipient.');

    const rate = await this.guardianRequireFreshRate(this.value.at);
    if (rate instanceof Error) return rate;
    const tnkE18 = this.parseTnkE18(this.value.tnk_e18);
    if (tnkE18 instanceof Error) return tnkE18;
    const convertedMu = this.tnkE18ToMu(tnkE18, rate.tnk_usd_e6);
    if (convertedMu instanceof Error) return convertedMu;
    if (convertedMu < this.value.mu) {
      return new Error('Payout TNK amount is below the mu amount at the oracle rate.');
    }

    const earning = await this.earningRecord(this.value.who);
    const payoutGuardian = this.guardianCheckPayoutConfirm(earning, this.value.mu);
    if (payoutGuardian instanceof Error) return payoutGuardian;
    const releasedMu = payoutGuardian.released_mu;
    if (releasedMu < this.value.mu) return new Error('Insufficient released earnings.');
    const paidCumMu = this.safeAddMu(earning.paid_cum_mu, this.value.mu);
    if (paidCumMu instanceof Error) return paidCumMu;

    const updated = {
      ...earning,
      paid_cum_mu: paidCumMu,
      updated_at: this.tx,
      last_payout_rate_ts: rate.ts,
      last_payout_msb_tx_hash: this.value.msb_tx_hash,
    };
    await this.put(`earn/${this.value.who}`, updated);
    console.log('mayhem payoutConfirm', {
      who: this.value.who,
      mu: this.value.mu,
      tnk_e18: this.value.tnk_e18,
      rate_ts: rate.ts,
    });
    return {
      ok: true,
      op: 'payoutConfirm',
      who: this.value.who,
      mu: this.value.mu,
      rate_ts: rate.ts,
    };
  }

  async currentRules() {
    return await this.get(CURRENT_RULES_KEY);
  }

  async isAdmin(sender = this.address) {
    const admin = await this.get('admin');
    return admin === null || admin === sender;
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
    return null;
  }

  async requireProvider(sender = this.address) {
    const provider = await this.get(`prov/${sender}`);
    if (!provider || provider.status !== 'active') return new Error('Provider registration required.');
    return null;
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

  modelRefPrice(modelRef, directKey, nestedKey) {
    if (Number.isInteger(modelRef?.[directKey])) return modelRef[directKey];
    if (Number.isInteger(modelRef?.price_ref_mu?.[nestedKey])) return modelRef.price_ref_mu[nestedKey];
    return null;
  }

  priceWithinBounds(price, ref, params = {
    price_min_bps: PARAM_DEFINITIONS.price_min_bps.default,
    price_max_bps: PARAM_DEFINITIONS.price_max_bps.default,
  }) {
    if (!Number.isInteger(ref) || ref <= 0) return false;
    return (
      price * 10_000 >= ref * params.price_min_bps &&
      price * 10_000 <= ref * params.price_max_bps
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
      if (!Number.isInteger(value.match_bps)) return new Error('Canary probe requires match_bps.');
      if (!value.canary_set) return new Error('Canary probe requires canary_set.');
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

  guardianCheckPayoutConfirm(earning, payoutMu) {
    const earningError = this.guardianValidateEarningRecord(earning, earning.provider);
    if (earningError) return earningError;
    if (!Number.isSafeInteger(payoutMu) || payoutMu <= 0) {
      return new Error('Guardian non-negative payout invariant failed.');
    }
    const releasedMu = earning.total_mu - earning.held_mu - earning.paid_cum_mu;
    if (!Number.isSafeInteger(releasedMu) || releasedMu < 0) {
      return new Error('Guardian earnings conservation invariant failed.');
    }
    return { ok: true, released_mu: releasedMu };
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
      return new Error('Epoch roots must include dep, use, earn, fee, and pay.');
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

  normalizeReceiptEnvelope(value) {
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
    const usageSource = bodySource.usage;
    if (!usageSource || typeof usageSource !== 'object' || Array.isArray(usageSource)) {
      return new Error('Fraud proof receipt usage must be an object.');
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
      usage: {
        in: hasOwn(usageSource, 'in') ? usageSource.in : usageSource.in_tokens,
        out: hasOwn(usageSource, 'out') ? usageSource.out : usageSource.out_tokens,
      },
      mu_owed_cum: bodySource.mu_owed_cum,
      prompt_hash: bodySource.prompt_hash,
      ts: bodySource.ts,
    };

    const bodyError = this.validateReceiptBody(body);
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

  validateReceiptBody(body) {
    if (body.schema_version !== SESSION_RECEIPT_SCHEMA_VERSION) {
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
    if (!Number.isSafeInteger(body.usage.in) || body.usage.in < 0) {
      return new Error('Invalid receipt input usage.');
    }
    if (!Number.isSafeInteger(body.usage.out) || body.usage.out < 0) {
      return new Error('Invalid receipt output usage.');
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
    const message = receiptMessage(envelope.body);
    const enclaveKey = envelope.enclave_pubkey ?? (
      this.isHexBytes(envelope.body.enclave_id, 32) ? envelope.body.enclave_id : null
    );
    if (!enclaveKey) return false;
    return (
      verify.call(this.protocol.peer.wallet, envelope.enclave_sig, message, enclaveKey) === true &&
      verify.call(this.protocol.peer.wallet, envelope.user_sig, message, envelope.body.user) === true
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
    for (const key of ['use', 'earn', 'fee', 'pay']) {
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
    await this.put(`ev/pay/${epoch}`, {
      type: 'payout_root',
      epoch,
      merkle_root: roots.pay,
      count: totals.pay_count,
      mu_total: totals.pay_mu,
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

  tnkE18ToMu(tnkE18, tnkUsdE6) {
    if (!Number.isSafeInteger(tnkUsdE6) || tnkUsdE6 <= 0) {
      return new Error('Invalid TNK/USD rate.');
    }
    const mu = (tnkE18 * BigInt(tnkUsdE6)) / TNK_E18;
    if (mu > BigInt(Number.MAX_SAFE_INTEGER)) return new Error('mu value overflow.');
    return Number(mu);
  }

  async requireFreshRate(at) {
    const rate = await this.get('rate/latest');
    if (!rate) return new Error('Fresh rate oracle required.');
    if (rate.ts > at) return new Error('Rate oracle timestamp is in the future.');
    const params = await this.activeParamsAt(at, ['rate_staleness_seconds']);
    if (at - rate.ts > params.rate_staleness_seconds) {
      return new Error('Rate oracle is stale.');
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

  async epochApplyHash(value) {
    const digest = await blake3(b4a.from(stableJson({
      domain: 'mayhem-epoch-apply-v1',
      value,
    })));
    return b4a.toString(digest, 'hex');
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
    return verify.call(this.protocol.peer.wallet, sig, consentMessage(ver, hash), sender) === true;
  }
}

export default MayhemContract;
