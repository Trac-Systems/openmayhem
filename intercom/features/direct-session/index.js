import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';
import crypto from 'crypto';
import Protomux from 'protomux';
import {
  DEFAULT_MAX_JSON_STRING_BYTES,
  boundedJsonEncoding,
  decodedJsonByteLength,
  decodedJsonWasRejected,
  jsonShapeWithinBounds,
} from '../bounded-json.js';

const SESSION_PROTOCOL = 'mx/s';
const SESSION_CHANNEL_PREFIX = 'mx/s/';
const HEALTH_PROTOCOL = 'mx/s-health';
const DEFAULT_MAX_FRAME_BYTES = 256 * 1024;
const DEFAULT_RATE_BYTES_PER_SECOND = 1_000_000;
const DEFAULT_RATE_BURST_BYTES = 1_000_000;
const DEFAULT_RECEIVE_BATCH_HEADROOM_BYTES = 64 * 1024 * 1024;
const DEFAULT_SEND_DRAIN_TIMEOUT_MS = 0;
const DEFAULT_CONNECT_MAX_WAIT_MS = 120_000;
const DEFAULT_CONNECT_POLL_MS = 100;
const DEFAULT_OPEN_MAX_WAIT_MS = 120_000;
const DEFAULT_HEALTH_INTERVAL_MS = 5_000;
const DEFAULT_HEALTH_FRESH_MS = 15_000;
const DEFAULT_HEALTH_TIMEOUT_MS = 30_000;
const DEFAULT_MAX_SESSIONS = 1024;
const DEFAULT_MAX_SESSIONS_PER_CONNECTION = 128;
const MAX_FRAME_TYPE_BYTES = 64;
const MAX_HEALTH_FRAME_BYTES = 256;
const MAX_PENDING_HEALTH_PROBES = 32;

const normalizeKeyHex = (value) => {
  if (!value) return null;
  if (b4a.isBuffer(value)) return b4a.toString(value, 'hex');
  if (typeof value === 'string') return value.trim().toLowerCase();
  if (typeof value === 'object' && value.type === 'Buffer' && Array.isArray(value.data)) {
    try {
      return b4a.toString(b4a.from(value.data), 'hex');
    } catch (_e) {
      return null;
    }
  }
  return String(value).trim().toLowerCase();
};

const normalizeSessionId = (value) => {
  const sessionId = String(value || '').trim().toLowerCase();
  return /^[0-9a-f]{64}$/.test(sessionId) ? sessionId : null;
};

const sessionKey = (remote, sessionId) => `${remote}:${sessionId}`;

const safeIntegerOr = (value, fallback, { min = 0 } = {}) => (
  Number.isSafeInteger(value) && value >= min ? value : fallback
);

class DirectSession extends Feature {
  constructor(peer, config = {}) {
    super(peer, config);
    this.key = 'direct-session';
    this.started = false;
    this.debug = config.debug === true;
    this.maxFrameBytes = safeIntegerOr(config.maxFrameBytes, DEFAULT_MAX_FRAME_BYTES, { min: 1 });
    this.maxStringBytes = safeIntegerOr(
      config.maxStringBytes,
      Math.min(this.maxFrameBytes, DEFAULT_MAX_JSON_STRING_BYTES),
      { min: 1 }
    );
    this.rateBytesPerSecond = safeIntegerOr(
      config.rateBytesPerSecond,
      DEFAULT_RATE_BYTES_PER_SECOND
    );
    this.rateBurstBytes = Math.max(
      this.maxFrameBytes,
      safeIntegerOr(config.rateBurstBytes, DEFAULT_RATE_BURST_BYTES)
    );
    this.receiveBatchHeadroomBytes = Math.min(
      DEFAULT_RECEIVE_BATCH_HEADROOM_BYTES,
      safeIntegerOr(
        config.receiveBatchHeadroomBytes,
        DEFAULT_RECEIVE_BATCH_HEADROOM_BYTES
      )
    );
    this.sendDrainTimeoutMs = safeIntegerOr(
      config.sendDrainTimeoutMs,
      DEFAULT_SEND_DRAIN_TIMEOUT_MS,
      { min: 0 }
    );
    this.connectMaxWaitMs = safeIntegerOr(
      config.connectMaxWaitMs,
      DEFAULT_CONNECT_MAX_WAIT_MS,
      { min: 1 }
    );
    this.connectPollMs = safeIntegerOr(config.connectPollMs, DEFAULT_CONNECT_POLL_MS, { min: 1 });
    this.openMaxWaitMs = safeIntegerOr(
      config.openMaxWaitMs,
      DEFAULT_OPEN_MAX_WAIT_MS,
      { min: 1 }
    );
    this.healthIntervalMs = safeIntegerOr(
      config.healthIntervalMs,
      DEFAULT_HEALTH_INTERVAL_MS,
      { min: 1 }
    );
    this.healthFreshMs = safeIntegerOr(
      config.healthFreshMs,
      DEFAULT_HEALTH_FRESH_MS,
      { min: this.healthIntervalMs }
    );
    this.healthTimeoutMs = safeIntegerOr(
      config.healthTimeoutMs,
      DEFAULT_HEALTH_TIMEOUT_MS,
      { min: 0 }
    );
    this.maxSessions = safeIntegerOr(config.maxSessions, DEFAULT_MAX_SESSIONS, { min: 1 });
    this.maxSessionsPerConnection = safeIntegerOr(
      config.maxSessionsPerConnection,
      DEFAULT_MAX_SESSIONS_PER_CONNECTION,
      { min: 1 }
    );
    this.sessions = new Map();
    this.preparedConnections = new WeakMap();
    this.pairedConnections = new WeakSet();
    this.featureOwnedConnections = new WeakSet();
    this.connectionErrors = new WeakMap();
    this.connectionReceiveLimiters = new WeakMap();
    this.connectionHealth = new Map();
    this.preferredConnections = new Map();
    this.explicitPeers = new Set();
    this.reconnectSuspended = new Set();
    this.lastConnectAttempt = null;
    this.onFrame = typeof config.onFrame === 'function' ? config.onFrame : null;
    this.onClose = typeof config.onClose === 'function' ? config.onClose : null;
    this.transportInfo = typeof config.transportInfo === 'function'
      ? config.transportInfo
      : null;
  }

