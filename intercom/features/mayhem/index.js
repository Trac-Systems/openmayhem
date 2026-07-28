import Feature from 'trac-peer/src/artifacts/feature.js';
import crypto from 'crypto';
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { keccak256 } from 'ethereum-cryptography/keccak';
import { secp256k1 } from 'ethereum-cryptography/secp256k1';
import PeerWallet from 'trac-wallet';
import {
  CONTRACT_VERSION,
  PAYOUT_INTENT_MAX_EXPIRY_EPOCHS_DEFAULT,
  adminContractTxDigest,
  providerPayoutBindingMessage,
  providerPayoutTargetBindingMessage,
} from '../../contract/contract.js';

const RELAY_CONTROL_REQUEST = 'mayhem_feature_request';
const RELAY_CONTROL_RESULT = 'mayhem_feature_result';
const RELAY_CONTROL_RESULT_ACK = 'mayhem_feature_result_ack';
const SERVICE_CONTROL_REQUEST = 'mayhem_service_request';
const SERVICE_CONTROL_RESULT = 'mayhem_service_result';
const SERVICE_CONTROL_RESULT_ACK = 'mayhem_service_result_ack';
const RELAY_VERSION = 1;
const SERVICE_SIGNING_VERSION = 1;
const SERVICE_SIGNING_DOMAIN = 'mayhem-service-request';
const FEATURE_RESULT_DIGEST_DOMAIN = 'mayhem-feature-result-v1';
const SERVICE_RESULT_DIGEST_DOMAIN = 'mayhem-service-result-v1';
const MAYHEM_RELAY_CHANNEL = '0000mayhem-relay';
const MAYHEM_RELAY_MAX_MESSAGE_BYTES = 16_384;
const DEFAULT_TIMEOUT_MS = 0;
const DEFAULT_RETRY_MS = 1_000;
const DEFAULT_CONNECT_TIMEOUT_MS = 15_000;
const DEFAULT_RESULT_TIMEOUT_MS = 120_000;
const DEFAULT_RESULT_POLL_MS = 50;
const DEFAULT_CACHE_TTL_MS = 300_000;
const DEFAULT_CACHE_MAX = 2_048;
const DEFAULT_PENDING_MAX = 256;
const DEFAULT_PROCESSED_IN_FLIGHT_MAX = 256;
const DEFAULT_RESULT_RETRY_MAX = 6;
const PROVIDER_PAYOUT_RAILS = new Set(['fiat', 'tap', 'tnk']);
const STRIPE_CONNECT_RELINK_CONSENT_VERSION = 1;
const STRIPE_CONNECT_RELINK_CONSENT_MAX_SECONDS = 600;
const STRIPE_CONNECT_ADOPT_CONSENT_MAX_SECONDS = 600;
const normalizeKey = (value) => String(value ?? '').trim().toLowerCase();

const verifyEd25519Hex = (wallet, signature, message, publicKey) => {
  if (!/^[0-9a-f]{128}$/.test(signature) ||
      !/^[0-9a-f]{64}$/.test(publicKey) ||
      !b4a.isBuffer(message) ||
      message.length === 0) {
    return false;
  }
  const signatureBytes = b4a.from(signature, 'hex');
  const publicKeyBytes = b4a.from(publicKey, 'hex');
  try {
    if (typeof PeerWallet?.verify === 'function' &&
        PeerWallet.verify(signatureBytes, message, publicKeyBytes) === true) {
      return true;
    }
  } catch (_error) {
    // Fall through to the peer wallet for older dependency layouts.
  }
  try {
    return typeof wallet?.verify === 'function' &&
      wallet.verify(signatureBytes, message, publicKeyBytes) === true;
  } catch (_error) {
    return false;
  }
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

const exactKeys = (value, keys) => (
  value &&
  typeof value === 'object' &&
  !Array.isArray(value) &&
  Object.keys(value).length === keys.length &&
  keys.every((key) => Object.hasOwn(value, key))
);

const ethereumPersonalMessageHash = (message) => {
  const body = b4a.from(message, 'utf8');
  const prefix = b4a.from(`\x19Ethereum Signed Message:\n${body.length}`, 'utf8');
  return keccak256(b4a.concat([prefix, body]));
};

const ethereumAddressFromPublicKey = (publicKey) =>
  `0x${b4a.toString(keccak256(publicKey.subarray(1)).subarray(12), 'hex')}`;

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

const featureResultDigestFor = (requestId, response) =>
  crypto
    .createHash('sha256')
    .update(
      stableJson({
        domain: FEATURE_RESULT_DIGEST_DOMAIN,
        request_id: String(requestId ?? ''),
        response: stableValue(response),
      })
    )
    .digest('hex');

const serviceResultDigestFor = (requestId, response) =>
  crypto
    .createHash('sha256')
    .update(
      stableJson({
        domain: SERVICE_RESULT_DIGEST_DOMAIN,
        request_id: String(requestId ?? ''),
        response: stableValue(response),
      })
    )
    .digest('hex');

const serviceSigningMessage = (service, value) => stableJson({
  actor: normalizeKey(value?.actor),
  admin: normalizeKey(value?.admin),
  domain: SERVICE_SIGNING_DOMAIN,
  payload: stableValue(value?.payload),
  service: String(service ?? ''),
  signing_version: SERVICE_SIGNING_VERSION,
  transport: normalizeKey(value?.transport),
});

const stripeConnectRelinkConsentMessage = (value) => stableJson({
  account_id: String(value?.account_id ?? ''),
  consent_expires_at: value?.consent_expires_at,
  context_revision: normalizeKey(value?.context_revision),
  country: String(value?.country ?? ''),
  domain: 'mayhem-stripe-connect-relink-consent-v1',
  request_nonce: normalizeKey(value?.request_nonce),
  signing_version: STRIPE_CONNECT_RELINK_CONSENT_VERSION,
  source_provider: normalizeKey(value?.source_provider),
  target_provider: normalizeKey(value?.provider),
});

const validStripeConnectRelinkConsent = (wallet, value) => {
  if (!exactKeys(value, [
    'provider',
    'source_provider',
    'account_id',
    'context_revision',
    'country',
    'request_nonce',
    'consent_expires_at',
    'source_consent_signature',
  ])) return false;
  const provider = String(value.provider ?? '');
  const sourceProvider = String(value.source_provider ?? '');
  const contextRevision = String(value.context_revision ?? '');
  const requestNonce = String(value.request_nonce ?? '');
  const signature = String(value.source_consent_signature ?? '');
  if (!/^[0-9a-f]{64}$/.test(provider) ||
      !/^[0-9a-f]{64}$/.test(sourceProvider) ||
      provider === sourceProvider ||
      !/^acct_[A-Za-z0-9._-]+$/.test(String(value.account_id ?? '')) ||
      !/^[0-9a-f]{64}$/.test(contextRevision) ||
      !/^[A-Z]{2}$/.test(String(value.country ?? '')) ||
      !/^[0-9a-f]{64}$/.test(requestNonce) ||
      !/^[0-9a-f]{128}$/.test(signature) ||
      !Number.isSafeInteger(value.consent_expires_at)) return false;
  const now = Math.floor(Date.now() / 1_000);
  if (value.consent_expires_at <= now ||
      value.consent_expires_at > now + STRIPE_CONNECT_RELINK_CONSENT_MAX_SECONDS) {
    return false;
  }
  return verifyEd25519Hex(
    wallet,
    signature,
    b4a.from(stripeConnectRelinkConsentMessage(value)),
    sourceProvider
  );
};

const validStripeConnectAdoptPayload = (value) => {
  if (!exactKeys(value, [
    'provider',
    'country',
    'request_nonce',
    'consent_expires_at',
  ])) return false;
  if (!/^[0-9a-f]{64}$/.test(String(value.provider ?? '')) ||
      !/^[A-Z]{2}$/.test(String(value.country ?? '')) ||
      !/^[0-9a-f]{64}$/.test(String(value.request_nonce ?? '')) ||
      !Number.isSafeInteger(value.consent_expires_at)) return false;
  const now = Math.floor(Date.now() / 1_000);
  return value.consent_expires_at > now &&
    value.consent_expires_at <= now + STRIPE_CONNECT_ADOPT_CONSENT_MAX_SECONDS;
};

const validStripeCheckoutPayload = (value) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const requiredKeys = [
    'who',
    'au',
    'request_nonce',
    'success_url',
    'cancel_url',
  ];
  const allowedKeys = new Set([...requiredKeys, 'idempotency_key']);
  if (requiredKeys.some((key) => !Object.hasOwn(value, key)) ||
      Object.keys(value).some((key) => !allowedKeys.has(key)) ||
      !/^[0-9a-f]{64}$/.test(String(value.who ?? '')) ||
      !/^[1-9][0-9]*$/.test(String(value.au ?? '')) ||
      !/^[0-9a-f]{64}$/.test(String(value.request_nonce ?? '')) ||
      (value.idempotency_key !== undefined &&
        (typeof value.idempotency_key !== 'string' || value.idempotency_key.length > 255))) {
    return false;
  }
  try {
    for (const field of ['success_url', 'cancel_url']) {
      const parsed = new URL(String(value[field] ?? ''));
      if (parsed.protocol !== 'https:' || parsed.username || parsed.password) return false;
    }
  } catch (_error) {
    return false;
  }
  return true;
};

