import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { Contract } from 'trac-peer';

const CONTRACT_VERSION = 1;
const CURRENT_RULES_KEY = 'rules/current';
const PAYOUT_METHODS = new Set(['tnk', 'stripe', 'coinbase']);
const PRICE_DENOMINATION = 'mu_usd';
const PRICE_RATE_LIMIT_SECONDS = 6 * 60 * 60;
const PARAM_ACTIVATION_DELAY_SECONDS = 24 * 60 * 60;
const PARAM_DEFINITIONS = Object.freeze({
  probation_successful_sessions: { default: 3, min: 0, max: 1_000_000 },
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
const ENCLAVE_UPDATE_FIELDS = [
  'backend',
  'artifact_root',
  'manifest_hash',
  'att_tier',
  'binary_hash',
  'caps',
];

export const consentMessage = (ver, hash) => `mayhem-consent${ver}${hash}`;
export const roomSidechannelName = (roomId) => `mx/room/${roomId}`;
export const deriveRoomId = async (modelId, creator, nonce) => {
  const digest = await blake3(b4a.from(`${modelId}${creator}${nonce}`));
  return b4a.toString(digest, 'hex').slice(0, 32);
};

const cloneValue = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));
const hasOwn = (value, key) => Object.prototype.hasOwnProperty.call(value, key);

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
    if (!PAYOUT_METHODS.has(this.value.payout_method)) {
      return new Error('Unsupported payout method.');
    }

    const key = `prov/${this.address}`;
    if ((await this.get(key)) !== null) return new Error('Provider already registered.');

    const record = {
      provider: this.address,
      payout: {
        addr: this.value.payout_addr,
        method: this.value.payout_method,
      },
      status: 'active',
      probation: {
        since: this.tx,
        successful_sessions: 0,
      },
      registered_at: this.tx,
      updated_at: this.tx,
    };
    await this.put(key, record);
    console.log('mayhem registerProvider', record);
    return { ok: true, op: 'registerProvider', provider: this.address };
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

  verifyConsentSignature(sender, ver, hash, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, sig, consentMessage(ver, hash), sender) === true;
  }
}

export default MayhemContract;