  start() {
    if (this.started) return;
    if (!this.peer?.swarm) {
      throw new Error('DirectSession requires peer.swarm to be initialized.');
    }
    this.started = true;
    this.peer.swarm.on('connection', (connection) => this._prepareConnection(connection));
    if (this.peer.swarm.connections) {
      for (const connection of this.peer.swarm.connections) {
        this._prepareConnection(connection);
      }
    }
  }

  stats() {
    return {
      started: this.started === true,
      protocol: SESSION_PROTOCOL,
      maxFrameBytes: this.maxFrameBytes,
      maxStringBytes: this.maxStringBytes,
      rateBytesPerSecond: this.rateBytesPerSecond,
      rateBurstBytes: this.rateBurstBytes,
      receiveRateBurstBytes: this._receiveRateBurstBytes(),
      receiveBatchHeadroomBytes: this.receiveBatchHeadroomBytes,
      receiveRateCapacityBytes: this._receiveRateCapacityBytes(),
      sendDrainTimeoutMs: this.sendDrainTimeoutMs,
      connectMaxWaitMs: this.connectMaxWaitMs,
      connectPollMs: this.connectPollMs,
      openMaxWaitMs: this.openMaxWaitMs,
      healthProtocol: HEALTH_PROTOCOL,
      healthIntervalMs: this.healthIntervalMs,
      healthFreshMs: this.healthFreshMs,
      healthTimeoutMs: this.healthTimeoutMs,
      maxSessions: this.maxSessions,
      maxSessionsPerConnection: this.maxSessionsPerConnection,
      preferredConnections: Array.from(this.preferredConnections.keys()),
      reconnectSuspended: Array.from(this.reconnectSuspended),
      sessionCount: this.sessions.size,
      sessions: Array.from(this.sessions.values()).map((session) => ({
        ...this._transportInfo(session.connection, session.remote),
        session_id: session.sessionId,
        channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
        remote: session.remote,
        opened: session.channel?.opened === true,
      })),
      connections: Array.from(this.connectionHealth.values()).map((health) => ({
        remote: this._remoteKey(health.connection),
        explicit: this.explicitPeers.has(this._remoteKey(health.connection)),
        health_capable: health.proven === true,
        healthy: this._healthIsFresh(health),
        last_ack_age_ms: health.lastAckAt > 0 ? Date.now() - health.lastAckAt : null,
        unhealthy_age_ms: health.unhealthySince > 0
          ? Date.now() - health.unhealthySince
          : null,
        opened: health.opened === true,
        initiator: health.connection?.isInitiator === true,
        keep_alive_ms: Number(health.connection?.keepAlive) || 0,
        raw_bytes_read: Number(health.connection?.rawBytesRead) || 0,
        raw_bytes_written: Number(health.connection?.rawBytesWritten) || 0,
      })),
      lastConnectAttempt: this.lastConnectAttempt,
    };
  }

  async open(remote, sessionId) {
    const normalizedRemote = this._normalizeRemote(remote);
    const normalizedSession = normalizeSessionId(sessionId);
    if (!normalizedRemote) throw new Error('Invalid remote peer key.');
    if (!normalizedSession) throw new Error('Invalid session_id.');
    const existing = this.sessions.get(sessionKey(normalizedRemote, normalizedSession));
    if (existing) {
      if (existing.channel?.opened !== true) {
        await this._awaitOpened(existing);
      }
      return this._sessionInfo(existing);
    }
    const connection = this._findConnection(normalizedRemote);
    if (!connection) throw new Error(`No direct connection to ${normalizedRemote}.`);
    const session = this._ensureSession(connection, normalizedSession);
    if (session.channel?.opened !== true) {
      await this._awaitOpened(session);
    }
    return this._sessionInfo(session);
  }

