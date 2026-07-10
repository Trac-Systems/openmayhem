import Feature from 'trac-peer/src/artifacts/feature.js';
import crypto from 'crypto';
import b4a from 'b4a';

const RELAY_CONTROL_REQUEST = 'mayhem_feature_request';
const RELAY_CONTROL_RESULT = 'mayhem_feature_result';
const RELAY_VERSION = 1;
const MAYHEM_RELAY_CHANNEL = '0000mayhem-relay';
const MAYHEM_RELAY_MAX_MESSAGE_BYTES = 16_384;
const DEFAULT_TIMEOUT_MS = 20_000;
const DEFAULT_RETRY_MS = 1_000;
const DEFAULT_CACHE_TTL_MS = 300_000;
const DEFAULT_CACHE_MAX = 2_048;

const normalizeKey = (value) => String(value ?? '').trim().toLowerCase();

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

const requestIdFor = (feature, key, value) =>
  crypto
    .createHash('sha256')
    .update(
      stableJson({
        domain: 'mayhem-feature-relay-v1',
        feature,
        key,
        value,
      })
    )
    .digest('hex');

const participantFor = (value) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  if (value.op === 'consent' || value.op === 'deposit_tnk') return normalizeKey(value.sender);
  if (value.op === 'tap_account_bind') return normalizeKey(value.user);
  if (value.op === 'provider_lifecycle') return normalizeKey(value.intent?.provider);
  if (value.op === 'spend_reserve') return normalizeKey(value.provider);
  return null;
};

const relayError = (message, requestId = null) => ({
  ok: false,
  accepted: false,
  status: 'rejected',
  relayed: true,
  request_id: requestId,
  message,
});

class MayhemFeature extends Feature {
  constructor(peer, config = {}) {
    super(peer, config);
    this.channel = String(config.channel || MAYHEM_RELAY_CHANNEL);
    this.maxMessageBytes = Number.isSafeInteger(config.maxMessageBytes) && config.maxMessageBytes > 0
      ? config.maxMessageBytes
      : MAYHEM_RELAY_MAX_MESSAGE_BYTES;
    this.timeoutMs = Number.isSafeInteger(config.timeoutMs) ? config.timeoutMs : DEFAULT_TIMEOUT_MS;
    this.retryMs = Number.isSafeInteger(config.retryMs) ? config.retryMs : DEFAULT_RETRY_MS;
    this.cacheTtlMs = Number.isSafeInteger(config.cacheTtlMs)
      ? config.cacheTtlMs
      : DEFAULT_CACHE_TTL_MS;
    this.cacheMax = Number.isSafeInteger(config.cacheMax) ? config.cacheMax : DEFAULT_CACHE_MAX;
    this.pending = new Map();
    this.processed = new Map();
  }

  async record(key, value) {
    return await this.append(key, value);
  }

  isRelayMessage(payload) {
    const control = payload?.message?.control;
    return control === RELAY_CONTROL_REQUEST || control === RELAY_CONTROL_RESULT;
  }

  relayMessageBytes(message) {
    return b4a.byteLength(stableJson(message), 'utf8');
  }

  async relay(key, value) {
    const feature = this.key || 'mayhem';
    const actor = participantFor(value);
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    if (!actor) throw new Error('Invalid relayed feature operation.');
    if (!self) throw new Error('Invalid relay transport identity.');
    const admin = await this._adminKey();
    if (this.peer.base?.writable && self === admin) {
      throw new Error('Invalid relay request from the admin writer.');
    }
    const sidechannel = this.peer?.sidechannel;
    if (!sidechannel?.started || typeof sidechannel.broadcast !== 'function') {
      throw new Error('Mayhem feature relay is not ready.');
    }

    const canonicalValue = stableValue(value);
    const requestId = requestIdFor(feature, key, canonicalValue);
    const existing = this.pending.get(requestId);
    if (existing) return await existing.promise;

    let resolvePending;
    const promise = new Promise((resolve) => {
      resolvePending = resolve;
    });
    const message = {
      control: RELAY_CONTROL_REQUEST,
      version: RELAY_VERSION,
      request_id: requestId,
      feature,
      key,
      value: canonicalValue,
    };
    if (this.relayMessageBytes(message) > this.maxMessageBytes) {
      throw new Error(`Mayhem feature relay payload exceeds ${this.maxMessageBytes} bytes.`);
    }
    const send = () => sidechannel.broadcast(this.channel, message);
    this.pending.set(requestId, { promise, resolve: resolvePending });

    if (!send()) {
      this.pending.delete(requestId);
      return relayError('Mayhem feature relay could not send the signed intent.', requestId);
    }

    const retry = setInterval(send, this.retryMs);
    const timeout = setTimeout(() => {
      const current = this.pending.get(requestId);
      if (!current) return;
      this.pending.delete(requestId);
      current.resolve(relayError('Mayhem feature relay timed out before the admin writer acknowledged it.', requestId));
    }, this.timeoutMs);

    try {
      return await promise;
    } finally {
      clearInterval(retry);
      clearTimeout(timeout);
    }
  }

