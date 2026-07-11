import Feature from 'trac-peer/src/artifacts/feature.js';
import crypto from 'crypto';
import b4a from 'b4a';

const RELAY_CONTROL_REQUEST = 'mayhem_feature_request';
const RELAY_CONTROL_RESULT = 'mayhem_feature_result';
const SERVICE_CONTROL_REQUEST = 'mayhem_service_request';
const SERVICE_CONTROL_RESULT = 'mayhem_service_result';
const RELAY_VERSION = 1;
const MAYHEM_RELAY_CHANNEL = '0000mayhem-relay';
const MAYHEM_RELAY_MAX_MESSAGE_BYTES = 16_384;
const DEFAULT_TIMEOUT_MS = 0;
const DEFAULT_RETRY_MS = 1_000;
const DEFAULT_CONNECT_TIMEOUT_MS = 15_000;
const DEFAULT_RESULT_POLL_MS = 50;
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

const serviceRequestIdFor = (service, value) =>
  crypto
    .createHash('sha256')
    .update(
      stableJson({
        domain: 'mayhem-service-relay-v1',
        service,
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

const serviceParticipantFor = (service, value) => {
  if (service !== 'stripe_checkout' || !value || typeof value !== 'object' || Array.isArray(value)) {
    return null;
  }
  return normalizeKey(value.who);
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
    this.timeoutMs = Number.isSafeInteger(config.timeoutMs) && config.timeoutMs >= 0
      ? config.timeoutMs
      : DEFAULT_TIMEOUT_MS;
    this.retryMs = Number.isSafeInteger(config.retryMs) && config.retryMs > 0
      ? config.retryMs
      : DEFAULT_RETRY_MS;
    this.connectTimeoutMs = Number.isSafeInteger(config.connectTimeoutMs) &&
      config.connectTimeoutMs > 0
      ? config.connectTimeoutMs
      : this.timeoutMs > 0
        ? Math.min(this.timeoutMs, DEFAULT_CONNECT_TIMEOUT_MS)
        : DEFAULT_CONNECT_TIMEOUT_MS;
    this.resultTimeoutMs = Number.isSafeInteger(config.resultTimeoutMs) &&
      config.resultTimeoutMs >= 0
      ? config.resultTimeoutMs
      : DEFAULT_TIMEOUT_MS;
    this.resultPollMs = Number.isSafeInteger(config.resultPollMs) && config.resultPollMs > 0
      ? config.resultPollMs
      : DEFAULT_RESULT_POLL_MS;
    this.cacheTtlMs = Number.isSafeInteger(config.cacheTtlMs)
      ? config.cacheTtlMs
      : DEFAULT_CACHE_TTL_MS;
    this.cacheMax = Number.isSafeInteger(config.cacheMax) ? config.cacheMax : DEFAULT_CACHE_MAX;
    this.serviceHandler = typeof config.serviceHandler === 'function' ? config.serviceHandler : null;
    this.pending = new Map();
    this.processed = new Map();
    this.servicePending = new Map();
    this.serviceProcessed = new Map();
    this.stopped = false;
  }

  async record(key, value) {
    return await this.append(key, value);
  }

  isRelayMessage(payload) {
    const control = payload?.message?.control;
    return control === RELAY_CONTROL_REQUEST ||
      control === RELAY_CONTROL_RESULT ||
      control === SERVICE_CONTROL_REQUEST ||
      control === SERVICE_CONTROL_RESULT;
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
    return await this._relayUntilAcknowledged({
      admin,
      sidechannel,
      requestId,
      message,
      pending: this.pending,
      unavailableMessage:
        'Mayhem feature relay could not establish its direct channel to the canonical admin.',
      unsentMessage: 'Mayhem feature relay could not send the signed intent.',
      timeoutMessage:
        'Mayhem feature relay timed out before the admin writer acknowledged it.',
    });
  }

  async requestService(service, value) {
    const actor = serviceParticipantFor(service, value);
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    if (!actor || !self || actor !== self) {
      throw new Error('Invalid Mayhem service request identity.');
    }
    const admin = await this._adminKey();
    if (this.peer.base?.writable && self === admin) {
      if (!this.serviceHandler) throw new Error('Mayhem admin service is not configured.');
      return await this.serviceHandler(service, stableValue(value));
    }
    const sidechannel = this.peer?.sidechannel;
    if (!sidechannel?.started || typeof sidechannel.broadcast !== 'function') {
      throw new Error('Mayhem service relay is not ready.');
    }

    const canonicalValue = stableValue(value);
    const requestId = serviceRequestIdFor(service, canonicalValue);
    const existing = this.servicePending.get(requestId);
    if (existing) return await existing.promise;

    const message = {
      control: SERVICE_CONTROL_REQUEST,
      version: RELAY_VERSION,
      request_id: requestId,
      service,
      value: canonicalValue,
    };
    if (this.relayMessageBytes(message) > this.maxMessageBytes) {
      throw new Error(`Mayhem service relay payload exceeds ${this.maxMessageBytes} bytes.`);
    }
    return await this._relayUntilAcknowledged({
      admin,
      sidechannel,
      requestId,
      message,
      pending: this.servicePending,
      unavailableMessage:
        'Mayhem service relay could not establish its direct channel to the canonical admin.',
      unsentMessage: 'Mayhem service relay could not send the signed request.',
      timeoutMessage: 'Mayhem service relay timed out before the admin replied.',
    });
  }

  async _relayUntilAcknowledged({
    admin,
    sidechannel,
    requestId,
    message,
    pending,
    unavailableMessage,
    unsentMessage,
    timeoutMessage,
  }) {
    let resolvePending;
    const promise = new Promise((resolve) => {
      resolvePending = resolve;
    });
    pending.set(requestId, { promise, resolve: resolvePending });

    let connected = false;
    let sent = false;
    let attemptInFlight = false;
    const attempt = async () => {
      if (attemptInFlight || !pending.has(requestId)) return;
      attemptInFlight = true;
      try {
        if (!(await this._connectAdminTransport(admin, sidechannel))) return;
        connected = true;
        if (sidechannel.broadcast(this.channel, message)) sent = true;
      } catch (_error) {
        // A later retry makes a fresh direct-admin connection attempt.
      } finally {
        attemptInFlight = false;
      }
    };

    const retry = setInterval(() => void attempt(), this.retryMs);
    const timeout = this.timeoutMs > 0
      ? setTimeout(() => {
          const current = pending.get(requestId);
          if (!current) return;
          pending.delete(requestId);
          const messageText = !connected
            ? unavailableMessage
            : !sent
              ? unsentMessage
              : timeoutMessage;
          current.resolve(relayError(messageText, requestId));
        }, this.timeoutMs)
      : null;
    void attempt();

    try {
      return await promise;
    } finally {
      clearInterval(retry);
      if (timeout !== null) clearTimeout(timeout);
    }
  }

  async handleSidechannelMessage(channel, payload) {
    if (channel !== this.channel || !this.isRelayMessage(payload)) return false;
    if (this.relayMessageBytes(payload.message) > this.maxMessageBytes) return true;
    if (payload.message.control === RELAY_CONTROL_REQUEST) {
      await this._handleRequest(payload);
    } else if (payload.message.control === RELAY_CONTROL_RESULT) {
      await this._handleResult(payload);
    } else if (payload.message.control === SERVICE_CONTROL_REQUEST) {
      await this._handleServiceRequest(payload);
    } else {
      await this._handleServiceResult(payload);
    }
    return true;
  }

  async _adminKey() {
    const admin = await this.peer?.base?.view?.get('admin');
    return normalizeKey(admin?.value);
  }

  async _connectAdminTransport(admin, sidechannel) {
    const target = normalizeKey(admin);
    if (!/^[0-9a-f]{64}$/.test(target) ||
        typeof sidechannel?.connectDirectPeer !== 'function') return false;
    return await sidechannel.connectDirectPeer(target, this.channel, this.connectTimeoutMs);
  }

  _verifyEnvelope(payload, expectedKey) {
    const sidechannel = this.peer?.sidechannel;
    return sidechannel?.verifyPayload?.(payload, expectedKey) === true;
  }

  _pruneProcessed() {
    this._pruneMap(this.processed);
  }

  _pruneMap(map) {
    const cutoff = Date.now() - this.cacheTtlMs;
    for (const [key, entry] of map) {
      if (entry.pending === true) continue;
      if (entry.at >= cutoff && map.size <= this.cacheMax) break;
      map.delete(key);
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
      cached = { at: Date.now(), pending: true, promise };
      this.processed.set(expectedId, cached);
    }
    const response = await cached.promise;
    cached.pending = false;
    cached.at = Date.now();
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

  async _handleServiceRequest(payload) {
    if (!this.peer.base?.writable || !this.serviceHandler) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    if (!admin || self !== admin) return;

    const message = payload.message;
    const actor = serviceParticipantFor(message.service, message.value);
    const transport = normalizeKey(payload.from);
    if (!actor || actor !== transport || !this._verifyEnvelope(payload, transport)) return;
    if (message.version !== RELAY_VERSION) return;
    const expectedId = serviceRequestIdFor(message.service, message.value);
    if (message.request_id !== expectedId) return;

    this._pruneMap(this.serviceProcessed);
    let cached = this.serviceProcessed.get(expectedId);
    if (!cached) {
      const promise = Promise.resolve()
        .then(() => this.serviceHandler(message.service, stableValue(message.value)))
        .catch((error) => relayError(error?.message || 'Mayhem admin service request failed.', expectedId));
      cached = { at: Date.now(), pending: true, promise };
      this.serviceProcessed.set(expectedId, cached);
    }
    const response = await cached.promise;
    cached.pending = false;
    cached.at = Date.now();
    if (response?.ok !== true) this.serviceProcessed.delete(expectedId);
    this.peer.sidechannel.broadcast(this.channel, {
      control: SERVICE_CONTROL_RESULT,
      version: RELAY_VERSION,
      request_id: expectedId,
      to: transport,
      response,
    });
  }

  async _handleServiceResult(payload) {
    const message = payload.message;
    const requestId = String(message.request_id || '');
    const pending = this.servicePending.get(requestId);
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
    this.servicePending.delete(requestId);
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
    await this.peer.base.append(null);
    const featureResult =
      (await this._waitForResult(resultKey, this.resultTimeoutMs, previousResult)) ?? previousResult;
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

  async _waitForResult(key, timeoutMs = 0, previousResult = null) {
    const deadline = timeoutMs > 0 ? Date.now() + timeoutMs : null;
    const previousJson = previousResult === null ? null : stableJson(previousResult);
    while (!this.stopped && (deadline === null || Date.now() <= deadline)) {
      const result = await this.peer.base.view.get(key);
      if (result !== null && (previousJson === null || stableJson(result.value) !== previousJson)) {
        return result.value;
      }
      await this.sleep(this.resultPollMs);
    }
    return null;
  }

  async start() {
    this.stopped = false;
  }

  async stop() {
    this.stopped = true;
    for (const [requestId, pending] of this.pending) {
      pending.resolve(relayError('Mayhem feature relay stopped.', requestId));
    }
    this.pending.clear();
    this.processed.clear();
    for (const [requestId, pending] of this.servicePending) {
      pending.resolve(relayError('Mayhem service relay stopped.', requestId));
    }
    this.servicePending.clear();
    this.serviceProcessed.clear();
  }
}

export {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_MESSAGE_BYTES,
  participantFor,
  requestIdFor,
  serviceRequestIdFor,
};
export default MayhemFeature;