  async connectPeer(remote, waitMs = 10_000) {
    const normalizedRemote = this._normalizeRemote(remote);
    if (!normalizedRemote) throw new Error('Invalid remote peer key.');
    if (!this.peer?.swarm?.joinPeer) {
      throw new Error('Peer swarm does not support targeted peer joins.');
    }
    // Mark the peer explicit even when a transient topic connection already exists.
    // Hyperswarm then maintains the direct connection after either side reconnects.
    const attempt = {
      remote: normalizedRemote,
      started_at: Date.now(),
      state: 'pending',
      error: null,
    };
    this.lastConnectAttempt = attempt;
    const wasExplicit = this.explicitPeers.has(normalizedRemote);
    this.explicitPeers.add(normalizedRemote);
    if (wasExplicit && this._connectionsForRemote(normalizedRemote).length === 0) {
      // joinPeer is a no-op for a peer already parked in Hyperswarm's explicit
      // retry backoff. Refresh that public intent before this caller waits.
      this._rejoinExplicitPeer(normalizedRemote);
    } else {
      this.peer.swarm.joinPeer(b4a.from(normalizedRemote, 'hex'));
    }
    const maxWaitMs = Math.max(1, Math.min(Number(waitMs) || 10_000, this.connectMaxWaitMs));
    const deadline = Date.now() + maxWaitMs;
    const probed = new WeakSet();
    let fallback = null;
    while (Date.now() < deadline) {
      const connections = this._connectionsForRemote(normalizedRemote);
      for (const connection of connections) {
        this._prepareConnection(connection);
        if (probed.has(connection)) {
          const health = this.connectionHealth.get(connection);
          if (this._healthIsFresh(health)) {
            attempt.state = 'connected';
            attempt.verified = true;
            attempt.completed_at = Date.now();
            return this._peerInfo(normalizedRemote, true, connection);
          }
          continue;
        }
        const remaining = Math.max(1, deadline - Date.now());
        // Health verification is advisory. Peers running releases without the
        // mx/s-health protocol never open the channel, and a shared Hyperswarm
        // connection also carries base replication — so a probe failure must never
        // destroy the connection; the connection is kept as an unverified fallback.
        const probeMs = Math.min(
          remaining,
          this.healthTimeoutMs > 0 ? this.healthTimeoutMs : this.healthFreshMs
        );
        try {
          await this._ensureConnectionHealthy(connection, probeMs);
          attempt.state = 'connected';
          attempt.verified = true;
          attempt.completed_at = Date.now();
          return this._peerInfo(normalizedRemote, true, connection);
        } catch (error) {
          attempt.error = error?.message ?? String(error);
          probed.add(connection);
          const health = this.connectionHealth.get(connection);
          if (this.healthTimeoutMs > 0 && health?.proven === true) {
            if (health && health.unhealthySince === 0) health.unhealthySince = Date.now();
            continue;
          }
          if (!fallback && connection.destroyed !== true && connection.closed !== true) {
            fallback = connection;
          }
        }
      }
      if (fallback && fallback.destroyed !== true && fallback.closed !== true) {
        attempt.state = 'connected';
        attempt.verified = false;
        attempt.completed_at = Date.now();
        if (this.debug) {
          console.log(
            `[direct-session] using unverified (legacy health) connection to ${normalizedRemote}`
          );
        }
        return this._peerInfo(normalizedRemote, true, fallback);
      }
      await new Promise((resolve) => setTimeout(resolve, this.connectPollMs));
    }
    attempt.state = 'failed';
    attempt.completed_at = Date.now();
    throw new Error(`Timed out connecting to peer ${normalizedRemote}.`);
  }

  async send(remote, sessionId, frame) {
    const session = await this.open(remote, sessionId);
    const key = sessionKey(session.remote, session.session_id);
    const record = this.sessions.get(key);
    if (!record?.message) throw new Error(`Session ${session.session_id} is not available.`);
    this._validateFrame(frame);
    const frameBytes = this._frameBytes(frame);
    await this._acquireSendRate(record, frameBytes);
    if (record.closed || record.channel?.closed || record.channel?.destroyed) {
      throw new Error(`Session ${session.session_id} closed before send.`);
    }
    if (this.debug) {
      console.log(
        `[direct-session:${session.session_id}] send ${frame?.t || 'frame'} to ${session.remote}`
      );
    }
    let ok = false;
    try {
      ok = record.message.send(frame);
    } catch (error) {
      this._closeRecord(record, error, true);
      throw error;
    }
    if (!ok && this.debug) {
      console.log(`[direct-session:${session.session_id}] send accepted with backpressure`);
    }
    if (!ok) {
      if (record.closed || record.channel?.closed || record.channel?.destroyed) {
        throw new Error(`Session ${session.session_id} closed during send.`);
      }
      await this._waitForDrain(record);
    }
    return this._sessionInfo(record);
  }

  close(remote, sessionId) {
    const normalizedRemote = this._normalizeRemote(remote);
    const normalizedSession = normalizeSessionId(sessionId);
    if (!normalizedRemote) throw new Error('Invalid remote peer key.');
    if (!normalizedSession) throw new Error('Invalid session_id.');
    const key = sessionKey(normalizedRemote, normalizedSession);
    const session = this.sessions.get(key);
    if (!session) return { session_id: normalizedSession, remote: normalizedRemote, closed: false };
    this._closeRecord(session, new Error(`Session ${normalizedSession} closed locally.`), true);
    return { session_id: normalizedSession, remote: normalizedRemote, closed: true };
  }

  suspendReconnect(remote) {
    const normalizedRemote = this._normalizeRemote(remote);
    if (!normalizedRemote) throw new Error('Invalid remote peer key.');
    this.reconnectSuspended.add(normalizedRemote);
    const swarm = this.peer?.swarm;
    swarm?.peers?.get?.(normalizedRemote)?.reconnect?.(false);
    swarm?.leavePeer?.(b4a.from(normalizedRemote, 'hex'));
    return true;
  }

  resumeReconnect(remote, reconnect = true) {
    const normalizedRemote = this._normalizeRemote(remote);
    if (!normalizedRemote) throw new Error('Invalid remote peer key.');
    const resumed = this.reconnectSuspended.delete(normalizedRemote);
    this.peer?.swarm?.peers?.get?.(normalizedRemote)?.reconnect?.(true);
    if (resumed && reconnect) this._rejoinExplicitPeer(normalizedRemote);
    return resumed;
  }

  async proveConnection(connection, waitMs) {
    const remote = this._normalizeRemote(this._remoteKey(connection));
    if (!remote) throw new Error('Connection is missing a valid remote peer key.');
    if (connection?.destroyed === true || connection?.closed === true) {
      throw new Error('Connection closed before health proof.');
    }
    const mux = this._prepareHealthConnectionUnchecked(connection, true);
    if (!mux || !this.connectionHealth.has(connection)) {
      throw new Error('Connection health channel is unavailable.');
    }
    const boundedWaitMs = safeIntegerOr(waitMs, this.healthTimeoutMs || this.healthFreshMs, {
      min: 1,
    });
    await this._ensureConnectionHealthy(connection, boundedWaitMs);
    const health = this.connectionHealth.get(connection);
    if (!health?.proven || !this._healthIsFresh(health)) {
      throw new Error('Connection did not complete a fresh bidirectional health proof.');
    }
    return { remote, proven: true };
  }