const participantFor = (value) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  if (value.op === 'admin_contract_tx') return normalizeKey(value.address);
  if (value.op === 'consent' || value.op === 'deposit_tnk') return normalizeKey(value.sender);
  if (value.op === 'tap_account_bind') return normalizeKey(value.user);
  if (value.op === 'provider_lifecycle') {
    return normalizeKey(value.intent?.provider);
  }
  if (value.op === 'bind_provider_payout' &&
      PROVIDER_PAYOUT_RAILS.has(value.intent?.rail)) {
    return normalizeKey(value.intent?.provider);
  }
  if (value.op === 'spend_reserve_targeted') {
    return normalizeKey(value.provider);
  }
  if (value.op === 'record_usage_receipt') {
    const provider = normalizeKey(value.receipt?.body?.provider);
    return /^[0-9a-f]{64}$/.test(provider) ? provider : null;
  }
  if (value.op === 'close_usage_reservation') {
    const provider = normalizeKey(value.provider);
    const actor = normalizeKey(value.actor);
    if (value.actor_role === 'provider' && actor === provider &&
        /^[0-9a-f]{64}$/.test(provider)) return provider;
    return null;
  }
  if (value.op === 'expire_usage_reservation') {
    const user = normalizeKey(value.user);
    return value.actor_role === 'user' &&
      normalizeKey(value.actor) === user &&
      /^[0-9a-f]{64}$/.test(user)
      ? user
      : null;
  }
  if ([
    'prepare_targeted_payout',
    'prepare_targeted_payout_epoch',
    'prepare_targeted_fiat_attempt',
    'finalize_targeted_fiat_attempt',
    'settle_targeted_tnk_output',
    'settle_targeted_fiat_output',
    'close_targeted_payout_epoch',
  ].includes(value.op)) {
    const admin = normalizeKey(value.admin);
    return /^[0-9a-f]{64}$/.test(admin) ? admin : null;
  }
  return null;
};

