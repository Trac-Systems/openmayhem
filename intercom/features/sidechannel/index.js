import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';
import crypto from 'crypto';
import PeerWallet from 'trac-wallet';
import {
  DEFAULT_MAX_JSON_STRING_BYTES,
  boundedJsonEncoding,
  decodedJsonByteLength,
  decodedJsonWasRejected,
  jsonShapeWithinBounds,
} from '../bounded-json.js';

const DEFAULT_MUX_RETRY_MAX = 5;
const DEFAULT_MUX_RETRY_DELAY_MS = 50;
const DEFAULT_OPEN_RETRY_MAX = 5;
const DEFAULT_OPEN_RETRY_BASE_MS = 100;
const DEFAULT_OPEN_RETRY_RESET_MS = 2_000;
const DEFAULT_FLUSH_TIMEOUT_MS = 10_000;
const DEFAULT_DIRECT_CONNECT_MAX_WAIT_MS = 120_000;
const DEFAULT_DIRECT_CONNECT_POLL_MS = 100;
const DEFAULT_MAX_CHANNELS = 1024;
const DEFAULT_MAX_CHANNEL_NAME_BYTES = 256;
const DEFAULT_CHANNEL_OPEN_TIMEOUT_MS = 120_000;
const DEFAULT_RELAY_RATE_BYTES_PER_SECOND = 256_000;
const DEFAULT_RELAY_BURST_BYTES = 1_000_000;
const DEFAULT_RELAY_SOURCE_RATE_BYTES_PER_SECOND = 64_000;
const DEFAULT_RELAY_SOURCE_BURST_BYTES = 256_000;
const DEFAULT_MAX_RELAY_SOURCES = 1024;
const DEFAULT_POW_YIELD_EVERY = 4_096;
const DEFAULT_MAX_POW_QUEUE = 64;
const MAX_MESSAGE_ID_BYTES = 256;

const safeIntegerOr = (value, fallback, { min = 0 } = {}) => (
  Number.isSafeInteger(value) && value >= min ? value : fallback
);

// Join topics must be deterministic and collision-resistant.
// The previous implementation (alloc(32).fill(name)) could collide for different names.
const toTopic = (name) =>
  crypto.createHash('sha256').update(`sidechannel:${normalizeChannel(name)}`).digest();
const toProtocol = (name) => `sidechannel/${name}`;