  _normalizeRemote(remote) {
    const normalized = normalizeKeyHex(remote);
    return normalized && /^[0-9a-f]{64}$/.test(normalized) ? normalized : null;
  }

  _prepareConnection(connection) {
    try {
      return this._prepareConnectionUnchecked(connection);
    } catch (error) {
      this._reportEventError('connection setup', error);
      return false;
    }
  }

  _prepareConnectionUnchecked(connection) {
    const mux = this._prepareHealthConnectionUnchecked(connection);
    if (!mux || this.pairedConnections.has(connection)) return;
    this.pairedConnections.add(connection);
    mux.pair({ protocol: SESSION_PROTOCOL }, (id) => {
      const sessionId = id ? b4a.toString(id, 'hex') : null;
      if (!normalizeSessionId(sessionId)) return;
      try {
        if (!this._hasSessionCapacity(connection, sessionId)) {
          if (this.debug) console.log(`[direct-session:${sessionId}] reject (session capacity)`);
          return;
        }
        this._ensureSession(connection, sessionId);
      } catch (error) {
        this._reportEventError(`inbound session ${sessionId}`, error);
      }
    });
  }

  _prepareHealthConnectionUnchecked(connection, featureOwned = false) {
    if (!connection) return null;
    if (featureOwned) this.featureOwnedConnections.add(connection);
    const prepared = this.preparedConnections.get(connection);
    if (prepared) return prepared;
    const mux = this._muxForConnection(connection);
    if (!mux) return;
    this.preparedConnections.set(connection, mux);
    connection.on('error', (error) => {
      this.connectionErrors.set(connection, error);
    });
    this._prepareHealthChannel(connection, mux);
    connection.on('close', () => {
      const remote = this._remoteKey(connection);
      const wasPreferred = this.clearPreferredConnection(remote, connection);
      this._dropConnection(connection);
      this._dropHealthConnection(connection);
      if (!wasPreferred && !this.featureOwnedConnections.has(connection)) {
        this._rejoinExplicitPeer(remote);
      }
    });
    return mux;
  }

  _rejoinExplicitPeer(remote) {
    if (
      !remote
      || !this.explicitPeers.has(remote)
      || this.reconnectSuspended.has(remote)
      || typeof this.peer?.swarm?.joinPeer !== 'function'
    ) {
      return false;
    }
    const key = b4a.from(remote, 'hex');
    // Hyperswarm's retry ladder eventually parks an already-explicit peer for
    // minutes. Cycle only the public explicit-peer intent so joinPeer queues a
    // fresh attempt instead of returning early for that parked peer.
    this.peer.swarm.leavePeer?.(key);
    this.peer.swarm.joinPeer(key);
    return true;
  }

  _muxForConnection(connection) {
    try {
      const mux = Protomux.from(connection);
      connection.userData = mux;
      return mux;
    } catch (_e) {
      return null;
    }
  }

  _findConnection(remote) {
    const preferred = this.preferredConnections.get(remote) ?? null;
    if (preferred?.destroyed === true || preferred?.closed === true) {
      this.preferredConnections.delete(remote);
    } else if (preferred && this._remoteKey(preferred) === remote) {
      return preferred;
    }
    return this._connectionsForRemote(remote)[0] ?? null;
  }

  _connectionsForRemote(remote) {
    if (!this.peer?.swarm?.connections) return [];
    return Array.from(this.peer.swarm.connections)
      .filter((connection) => (
        this._remoteKey(connection) === remote
        && connection?.destroyed !== true
        && connection?.closed !== true
      ))
      .sort((left, right) => {
        const leftAck = this.connectionHealth.get(left)?.lastAckAt ?? 0;
        const rightAck = this.connectionHealth.get(right)?.lastAckAt ?? 0;
        return rightAck - leftAck;
      });
  }

  preferConnection(remote, connection) {
    const normalizedRemote = this._normalizeRemote(remote);
    if (!normalizedRemote) throw new Error('Invalid remote peer key.');
    if (!connection || this._remoteKey(connection) !== normalizedRemote) {
      throw new Error('Preferred connection does not match the remote peer.');
    }
    if (connection.destroyed === true || connection.closed === true) {
      throw new Error('Preferred connection is closed.');
    }
    const health = this.connectionHealth.get(connection);
    if (!health?.proven || !this._healthIsFresh(health)) {
      throw new Error('Preferred connection lacks a fresh bidirectional health proof.');
    }
    this._prepareConnectionUnchecked(connection);
    const previous = this.preferredConnections.get(normalizedRemote) ?? null;
    this.preferredConnections.set(normalizedRemote, connection);
    return previous;
  }

  clearPreferredConnection(remote, connection = null) {
    const normalizedRemote = this._normalizeRemote(remote);
    if (!normalizedRemote) return false;
    const current = this.preferredConnections.get(normalizedRemote);
    if (!current || (connection && current !== connection)) return false;
    this.preferredConnections.delete(normalizedRemote);
    return true;
  }

  _prepareHealthChannel(connection, mux) {
    if (this.connectionHealth.has(connection)) return;
    const health = {
      connection,
      mux,
      channel: null,
      message: null,
      opened: false,
      lastAckAt: 0,
      unhealthySince: 0,
      proven: false,
      timer: null,
      probes: new Map(),
      waiters: new Map(),
    };
    this.connectionHealth.set(connection, health);
    health.timer = setInterval(() => this._healthTick(health), this.healthIntervalMs);
    health.timer.unref?.();
    if (typeof mux.pair === 'function') {
      mux.pair({ protocol: HEALTH_PROTOCOL }, () => {
        this._openHealthChannel(health);
      });
    }
    this._openHealthChannel(health);
  }