const serviceParticipantFor = (service, value) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return null;
  }
  if (service === 'stripe_checkout') return normalizeKey(value.who);
  if ([
    'provider_payout_context',
    'stripe_connect_adopt',
    'stripe_connect_onboard',
    'stripe_connect_status',
    'stripe_connect_relink',
  ].includes(service)) {
    return normalizeKey(value.provider);
  }
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
    this.timeoutMs = Number.isSafeInteger(config.timeoutMs) && config.timeoutMs >= 0
      ? config.timeoutMs
      : DEFAULT_TIMEOUT_MS;
    this.retryMs = Number.isSafeInteger(config.retryMs) && config.retryMs > 0
      ? config.retryMs
      : DEFAULT_RETRY_MS;
    this.resultRetryMs = Number.isSafeInteger(config.resultRetryMs) && config.resultRetryMs > 0
      ? config.resultRetryMs
      : this.retryMs;
    this.resultRetryMax = Number.isSafeInteger(config.resultRetryMax) && config.resultRetryMax > 0
      ? config.resultRetryMax
      : DEFAULT_RESULT_RETRY_MAX;
    this.connectTimeoutMs = Number.isSafeInteger(config.connectTimeoutMs) &&
      config.connectTimeoutMs > 0
      ? config.connectTimeoutMs
      : this.timeoutMs > 0
        ? Math.min(this.timeoutMs, DEFAULT_CONNECT_TIMEOUT_MS)
        : DEFAULT_CONNECT_TIMEOUT_MS;
    this.resultTimeoutMs = Number.isSafeInteger(config.resultTimeoutMs) &&
      config.resultTimeoutMs > 0
      ? config.resultTimeoutMs
      : DEFAULT_RESULT_TIMEOUT_MS;
    this.resultPollMs = Number.isSafeInteger(config.resultPollMs) && config.resultPollMs > 0
      ? config.resultPollMs
      : DEFAULT_RESULT_POLL_MS;
    this.cacheTtlMs = Number.isSafeInteger(config.cacheTtlMs)
      ? config.cacheTtlMs
      : DEFAULT_CACHE_TTL_MS;
    this.cacheMax = Number.isSafeInteger(config.cacheMax) ? config.cacheMax : DEFAULT_CACHE_MAX;
    this.pendingMax = Number.isSafeInteger(config.pendingMax) && config.pendingMax > 0
      ? config.pendingMax
      : DEFAULT_PENDING_MAX;
    this.processedInFlightMax = Number.isSafeInteger(config.processedInFlightMax) &&
      config.processedInFlightMax > 0
      ? config.processedInFlightMax
      : DEFAULT_PROCESSED_IN_FLIGHT_MAX;
    this.serviceHandler = typeof config.serviceHandler === 'function' ? config.serviceHandler : null;
    this.adminTxHandler = typeof config.adminTxHandler === 'function'
      ? config.adminTxHandler
      : async (value) => {
          const result = await this.peer.protocol.instance.simulateTransaction(
            '0'.repeat(64),
            value.prepared_command,
            { address: value.address }
          );
          return { result };
        };
    this.pending = new Map();
    this.processed = new Map();
    this.completed = new Map();
    this.servicePending = new Map();
    this.serviceProcessed = new Map();
    this.serviceCompleted = new Map();
    this.stopped = false;
  }

  async record(key, value) {
    return await this.submit(key, value);
  }

  async submit(key, value, { nonce = null } = {}) {
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    if (!this.peer.base?.writable || !admin || self !== admin) {
      throw new Error('Peer subnet is not the canonical writable admin.');
    }
    if (value?.op === 'admin_contract_tx') {
      const validationError = await this._validateAdminTx(key, value);
      if (validationError) {
        return {
          ok: false,
          accepted: false,
          status: 'rejected',
          message: validationError,
        };
      }
      if (value.sim) return await this._simulateAdminTx(value);
    }

    const featureNonce = nonce ??
      this.peer.protocol.instance.generateNonce?.() ??
      crypto.randomBytes(32).toString('hex');
    const hash = this.peer.wallet.sign(`${JSON.stringify(value)}${featureNonce}`);
    const resultKey = `fr/${hash}`;
    const previousResult = (await this.peer.base.view.get(resultKey))?.value ?? null;
    if (previousResult?.ok === true || previousResult?.ok === false) {
      return this._featureResponse(key, hash, resultKey, previousResult);
    }
    const operation = {
      type: 'feature',
      key: `${this.key}_${key}`,
      value: {
        dispatch: {
          type: `${this.key}_feature`,
          contract_version: CONTRACT_VERSION,
          key,
          hash,
          value,
          nonce: featureNonce,
          address: this.peer.wallet.publicKey,
        },
      },
    };
    const featureMaxBytes = this.peer.protocol.instance.featMaxBytes?.();
    const operationBytes = b4a.byteLength(JSON.stringify(operation));
    if (Number.isSafeInteger(featureMaxBytes) && operationBytes > featureMaxBytes) {
      return {
        ok: false,
        accepted: false,
        status: 'rejected',
        message:
          `Feature operation is ${operationBytes} bytes, exceeding the protocol limit ` +
          `${featureMaxBytes}; increase MAYHEM_FEATURE_MAX_BYTES consistently on the canonical network.`,
      };
    }
    await this.peer.base.append(operation);
    await this.peer.base.append(null);
    const featureResult = await this._waitForResult(
      resultKey,
      this.resultTimeoutMs,
      previousResult
    );
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

  isRelayMessage(payload) {
    const control = payload?.message?.control;
    return control === RELAY_CONTROL_REQUEST ||
      control === RELAY_CONTROL_RESULT ||
      control === RELAY_CONTROL_RESULT_ACK ||
      control === SERVICE_CONTROL_REQUEST ||
      control === SERVICE_CONTROL_RESULT ||
      control === SERVICE_CONTROL_RESULT_ACK;
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
    if (
      value?.op === 'admin_contract_tx' &&
      actor !== admin
    ) {
      throw new Error('Admin-signed contract relay requires the current admin authority.');
    }
    if (this.peer.base?.writable && self === admin) {
      throw new Error('Invalid relay request from the admin writer.');
    }
    const sidechannel = this.peer?.sidechannel;
    if (!sidechannel?.started || typeof sidechannel.broadcast !== 'function') {
      throw new Error('Mayhem feature relay is not ready.');
    }

    const relayedValue = value;
    const requestId = requestIdFor(feature, key, relayedValue);
    this._pruneMap(this.completed);
    const completed = this.completed.get(requestId);
    if (completed) {
      completed.at = Date.now();
      return completed.response;
    }
    const existing = this.pending.get(requestId);
    if (existing) return await existing.promise;
    if (this.pending.size >= this.pendingMax) {
      throw new Error('Mayhem feature relay pending-request limit reached.');
    }

    const message = {
      control: RELAY_CONTROL_REQUEST,
      version: RELAY_VERSION,
      request_id: requestId,
      feature,
      key,
      value: relayedValue,
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
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    const admin = await this._adminKey();
    const envelope = this._verifyServiceRequest(service, value, { admin, transport: self });
    const actor = envelope?.actor;
    if (!actor || !self) {
      throw new Error('Invalid Mayhem service request signature.');
    }
    if (this.peer.base?.writable && self === admin) {
      return await this._handleService(service, envelope.payload, envelope);
    }
    const sidechannel = this.peer?.sidechannel;
    if (!sidechannel?.started || typeof sidechannel.broadcast !== 'function') {
      throw new Error('Mayhem service relay is not ready.');
    }

    const canonicalValue = envelope;
    const requestId = serviceRequestIdFor(service, canonicalValue);
    this._pruneMap(this.serviceCompleted);
    const completed = this.serviceCompleted.get(requestId);
    if (completed) {
      completed.at = Date.now();
      return completed.response;
    }
    const existing = this.servicePending.get(requestId);
    if (existing) return await existing.promise;
    if (this.servicePending.size >= this.pendingMax) {
      throw new Error('Mayhem service relay pending-request limit reached.');
    }

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

    // Each resend re-mines the sidechannel PoW, so retransmits back off instead of
    // firing at a fixed cadence; a lost frame still recovers within seconds while a
    // slow admin acknowledgment no longer keeps the peer mining continuously.
    let retryDelay = this.retryMs;
    let retryTimer = null;
    const scheduleRetry = () => {
      retryTimer = setTimeout(() => {
        void attempt().finally(() => {
          if (!pending.has(requestId)) return;
          retryDelay = Math.min(retryDelay * 2, this.retryMs * 8);
          scheduleRetry();
        });
      }, retryDelay);
      if (typeof retryTimer?.unref === 'function') retryTimer.unref();
    };
    scheduleRetry();
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
      if (retryTimer !== null) clearTimeout(retryTimer);
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
    } else if (payload.message.control === RELAY_CONTROL_RESULT_ACK) {
      await this._handleResultAck(payload);
    } else if (payload.message.control === SERVICE_CONTROL_REQUEST) {
      await this._handleServiceRequest(payload);
    } else if (payload.message.control === SERVICE_CONTROL_RESULT) {
      await this._handleServiceResult(payload);
    } else {
      await this._handleServiceResultAck(payload);
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

  _verifyServiceRequest(service, value, expected = {}) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
    const allowedKeys = new Set([
      'actor',
      'admin',
      'payload',
      'signature',
      'signing_version',
      'transport',
    ]);
    if (Object.keys(value).some((key) => !allowedKeys.has(key))) return null;
    if (value.signing_version !== SERVICE_SIGNING_VERSION) return null;
    const actor = normalizeKey(value.actor);
    const admin = normalizeKey(value.admin);
    const transport = normalizeKey(value.transport);
    const signature = String(value.signature ?? '').toLowerCase();
    const payload = stableValue(value.payload);
    if (!/^[0-9a-f]{64}$/.test(actor) ||
        !/^[0-9a-f]{64}$/.test(admin) ||
        !/^[0-9a-f]{64}$/.test(transport) ||
        !/^[0-9a-f]{128}$/.test(signature)) return null;
    if (admin !== normalizeKey(expected.admin) || transport !== normalizeKey(expected.transport)) {
      return null;
    }
    if (serviceParticipantFor(service, payload) !== actor) return null;
    if (typeof this.peer?.wallet?.verify !== 'function') return null;
    if (service === 'stripe_connect_relink' &&
        !validStripeConnectRelinkConsent(this.peer.wallet, payload)) return null;
    if (service === 'stripe_checkout' && !validStripeCheckoutPayload(payload)) return null;
    if (service === 'stripe_connect_adopt' &&
        !validStripeConnectAdoptPayload(payload)) return null;
    if (!verifyEd25519Hex(
      this.peer.wallet,
      signature,
      b4a.from(serviceSigningMessage(service, { actor, admin, payload, transport })),
      actor
    )) return null;
    return {
      actor,
      admin,
      payload,
      signature,
      signing_version: SERVICE_SIGNING_VERSION,
      transport,
    };
  }

  async _providerPayoutContext(value) {
    if (!exactKeys(value, ['provider', 'rail', 'request_nonce'])) {
      throw new Error('Invalid provider payout context request.');
    }
    const provider = normalizeKey(value.provider);
    const rail = String(value.rail ?? '').toLowerCase();
    const requestNonce = normalizeKey(value.request_nonce);
    if (!/^[0-9a-f]{64}$/.test(provider) ||
        !PROVIDER_PAYOUT_RAILS.has(rail) ||
        !/^[0-9a-f]{64}$/.test(requestNonce)) {
      throw new Error('Invalid provider payout context request.');
    }

    const admin = await this._adminKey();
    const providerRecord = (await this.peer.base.view.get(`prov/${provider}`))?.value;
    if (!providerRecord ||
        providerRecord.status !== 'active' ||
        !Array.isArray(providerRecord.accepted_rails) ||
        !providerRecord.accepted_rails.includes(rail)) {
      throw new Error(`Provider must be active and accept the ${rail} rail.`);
    }
    const payments = (await this.peer.base.view.get('payments/current'))?.value;
    const contextPointer = (await this.peer.base.view.get('payout/context/current'))?.value;
    const contextRevision = normalizeKey(contextPointer?.revision);
    const paymentConfigVersion = contextPointer?.payment_config_version;
    const context = Number.isSafeInteger(paymentConfigVersion) &&
      /^[0-9a-f]{64}$/.test(contextRevision)
      ? (
          await this.peer.base.view.get(
            `payout/context/${paymentConfigVersion}/${contextRevision}`
          )
        )?.value
      : null;
    if (!admin ||
        !payments ||
        payments.set_by !== admin ||
        payments.set_by_role !== 'admin' ||
        payments.ver !== paymentConfigVersion ||
        !Array.isArray(payments.rails) ||
        !payments.rails.includes(rail) ||
        !context ||
        context.revision !== contextRevision ||
        context.payment_config_version !== paymentConfigVersion ||
        context.admin !== admin ||
        context.network !== payments.tnk?.network ||
        context.published_by !== admin ||
        context.published_by_role !== 'admin' ||
        !/^[0-9a-f]{64}$/.test(String(context.bootstrap ?? ''))) {
      throw new Error('Canonical provider payout context is unavailable.');
    }

    const applyState = (await this.peer.base.view.get('epoch/apply/state'))?.value ?? {
      updated_epoch: 0,
      pending_epoch: null,
    };
    const updatedEpoch = applyState.updated_epoch;
    const pendingEpoch = applyState.pending_epoch ?? null;
    const expirySchedule = (
      await this.peer.base.view.get('payout/params/payout_intent_max_expiry_epochs')
    )?.value;
    const activeExpiryEntry = expirySchedule?.pending &&
      expirySchedule.pending.effective_epoch <= updatedEpoch
      ? expirySchedule.pending
      : expirySchedule?.current;
    const maxExpiryEpochs = activeExpiryEntry?.value ??
      PAYOUT_INTENT_MAX_EXPIRY_EPOCHS_DEFAULT;
    if (!Number.isSafeInteger(updatedEpoch) ||
        updatedEpoch < 0 ||
        (pendingEpoch !== null &&
          (!Number.isSafeInteger(pendingEpoch) || pendingEpoch < updatedEpoch)) ||
        !Number.isSafeInteger(maxExpiryEpochs) ||
        maxExpiryEpochs < 1) {
      throw new Error('Canonical provider payout epoch state is invalid.');
    }

    const payoutPointer = (
      await this.peer.base.view.get(`payout/current/${rail}/${provider}`)
    )?.value;
    const previousRevision = payoutPointer?.latest_revision ?? null;
    if (previousRevision !== null &&
        !/^[0-9a-f]{64}$/.test(String(previousRevision))) {
      throw new Error('Canonical provider payout revision is invalid.');
    }
    const binding = previousRevision === null
      ? null
      : (
          await this.peer.base.view.get(
            `payout/binding/${rail}/${provider}/${previousRevision}`
          )
        )?.value ?? null;
    if (previousRevision !== null &&
        (!binding ||
          binding.provider !== provider ||
          binding.rail !== rail ||
          binding.revision !== previousRevision)) {
      throw new Error('Canonical provider payout binding is unavailable.');
    }
    const chainId = rail === 'tap' ? payments.tap?.chain_id : null;
    if (rail === 'tap' && (!Number.isSafeInteger(chainId) || chainId < 1)) {
      throw new Error('Canonical TAP payout chain is invalid.');
    }

    return {
      ok: true,
      provider,
      rail,
      request_nonce: requestNonce,
      network: context.network,
      admin,
      bootstrap: context.bootstrap,
      context_revision: contextRevision,
      payment_config_version: paymentConfigVersion,
      chain_id: chainId,
      updated_epoch: updatedEpoch,
      pending_epoch: pendingEpoch,
      max_expiry_epochs: maxExpiryEpochs,
      previous_revision: previousRevision,
      current_revision: payoutPointer?.current_revision ?? null,
      pending_revision: payoutPointer?.pending_revision ?? null,
      binding,
    };
  }

  async _handleService(service, value, authorization) {
    if (service === 'provider_payout_context') {
      return await this._providerPayoutContext(value);
    }
    if (!this.serviceHandler) {
      throw new Error('Mayhem admin service is not configured.');
    }
    return await this.serviceHandler(service, value, authorization);
  }

  _pruneProcessed() {
    this._pruneMap(this.processed);
  }

  _pruneMap(map) {
    const cutoff = Date.now() - this.cacheTtlMs;
    for (const [key, entry] of map) {
      if (entry.pending === true) continue;
      if (entry.at >= cutoff && map.size <= this.cacheMax) break;
      this._clearServiceResultRetry(entry);
      map.delete(key);
    }
  }

  _clearServiceResultRetry(entry) {
    if (entry?.resultTimer !== null && entry?.resultTimer !== undefined) {
      clearTimeout(entry.resultTimer);
    }
    if (entry && typeof entry === 'object') {
      entry.resultTimer = null;
      entry.deliveryStarted = false;
      entry.deliveryToken = (entry.deliveryToken ?? 0) + 1;
    }
  }

  _processedInFlight(map) {
    let count = 0;
    for (const entry of map.values()) {
      if (entry.pending === true) count += 1;
    }
    return count;
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
    if (
      message.value?.op === 'admin_contract_tx' &&
      actor !== admin
    ) {
      return;
    }
    if (message.version !== RELAY_VERSION || message.feature !== (this.key || 'mayhem')) return;
    if (typeof message.key !== 'string' || message.key.length < 1 || message.key.length > 256) return;
    const expectedId = requestIdFor(message.feature, message.key, message.value);
    if (message.request_id !== expectedId) return;
    if (
      message.value?.op === 'bind_provider_payout' &&
      !(await this._validateProviderPayoutBinding(message.key, message.value, admin))
    ) {
      const rejected = this._prepareFeatureResult(
        expectedId,
        transport,
        relayError('Provider payout binding failed relay verification.', expectedId)
      );
      this.peer.sidechannel.broadcast(this.channel, rejected.message);
      return;
    }

    this._pruneProcessed();
    let cached = this.processed.get(expectedId);
    if (!cached && this._processedInFlight(this.processed) >= this.processedInFlightMax) {
      const busy = this._prepareFeatureResult(
        expectedId,
        transport,
        relayError('Admin feature relay is busy; retry this signed request.', expectedId)
      );
      this.peer.sidechannel.broadcast(this.channel, {
        ...busy.message,
      });
      return;
    }
    let created = false;
    if (!cached) {
      const relayedValue = message.value;
      const promise = this._applyRelayed(message.key, relayedValue, expectedId).catch((error) =>
        relayError(error?.message || 'Admin writer failed to apply relayed feature.', expectedId)
      ).then((response) => this._prepareFeatureResult(expectedId, transport, response));
      cached = {
        at: Date.now(),
        pending: true,
        promise,
        transport,
        result: null,
        resultTimer: null,
      };
      this.processed.set(expectedId, cached);
      created = true;
    }
    if (!created) {
      cached.at = Date.now();
      if (cached.pending) return;
      this._clearServiceResultRetry(cached);
      this._startFeatureResultDelivery(expectedId, cached);
      return;
    }
    const result = await cached.promise;
    cached.result = result;
    cached.pending = false;
    cached.at = Date.now();
    this._startFeatureResultDelivery(expectedId, cached);
  }

  async _validateProviderPayoutBinding(key, value, admin) {
    if (!exactKeys(value, ['op', 'intent', 'provider_signature']) ||
        value.op !== 'bind_provider_payout') return false;
    const intent = value.intent;
    const intentKeys = [
      'op',
      'network',
      'admin',
      'bootstrap',
      'context_revision',
      'provider',
      'rail',
      'currency',
      'chain_id',
      'target',
      'target_wallet',
      'target_signature',
      'previous_revision',
      'payment_config_version',
      'nonce',
      'expires_after_epoch',
    ];
    if (!exactKeys(intent, intentKeys) ||
        intent.op !== 'bind_provider_payout' ||
        !PROVIDER_PAYOUT_RAILS.has(intent.rail) ||
        intent.admin !== admin ||
        !/^[0-9a-f]{64}$/.test(intent.bootstrap) ||
        !/^[0-9a-f]{64}$/.test(intent.context_revision) ||
        !/^[0-9a-f]{64}$/.test(intent.provider) ||
        !/^[0-9a-f]{64}$/.test(intent.nonce) ||
        !/^[0-9a-f]{128}$/.test(value.provider_signature) ||
        (intent.previous_revision !== null &&
          !/^[0-9a-f]{64}$/.test(intent.previous_revision)) ||
        !Number.isSafeInteger(intent.payment_config_version) ||
        intent.payment_config_version < 1 ||
        !Number.isSafeInteger(intent.expires_after_epoch) ||
        intent.expires_after_epoch < 1) {
      return false;
    }
    if (!verifyEd25519Hex(
      this.peer.wallet,
      value.provider_signature,
      b4a.from(providerPayoutBindingMessage(intent)),
      intent.provider
    )) {
      return false;
    }
    if (intent.rail === 'fiat') {
      if (!/^acct_[A-Za-z0-9._-]+$/.test(intent.target) ||
          !['usd', 'eur'].includes(intent.currency) ||
          intent.chain_id !== null ||
          intent.target_wallet !== null ||
          intent.target_signature !== null) {
        return false;
      }
    } else if (intent.rail === 'tnk') {
      if (intent.currency !== null ||
          intent.chain_id !== null ||
          !/^[0-9a-f]{64}$/.test(intent.target_wallet) ||
          !/^[0-9a-f]{128}$/.test(intent.target_signature) ||
          !verifyEd25519Hex(
            this.peer.wallet,
            intent.target_signature,
            b4a.from(providerPayoutTargetBindingMessage(intent)),
            intent.target_wallet
          )) {
        return false;
      }
      const prefix = intent.network === 'mainnet'
        ? 'trac'
        : intent.network === 'testnet1'
          ? 'testtrac'
          : null;
      if (!prefix) return false;
      const target = PeerWallet.encodeBech32mSafe(
        prefix,
        b4a.from(intent.target_wallet, 'hex')
      );
      if (target !== intent.target) return false;
    } else {
      if (intent.currency !== null ||
          !Number.isSafeInteger(intent.chain_id) ||
          intent.chain_id < 1 ||
          intent.target_wallet !== null ||
          !/^0x[0-9a-f]{40}$/.test(intent.target) ||
          !/^0x[0-9a-f]{130}$/.test(intent.target_signature)) {
        return false;
      }
      try {
        const bytes = b4a.from(intent.target_signature.slice(2), 'hex');
        let recovery = bytes[64];
        if (recovery === 27 || recovery === 28) recovery -= 27;
        if (recovery !== 0 && recovery !== 1) return false;
        const signature = secp256k1.Signature
          .fromCompact(bytes.subarray(0, 64))
          .addRecoveryBit(recovery);
        if (signature.hasHighS()) return false;
        const publicKey = signature
          .recoverPublicKey(
            ethereumPersonalMessageHash(providerPayoutTargetBindingMessage(intent))
          )
          .toRawBytes(false);
        if (ethereumAddressFromPublicKey(publicKey) !== intent.target) return false;
      } catch {
        return false;
      }
    }
    const revision = b4a.toString(
      await blake3(b4a.from(providerPayoutBindingMessage(intent))),
      'hex'
    );
    const expectedKey = `payout/binding/${intent.rail}/${intent.provider}/${revision}`;
    if (key !== expectedKey) return false;
    const nonceRecord = (
      await this.peer.base.view.get(`payout/nonce/${intent.provider}/${intent.nonce}`)
    )?.value;
    if (nonceRecord) {
      const binding = (await this.peer.base.view.get(expectedKey))?.value;
      return nonceRecord.revision === revision &&
        binding?.type === 'provider_payout_binding' &&
        binding.revision === revision &&
        binding.provider === intent.provider &&
        binding.rail === intent.rail &&
        binding.nonce === intent.nonce &&
        binding.provider_signature === value.provider_signature &&
        binding.target_signature === intent.target_signature;
    }

    const contextPointer = (
      await this.peer.base.view.get('payout/context/current')
    )?.value;
    const context = (
      await this.peer.base.view.get(
        `payout/context/${intent.payment_config_version}/${intent.context_revision}`
      )
    )?.value;
    if (!contextPointer ||
        contextPointer.revision !== intent.context_revision ||
        contextPointer.payment_config_version !== intent.payment_config_version ||
        !context ||
        context.revision !== intent.context_revision ||
        context.network !== intent.network ||
        context.admin !== intent.admin ||
        context.bootstrap !== intent.bootstrap ||
        context.payment_config_version !== intent.payment_config_version ||
        context.published_by !== admin ||
        context.published_by_role !== 'admin') {
      return false;
    }
    const payments = (await this.peer.base.view.get('payments/current'))?.value;
    if (!payments ||
        payments.set_by !== admin ||
        payments.set_by_role !== 'admin' ||
        payments.ver !== intent.payment_config_version ||
        payments.tnk?.network !== intent.network ||
        !Array.isArray(payments.rails) ||
        !payments.rails.includes(intent.rail)) {
      return false;
    }
    const provider = (await this.peer.base.view.get(`prov/${intent.provider}`))?.value;
    if (!provider ||
        provider.status !== 'active' ||
        !Array.isArray(provider.accepted_rails) ||
        !provider.accepted_rails.includes(intent.rail)) {
      return false;
    }
    const applyState = (await this.peer.base.view.get('epoch/apply/state'))?.value ?? {
      updated_epoch: 0,
      pending_epoch: null,
    };
    const expirySchedule = (
      await this.peer.base.view.get('payout/params/payout_intent_max_expiry_epochs')
    )?.value;
    const activeExpiryEntry = expirySchedule?.pending &&
      expirySchedule.pending.effective_epoch <= applyState.updated_epoch
      ? expirySchedule.pending
      : expirySchedule?.current;
    const maxExpiryEpochs = activeExpiryEntry?.value ??
      PAYOUT_INTENT_MAX_EXPIRY_EPOCHS_DEFAULT;
    if (!Number.isSafeInteger(applyState.updated_epoch) ||
        (applyState.pending_epoch ?? null) !== null ||
        intent.expires_after_epoch <= applyState.updated_epoch ||
        intent.expires_after_epoch - applyState.updated_epoch >
          maxExpiryEpochs) {
      return false;
    }
    const pointer = (
      await this.peer.base.view.get(`payout/current/${intent.rail}/${intent.provider}`)
    )?.value;
    if (intent.previous_revision !== (pointer?.latest_revision ?? null)) return false;
    if (intent.rail === 'fiat') {
      const verifiedPointer = (
        await this.peer.base.view.get(
          `payout/stripe-verified/target/${intent.provider}/${intent.target}`
        )
      )?.value;
      const verification = verifiedPointer?.record_key
        ? (await this.peer.base.view.get(verifiedPointer.record_key))?.value
        : null;
      if (!verifiedPointer ||
          verifiedPointer.provider !== intent.provider ||
          verifiedPointer.target !== intent.target ||
          verifiedPointer.revision !== verification?.revision ||
          verifiedPointer.processor_revision !== verification?.processor_revision ||
          verification?.type !== 'stripe_payout_verification' ||
          verification.provider !== intent.provider ||
          verification.target !== intent.target ||
          verification.currency !== intent.currency ||
          verification.context_revision !== intent.context_revision ||
          verification.payment_config_version !== intent.payment_config_version ||
          verification.details_submitted !== true ||
          verification.payouts_enabled !== true ||
          verification.transfers_enabled !== true ||
          verification.verified_by !== admin ||
          verification.verified_by_role !== 'admin') {
        return false;
      }
    } else if (intent.rail === 'tap' && intent.chain_id !== payments.tap?.chain_id) {
      return false;
    }
    return true;
  }

  async _handleResult(payload) {
    const message = payload.message;
    const requestId = String(message.request_id || '');
    const pending = this.pending.get(requestId);
    this._pruneMap(this.completed);
    const completed = this.completed.get(requestId);
    if (!pending && !completed) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    const resultDigest = normalizeKey(message.result_digest);
    if (
      message.version !== RELAY_VERSION ||
      normalizeKey(message.to) !== self ||
      normalizeKey(payload.from) !== admin ||
      !this._verifyEnvelope(payload, admin) ||
      !/^[0-9a-f]{64}$/.test(resultDigest) ||
      featureResultDigestFor(requestId, message.response) !== resultDigest ||
      (completed && completed.resultDigest !== resultDigest)
    ) {
      return;
    }
    this._sendFeatureResultAck(admin, requestId, resultDigest);
    if (!pending) {
      completed.at = Date.now();
      return;
    }
    this.pending.delete(requestId);
    const response = { ...message.response, relayed: true, request_id: requestId };
    this.completed.set(requestId, {
      at: Date.now(),
      response,
      resultDigest,
    });
    this._pruneMap(this.completed);
    pending.resolve(response);
  }

  _prepareFeatureResult(requestId, transport, response) {
    const build = (boundedResponse) => {
      const resultDigest = featureResultDigestFor(requestId, boundedResponse);
      return {
        resultDigest,
        message: {
          control: RELAY_CONTROL_RESULT,
          version: RELAY_VERSION,
          request_id: requestId,
          result_digest: resultDigest,
          to: transport,
          response: boundedResponse,
        },
      };
    };
    let result = build(stableValue(response));
    if (this.relayMessageBytes(result.message) <= this.maxMessageBytes) return result;
    result = build(relayError(
      `Mayhem feature result exceeds ${this.maxMessageBytes} bytes.`,
      requestId
    ));
    return result;
  }

  _startFeatureResultDelivery(requestId, cached) {
    if (cached.deliveryStarted || !cached.result?.message) return;
    cached.deliveryStarted = true;
    cached.acked = false;
    cached.resultAttempts = 0;
    cached.deliveryToken = (cached.deliveryToken ?? 0) + 1;
    const deliveryToken = cached.deliveryToken;
    let retryDelay = this.resultRetryMs;
    const active = () => (
      !this.stopped &&
      !cached.acked &&
      cached.deliveryStarted &&
      cached.deliveryToken === deliveryToken &&
      this.processed.get(requestId) === cached
    );
    const schedule = () => {
      if (!active() || cached.resultAttempts >= this.resultRetryMax) {
        if (cached.deliveryToken === deliveryToken) cached.deliveryStarted = false;
        return;
      }
      cached.resultTimer = setTimeout(() => {
        cached.resultTimer = null;
        void attempt();
      }, retryDelay);
      if (typeof cached.resultTimer?.unref === 'function') cached.resultTimer.unref();
      retryDelay = Math.min(retryDelay * 2, this.resultRetryMs * 8);
    };
    const attempt = async () => {
      if (!active() || cached.resultAttempts >= this.resultRetryMax) return;
      cached.resultAttempts += 1;
      try {
        const sidechannel = this.peer?.sidechannel;
        if (!sidechannel || typeof sidechannel.broadcast !== 'function') return;
        if (!sidechannel.started) return;
        if (typeof sidechannel.connectDirectPeer === 'function') {
          try {
            void Promise.resolve(sidechannel.connectDirectPeer(
              cached.transport,
              this.channel,
              this.connectTimeoutMs
            )).catch(() => {});
          } catch (_error) {
            // Direct reconnect is best-effort; the joined relay channel still carries the result.
          }
        }
        if (!active()) return;
        sidechannel.broadcast(this.channel, cached.result.message);
      } catch (_error) {
        // A bounded later attempt reconnects the current result transport.
      } finally {
        schedule();
      }
    };
    void attempt();
  }

  _sendFeatureResultAck(admin, requestId, resultDigest) {
    const sidechannel = this.peer?.sidechannel;
    if (!sidechannel?.started || typeof sidechannel.broadcast !== 'function') return false;
    return sidechannel.broadcast(this.channel, {
      control: RELAY_CONTROL_RESULT_ACK,
      version: RELAY_VERSION,
      request_id: requestId,
      result_digest: resultDigest,
      to: admin,
    }) === true;
  }

  async _handleResultAck(payload) {
    if (!this.peer.base?.writable) return;
    const message = payload.message;
    const requestId = String(message.request_id || '');
    const cached = this.processed.get(requestId);
    if (!cached?.result) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    const transport = normalizeKey(payload.from);
    if (
      message.version !== RELAY_VERSION ||
      normalizeKey(message.to) !== self ||
      self !== admin ||
      transport !== cached.transport ||
      normalizeKey(message.result_digest) !== cached.result.resultDigest ||
      !this._verifyEnvelope(payload, cached.transport)
    ) {
      return;
    }
    cached.acked = true;
    cached.at = Date.now();
    this._clearServiceResultRetry(cached);
  }

  _prepareServiceResult(requestId, transport, response) {
    const build = (boundedResponse) => {
      const resultDigest = serviceResultDigestFor(requestId, boundedResponse);
      return {
        resultDigest,
        message: {
          control: SERVICE_CONTROL_RESULT,
          version: RELAY_VERSION,
          request_id: requestId,
          result_digest: resultDigest,
          to: transport,
          response: boundedResponse,
        },
      };
    };
    let result = build(stableValue(response));
    if (this.relayMessageBytes(result.message) <= this.maxMessageBytes) return result;
    result = build(relayError(
      `Mayhem service result exceeds ${this.maxMessageBytes} bytes.`,
      requestId
    ));
    return result;
  }

  _startServiceResultDelivery(requestId, cached) {
    if (cached.deliveryStarted || !cached.result?.message) return;
    cached.deliveryStarted = true;
    cached.acked = false;
    cached.resultAttempts = 0;
    cached.deliveryToken = (cached.deliveryToken ?? 0) + 1;
    const deliveryToken = cached.deliveryToken;
    let retryDelay = this.resultRetryMs;
    const active = () => (
      !this.stopped &&
      !cached.acked &&
      cached.deliveryStarted &&
      cached.deliveryToken === deliveryToken &&
      this.serviceProcessed.get(requestId) === cached
    );
    const schedule = () => {
      if (!active() || cached.resultAttempts >= this.resultRetryMax) {
        if (cached.deliveryToken === deliveryToken) cached.deliveryStarted = false;
        return;
      }
      cached.resultTimer = setTimeout(() => {
        cached.resultTimer = null;
        void attempt();
      }, retryDelay);
      if (typeof cached.resultTimer?.unref === 'function') cached.resultTimer.unref();
      retryDelay = Math.min(retryDelay * 2, this.resultRetryMs * 8);
    };
    const attempt = async () => {
      if (!active() || cached.resultAttempts >= this.resultRetryMax) return;
      cached.resultAttempts += 1;
      try {
        const sidechannel = this.peer?.sidechannel;
        if (!sidechannel || typeof sidechannel.broadcast !== 'function') return;
        if (!sidechannel.started) return;
        if (typeof sidechannel.connectDirectPeer === 'function') {
          try {
            void Promise.resolve(sidechannel.connectDirectPeer(
              cached.transport,
              this.channel,
              this.connectTimeoutMs
            )).catch(() => {});
          } catch (_error) {
            // Direct reconnect is best-effort; the joined relay channel still carries the result.
          }
        }
        if (!active()) return;
        sidechannel.broadcast(this.channel, cached.result.message);
      } catch (_error) {
        // A bounded later attempt re-reads and reconnects the current sidechannel.
      } finally {
        schedule();
      }
    };
    void attempt();
  }

  _sendServiceResultAck(admin, requestId, resultDigest) {
    const sidechannel = this.peer?.sidechannel;
    if (!sidechannel?.started || typeof sidechannel.broadcast !== 'function') return false;
    return sidechannel.broadcast(this.channel, {
      control: SERVICE_CONTROL_RESULT_ACK,
      version: RELAY_VERSION,
      request_id: requestId,
      result_digest: resultDigest,
      to: admin,
    }) === true;
  }

  async _handleServiceRequest(payload) {
    if (!this.peer.base?.writable) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    if (!admin || self !== admin) return;

    const message = payload.message;
    const transport = normalizeKey(payload.from);
    const envelope = this._verifyServiceRequest(message.service, message.value, { admin, transport });
    const actor = envelope?.actor;
    if (!actor || !transport || !this._verifyEnvelope(payload, transport)) return;
    if (message.version !== RELAY_VERSION) return;
    const expectedId = serviceRequestIdFor(message.service, message.value);
    if (message.request_id !== expectedId) return;

    this._pruneMap(this.serviceProcessed);
    let cached = this.serviceProcessed.get(expectedId);
    if (!cached && this._processedInFlight(this.serviceProcessed) >= this.processedInFlightMax) {
      const busy = this._prepareServiceResult(
        expectedId,
        transport,
        relayError('Mayhem admin service is busy; retry this signed request.', expectedId)
      );
      this.peer.sidechannel.broadcast(this.channel, busy.message);
      return;
    }
    if (!cached) {
      const promise = Promise.resolve()
        .then(() => this._handleService(message.service, envelope.payload, envelope))
        .catch((error) => relayError(
          error?.message || 'Mayhem admin service request failed.',
          expectedId
        ))
        .then((response) => this._prepareServiceResult(expectedId, transport, response));
      cached = {
        at: Date.now(),
        pending: true,
        promise,
        transport,
        result: null,
        resultTimer: null,
      };
      this.serviceProcessed.set(expectedId, cached);
      this._pruneMap(this.serviceProcessed);
    }
    const result = await cached.promise;
    cached.result = result;
    cached.pending = false;
    cached.at = Date.now();
    if (cached.acked || !cached.deliveryStarted) {
      this._clearServiceResultRetry(cached);
      this._startServiceResultDelivery(expectedId, cached);
    }
  }

  async _handleServiceResult(payload) {
    const message = payload.message;
    const requestId = String(message.request_id || '');
    const pending = this.servicePending.get(requestId);
    this._pruneMap(this.serviceCompleted);
    const completed = this.serviceCompleted.get(requestId);
    if (!pending && !completed) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    const resultDigest = normalizeKey(message.result_digest);
    if (
      message.version !== RELAY_VERSION ||
      normalizeKey(message.to) !== self ||
      normalizeKey(payload.from) !== admin ||
      !this._verifyEnvelope(payload, admin) ||
      !/^[0-9a-f]{64}$/.test(resultDigest) ||
      serviceResultDigestFor(requestId, message.response) !== resultDigest ||
      (completed && completed.resultDigest !== resultDigest)
    ) {
      return;
    }
    this._sendServiceResultAck(admin, requestId, resultDigest);
    if (!pending) {
      completed.at = Date.now();
      return;
    }
    const response = { ...message.response, relayed: true, request_id: requestId };
    this.servicePending.delete(requestId);
    this.serviceCompleted.set(requestId, {
      at: Date.now(),
      response,
      resultDigest,
    });
    this._pruneMap(this.serviceCompleted);
    pending.resolve(response);
  }

  async _handleServiceResultAck(payload) {
    if (!this.peer.base?.writable) return;
    const message = payload.message;
    const requestId = String(message.request_id || '');
    const cached = this.serviceProcessed.get(requestId);
    if (!cached?.result) return;
    const admin = await this._adminKey();
    const self = normalizeKey(this.peer?.wallet?.publicKey);
    const transport = normalizeKey(payload.from);
    if (
      message.version !== RELAY_VERSION ||
      normalizeKey(message.to) !== self ||
      self !== admin ||
      transport !== cached.transport ||
      normalizeKey(message.result_digest) !== cached.result.resultDigest ||
      !this._verifyEnvelope(payload, cached.transport)
    ) {
      return;
    }
    cached.acked = true;
    cached.at = Date.now();
    this._clearServiceResultRetry(cached);
  }

  async _applyRelayed(key, value, requestId) {
    if (value?.op === 'admin_contract_tx') {
      return await this._applyRelayedAdminTx(key, value, requestId);
    }
    return await this._submitRelayedFeature(key, value, requestId);
  }

  async _submitRelayedFeature(key, value, requestId) {
    let response = await this.submit(key, value, { nonce: requestId });
    while (!this.stopped && response?.status === 'pending') {
      const featureResult = await this._waitForResult(
        response.result_key,
        this.resultTimeoutMs
      );
      if (!featureResult) continue;
      response = this._featureResponse(
        key,
        response.hash,
        response.result_key,
        featureResult
      );
    }
    return this.stopped && response?.status === 'pending'
      ? relayError('Mayhem feature relay stopped before the canonical result appeared.', requestId)
      : response;
  }

  async _applyRelayedAdminTx(key, value, requestId) {
    const validationError = await this._validateAdminTx(key, value);
    if (validationError) return relayError(validationError, requestId);
    if (!value.sim) return await this._submitRelayedFeature(key, value, requestId);
    const response = await this._simulateAdminTx(value);
    return {
      ...response,
      relayed: true,
      request_id: requestId,
    };
  }

  async _validateAdminTx(key, value) {
    const allowed = new Set([
      'op',
      'tx',
      'prepared_command',
      'address',
      'signature',
      'nonce',
      'sim',
      'context',
    ]);
    if (
      !value ||
      typeof value !== 'object' ||
      Array.isArray(value) ||
      Object.keys(value).some((field) => !allowed.has(field)) ||
      [...allowed].some((field) => !Object.hasOwn(value, field))
    ) {
      return 'Invalid relayed admin contract transaction.';
    }
    const admin = await this._adminKey();
    if (
      normalizeKey(value.address) !== admin ||
      !/^[0-9a-f]{64}$/.test(value.tx) ||
      !/^[0-9a-f]{128}$/.test(value.signature) ||
      !/^[0-9a-f]{64}$/.test(value.nonce) ||
      typeof value.sim !== 'boolean' ||
      !value.prepared_command ||
      typeof value.prepared_command !== 'object' ||
      Array.isArray(value.prepared_command) ||
      typeof value.prepared_command.type !== 'string' ||
      !Object.hasOwn(value.prepared_command, 'value') ||
      !value.context ||
      typeof value.context !== 'object' ||
      Array.isArray(value.context) ||
      JSON.stringify(Object.keys(value.context).sort()) !== JSON.stringify([
        'contract_version',
        'msb_bootstrap',
        'network_id',
        'subnet_bootstrap',
      ]) ||
      key !== `admin/contract-tx/${value.tx}`
    ) {
      return 'Invalid relayed admin contract transaction.';
    }
    const configuredBootstrap = this.peer?.config?.bootstrap;
    const subnetBootstrap = b4a.isBuffer(configuredBootstrap)
      ? b4a.toString(configuredBootstrap, 'hex')
      : typeof configuredBootstrap === 'string' && configuredBootstrap
        ? configuredBootstrap.toLowerCase()
        : b4a.isBuffer(this.peer?.base?.key)
          ? b4a.toString(this.peer.base.key, 'hex')
          : '';
    const runtimeContext = {
      contract_version: CONTRACT_VERSION,
      msb_bootstrap: String(this.peer?.msbClient?.bootstrapHex ?? '').toLowerCase(),
      network_id: this.peer?.msbClient?.networkId,
      subnet_bootstrap: subnetBootstrap,
    };
    if (stableJson(value.context) !== stableJson(runtimeContext)) {
      return 'Admin contract transaction context does not match this contract network.';
    }
    const expectedTx = await adminContractTxDigest(value);
    if (expectedTx !== value.tx) {
      return 'Invalid admin contract transaction digest.';
    }
    if (!verifyEd25519Hex(
      this.peer.wallet,
      value.signature,
      b4a.from(value.tx, 'hex'),
      value.address
    )) {
      return 'Invalid admin contract transaction signature.';
    }
    return null;
  }

  async _simulateAdminTx(value) {
    const submitted = await this.adminTxHandler({
      tx: value.tx,
      prepared_command: value.prepared_command,
      address: value.address,
      signature: value.signature,
      nonce: value.nonce,
      sim: true,
    });
    const result = submitted?.result ?? submitted;
    const explicitSuccess = result?.ok === true ||
      (result?.local === true && result?.txo !== undefined);
    if (!explicitSuccess || result?.error) {
      return relayError(
        result?.error?.message ?? 'Admin contract transaction was rejected.'
      );
    }
    return {
      ok: true,
      accepted: true,
      status: 'simulated',
      tx: value.tx,
      result,
    };
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

  async _waitForResult(key, timeoutMs = DEFAULT_RESULT_TIMEOUT_MS, previousResult = null) {
    const boundedTimeoutMs = Number.isSafeInteger(timeoutMs) && timeoutMs > 0
      ? timeoutMs
      : DEFAULT_RESULT_TIMEOUT_MS;
    const deadline = Date.now() + boundedTimeoutMs;
    const previousJson = previousResult === null ? null : stableJson(previousResult);
    while (!this.stopped && Date.now() <= deadline) {
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
    for (const cached of this.processed.values()) {
      this._clearServiceResultRetry(cached);
    }
    this.processed.clear();
    this.completed.clear();
    for (const [requestId, pending] of this.servicePending) {
      pending.resolve(relayError('Mayhem service relay stopped.', requestId));
    }
    this.servicePending.clear();
    for (const cached of this.serviceProcessed.values()) {
      this._clearServiceResultRetry(cached);
    }
    this.serviceProcessed.clear();
    this.serviceCompleted.clear();
  }
}

export {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_MESSAGE_BYTES,
  participantFor,
  requestIdFor,
  serviceRequestIdFor,
  serviceSigningMessage,
  stripeConnectRelinkConsentMessage,
};
export default MayhemFeature;