  async handleSidechannelMessage(channel, payload) {
    if (channel !== this.channel || !this.isRelayMessage(payload)) return false;
    if (this.relayMessageBytes(payload.message) > this.maxMessageBytes) return true;
    if (payload.message.control === RELAY_CONTROL_REQUEST) {
      await this._handleRequest(payload);
    } else {
      await this._handleResult(payload);
    }
    return true;
  }

  async _adminKey() {
    const admin = await this.peer?.base?.view?.get('admin');
    return normalizeKey(admin?.value);
  }

  _verifyEnvelope(payload, expectedKey) {
    const sidechannel = this.peer?.sidechannel;
    return sidechannel?.verifyPayload?.(payload, expectedKey) === true;
  }

  _pruneProcessed() {
    const cutoff = Date.now() - this.cacheTtlMs;
    for (const [key, entry] of this.processed) {
      if (entry.at >= cutoff && this.processed.size <= this.cacheMax) break;
      this.processed.delete(key);
    }
  }

  async _handleRequest(payload) {
    if (!this.peer.base?.writable) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    if (!admin || self !== admin) return;

    const message = payload.message;
    const actor = participantFor(message.value);
    const transport = normalizeKey(payload.from);
    if (!actor || !transport || !this._verifyEnvelope(payload, transport)) return;
    if (message.version !== RELAY_VERSION || message.feature !== (this.key || 'mayhem')) return;
    if (typeof message.key !== 'string' || message.key.length < 1 || message.key.length > 256) return;
    const expectedId = requestIdFor(message.feature, message.key, message.value);
    if (message.request_id !== expectedId) return;

    this._pruneProcessed();
    let cached = this.processed.get(expectedId);
    if (!cached) {
      const promise = this._applyRelayed(message.key, stableValue(message.value), expectedId).catch((error) =>
        relayError(error?.message || 'Admin writer failed to apply relayed feature.', expectedId)
      );
      cached = { at: Date.now(), promise };
      this.processed.set(expectedId, cached);
    }
    const response = await cached.promise;
    if (response?.ok !== true) this.processed.delete(expectedId);
    this.peer.sidechannel.broadcast(this.channel, {
      control: RELAY_CONTROL_RESULT,
      version: RELAY_VERSION,
      request_id: expectedId,
      to: transport,
      response,
    });
  }

  async _handleResult(payload) {
    const message = payload.message;
    const requestId = String(message.request_id || '');
    const pending = this.pending.get(requestId);
    if (!pending) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    if (
      message.version !== RELAY_VERSION ||
      normalizeKey(message.to) !== self ||
      normalizeKey(payload.from) !== admin ||
      !this._verifyEnvelope(payload, admin)
    ) {
      return;
    }
    this.pending.delete(requestId);
    pending.resolve({ ...message.response, relayed: true, request_id: requestId });
  }

  async _applyRelayed(key, value, requestId) {
    const nonce = requestId;
    const hash = this.peer.wallet.sign(`${JSON.stringify(value)}${nonce}`);
    const resultKey = `fr/${hash}`;
    const previousResult = (await this.peer.base.view.get(resultKey))?.value ?? null;
    if (previousResult?.ok === true) {
      return this._featureResponse(key, hash, resultKey, previousResult);
    }
    await this.peer.base.append({
      type: 'feature',
      key: `${this.key}_${key}`,
      value: {
        dispatch: {
          type: `${this.key}_feature`,
          key,
          hash,
          value,
          nonce,
          address: this.peer.wallet.publicKey,
        },
      },
    });
    await this.peer.base.update();
    const featureResult =
      (await this._waitForResult(resultKey, 10_000, previousResult)) ?? previousResult;
    if (!featureResult) {
      return {
        ok: false,
        accepted: true,
        status: 'pending',
        feature: this.key,
        key,
        hash,
        message: 'Feature append was accepted but no applied result appeared before timeout.',
        result_key: resultKey,
      };
    }
    return this._featureResponse(key, hash, resultKey, featureResult);
  }

  _featureResponse(key, hash, resultKey, featureResult) {
    const ok = featureResult.ok === true;
    return {
      ok,
      accepted: true,
      status: featureResult.status,
      feature: this.key,
      key,
      hash,
      message: ok ? 'Feature applied.' : featureResult.error?.message ?? 'Feature rejected.',
      result_key: resultKey,
      result: featureResult,
    };
  }

  async _waitForResult(key, timeoutMs = 10_000, previousResult = null) {
    const deadline = Date.now() + timeoutMs;
    const previousJson = previousResult === null ? null : stableJson(previousResult);
    while (Date.now() <= deadline) {
      const result = await this.peer.base.view.get(key);
      if (result !== null && (previousJson === null || stableJson(result.value) !== previousJson)) {
        return result.value;
      }
      await this.sleep(50);
    }
    return null;
  }

  async start() {}

  async stop() {
    for (const [requestId, pending] of this.pending) {
      pending.resolve(relayError('Mayhem feature relay stopped.', requestId));
    }
    this.pending.clear();
    this.processed.clear();
  }
}

export {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_MESSAGE_BYTES,
  participantFor,
  requestIdFor,
};
export default MayhemFeature;