  _openHealthChannel(health) {
    if (!health || health.connection?.destroyed === true) return;
    if (health.channel && health.channel.closed !== true && health.channel.destroyed !== true) {
      return;
    }
    let channel = null;
    try {
      channel = health.mux.createChannel({
        protocol: HEALTH_PROTOCOL,
        onopen: () => {
          health.opened = true;
          this._sendHealthPing(health);
        },
        onclose: () => {
          health.opened = false;
          this._rejectHealthWaiters(health, new Error('Direct transport health channel closed.'));
        },
      });
    } catch (error) {
      this._reportEventError('health channel setup', error);
      return;
    }
    if (!channel) return;
    health.channel = channel;
    health.message = channel.addMessage({
      encoding: boundedJsonEncoding(MAX_HEALTH_FRAME_BYTES, 'Direct transport health frame'),
      onmessage: (frame) => this._handleHealthFrame(health, frame),
    });
    channel.open();
  }

  _healthTick(health) {
    if (!health || health.connection?.destroyed === true || health.connection?.closed === true) {
      this._dropHealthConnection(health?.connection);
      return;
    }
    const remote = this._remoteKey(health.connection);
    const explicitlyPinned = remote && this.explicitPeers.has(remote);
    const preferred = remote && this.preferredConnections.get(remote) === health.connection;
    if (this._healthIsFresh(health)) {
      health.unhealthySince = 0;
    } else if (
      (explicitlyPinned || preferred)
      && health.proven === true
      && this.healthTimeoutMs > 0
    ) {
      const now = Date.now();
      if (health.unhealthySince === 0) {
        health.unhealthySince = now;
      } else if (now - health.unhealthySince >= this.healthTimeoutMs) {
        if (this.debug) {
          console.log(`[direct-session] retiring unresponsive explicit connection to ${remote}`);
        }
        try {
          health.connection.destroy?.();
        } catch (_error) {}
        this._dropConnection(health.connection);
        this._dropHealthConnection(health.connection);
        return;
      }
    }
    if (!health.opened) {
      this._openHealthChannel(health);
      return;
    }
    this._sendHealthPing(health);
  }

  _nextHealthNonce() {
    return b4a.toString(crypto.randomBytes(16), 'hex');
  }

  _sendHealthPing(health, waiter = null) {
    if (!health?.opened || !health.message) return false;
    const nonce = this._nextHealthNonce();
    const now = Date.now();
    this._purgeHealthProbes(health, now);
    health.probes ??= new Map();
    if (health.probes.size >= MAX_PENDING_HEALTH_PROBES) {
      let oldestNonce = null;
      let oldestAt = Number.POSITIVE_INFINITY;
      for (const [candidate, sentAt] of health.probes) {
        if (sentAt < oldestAt) {
          oldestNonce = candidate;
          oldestAt = sentAt;
        }
      }
      if (oldestNonce !== null) health.probes.delete(oldestNonce);
    }
    health.probes.set(nonce, now);
    if (waiter) health.waiters.set(nonce, waiter);
    try {
      health.message.send({ t: 'ping', n: nonce });
      return nonce;
    } catch (error) {
      health.probes.delete(nonce);
      if (waiter) health.waiters.delete(nonce);
      if (waiter?.timer) clearTimeout(waiter.timer);
      waiter?.reject(error);
      return false;
    }
  }

  _purgeHealthProbes(health, now = Date.now()) {
    const ttlMs = Math.max(
      this.healthIntervalMs,
      this.healthFreshMs,
      this.healthTimeoutMs
    );
    const cutoff = now - ttlMs;
    for (const [nonce, sentAt] of health?.probes ?? []) {
      if (!Number.isFinite(sentAt) || sentAt < cutoff) health.probes.delete(nonce);
    }
  }

  _handleHealthFrame(health, frame) {
    if (
      !frame
      || typeof frame !== 'object'
      || Array.isArray(frame)
      || decodedJsonWasRejected(frame)
      || typeof frame.n !== 'string'
      || frame.n.length === 0
      || frame.n.length > 64
    ) {
      return;
    }
    if (frame.t === 'ping') {
      try {
        health.message?.send({ t: 'pong', n: frame.n });
      } catch (_error) {}
      return;
    }
    if (frame.t !== 'pong') return;
    const now = Date.now();
    this._purgeHealthProbes(health, now);
    if (!health.probes?.has(frame.n)) return;
    health.probes.delete(frame.n);
    health.lastAckAt = now;
    health.unhealthySince = 0;
    health.proven = true;
    const waiter = health.waiters.get(frame.n);
    if (!waiter) return;
    health.waiters.delete(frame.n);
    if (waiter.timer) clearTimeout(waiter.timer);
    waiter.resolve();
  }

  _healthIsFresh(health) {
    return health?.lastAckAt > 0 && Date.now() - health.lastAckAt <= this.healthFreshMs;
  }

  async _ensureConnectionHealthy(connection, waitMs) {
    const health = this.connectionHealth.get(connection);
    if (!health) throw new Error('Direct transport health channel is unavailable.');
    if (this._healthIsFresh(health)) return;
    if (!health.opened) {
      await this._awaitHealthChannelOpened(health, waitMs);
    }
    if (this._healthIsFresh(health)) return;
    await new Promise((resolve, reject) => {
      const waiter = { resolve, reject, timer: null };
      waiter.timer = setTimeout(() => {
        for (const [nonce, candidate] of health.waiters) {
          if (candidate === waiter) health.waiters.delete(nonce);
        }
        reject(new Error(`Direct transport health probe timed out after ${waitMs} ms.`));
      }, waitMs);
      waiter.timer.unref?.();
      if (!this._sendHealthPing(health, waiter)) {
        if (waiter.timer) clearTimeout(waiter.timer);
        reject(new Error('Direct transport health probe could not be sent.'));
      }
    });
  }