const stableStringify = (value) => {
  if (value === null || value === undefined) return 'null';
  if (typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(',')}]`;
  }
  const keys = Object.keys(value).sort();
  return `{${keys.map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`).join(',')}}`;
};

const sha256Hex = (input) => crypto.createHash('sha256').update(input).digest('hex');

const normalizeKeyHex = (value) => {
  if (!value) return null;
  if (b4a.isBuffer(value)) return b4a.toString(value, 'hex');
  if (typeof value === 'string') return value.trim().toLowerCase();
  // JSON.stringify(Buffer.from(...)) yields { type: 'Buffer', data: [...] }.
  // Sidechannels use `c.json` encoding, so decode-side keys can arrive in this form.
  if (typeof value === 'object' && value.type === 'Buffer' && Array.isArray(value.data)) {
    try {
      return b4a.toString(b4a.from(value.data), 'hex');
    } catch (_e) {
      return null;
    }
  }
  return String(value).trim().toLowerCase();
};

const normalizeChannel = (value) => String(value || '').trim();

const countLeadingZeroBits = (hex) => {
  let bits = 0;
  for (let i = 0; i < hex.length; i += 1) {
    const nibble = Number.parseInt(hex[i], 16);
    if (nibble === 0) {
      bits += 4;
      continue;
    }
    // Count leading zeros in this nibble.
    for (let mask = 8; mask > 0; mask >>= 1) {
      if (nibble & mask) return bits;
      bits += 1;
    }
  }
  return bits;
};

class Sidechannel extends Feature {
  constructor(peer, config = {}) {
    super(peer, config);
    this.key = 'sidechannel';
    this.channels = new Map();
    this.connections = new Map();
    this.rateLimits = new Map();
    this.preparedConnections = new WeakSet();
    this.closedConnections = new WeakSet();
    this.started = false;
    this._startPromise = null;
    this._startGeneration = 0;
    this._connectionListenerAttached = false;
    this._dhtBootPromise = null;
    this.onMessage = typeof config.onMessage === 'function' ? config.onMessage : null;
    this.debug = config.debug === true;
    this.muxRetryMax = safeIntegerOr(config.muxRetryMax, DEFAULT_MUX_RETRY_MAX);
    this.muxRetryDelayMs = safeIntegerOr(
      config.muxRetryDelayMs,
      DEFAULT_MUX_RETRY_DELAY_MS,
      { min: 1 }
    );
    this.openRetryMax = safeIntegerOr(config.openRetryMax, DEFAULT_OPEN_RETRY_MAX);
    this.openRetryBaseMs = safeIntegerOr(
      config.openRetryBaseMs,
      DEFAULT_OPEN_RETRY_BASE_MS,
      { min: 1 }
    );
    this.openRetryResetMs = safeIntegerOr(
      config.openRetryResetMs,
      DEFAULT_OPEN_RETRY_RESET_MS,
      { min: 1 }
    );
    this.flushTimeoutMs = safeIntegerOr(
      config.flushTimeoutMs,
      DEFAULT_FLUSH_TIMEOUT_MS,
      { min: 1 }
    );
    this.directConnectMaxWaitMs = safeIntegerOr(
      config.directConnectMaxWaitMs,
      DEFAULT_DIRECT_CONNECT_MAX_WAIT_MS,
      { min: 1 }
    );
    this.directConnectPollMs = safeIntegerOr(
      config.directConnectPollMs,
      DEFAULT_DIRECT_CONNECT_POLL_MS,
      { min: 1 }
    );
    this.maxMessageBytes = Number.isSafeInteger(config.maxMessageBytes) && config.maxMessageBytes > 0
      ? config.maxMessageBytes
      : 1_000_000;
    this.maxStringBytes = safeIntegerOr(
      config.maxStringBytes,
      Math.min(this.maxMessageBytes, DEFAULT_MAX_JSON_STRING_BYTES),
      { min: 1 }
    );
    this.maxChannels = safeIntegerOr(config.maxChannels, DEFAULT_MAX_CHANNELS, { min: 1 });
    this.maxChannelNameBytes = safeIntegerOr(
      config.maxChannelNameBytes,
      DEFAULT_MAX_CHANNEL_NAME_BYTES,
      { min: 1 }
    );
    this.channelOpenTimeoutMs = safeIntegerOr(
      config.channelOpenTimeoutMs,
      DEFAULT_CHANNEL_OPEN_TIMEOUT_MS,
      { min: 1 }
    );
    this.maxMessageBytesByChannel = new Map();
    const channelLimitEntries = config.maxMessageBytesByChannel instanceof Map
      ? Array.from(config.maxMessageBytesByChannel.entries())
      : config.maxMessageBytesByChannel && typeof config.maxMessageBytesByChannel === 'object'
        ? Object.entries(config.maxMessageBytesByChannel)
        : [];
    for (const [channel, value] of channelLimitEntries) {
      const normalized = normalizeChannel(channel);
      if (normalized && Number.isSafeInteger(value) && value > 0) {
        this.maxMessageBytesByChannel.set(normalized, value);
      }
    }
    this.entryChannel = typeof config.entryChannel === 'string' ? config.entryChannel : null;
    this.allowRemoteOpen = config.allowRemoteOpen !== false;
    this.autoJoinOnOpen = config.autoJoinOnOpen === true;
    this.relayEnabled = config.relayEnabled !== false;
    this.relayTtl = 1;
    this.relayRateBytesPerSecond = safeIntegerOr(
      config.relayRateBytesPerSecond,
      DEFAULT_RELAY_RATE_BYTES_PER_SECOND,
      { min: 1 }
    );
    this.relayBurstBytes = safeIntegerOr(
      config.relayBurstBytes,
      DEFAULT_RELAY_BURST_BYTES,
      { min: 1 }
    );
    this.relaySourceRateBytesPerSecond = safeIntegerOr(
      config.relaySourceRateBytesPerSecond,
      DEFAULT_RELAY_SOURCE_RATE_BYTES_PER_SECOND,
      { min: 1 }
    );
    this.relaySourceBurstBytes = safeIntegerOr(
      config.relaySourceBurstBytes,
      DEFAULT_RELAY_SOURCE_BURST_BYTES,
      { min: 1 }
    );
    this.maxRelaySources = safeIntegerOr(
      config.maxRelaySources,
      DEFAULT_MAX_RELAY_SOURCES,
      { min: 1 }
    );
    const now = this._now();
    this.relayLimiter = {
      tokens: this.relayBurstBytes,
      lastRefill: now,
      lastSeen: now,
    };
    this.relaySourceLimits = new Map();
    this.relayCounters = {
      messages: 0,
      bytes: 0,
      node_budget_drops: 0,
      source_budget_drops: 0,
      policy_drops: 0,
      transitive_drops: 0,
      unauthenticated_drops: 0,
    };
    this.maxSeen = Number.isSafeInteger(config.maxSeen) ? config.maxSeen : 5000;
    this.seenTtlMs = Number.isSafeInteger(config.seenTtlMs) ? config.seenTtlMs : 120_000;
    this.rateBytesPerSecond = Number.isSafeInteger(config.rateBytesPerSecond)
      ? config.rateBytesPerSecond
      : 64_000;
    this.rateBurstBytes = Number.isSafeInteger(config.rateBurstBytes)
      ? config.rateBurstBytes
      : 256_000;
    this.maxStrikes = Number.isSafeInteger(config.maxStrikes) ? config.maxStrikes : 3;
    this.strikeWindowMs = Number.isSafeInteger(config.strikeWindowMs) ? config.strikeWindowMs : 5000;
    this.blockMs = Number.isSafeInteger(config.blockMs) ? config.blockMs : 30_000;
    this.seen = new Map();
    this.powEnabled = config.powEnabled === true;
    this.powDifficulty = Number.isInteger(config.powDifficulty) ? config.powDifficulty : 0;
    this.powYieldEvery = safeIntegerOr(
      config.powYieldEvery,
      DEFAULT_POW_YIELD_EVERY,
      { min: 1 }
    );
    this.maxPowQueue = safeIntegerOr(
      config.maxPowQueue,
      DEFAULT_MAX_POW_QUEUE,
      { min: 1 }
    );
    this.powQueueDepth = 0;
    this.powQueue = Promise.resolve();
    this.powRequireEntry = config.powRequireEntry === true;
    this.powRequiredChannels = Array.isArray(config.powRequiredChannels)
      ? new Set(config.powRequiredChannels.map((c) => String(c)))
      : null;
    this.powExemptControlsByChannel = new Map();
    const powExemptEntries = config.powExemptControlsByChannel instanceof Map
      ? Array.from(config.powExemptControlsByChannel.entries())
      : config.powExemptControlsByChannel && typeof config.powExemptControlsByChannel === 'object'
        ? Object.entries(config.powExemptControlsByChannel)
        : [];
    for (const [channel, controls] of powExemptEntries) {
      if (!Array.isArray(controls)) continue;
      const normalizedChannel = normalizeChannel(channel);
      const normalizedControls = controls
        .map((control) => String(control || '').trim())
        .filter(Boolean);
      if (normalizedChannel && normalizedControls.length > 0) {
        this.powExemptControlsByChannel.set(normalizedChannel, new Set(normalizedControls));
      }
    }
    this.inviteRequired = config.inviteRequired === true;
    this.inviteRequiredChannels = Array.isArray(config.inviteRequiredChannels)
      ? new Set(config.inviteRequiredChannels.map((c) => String(c)))
      : null;
    this.inviteRequiredPrefixes = Array.isArray(config.inviteRequiredPrefixes)
      ? config.inviteRequiredPrefixes.map((c) => String(c))
      : null;
    const inviterKeys = Array.isArray(config.inviterKeys)
      ? config.inviterKeys
          .map((value) => normalizeKeyHex(value))
          .filter((value) => value && value.length > 0)
      : [];
    if (this.inviteRequired && inviterKeys.length === 0) {
      const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
      if (selfKey) inviterKeys.push(selfKey);
    }
    this.inviterKeys = inviterKeys.length > 0 ? new Set(inviterKeys) : null;
    this.inviteTtlMs = Number.isSafeInteger(config.inviteTtlMs) ? config.inviteTtlMs : 0;
    this.invitedPeers = new Map();
    this.localInvites = new Map();
    // Stores the last accepted invite object (for auth handshakes).
    this.localInviteObjects = new Map();
    this.ownerWriteOnly = config.ownerWriteOnly === true;
    this.ownerWriteChannels = Array.isArray(config.ownerWriteChannels)
      ? new Set(config.ownerWriteChannels.map((c) => normalizeChannel(c)))
      : null;
    this.welcomeRequired = config.welcomeRequired !== false;
    this.ownerKeys = new Map();
    const ownerEntries = config.ownerKeys instanceof Map
      ? Array.from(config.ownerKeys.entries())
      : Array.isArray(config.ownerKeys)
        ? config.ownerKeys
        : config.ownerKeys && typeof config.ownerKeys === 'object'
          ? Object.entries(config.ownerKeys)
          : [];
    for (const entry of ownerEntries) {
      const [channel, key] = Array.isArray(entry) ? entry : [];
      const normalizedChannel = normalizeChannel(channel);
      const normalizedKey = normalizeKeyHex(key);
      if (normalizedChannel && normalizedKey) this.ownerKeys.set(normalizedChannel, normalizedKey);
    }
    this.defaultOwnerKey = normalizeKeyHex(config.defaultOwnerKey);
    this.welcomeByChannel = new Map();
    const welcomeEntries = config.welcomeByChannel instanceof Map
      ? Array.from(config.welcomeByChannel.entries())
      : Array.isArray(config.welcomeByChannel)
        ? config.welcomeByChannel
        : config.welcomeByChannel && typeof config.welcomeByChannel === 'object'
          ? Object.entries(config.welcomeByChannel)
          : [];
    for (const entry of welcomeEntries) {
      const [channel, welcome] = Array.isArray(entry) ? entry : [];
      const normalizedChannel = normalizeChannel(channel);
      if (normalizedChannel && welcome) {
        this.welcomeByChannel.set(normalizedChannel, welcome);
      }
    }
    this.welcomedChannels = new Set();
    for (const [channel, welcome] of this.welcomeByChannel.entries()) {
      this._verifyWelcome(welcome, channel, null);
    }
    const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
    if (selfKey) {
      for (const [channel, key] of this.ownerKeys.entries()) {
        if (key === selfKey) this._rememberWelcome(channel);
      }
      if (this.defaultOwnerKey && this.defaultOwnerKey === selfKey && this.entryChannel) {
        this._rememberWelcome(this.entryChannel);
      }
    }

    const initial = Array.isArray(config.channels) ? config.channels : [];
    for (const name of initial) this._registerChannel(name);
  }

  _now() {
    return Date.now();
  }

  _isEntry(channel) {
    const normalized = normalizeChannel(channel);
    const entry = this.entryChannel ? normalizeChannel(this.entryChannel) : '';
    return normalized.length > 0 && entry.length > 0 && normalized === entry;
  }

  _maxMessageBytes(channel) {
    return this.maxMessageBytesByChannel.get(normalizeChannel(channel)) ?? this.maxMessageBytes;
  }

  _maxStringBytes(channel) {
    return Math.min(this.maxStringBytes, this._maxMessageBytes(channel));
  }

  _getRemoteKey(connection) {
    return normalizeKeyHex(connection?.remotePublicKey) || 'unknown';
  }

  _purgeSeen(now) {
    const cutoff = now - this.seenTtlMs;
    for (const [id, ts] of this.seen) {
      if (ts < cutoff) this.seen.delete(id);
    }
  }

  _rememberSeen(id, now) {
    if (!id) return false;
    this._purgeSeen(now);
    if (this.seen.has(id)) return true;
    this.seen.set(id, now);
    if (this.seen.size > this.maxSeen) {
      let oldestId = null;
      let oldestAt = Number.POSITIVE_INFINITY;
      for (const [candidateId, candidateAt] of this.seen) {
        if (candidateAt < oldestAt) {
          oldestId = candidateId;
          oldestAt = candidateAt;
        }
      }
      if (oldestId !== null) this.seen.delete(oldestId);
    }
    return false;
  }

  _getLimiter(connection) {
    let state = this.rateLimits.get(connection);
    if (!state) {
      const now = this._now();
      state = {
        tokens: this.rateBurstBytes,
        lastRefill: now,
        strikes: 0,
        strikeResetAt: now + this.strikeWindowMs,
        blockedUntil: 0,
      };
      this.rateLimits.set(connection, state);
    }
    return state;
  }

  _isBlocked(connection) {
    const state = this.rateLimits.get(connection);
    if (!state) return false;
    return this._now() < state.blockedUntil;
  }

  _checkRate(connection, bytes) {
    if (this.rateBytesPerSecond <= 0) return true;
    const state = this._getLimiter(connection);
    const now = this._now();
    if (now < state.blockedUntil) return false;

    if (now > state.strikeResetAt) {
      state.strikes = 0;
      state.strikeResetAt = now + this.strikeWindowMs;
    }

    const elapsedMs = now - state.lastRefill;
    if (elapsedMs > 0) {
      const refill = (elapsedMs / 1000) * this.rateBytesPerSecond;
      state.tokens = Math.min(this.rateBurstBytes, state.tokens + refill);
      state.lastRefill = now;
    }

    if (bytes > state.tokens) {
      state.strikes += 1;
      if (state.strikes >= this.maxStrikes) {
        state.blockedUntil = now + this.blockMs;
        if (this.debug) {
          console.log(`[sidechannel] rate-limit block ${this._getRemoteKey(connection)} for ${this.blockMs}ms`);
        }
      }
      return false;
    }

    state.tokens -= bytes;
    return true;
  }

  _buildPayload(channel, message, invite = null, { deferPow = false } = {}) {
    const ts = this._now();
    // Encode keys as hex strings, not Buffers, because we transmit payloads via JSON encoding.
    const from = normalizeKeyHex(this.peer?.wallet?.publicKey) ?? null;
    const id = `${from ?? 'anon'}:${ts}:${Math.random().toString(36).slice(2, 10)}`;
    const payload = {
      type: 'sidechannel',
      id,
      channel,
      from,
      origin: from,
      message,
      ts,
      ttl: this.relayTtl,
    };
    if (invite) payload.invite = invite;
    if (!deferPow) {
      this._attachPow(payload);
      // Message-level signatures allow receivers to enforce "owner-only write" even when
      // messages are relayed (the transport peer can be a relay, not the original sender).
      this._attachSig(payload);
    }
    return payload;
  }

  requestOpen(newChannel, viaChannel = null, invite = null, welcome = null) {
    const target = String(newChannel || '').trim();
    if (!target) return false;
    const via = String(viaChannel || this.entryChannel || '').trim();
    if (!via) return false;
    if (invite) this._acceptLocalInvite(invite, target);
    const inviteWelcome = invite?.welcome;
    const desiredWelcome = welcome || inviteWelcome;
    if (desiredWelcome) this._verifyWelcome(desiredWelcome, target, null);
    return this.broadcast(via, {
      control: 'open_channel',
      channel: target,
      invite: invite || undefined,
      welcome: desiredWelcome || undefined,
    });
  }

  _relay(channel, payload, originConnection) {
    if (!this.relayEnabled) return;
    if (!this._relayPolicyAllows(channel)) {
      this.relayCounters.policy_drops += 1;
      return;
    }
    const author = normalizeKeyHex(payload?.from);
    if (!author || !this.verifyPayload(payload, author)) {
      this.relayCounters.unauthenticated_drops += 1;
      return;
    }
    if (this._getRemoteKey(originConnection) !== author) {
      this.relayCounters.transitive_drops += 1;
      return;
    }
    const control = payload?.message?.control;
    // Never relay handshake/control messages; they are for direct neighbor authorization.
    if (control === 'auth' || control === 'welcome') return;
    const ttl = Number.isFinite(payload?.ttl) ? payload.ttl : 0;
    if (ttl <= 0) return;
    const relayed = {
      ...payload,
      ttl: ttl - 1,
      relayedBy: normalizeKeyHex(this.peer?.wallet?.publicKey) ?? null,
    };
    let relayedBytes = 0;
    try {
      if (
        !jsonShapeWithinBounds(
          relayed,
          undefined,
          undefined,
          this._maxStringBytes(channel)
        )
      ) {
        return;
      }
      relayedBytes = b4a.byteLength(JSON.stringify(relayed), 'utf8');
    } catch (_error) {
      return;
    }
    if (relayedBytes > this._maxMessageBytes(channel)) return;
    for (const [connection, perConn] of this.connections.entries()) {
      if (connection === originConnection) continue;
      if (!this._remoteAuthorized(channel, connection)) continue;
      const record = perConn.get(channel);
      if (record?.message) {
        if (!this._checkRelayBudget(author, relayedBytes)) continue;
        try {
          record.message.send(relayed);
          this.relayCounters.messages += 1;
          this.relayCounters.bytes += relayedBytes;
        } catch (error) {
          this._reportEventError(`relay ${channel}`, error, connection);
        }
      }
    }
  }

  _relayPolicyAllows(channel) {
    if (this._isEntry(channel)) return false;
    return (
      this._powRequired(channel)
      || this._inviteRequired(channel)
      || this._ownerWriteOnly(channel)
    );
  }

  _refillRelayLimiter(limiter, rateBytesPerSecond, burstBytes, now) {
    const elapsedMs = now - limiter.lastRefill;
    if (elapsedMs > 0) {
      const refill = (elapsedMs / 1000) * rateBytesPerSecond;
      limiter.tokens = Math.min(burstBytes, limiter.tokens + refill);
      limiter.lastRefill = now;
    }
    limiter.lastSeen = now;
  }

  _relaySourceLimiter(author, now) {
    let limiter = this.relaySourceLimits.get(author);
    if (limiter) return limiter;
    if (this.relaySourceLimits.size >= this.maxRelaySources) {
      let oldestAuthor = null;
      let oldestAt = Number.POSITIVE_INFINITY;
      for (const [candidate, state] of this.relaySourceLimits) {
        if (state.lastSeen < oldestAt) {
          oldestAuthor = candidate;
          oldestAt = state.lastSeen;
        }
      }
      if (oldestAuthor !== null) this.relaySourceLimits.delete(oldestAuthor);
    }
    limiter = {
      tokens: this.relaySourceBurstBytes,
      lastRefill: now,
      lastSeen: now,
    };
    this.relaySourceLimits.set(author, limiter);
    return limiter;
  }

  _checkRelayBudget(author, bytes) {
    const now = this._now();
    this._refillRelayLimiter(
      this.relayLimiter,
      this.relayRateBytesPerSecond,
      this.relayBurstBytes,
      now
    );
    if (bytes > this.relayLimiter.tokens) {
      this.relayCounters.node_budget_drops += 1;
      return false;
    }
    const source = this._relaySourceLimiter(author, now);
    this._refillRelayLimiter(
      source,
      this.relaySourceRateBytesPerSecond,
      this.relaySourceBurstBytes,
      now
    );
    if (bytes > source.tokens) {
      this.relayCounters.source_budget_drops += 1;
      return false;
    }
    this.relayLimiter.tokens -= bytes;
    source.tokens -= bytes;
    return true;
  }

  _reportEventError(scope, error, connection = null) {
    console.error(
      `[sidechannel] ${scope} failed for ${this._getRemoteKey(connection)} without stopping the peer:`,
      error?.message ?? error
    );
  }

  _powExempt(channel, message = null) {
    const control = typeof message?.control === 'string' ? message.control : '';
    if (!control) return false;
    return this.powExemptControlsByChannel.get(normalizeChannel(channel))?.has(control) === true;
  }

  _powRequired(channel, message = null) {
    if (this._powExempt(channel, message)) return false;
    if (!this.powEnabled || this.powDifficulty <= 0) return false;
    if (this.powRequiredChannels) return this.powRequiredChannels.has(channel);
    if (this.powRequireEntry) return channel === this.entryChannel;
    return true;
  }

  _inviteRequired(channel) {
    if (this._isEntry(channel)) return false;
    if (!this.inviteRequired) return false;
    const hasList = this.inviteRequiredChannels || this.inviteRequiredPrefixes;
    if (this.inviteRequiredChannels && this.inviteRequiredChannels.has(channel)) return true;
    if (this.inviteRequiredPrefixes) {
      for (const prefix of this.inviteRequiredPrefixes) {
        if (prefix && channel.startsWith(prefix)) return true;
      }
    }
    // If the caller configured a list/prefix set, invites are only required for those entries.
    if (hasList) return false;
    return true;
  }

  _ownerWriteOnly(channel) {
    if (this._isEntry(channel)) return false;
    if (this.ownerWriteOnly) return true;
    if (this.ownerWriteChannels) return this.ownerWriteChannels.has(normalizeChannel(channel));
    return false;
  }

  _getInviteMap(channel) {
    if (!this.invitedPeers.has(channel)) this.invitedPeers.set(channel, new Map());
    return this.invitedPeers.get(channel);
  }

  _isInvited(channel, pubkey) {
    const map = this.invitedPeers.get(channel);
    if (!map) return false;
    const expiresAt = map.get(pubkey);
    if (!Number.isFinite(expiresAt)) {
      map.delete(pubkey);
      return false;
    }
    if (expiresAt <= this._now()) {
      map.delete(pubkey);
      return false;
    }
    return true;
  }

  _rememberInvite(channel, pubkey, expiresAt) {
    if (!Number.isFinite(expiresAt)) return;
    const map = this._getInviteMap(channel);
    map.set(pubkey, expiresAt);
  }

  _rememberLocalInvite(channel, expiresAt) {
    if (!Number.isFinite(expiresAt)) return;
    this.localInvites.set(normalizeChannel(channel), expiresAt);
  }

  _isLocallyInvited(channel) {
    const key = normalizeChannel(channel);
    const expiresAt = this.localInvites.get(key);
    if (!Number.isFinite(expiresAt)) {
      this.localInvites.delete(key);
      return false;
    }
    if (expiresAt <= this._now()) {
      this.localInvites.delete(key);
      return false;
    }
    return true;
  }

  _normalizeInvitePayload(payload) {
    return {
      channel: String(payload?.channel ?? ''),
      inviteePubKey: normalizeKeyHex(payload?.inviteePubKey) || '',
      inviterPubKey: normalizeKeyHex(payload?.inviterPubKey) || '',
      inviterAddress: payload?.inviterAddress ?? null,
      issuedAt: Number(payload?.issuedAt),
      expiresAt: Number(payload?.expiresAt),
      nonce: String(payload?.nonce ?? ''),
      version: Number.isFinite(payload?.version) ? Number(payload.version) : 1,
    };
  }

  _verifyInviteForKey(invite, channel, inviteeKey) {
    if (!invite || typeof invite !== 'object') return false;
    const payload = invite.payload && typeof invite.payload === 'object' ? invite.payload : invite;
    const sigHex = invite.sig || invite.signature;
    if (typeof sigHex !== 'string' || sigHex.length === 0) return false;
    const normalized = this._normalizeInvitePayload(payload);
    if (normalized.channel !== String(channel)) return false;
    if (normalized.inviteePubKey !== inviteeKey) return false;
    if (!normalized.inviterPubKey || normalized.inviterPubKey.length === 0) return false;
    if (this.inviterKeys && !this.inviterKeys.has(normalized.inviterPubKey)) return false;
    if (!Number.isFinite(normalized.issuedAt) || !Number.isFinite(normalized.expiresAt)) return false;
    if (normalized.expiresAt <= this._now()) return false;
    const message = stableStringify(normalized);
    let sigBuf = null;
    let pubBuf = null;
    try {
      sigBuf = b4a.from(sigHex, 'hex');
      pubBuf = b4a.from(normalized.inviterPubKey, 'hex');
    } catch (_e) {
      return false;
    }
    if (!PeerWallet.verify(sigBuf, b4a.from(message), pubBuf)) return false;
    return normalized;
  }

  _verifyInvite(invite, channel, connection) {
    const remoteKey = this._getRemoteKey(connection);
    const normalized = this._verifyInviteForKey(invite, channel, remoteKey);
    if (!normalized) return false;
    this._rememberInvite(channel, remoteKey, normalized.expiresAt);
    return true;
  }

  _acceptLocalInvite(invite, channel) {
    const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
    if (!selfKey) return false;
    const normalized = this._verifyInviteForKey(invite, channel, selfKey);
    if (!normalized) return false;
    this._rememberLocalInvite(channel, normalized.expiresAt);
    this.localInviteObjects.set(normalizeChannel(channel), invite);
    const embeddedWelcome = invite?.welcome;
    if (embeddedWelcome) {
      this._verifyWelcome(embeddedWelcome, channel, null);
    }
    return true;
  }

  _checkInvite(payload, channel, connection) {
    if (!this._inviteRequired(channel)) return true;
    const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
    const selfIsInviter = this.inviterKeys && selfKey && this.inviterKeys.has(selfKey);
    if (!selfIsInviter && !this._isLocallyInvited(channel)) return false;
    const remoteKey = this._getRemoteKey(connection);
    if (this.inviterKeys && this.inviterKeys.has(remoteKey)) return true;
    if (this._isInvited(channel, remoteKey)) return true;
    const invite = payload?.invite || payload?.message?.invite;
    if (invite && this._verifyInvite(invite, channel, connection)) return true;
    return false;
  }

  _normalizeWelcomePayload(payload) {
    return {
      channel: normalizeChannel(payload?.channel),
      ownerPubKey: normalizeKeyHex(payload?.ownerPubKey) || '',
      text: String(payload?.text ?? ''),
      issuedAt: Number(payload?.issuedAt),
      version: Number.isFinite(payload?.version) ? Number(payload.version) : 1,
    };
  }

  _getOwnerKey(channel) {
    const normalized = normalizeChannel(channel);
    if (this.ownerKeys.has(normalized)) return this.ownerKeys.get(normalized);
    return this.defaultOwnerKey;
  }

  _welcomeRequired(channel) {
    if (this._isEntry(channel)) return false;
    if (!this.welcomeRequired) return false;
    return true;
  }

  _isWelcomed(channel) {
    return this.welcomedChannels.has(normalizeChannel(channel));
  }

  _rememberWelcome(channel) {
    this.welcomedChannels.add(normalizeChannel(channel));
  }

  _verifyWelcome(welcome, channel, connection) {
    if (!welcome || typeof welcome !== 'object') return false;
    const payload = welcome.payload && typeof welcome.payload === 'object' ? welcome.payload : welcome;
    const sigHex = welcome.sig || welcome.signature;
    if (typeof sigHex !== 'string' || sigHex.length === 0) return false;
    const normalized = this._normalizeWelcomePayload(payload);
    if (normalized.channel !== normalizeChannel(channel)) return false;
    const ownerKey = this._getOwnerKey(channel);
    if (!ownerKey || normalized.ownerPubKey !== ownerKey) return false;
    if (!Number.isFinite(normalized.issuedAt)) return false;
    const message = stableStringify(normalized);
    let sigBuf = null;
    let pubBuf = null;
    try {
      sigBuf = b4a.from(sigHex, 'hex');
      pubBuf = b4a.from(ownerKey, 'hex');
    } catch (_e) {
      return false;
    }
    if (!PeerWallet.verify(sigBuf, b4a.from(message), pubBuf)) return false;
    this._rememberWelcome(channel);
    // Persist the verified welcome in-memory so the owner can auto-send it to new connections
    // without requiring a restart (welcome is still bound to the configured owner key).
    this.welcomeByChannel.set(normalizeChannel(channel), welcome);
    return true;
  }

  _isWelcomeMessage(payload) {
    return payload?.message?.control === 'welcome';
  }

  _extractWelcome(payload) {
    if (!payload || typeof payload !== 'object') return null;
    return (
      payload?.message?.welcome ||
      payload?.welcome ||
      payload?.invite?.welcome ||
      payload?.message?.invite?.welcome ||
      null
    );
  }

  _getConfiguredWelcome(channel) {
    return this.welcomeByChannel.get(normalizeChannel(channel)) || null;
  }

  getWelcome(channel) {
    return this._getConfiguredWelcome(channel);
  }

  _powBase(payload, nonce) {
    return stableStringify({
      id: payload?.id ?? null,
      channel: payload?.channel ?? null,
      from: payload?.from ?? null,
      origin: payload?.origin ?? null,
      message: payload?.message ?? null,
      ts: payload?.ts ?? null,
      nonce,
    });
  }

  _attachPow(payload) {
    const channel = payload?.channel ?? '';
    if (!this._powRequired(channel, payload?.message)) return;
    const difficulty = this.powDifficulty;
    // The mining loop runs ~2^difficulty iterations. Re-serializing the whole
    // payload per nonce froze the event loop for tens of seconds per message
    // (2026-07-13 live incident: every relayed admission op took 24-43+ seconds
    // and retries re-mined fresh payloads, livelocking the peer). The nonce is a
    // number in the alphabetically sorted key order (channel, from, id, message,
    // nonce, origin, ts), so the base splits into a constant prefix/suffix and
    // each iteration is a cheap concat + sha256. `_powBase` stays the wire
    // format; the split is verified against it before the pow is attached.
    const prefix =
      `{"channel":${stableStringify(payload?.channel ?? null)}` +
      `,"from":${stableStringify(payload?.from ?? null)}` +
      `,"id":${stableStringify(payload?.id ?? null)}` +
      `,"message":${stableStringify(payload?.message ?? null)}` +
      ',"nonce":';
    const suffix =
      `,"origin":${stableStringify(payload?.origin ?? null)}` +
      `,"ts":${stableStringify(payload?.ts ?? null)}}`;
    if (prefix + '0' + suffix !== this._powBase(payload, 0)) {
      // Never mine an incompatible base: fall back to the canonical builder.
      let nonce = 0;
      while (true) {
        const hash = sha256Hex(this._powBase(payload, nonce));
        if (countLeadingZeroBits(hash) >= difficulty) {
          payload.pow = { nonce, difficulty };
          return;
        }
        nonce += 1;
      }
    }
    let nonce = 0;
    while (true) {
      const hash = sha256Hex(prefix + nonce + suffix);
      if (countLeadingZeroBits(hash) >= difficulty) {
        payload.pow = { nonce, difficulty };
        return;
      }
      nonce += 1;
    }
  }

  async _attachPowCooperatively(payload) {
    const channel = payload?.channel ?? '';
    if (!this._powRequired(channel, payload?.message)) return;
    const difficulty = this.powDifficulty;
    const prefix =
      `{"channel":${stableStringify(payload?.channel ?? null)}` +
      `,"from":${stableStringify(payload?.from ?? null)}` +
      `,"id":${stableStringify(payload?.id ?? null)}` +
      `,"message":${stableStringify(payload?.message ?? null)}` +
      ',"nonce":';
    const suffix =
      `,"origin":${stableStringify(payload?.origin ?? null)}` +
      `,"ts":${stableStringify(payload?.ts ?? null)}}`;
    const optimized = prefix + '0' + suffix === this._powBase(payload, 0);
    let nonce = 0;

    // Timers are available in both Pear/Bare and Node. Yield once before mining,
    // then between bounded chunks so heartbeats and session traffic keep moving.
    await new Promise((resolve) => setTimeout(resolve, 0));
    while (true) {
      const chunkEnd = nonce + this.powYieldEvery;
      for (; nonce < chunkEnd; nonce += 1) {
        const input = optimized
          ? prefix + nonce + suffix
          : this._powBase(payload, nonce);
        if (countLeadingZeroBits(sha256Hex(input)) >= difficulty) {
          payload.pow = { nonce, difficulty };
          return;
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
  }

  _checkPow(payload, channel) {
    if (!this._powRequired(channel, payload?.message)) return true;
    const pow = payload?.pow;
    if (!pow || typeof pow.nonce !== 'number') return false;
    const difficulty = this.powDifficulty;
    if (!Number.isInteger(difficulty) || difficulty <= 0) return false;
    const hash = sha256Hex(this._powBase(payload, pow.nonce));
    return countLeadingZeroBits(hash) >= difficulty;
  }

  _sigPayload(payload) {
    // Normalize to JSON-compatible data so the signature base matches what receivers
    // observe after compact-encoding's JSON roundtrip.
    let message = null;
    try {
      message = JSON.parse(JSON.stringify(payload?.message ?? null));
    } catch (_e) {
      message = null;
    }

    return {
      kind: 'sidechannel_message_v1',
      id: payload?.id ?? null,
      channel: payload?.channel ?? null,
      from: normalizeKeyHex(payload?.from) ?? null,
      origin: normalizeKeyHex(payload?.origin) ?? null,
      ts: payload?.ts ?? null,
      message,
    };
  }

  _sigBase(payload) {
    return stableStringify(this._sigPayload(payload));
  }

  _attachSig(payload) {
    if (!payload || typeof payload !== 'object') return false;
    if (!this.peer?.wallet || typeof this.peer.wallet.sign !== 'function') return false;
    const msg = this._sigBase(payload);
    let sig = null;
    try {
      sig = this.peer.wallet.sign(b4a.from(msg));
    } catch (_e) {
      return false;
    }
    let sigHex = '';
    if (typeof sig === 'string') {
      sigHex = sig.trim();
    } else if (sig && sig.length > 0) {
      sigHex = b4a.toString(sig, 'hex');
    }
    if (!sigHex) return false;
    payload.sig = sigHex.toLowerCase();
    if (this.debug) {
      const control = payload?.message?.control;
      if (control !== 'auth' && control !== 'welcome') {
        const hash = sha256Hex(msg);
        console.log(
          `[sidechannel:${payload?.channel ?? 'unknown'}] sign hash=${hash} sigLen=${payload.sig.length}`
        );
      }
    }
    return true;
  }

  _verifySig(payload, pubkeyHex) {
    const sigHex = payload?.sig || payload?.signature;
    if (typeof sigHex !== 'string' || !/^[0-9a-fA-F]{128}$/.test(sigHex)) return false;
    if (typeof pubkeyHex !== 'string' || !/^[0-9a-fA-F]{64}$/.test(pubkeyHex)) return false;
    let sigBuf = null;
    let pubBuf = null;
    try {
      sigBuf = b4a.from(sigHex, 'hex');
      pubBuf = b4a.from(String(pubkeyHex).trim().toLowerCase(), 'hex');
    } catch (_e) {
      return false;
    }
    const msg = this._sigBase(payload);
    return PeerWallet.verify(sigBuf, b4a.from(msg), pubBuf);
  }

  _payloadStructureValid(payload, channel) {
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return false;
    if (payload.type !== 'sidechannel') return false;
    if (normalizeChannel(payload.channel) !== normalizeChannel(channel)) return false;
    if (
      typeof payload.id !== 'string'
      || payload.id.length === 0
      || b4a.byteLength(payload.id, 'utf8') > MAX_MESSAGE_ID_BYTES
    ) {
      return false;
    }
    if (!Number.isSafeInteger(payload.ts)) return false;
    if (!Number.isInteger(payload.ttl) || payload.ttl < 0 || payload.ttl > this.relayTtl) {
      return false;
    }
    if (payload.relayedBy !== undefined && payload.relayedBy !== null) {
      const relayedBy = normalizeKeyHex(payload.relayedBy);
      if (!relayedBy || !/^[0-9a-f]{64}$/.test(relayedBy)) return false;
    }
    return true;
  }

  verifyPayload(payload, expectedKey = payload?.from) {
    const author = normalizeKeyHex(payload?.from);
    const origin = normalizeKeyHex(payload?.origin);
    const expected = normalizeKeyHex(expectedKey);
    return (
      !!author
      && /^[0-9a-f]{64}$/.test(author)
      && origin === author
      && author === expected
      && this._verifySig(payload, expected)
    );
  }

  _registerChannel(name) {
    const channel = String(name || '').trim();
    if (!channel) return null;
    if (this.channels.has(channel)) return this.channels.get(channel);
    if (b4a.byteLength(channel, 'utf8') > this.maxChannelNameBytes) return null;
    if (this.channels.size >= this.maxChannels) return null;
    if (this._inviteRequired(channel)) {
      const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
      const selfIsInviter = this.inviterKeys && selfKey && this.inviterKeys.has(selfKey);
      if (!selfIsInviter && !this._isLocallyInvited(channel)) {
        console.log(`[sidechannel:${channel}] join denied (invite required).`);
        return null;
      }
    }
    const entry = {
      name: channel,
      topic: toTopic(channel),
      protocol: toProtocol(channel),
      swarmJoined: false,
      announced: false,
      _announceWaiters: null,
      _announcing: null,
    };
    this.channels.set(channel, entry);
    return entry;
  }

  async _flushSwarm(context) {
    if (typeof this.peer?.swarm?.flush !== 'function') return true;
    let timer = null;
    const flush = Promise.resolve()
      .then(() => this.peer.swarm.flush())
      .then(() => true)
      .catch((error) => {
        this._reportEventError(context, error);
        return false;
      });
    const bounded = new Promise((resolve) => {
      timer = setTimeout(() => {
        this._reportEventError(
          context,
          new Error(`swarm flush did not complete within ${this.flushTimeoutMs}ms`)
        );
        resolve(false);
      }, this.flushTimeoutMs);
    });
    try {
      return await Promise.race([flush, bounded]);
    } finally {
      if (timer !== null) clearTimeout(timer);
    }
  }

  _markAnnounced(entry, announced = true) {
    if (!entry) return;
    entry.announced = announced === true;
    const waiters = entry._announceWaiters;
    entry._announceWaiters = null;
    if (!waiters) return;
    for (const resolve of waiters) resolve(entry.announced);
  }

  _whenAnnounced(entry) {
    if (!entry) return Promise.resolve(false);
    if (entry.announced) return Promise.resolve(true);
    if (!entry._announceWaiters) entry._announceWaiters = [];
    return new Promise((resolve) => entry._announceWaiters.push(resolve));
  }

  async _announceChannels(entries) {
    if (!this.peer?.swarm) return entries.map(() => false);
    const fresh = entries.filter((entry) => (
      entry &&
      this.channels.get(entry.name) === entry &&
      !entry.announced &&
      !entry._announcing
    ));
    let batch = null;
    if (fresh.length > 0) {
      batch = (async () => {
        let ok = true;
        try {
          for (const entry of fresh) {
            if (!entry.swarmJoined) {
              this.peer.swarm.join(entry.topic, { server: true, client: true });
              entry.swarmJoined = true;
            }
          }
          ok = await this._flushSwarm(
            `flush after joining ${fresh.length} sidechannel(s)`
          );
        } catch (error) {
          ok = false;
          this._reportEventError(`announce ${fresh.length} sidechannel(s)`, error);
        }
        for (const entry of fresh) {
          this._markAnnounced(
            entry,
            ok && entry.swarmJoined && this.channels.get(entry.name) === entry
          );
        }
      })();
      for (const entry of fresh) entry._announcing = batch;
    }

    const waiting = entries
      .map((entry) => entry?._announcing)
      .filter(Boolean);
    try {
      await Promise.all(waiting);
    } finally {
      if (batch) {
        for (const entry of fresh) {
          if (entry._announcing === batch) entry._announcing = null;
        }
      }
    }
    return entries.map((entry) => (
      !!entry && this.channels.get(entry.name) === entry && entry.announced === true
    ));
  }

  _announceChannel(entry) {
    return this._announceChannels([entry]).then(([announced]) => announced);
  }

  _sendWelcome(record, entry, connection) {
    const welcome = this._getConfiguredWelcome(entry.name);
    if (!welcome) return;
    const ownerKey = this._getOwnerKey(entry.name);
    const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
    if (!ownerKey || !selfKey || ownerKey !== selfKey) return;
    // For invite-only channels, don't send plaintext control payloads to unauthorized peers.
    if (connection && this._inviteRequired(entry.name)) {
      const remoteKey = this._getRemoteKey(connection);
      const remoteIsInviter = this.inviterKeys && this.inviterKeys.has(remoteKey);
      if (!remoteIsInviter && !this._isInvited(entry.name, remoteKey)) return;
    }
    if (!record?.message) return;
    const payload = this._buildPayload(entry.name, { control: 'welcome', welcome });
    this._rememberSeen(payload.id, this._now());
    record.message.send(payload);
  }

  _sendAuth(record, entry) {
    if (!record?.message || record.authSent) return;
    if (!this._inviteRequired(entry.name)) return;
    const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
    const selfIsInviter = this.inviterKeys && selfKey && this.inviterKeys.has(selfKey);
    if (selfIsInviter) return;
    if (!this._isLocallyInvited(entry.name)) return;
    const invite = this.localInviteObjects.get(normalizeChannel(entry.name));
    if (!invite) return;
    const payload = this._buildPayload(entry.name, {
      control: 'auth',
      invite,
    });
    this._rememberSeen(payload.id, this._now());
    record.message.send(payload);
    record.authSent = true;
  }

  _remoteAuthorized(channel, connection) {
    if (!this._inviteRequired(channel)) return true;
    const remoteKey = this._getRemoteKey(connection);
    if (this.inviterKeys && this.inviterKeys.has(remoteKey)) return true;
    return this._isInvited(channel, remoteKey);
  }

  _openChannelForConnection(connection, entry) {
    try {
      return this._openChannelForConnectionUnchecked(connection, entry);
    } catch (error) {
      this._reportEventError(`open ${entry?.name ?? 'unknown channel'}`, error, connection);
      return false;
    }
  }

  _openChannelForConnectionUnchecked(connection, entry) {
    if (this.closedConnections.has(connection)) return;
    const mux = connection.userData;
    if (!mux || typeof mux.createChannel !== 'function') {
      const tries = (connection.__sidechannelMuxTries || 0) + 1;
      connection.__sidechannelMuxTries = tries;
      if (tries <= this.muxRetryMax) {
        setTimeout(() => {
          if (!this.closedConnections.has(connection)) {
            this._openChannelForConnection(connection, entry);
          }
        }, this.muxRetryDelayMs);
      } else if (this.debug) {
        console.log(`[sidechannel:${entry.name}] mux not ready for connection.`);
      }
      return;
    }

    let perConn = this.connections.get(connection);
    if (!perConn) {
      perConn = new Map();
      this.connections.set(connection, perConn);
    }
    if (perConn.has(entry.name)) return;
    if (!perConn._paired) perConn._paired = new Set();
    // Track open retries per connection+channel to avoid infinite retry loops
    // when the remote peer hasn't joined/paired the protocol yet.
    if (!perConn._openRetries) perConn._openRetries = new Map();
    if (!perConn._paired.has(entry.protocol)) {
      perConn._paired.add(entry.protocol);
      if (typeof mux.pair === 'function') {
        mux.pair({ protocol: entry.protocol }, () => {
          this._openChannelForConnection(connection, entry);
        });
      }
    }

    if (this.debug) {
      const remoteKey = connection?.remotePublicKey
        ? b4a.toString(connection.remotePublicKey, 'hex')
        : 'unknown';
      console.log(`[sidechannel:${entry.name}] opening channel for ${remoteKey}`);
    }

    let record = null;
    const channel = mux.createChannel({
      protocol: entry.protocol,
      onopen: () => {
        if (record?.openTimer) clearTimeout(record.openTimer);
      },
      onclose: () => {
        if (record?.openTimer) clearTimeout(record.openTimer);
        if (perConn.get(entry.name) === record) perConn.delete(entry.name);
      }
    });
    if (!channel) {
      if (this.debug) {
        console.log(`[sidechannel:${entry.name}] channel already open or closed.`);
      }
      return;
    }

    const message = channel.addMessage({
      encoding: boundedJsonEncoding(
        this._maxMessageBytes(entry.name),
        `Sidechannel ${entry.name} message`,
        { maxStringBytes: this._maxStringBytes(entry.name) }
      ),
      onmessage: (payload) => {
        try {
          if (this._isBlocked(connection)) return;
          let payloadBytes = decodedJsonByteLength(payload);
          if (decodedJsonWasRejected(payload)) {
            this._checkRate(connection, payloadBytes ?? this._maxMessageBytes(entry.name));
            return;
          }
          if (payloadBytes === null) {
            let payloadJson = null;
            try {
              payloadJson = JSON.stringify(payload);
            } catch (_e) {
              return;
            }
            payloadBytes = b4a.byteLength(payloadJson, 'utf8');
          }
          const maxMessageBytes = this._maxMessageBytes(entry.name);
          if (payloadBytes > maxMessageBytes) {
            this._checkRate(connection, payloadBytes);
            if (this.debug) {
              console.log(
                `[sidechannel:${entry.name}] drop (message too large: ${payloadBytes} > ${maxMessageBytes})`
              );
            }
            return;
          }
          if (!this._checkRate(connection, payloadBytes)) {
            if (this.debug) {
              console.log(`[sidechannel:${entry.name}] drop (rate limit) from ${this._getRemoteKey(connection)}`);
            }
            return;
          }
          if (!this._payloadStructureValid(payload, entry.name)) {
            if (this.debug) {
              console.log(
                `[sidechannel:${entry.name}] drop (invalid envelope) from ${this._getRemoteKey(connection)}`
              );
            }
            return;
          }
          if (this.debug) {
            console.log(
              `[sidechannel:${entry.name}] recv ${payloadBytes} bytes from ${this._getRemoteKey(connection)}`
            );
          }
          if (!this._checkInvite(payload, entry.name, connection)) {
            if (this.debug) {
              console.log(`[sidechannel:${entry.name}] drop (invite) from ${this._getRemoteKey(connection)}`);
            }
            return;
          }
          if (!this._checkPow(payload, entry.name)) {
            if (this.debug) {
              console.log(`[sidechannel:${entry.name}] drop (invalid pow) from ${this._getRemoteKey(connection)}`);
            }
            return;
          }
          if (!this.verifyPayload(payload)) {
            this.relayCounters.unauthenticated_drops += 1;
            if (this.debug) {
              console.log(
                `[sidechannel:${entry.name}] drop (unauthenticated sender) from ${this._getRemoteKey(connection)}`
              );
            }
            return;
          }


          // Allow a minimal auth handshake even on owner-only channels so invite-only + owner-only
          // channels can authorize listeners without giving them write access.
          const controlEarly = payload?.message?.control;
          const isAuthControl = controlEarly === 'auth';
          const isWelcomeControl = controlEarly === 'welcome';
          if (this._ownerWriteOnly(entry.name) && !isAuthControl && !isWelcomeControl) {
            const ownerKey = this._getOwnerKey(entry.name);
            const author = normalizeKeyHex(payload?.from);
            // NOTE: payload.from is user-supplied; verify message signature to prevent spoofing.
            const sigOk = ownerKey ? this._verifySig(payload, ownerKey) : false;
            if (!ownerKey || !author || author !== ownerKey || !sigOk) {
              if (this.debug) {
                const sigHex = payload?.sig || payload?.signature || '';
                const hash = sha256Hex(this._sigBase(payload));
                console.log(
                  `[sidechannel:${entry.name}] drop (owner-only) author=${author} owner=${ownerKey} sigOk=${sigOk} sigLen=${sigHex.length} hash=${hash} fromRemote=${this._getRemoteKey(connection)}`
                );
              }
              return;
            }
          }
          const payloadId = payload.id;
          const now = this._now();
          if (this._rememberSeen(payloadId, now)) {
            if (this.debug) {
              console.log(`[sidechannel:${entry.name}] drop (duplicate) ${payloadId}`);
            }
            return;
          }
          const control = payload?.message?.control;
          const requestedChannel = payload?.message?.channel;
          const isWelcome = this._isWelcomeMessage(payload);
          const embeddedWelcome = this._extractWelcome(payload);
          let welcomeOk = false;
          if (embeddedWelcome) {
            welcomeOk = this._verifyWelcome(embeddedWelcome, entry.name, connection);
            if (!welcomeOk && isWelcome) {
              if (this.debug) {
                console.log(`[sidechannel:${entry.name}] drop (invalid welcome) from ${this._getRemoteKey(connection)}`);
              }
              return;
            }
          } else if (isWelcome) {
            if (this.debug) {
              console.log(`[sidechannel:${entry.name}] drop (missing welcome) from ${this._getRemoteKey(connection)}`);
            }
            return;
          }
          if (this._welcomeRequired(entry.name) && !this._isWelcomed(entry.name) && !welcomeOk) {
            if (this.debug) {
              console.log(`[sidechannel:${entry.name}] drop (awaiting welcome) from ${this._getRemoteKey(connection)}`);
            }
            return;
          }
          if (control === 'open_channel' && this.allowRemoteOpen && typeof requestedChannel === 'string') {
            const target = requestedChannel.trim();
            if (target.length > 0) {
              const welcome = payload?.message?.welcome || payload?.message?.invite?.welcome;
              if (welcome) {
                if (!this._verifyWelcome(welcome, target, connection)) {
                  if (this.debug) {
                    console.log(`[sidechannel] open denied (welcome) for ${target} from ${this._getRemoteKey(connection)}`);
                  }
                  return;
                }
              } else if (this._welcomeRequired(target)) {
                if (this.debug) {
                  console.log(
                    `[sidechannel] open denied (missing welcome) for ${target} from ${this._getRemoteKey(connection)}`
                  );
                }
                return;
              }
              if (this._inviteRequired(target)) {
                const invite = payload?.message?.invite;
                if (!invite || !this._verifyInvite(invite, target, connection)) {
                  if (this.debug) {
                    console.log(`[sidechannel] open denied (invite) for ${target} from ${this._getRemoteKey(connection)}`);
                  }
                  return;
                }
              }
              if (this.autoJoinOnOpen) {
                this.addChannel(target).catch((error) => {
                  this._reportEventError(`auto-join ${target}`, error, connection);
                });
                console.log(`[sidechannel] auto-joined channel: ${target}`);
              } else {
                console.log(`[sidechannel] channel request received: ${target}`);
              }
            }
          } else {
            // Security: sidechannel handshake frames are transport control, never app content.
            if (control === 'auth' || control === 'welcome') return;
            if (this._ownerWriteOnly(entry.name)) {
              const ownerKey = this._getOwnerKey(entry.name);
              const author = normalizeKeyHex(payload?.from);
              if (!ownerKey || !author || author !== ownerKey || !this._verifySig(payload, ownerKey)) {
                if (this.debug) {
                  console.log(
                    `[sidechannel:${entry.name}] drop (owner-only content) from ${this._getRemoteKey(connection)}`
                  );
                }
                return;
              }
            }
            if (this.onMessage) {
              const handled = this.onMessage(entry.name, payload, connection);
              if (handled && typeof handled.catch === 'function') {
                handled.catch((error) => {
                  this._reportEventError(`message handler ${entry.name}`, error, connection);
                });
              }
            } else {
              const from = payload?.from ?? 'unknown';
              const msg = payload?.message ?? payload;
              console.log(`[sidechannel:${entry.name}] ${from}:`, msg);
            }
          }
          this._relay(entry.name, payload, connection);
        } catch (error) {
          this._reportEventError(`incoming message ${entry.name}`, error, connection);
        }
      }
    });

    record = { channel, message, retries: 0, authSent: false, openTimer: null };
    perConn.set(entry.name, record);

    record.openTimer = setTimeout(() => {
      if (perConn.get(entry.name) !== record || record.channel?.opened === true) return;
      try {
        record.channel?.close?.();
      } catch (_e) {}
      perConn.delete(entry.name);
      if (this.debug) {
        console.log(`[sidechannel:${entry.name}] open timed out for ${this._getRemoteKey(connection)}`);
      }
    }, this.channelOpenTimeoutMs);

    channel.open();
    channel
      .fullyOpened()
      .then((opened) => {
        if (
          this.closedConnections.has(connection) ||
          perConn.get(entry.name) !== record ||
          this.channels.get(entry.name) !== entry
        ) {
          return;
        }
        if (this.debug) {
          console.log(
            `[sidechannel:${entry.name}] channel open=${opened} for ${this._getRemoteKey(connection)}`
          );
        }
        if (opened) {
          if (record.openTimer) clearTimeout(record.openTimer);
          if (perConn._openRetries) perConn._openRetries.delete(entry.name);
          this._sendWelcome(record, entry, connection);
          this._sendAuth(record, entry);
          return;
        }
        if (record.openTimer) clearTimeout(record.openTimer);
        const now = this._now();
        const state = perConn._openRetries?.get(entry.name) || { count: 0, lastAt: 0 };
        // If the last attempt was a while ago, start a fresh retry burst.
        const baseCount = now - (state.lastAt || 0) > this.openRetryResetMs
          ? 0
          : Number(state.count) || 0;
        const retryCount = baseCount + 1;
        if (perConn._openRetries) {
          perConn._openRetries.set(entry.name, { count: retryCount, lastAt: now });
        }
        if (retryCount <= this.openRetryMax) {
          try {
            record?.channel?.close?.();
          } catch (_e) {}
          perConn.delete(entry.name);
          setTimeout(
            () => {
              if (!this.closedConnections.has(connection)) {
                this._openChannelForConnection(connection, entry);
              }
            },
            this.openRetryBaseMs * retryCount
          );
          return;
        }
        if (this.debug) {
          console.log(`[sidechannel:${entry.name}] giving up (open retries exceeded) for ${this._getRemoteKey(connection)}`);
        }
        try {
          record?.channel?.close?.();
        } catch (_e) {}
        perConn.delete(entry.name);
      })
      .catch((error) => {
        if (record?.openTimer) clearTimeout(record.openTimer);
        this._reportEventError(`open ${entry.name}`, error, connection);
      });
  }

  async addChannels(names) {
    const entries = [...new Set(
      Array.from(names || [], (name) => normalizeChannel(name)).filter(Boolean)
    )].map((name) => this._registerChannel(name));
    if (entries.length === 0 || entries.some((entry) => !entry)) return null;
    const announced = this.started
      ? await this._announceChannels(entries)
      : await Promise.all(entries.map((entry) => this._whenAnnounced(entry)));
    if (announced.some((ok) => !ok)) return null;

    for (const entry of entries) {
      if (this.channels.get(entry.name) !== entry) return null;
      for (const connection of this.connections.keys()) {
        this._openChannelForConnection(connection, entry);
      }
    }
    return entries.map((entry) => entry.name);
  }

  async addChannel(name) {
    const joined = await this.addChannels([name]);
    return joined?.length === 1;
  }

  _directPeerChannelReady(remote, channel) {
    const target = normalizeKeyHex(remote);
    if (!target) return false;
    for (const [connection, perConn] of this.connections) {
      if (this._getRemoteKey(connection) !== target) continue;
      const record = perConn.get(channel);
      if (record?.message && record.channel?.opened === true) return true;
    }
    return false;
  }

  async connectDirectPeer(remote, channel, waitMs = 15_000) {
    const target = normalizeKeyHex(remote);
    const entry = this._registerChannel(channel);
    if (!target || !entry || typeof this.peer?.swarm?.joinPeer !== 'function') return false;
    if (this._directPeerChannelReady(target, entry.name)) return true;

    try {
      this.peer.swarm.joinPeer(b4a.from(target, 'hex'));
    } catch (_error) {
      return false;
    }

    const maxWaitMs = Math.max(
      1,
      Math.min(Number(waitMs) || 15_000, this.directConnectMaxWaitMs)
    );
    const deadline = Date.now() + maxWaitMs;
    while (Date.now() < deadline) {
      for (const connection of this.peer.swarm.connections || []) {
        if (this._getRemoteKey(connection) === target) {
          this._openChannelForConnection(connection, entry);
        }
      }
      if (this._directPeerChannelReady(target, entry.name)) return true;
      await new Promise((resolve) => setTimeout(resolve, this.directConnectPollMs));
    }
    return false;
  }

  async removeChannel(name) {
    const channel = String(name || '').trim();
    if (!channel) return false;
    if (this._isEntry(channel)) return false; // Entry rendezvous is global; do not leave it dynamically.
    const entry = this.channels.get(channel);
    if (!entry) return false;

    // Close mux protocol channels for this topic across all active connections.
    for (const [, perConn] of this.connections.entries()) {
      const record = perConn.get(entry.name);
      if (record) {
        try {
          record?.channel?.close?.();
        } catch (_e) {}
        perConn.delete(entry.name);
      }
      try {
        perConn?._openRetries?.delete?.(entry.name);
      } catch (_e) {}
      try {
        perConn?._paired?.delete?.(entry.protocol);
      } catch (_e) {}
    }

    const normalized = normalizeChannel(entry.name);
    const wasJoined = entry.swarmJoined === true;
    entry.swarmJoined = false;
    this._markAnnounced(entry, false);

    // Drop in-memory per-channel state to avoid unbounded growth from ephemeral channels.
    this.channels.delete(entry.name);
    this.invitedPeers.delete(entry.name);
    this.localInvites.delete(normalized);
    this.localInviteObjects.delete(normalized);
    this.welcomeByChannel.delete(normalized);
    this.welcomedChannels.delete(normalized);

    // Best-effort: stop swarm discovery for the topic if supported.
    if (wasJoined && this.peer?.swarm) {
      try {
        if (typeof this.peer.swarm.leave === 'function') {
          this.peer.swarm.leave(entry.topic);
        }
      } catch (_e) {}
      try {
        await this._flushSwarm(`flush after leaving ${entry.name}`);
      } catch (_e) {}
    }

    return true;
  }

  acceptInvite(name, invite = null, welcome = null) {
    const channel = String(name || '').trim();
    if (!channel) return false;
    if (invite) {
      this._acceptLocalInvite(invite, channel);
      if (invite?.welcome) {
        this._verifyWelcome(invite.welcome, channel, null);
      }
    }
    if (welcome) {
      this._verifyWelcome(welcome, channel, null);
    }
    return true;
  }

  _payloadWithinBounds(channel, payload) {
    let shapeWithinBounds = false;
    try {
      shapeWithinBounds = jsonShapeWithinBounds(
        payload,
        undefined,
        undefined,
        this._maxStringBytes(channel)
      );
    } catch (_error) {
      shapeWithinBounds = false;
    }
    if (!shapeWithinBounds) {
      console.log(`[sidechannel:${channel}] message exceeds JSON shape or string bounds.`);
      return false;
    }
    let payloadJson = null;
    try {
      payloadJson = JSON.stringify(payload);
    } catch (_e) {
      console.log(`[sidechannel:${channel}] message rejected (non-serializable payload).`);
      return false;
    }
    const payloadBytes = b4a.byteLength(payloadJson, 'utf8');
    const maxMessageBytes = this._maxMessageBytes(channel);
    if (payloadBytes > maxMessageBytes) {
      console.log(
        `[sidechannel:${channel}] message too large (${payloadBytes} bytes > ${maxMessageBytes}).`
      );
      return false;
    }
    return true;
  }

  _sendBroadcastPayload(entry, payload, allowPreAuthSend) {
    const channel = entry.name;
    if (this.debug) {
      console.log(`[sidechannel:${channel}] sending to ${this.connections.size} connections`);
    }
    this._rememberSeen(payload.id, this._now());
    for (const [connection, perConn] of this.connections.entries()) {
      if (!allowPreAuthSend && !this._remoteAuthorized(channel, connection)) {
        if (this.debug) {
          console.log(`[sidechannel:${channel}] skip (unauthorized) ${this._getRemoteKey(connection)}`);
        }
        continue;
      }
      const record = perConn.get(channel);
      if (record?.message) {
        if (!record.channel?.opened) {
          record.channel
            ?.fullyOpened()
            .then((opened) => {
              if (
                opened &&
                !this.closedConnections.has(connection) &&
                this.connections.get(connection)?.get(channel) === record &&
                this.channels.get(channel) === entry
              ) {
                record.message.send(payload);
              }
            })
            .catch((error) => {
              this._reportEventError(`deferred send ${channel}`, error, connection);
            });
        } else {
          try {
            record.message.send(payload);
          } catch (error) {
            this._reportEventError(`send ${channel}`, error, connection);
          }
        }
      } else if (this.debug) {
        console.log(`[sidechannel:${channel}] no message session for connection.`);
      }
    }
  }

  _queuePowBroadcast(entry, payload, allowPreAuthSend) {
    if (this.powQueueDepth >= this.maxPowQueue) return false;
    this.powQueueDepth += 1;
    const mineAndSend = async () => {
      try {
        await this._attachPowCooperatively(payload);
        this._attachSig(payload);
        if (this.channels.get(entry.name) !== entry) return;
        if (!this._payloadWithinBounds(entry.name, payload)) return;
        this._sendBroadcastPayload(entry, payload, allowPreAuthSend);
      } catch (error) {
        this._reportEventError(`PoW broadcast ${entry.name}`, error);
      } finally {
        this.powQueueDepth -= 1;
      }
    };
    this.powQueue = this.powQueue.then(mineAndSend, mineAndSend);
    return true;
  }

  broadcast(name, message, options = {}) {
    const channel = String(name || '').trim();
    if (!channel) return false;
    const isAuthControl =
      message && typeof message === 'object' && String(message.control || '') === 'auth';
    const allowPreAuthSend = isAuthControl;
    if (this._ownerWriteOnly(channel) && !isAuthControl) {
      const ownerKey = this._getOwnerKey(channel);
      const selfKey = normalizeKeyHex(this.peer?.wallet?.publicKey);
      if (!ownerKey || !selfKey || ownerKey !== selfKey) return false;
    }
    if (options.invite) {
      this._acceptLocalInvite(options.invite, channel);
      if (options.invite?.welcome) {
        this._verifyWelcome(options.invite.welcome, channel, null);
      }
    }
    const entry = this._registerChannel(channel);
    if (!entry) return false;
    if (this.peer?.swarm?.connections) {
      for (const connection of this.peer.swarm.connections) {
        this._openChannelForConnection(connection, entry);
      }
    }
    const cooperativePow = this._powRequired(channel, message);
    const payload = this._buildPayload(
      channel,
      message,
      options.invite,
      { deferPow: cooperativePow }
    );
    if (cooperativePow) {
      const upperBoundPayload = {
        ...payload,
        pow: {
          nonce: Number.MAX_SAFE_INTEGER,
          difficulty: this.powDifficulty,
        },
        sig: '0'.repeat(128),
      };
      if (!this._payloadWithinBounds(channel, upperBoundPayload)) return false;
      return this._queuePowBroadcast(entry, payload, allowPreAuthSend);
    }
    if (!this._payloadWithinBounds(channel, payload)) return false;
    this._sendBroadcastPayload(entry, payload, allowPreAuthSend);
    return true;
  }

  async start() {
    if (this.started) return;
    if (!this._startPromise) {
      const generation = ++this._startGeneration;
      let startPromise = null;
      startPromise = this._start(generation)
        .catch((error) => {
          if (generation === this._startGeneration) {
            this.started = false;
            for (const entry of this.channels.values()) this._markAnnounced(entry, false);
          }
          throw error;
        })
        .finally(() => {
          if (this._startPromise === startPromise) this._startPromise = null;
        });
      this._startPromise = startPromise;
    }
    return this._startPromise;
  }

  async _start(generation) {
    if (!this.peer?.swarm) {
      throw new Error('Sidechannel requires peer.swarm to be initialized.');
    }

    // Hyperswarm can accept `join()` calls before the DHT is fully bootstrapped.
    // In practice this can lead to missed announces/lookups and permanent
    // non-discovery until restart. `fullyBootstrapped()` is the authoritative
    // readiness barrier, so wait for it before joining any sidechannel topic.
    const dht = this.peer.swarm.dht;
    let bootPromise = null;
    if (dht && typeof dht.fullyBootstrapped === 'function') {
      if (this.debug) console.log('[sidechannel] waiting for DHT bootstrap...');
      bootPromise = Promise.resolve()
        .then(() => dht.fullyBootstrapped());
      await bootPromise;
    }
    if (generation !== this._startGeneration) return false;
    this._dhtBootPromise = bootPromise;

    if (!this._connectionListenerAttached) {
      this.peer.swarm.on('connection', (connection) => this._prepareConnection(connection));
      this._connectionListenerAttached = true;
    }

    const initial = [];
    for (const entry of this.channels.values()) {
      if (!entry.swarmJoined) {
        this.peer.swarm.join(entry.topic, { server: true, client: true });
        entry.swarmJoined = true;
      }
      initial.push(entry);
    }
    const initialFlushed = await this._flushSwarm('startup swarm flush');
    if (generation !== this._startGeneration) return false;
    for (const entry of initial) {
      if (this.channels.get(entry.name) === entry) {
        this._markAnnounced(entry, initialFlushed && entry.swarmJoined);
      }
    }
    if (!initialFlushed) throw new Error('Sidechannel startup could not confirm channel discovery.');
    this.started = true;

    const stragglers = [];
    for (const entry of this.channels.values()) {
      if (!entry.announced && !entry._announcing) stragglers.push(entry);
    }
    if (stragglers.length > 0) {
      const announced = await this._announceChannels(stragglers);
      if (generation !== this._startGeneration) return false;
      if (announced.some((ok) => !ok)) {
        this.started = false;
        throw new Error('Sidechannel startup could not confirm every joined channel.');
      }
    }

    if (this.peer.swarm.connections) {
      for (const connection of this.peer.swarm.connections) {
        this._prepareConnection(connection);
      }
    }
  }

  _prepareConnection(connection) {
    if (!connection || this.closedConnections.has(connection) || this._isBlocked(connection)) return;
    if (!this.preparedConnections.has(connection)) {
      this.preparedConnections.add(connection);
      connection.on('close', () => this._dropConnection(connection));
    }
    for (const entry of this.channels.values()) {
      this._openChannelForConnection(connection, entry);
    }
  }

  _dropConnection(connection) {
    this.closedConnections.add(connection);
    const perConn = this.connections.get(connection);
    if (perConn) {
      for (const record of perConn.values()) {
        if (record?.openTimer) clearTimeout(record.openTimer);
      }
    }
    this.connections.delete(connection);
    this.rateLimits.delete(connection);
  }

  async stop() {
    this._startGeneration += 1;
    this._startPromise = null;
    this.started = false;
    this._dhtBootPromise = null;
    for (const entry of this.channels.values()) {
      const wasJoined = entry.swarmJoined === true;
      entry.swarmJoined = false;
      this._markAnnounced(entry, false);
      if (wasJoined && typeof this.peer?.swarm?.leave === 'function') {
        try {
          this.peer.swarm.leave(entry.topic);
        } catch (_error) {}
      }
    }
    for (const connection of this.connections.keys()) this._dropConnection(connection);
    this.connections.clear();
    this.rateLimits.clear();
    this.relaySourceLimits.clear();
  }
}

export default Sidechannel;