  async _awaitHealthChannelOpened(health, waitMs) {
    if (health.opened) return;
    let timer = null;
    try {
      await Promise.race([
        health.channel?.fullyOpened?.() ?? Promise.reject(new Error('Health channel unavailable.')),
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(new Error(`Direct transport health channel open timed out after ${waitMs} ms.`)),
            waitMs
          );
          timer.unref?.();
        }),
      ]);
      health.opened = health.channel?.opened === true;
      if (!health.opened) throw new Error('Direct transport health channel did not open.');
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  _rejectHealthWaiters(health, error) {
    for (const waiter of health?.waiters?.values?.() ?? []) {
      if (waiter.timer) clearTimeout(waiter.timer);
      waiter.reject(error);
    }
    health?.waiters?.clear?.();
    health?.probes?.clear?.();
  }

  _dropHealthConnection(connection) {
    if (!connection) return;
    const health = this.connectionHealth.get(connection);
    if (!health) return;
    this.connectionHealth.delete(connection);
    if (health.timer) clearInterval(health.timer);
    this._rejectHealthWaiters(health, new Error('Direct transport closed.'));
  }

  _remoteKey(connection) {
    return normalizeKeyHex(connection?.remotePublicKey);
  }

  _peerInfo(remote, connected, connection = null) {
    return {
      remote,
      connected,
      ...this._transportInfo(connection, remote, connected),
    };
  }

  _transportInfo(connection, remote, connected = true) {
    if (this.transportInfo) {
      try {
        const info = this.transportInfo(connection, remote);
        if (info && typeof info === 'object') {
          const relayed = info.relayed === true;
          return {
            direct: connected === true && !relayed,
            relayed: connected === true && relayed,
            relay: relayed && typeof info.relay === 'string' ? info.relay : null,
          };
        }
      } catch (error) {
        this._reportEventError('transport classification', error);
      }
    }
    return {
      direct: connected === true,
      relayed: false,
      relay: null,
    };
  }

  _ensureSession(connection, sessionId) {
    this._prepareConnection(connection);
    const remote = this._remoteKey(connection);
    if (!remote) throw new Error('Direct connection is missing remote key.');
    const key = sessionKey(remote, sessionId);
    const existing = this.sessions.get(key);
    if (existing && existing.connection === connection && !existing.closed) return existing;
    if (existing) {
      this._closeRecord(
        existing,
        new Error(`Session ${sessionId} moved to a replacement connection.`),
        true,
        false
      );
    }
    if (!this._hasSessionCapacity(connection, sessionId)) {
      throw new Error(`Direct session capacity reached for ${remote}.`);
    }

    const mux = this._muxForConnection(connection);
    if (!mux) throw new Error('Direct connection does not have a Protomux session.');
    const id = b4a.from(sessionId, 'hex');
    let record = null;
    const channel = mux.createChannel({
      protocol: SESSION_PROTOCOL,
      id,
      onopen: () => {
        if (this.debug) console.log(`[direct-session:${sessionId}] open ${remote}`);
      },
      onclose: () => {
        if (this.debug) console.log(`[direct-session:${sessionId}] close ${remote}`);
        if (record) this._closeRecord(record, new Error(`Session ${sessionId} closed.`), false);
      },
      ondrain: () => {
        if (record) this._resolveDrainWaiters(record);
      },
    });
    if (!channel) throw new Error(`Session ${sessionId} could not be opened.`);

    record = {
      sessionId,
      remote,
      connection,
      channel,
      message: null,
      sendLimiter: this._newLimiter(),
      receiveLimiter: this._newReceiveLimiter(),
      connectionReceiveLimiter: this._registerConnectionReceiveLimiter(connection),
      drainWaiters: new Set(),
      closed: false,
    };
    const message = channel.addMessage({
      encoding: boundedJsonEncoding(
        this.maxFrameBytes,
        'Direct session frame',
        { maxStringBytes: this.maxStringBytes }
      ),
      onmessage: (frame) => this._handleFrame(record, frame),
    });
    record.message = message;
    this.sessions.set(key, record);
    channel.open();
    return record;
  }

  _handleFrame(session, frame) {
    if (session.closed || session.channel?.closed || session.channel?.destroyed) return;
    if (!this._validateFrame(frame, false)) {
      this._closeRecord(
        session,
        new Error(`Session ${session.sessionId} sent an invalid frame.`),
        true
      );
      return;
    }
    const frameBytes = this._frameBytes(frame);
    if (!this._checkReceiveRate(session, frameBytes)) {
      if (this.debug) console.log(`[direct-session:${session.sessionId}] drop (rate limit)`);
      this._closeRecord(
        session,
        new Error(`Session ${session.sessionId} exceeded its receive rate.`),
        true
      );
      return;
    }
    if (this.debug) {
      console.log(
        `[direct-session:${session.sessionId}] recv ${frame?.t || 'frame'} from ${session.remote}`
      );
    }
    if (this.onFrame) {
      try {
        const transport = this._transportInfo(session.connection, session.remote);
        const result = this.onFrame({
          session_id: session.sessionId,
          channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
          protocol: SESSION_PROTOCOL,
          remote: session.remote,
          ...transport,
          frame,
        });
        if (result && typeof result.catch === 'function') {
          result.catch((error) => this._reportEventError(`session ${session.sessionId}`, error));
        }
      } catch (error) {
        this._reportEventError(`session ${session.sessionId}`, error);
      }
    }
  }

  _reportEventError(scope, error) {
    console.error(
      `[direct-session] ${scope} failed without stopping the peer:`,
      error?.message ?? error
    );
  }

  _validateFrame(frame, throwOnError = true) {
    const fail = (message) => {
      if (throwOnError) throw new Error(message);
      if (this.debug) console.log(`[direct-session] drop frame: ${message}`);
      return false;
    };
    if (!frame || typeof frame !== 'object' || Array.isArray(frame)) {
      return fail('Session frame must be a JSON object.');
    }
    if (decodedJsonWasRejected(frame)) {
      return fail('Session frame failed bounded JSON decoding.');
    }
    if (typeof frame.t !== 'string' || frame.t.length === 0) {
      return fail('Session frame missing t.');
    }
    if (b4a.byteLength(frame.t, 'utf8') > MAX_FRAME_TYPE_BYTES) {
      return fail(`Session frame t exceeds ${MAX_FRAME_TYPE_BYTES} bytes.`);
    }
    if (!jsonShapeWithinBounds(frame, undefined, undefined, this.maxStringBytes)) {
      return fail('Session frame exceeds JSON shape or string bounds.');
    }
    let size = decodedJsonByteLength(frame);
    if (size === null) {
      try {
        size = b4a.byteLength(JSON.stringify(frame), 'utf8');
      } catch (_e) {
        return fail('Session frame is not serializable.');
      }
    }
    if (size > this.maxFrameBytes) {
      return fail(`Session frame is too large (${size} > ${this.maxFrameBytes}).`);
    }
    return true;
  }

  _frameBytes(frame) {
    return decodedJsonByteLength(frame) ?? b4a.byteLength(JSON.stringify(frame), 'utf8');
  }

  _newLimiter(capacity = this.rateBurstBytes, tokens = capacity) {
    return {
      capacity,
      tokens,
      lastRefill: Date.now(),
    };
  }

  _newReceiveLimiter() {
    return this._newLimiter(
      this._receiveRateCapacityBytes(),
      this._receiveRateBurstBytes()
    );
  }

  _receiveRateBurstBytes() {
    return Math.min(
      Number.MAX_SAFE_INTEGER,
      this.rateBurstBytes + this.maxFrameBytes
    );
  }

  _receiveRateCapacityBytes() {
    return Math.min(
      Number.MAX_SAFE_INTEGER,
      this._receiveRateBurstBytes() + this.receiveBatchHeadroomBytes
    );
  }

  // Base bursts remain per session; only one earned transport-batch reserve is shared.
  _connectionReceiveCapacity(sessionCount) {
    if (sessionCount <= 0) return 0;
    return Math.min(
      Number.MAX_SAFE_INTEGER,
      (this._receiveRateBurstBytes() * sessionCount) + this.receiveBatchHeadroomBytes
    );
  }

  _connectionReceiveRate(sessionCount) {
    return Math.min(
      Number.MAX_SAFE_INTEGER,
      this.rateBytesPerSecond * sessionCount
    );
  }

  _refillLimiter(limiter, rateBytesPerSecond, now = Date.now()) {
    if (!limiter || rateBytesPerSecond <= 0) return;
    const elapsedMs = now - limiter.lastRefill;
    if (elapsedMs > 0) {
      const refill = (elapsedMs / 1000) * rateBytesPerSecond;
      limiter.tokens = Math.min(
        limiter.capacity ?? this.rateBurstBytes,
        limiter.tokens + refill
      );
      limiter.lastRefill = now;
    }
  }

  _checkRate(limiter, bytes) {
    if (this.rateBytesPerSecond <= 0) return true;
    this._refillLimiter(limiter, this.rateBytesPerSecond);
    if (bytes > limiter.tokens) return false;
    limiter.tokens -= bytes;
    return true;
  }

  _checkReceiveRate(session, bytes) {
    if (this.rateBytesPerSecond <= 0) return true;
    if (!session?.receiveLimiter) return false;
    const now = Date.now();
    const connectionLimiter = session.connectionReceiveLimiter ?? null;
    this._refillLimiter(session.receiveLimiter, this.rateBytesPerSecond, now);
    if (connectionLimiter) {
      this._refillLimiter(
        connectionLimiter,
        this._connectionReceiveRate(connectionLimiter.sessionCount),
        now
      );
    }
    if (
      bytes > session.receiveLimiter.tokens
      || (connectionLimiter && bytes > connectionLimiter.tokens)
    ) {
      return false;
    }
    session.receiveLimiter.tokens -= bytes;
    if (connectionLimiter) connectionLimiter.tokens -= bytes;
    return true;
  }

  _registerConnectionReceiveLimiter(connection) {
    if (!connection || (typeof connection !== 'object' && typeof connection !== 'function')) {
      return null;
    }
    let limiter = this.connectionReceiveLimiters.get(connection);
    if (!limiter) {
      limiter = {
        capacity: 0,
        tokens: 0,
        lastRefill: Date.now(),
        sessionCount: 0,
      };
      this.connectionReceiveLimiters.set(connection, limiter);
    }
    this._refillLimiter(
      limiter,
      this._connectionReceiveRate(limiter.sessionCount)
    );
    limiter.sessionCount += 1;
    limiter.capacity = this._connectionReceiveCapacity(limiter.sessionCount);
    limiter.tokens = Math.min(
      limiter.capacity,
      limiter.tokens + this._receiveRateBurstBytes()
    );
    return limiter;
  }

  _unregisterConnectionReceiveLimiter(session) {
    const limiter = session?.connectionReceiveLimiter;
    if (!limiter) return;
    this._refillLimiter(
      limiter,
      this._connectionReceiveRate(limiter.sessionCount)
    );
    limiter.sessionCount = Math.max(0, limiter.sessionCount - 1);
    limiter.capacity = this._connectionReceiveCapacity(limiter.sessionCount);
    limiter.tokens = Math.min(limiter.tokens, limiter.capacity);
    session.connectionReceiveLimiter = null;
    if (limiter.sessionCount === 0 && session.connection) {
      this.connectionReceiveLimiters.delete(session.connection);
    }
  }

  async _acquireSendRate(session, bytes) {
    if (this.rateBytesPerSecond <= 0) return;
    if (bytes > this.rateBurstBytes) {
      throw new Error(
        `Session ${session.sessionId} frame exceeds the configured send burst capacity.`
      );
    }
    while (!this._checkRate(session.sendLimiter, bytes)) {
      if (session.closed || session.channel?.closed || session.channel?.destroyed) {
        throw new Error(`Session ${session.sessionId} closed while waiting for send capacity.`);
      }
      const deficit = Math.max(1, bytes - session.sendLimiter.tokens);
      const waitMs = Math.max(1, Math.ceil((deficit * 1000) / this.rateBytesPerSecond));
      await new Promise((resolve) => setTimeout(resolve, waitMs));
    }
  }

  async _awaitOpened(session) {
    let timer = null;
    const timedOut = new Promise((_, reject) => {
      timer = setTimeout(() => {
        reject(new Error(`Session ${session.sessionId} open timed out.`));
      }, this.openMaxWaitMs);
    });
    try {
      const opened = await Promise.race([session.channel.fullyOpened(), timedOut]);
      if (!opened || session.closed) throw new Error(`Session ${session.sessionId} was not opened.`);
    } catch (error) {
      this._closeRecord(session, error, true);
      throw error;
    } finally {
      if (timer !== null) clearTimeout(timer);
    }
  }

  _hasSessionCapacity(connection, sessionId) {
    const remote = this._remoteKey(connection);
    if (remote && this.sessions.has(sessionKey(remote, sessionId))) return true;
    if (this.sessions.size >= this.maxSessions) return false;
    let connectionSessions = 0;
    for (const session of this.sessions.values()) {
      if (session.connection === connection && !session.closed) connectionSessions += 1;
    }
    return connectionSessions < this.maxSessionsPerConnection;
  }

  _closeRecord(session, error, closeChannel, emitClose = true) {
    if (!session || session.closed) return;
    session.closed = true;
    this._unregisterConnectionReceiveLimiter(session);
    const transportError = this.connectionErrors.get(session.connection) ?? null;
    const closeReason = error?.message ?? 'Direct session closed.';
    this._rejectDrainWaiters(session, error);
    const key = sessionKey(session.remote, session.sessionId);
    if (this.sessions.get(key) === session) this.sessions.delete(key);
    if (emitClose && this.onClose) {
      try {
        const transport = this._transportInfo(session.connection, session.remote);
        const result = this.onClose({
          session_id: session.sessionId,
          channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
          protocol: SESSION_PROTOCOL,
          remote: session.remote,
          ...transport,
          reason: transportError
            ? `${closeReason} Transport: ${transportError.code || 'ERROR'} ${transportError.message}`
            : closeReason,
          locally_initiated: closeChannel === true,
          transport_error: transportError?.message ?? null,
          transport_error_code: transportError?.code ?? null,
          transport_initiator: session.connection?.isInitiator === true,
          transport_bytes_read: Number(session.connection?.rawBytesRead) || 0,
          transport_bytes_written: Number(session.connection?.rawBytesWritten) || 0,
        });
        if (result && typeof result.catch === 'function') {
          result.catch((closeError) =>
            this._reportEventError(`session ${session.sessionId} close`, closeError)
          );
        }
      } catch (closeError) {
        this._reportEventError(`session ${session.sessionId} close`, closeError);
      }
    }
    if (closeChannel) {
      try {
        session.channel?.close?.();
      } catch (_e) {}
    }
  }

  _waitForDrain(session) {
    if (session.closed || session.channel?.closed || session.channel?.destroyed) {
      return Promise.reject(new Error(`Session ${session.sessionId} closed before drain.`));
    }
    if (session.channel?.drained === true) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const waiter = {
        resolve,
        reject,
        timer: null,
      };
      if (this.sendDrainTimeoutMs > 0) {
        waiter.timer = setTimeout(() => {
          session.drainWaiters.delete(waiter);
          reject(new Error(`Session ${session.sessionId} send drain timed out.`));
        }, this.sendDrainTimeoutMs);
      }
      session.drainWaiters.add(waiter);
    });
  }

  _resolveDrainWaiters(session) {
    for (const waiter of session.drainWaiters) {
      if (waiter.timer !== null) clearTimeout(waiter.timer);
      waiter.resolve();
    }
    session.drainWaiters.clear();
  }

  _rejectDrainWaiters(session, err) {
    for (const waiter of session.drainWaiters) {
      if (waiter.timer !== null) clearTimeout(waiter.timer);
      waiter.reject(err);
    }
    session.drainWaiters.clear();
  }

  _sessionInfo(session) {
    return {
      ...this._transportInfo(session.connection, session.remote),
      session_id: session.sessionId,
      channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
      protocol: SESSION_PROTOCOL,
      remote: session.remote,
      opened: session.channel?.opened === true,
    };
  }

  _dropConnection(connection) {
    const remote = this._remoteKey(connection);
    for (const session of this.sessions.values()) {
      if (
        session.connection === connection
        || (!session.connection && remote && session.remote === remote)
      ) {
        this._closeRecord(session, new Error(`Connection to ${session.remote} closed.`), false);
      }
    }
  }
}

export default DirectSession;
