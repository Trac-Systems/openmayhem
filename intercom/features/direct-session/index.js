import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';
import Protomux from 'protomux';
import {
  boundedJsonEncoding,
  decodedJsonByteLength,
  decodedJsonWasRejected,
} from '../bounded-json.js';

const SESSION_PROTOCOL = 'mx/s';
const SESSION_CHANNEL_PREFIX = 'mx/s/';
const HEALTH_PROTOCOL = 'mx/s-health';
const DEFAULT_MAX_FRAME_BYTES = 256 * 1024;
const DEFAULT_RATE_BYTES_PER_SECOND = 1_000_000;
const DEFAULT_RATE_BURST_BYTES = 1_000_000;
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
    this.rateBytesPerSecond = safeIntegerOr(
      config.rateBytesPerSecond,
      DEFAULT_RATE_BYTES_PER_SECOND
    );
    this.rateBurstBytes = Math.max(
      this.maxFrameBytes,
      safeIntegerOr(config.rateBurstBytes, DEFAULT_RATE_BURST_BYTES)
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
    this.pairedConnections = new WeakSet();
    this.connectionErrors = new WeakMap();
    this.connectionHealth = new Map();
    this.explicitPeers = new Set();
    this.verifiedPeers = new Set();
    this.healthNonce = 0;
    this.lastConnectAttempt = null;
    this.onFrame = typeof config.onFrame === 'function' ? config.onFrame : null;
    this.onClose = typeof config.onClose === 'function' ? config.onClose : null;
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
      rateBytesPerSecond: this.rateBytesPerSecond,
      rateBurstBytes: this.rateBurstBytes,
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
      sessionCount: this.sessions.size,
      sessions: Array.from(this.sessions.values()).map((session) => ({
        session_id: session.sessionId,
        channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
        remote: session.remote,
        opened: session.channel?.opened === true,
        direct: true,
        relayed: false,
      })),
      connections: Array.from(this.connectionHealth.values()).map((health) => ({
        remote: this._remoteKey(health.connection),
        explicit: this.explicitPeers.has(this._remoteKey(health.connection)),
        health_capable: this.verifiedPeers.has(this._remoteKey(health.connection)),
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
            this.verifiedPeers.add(normalizedRemote);
            attempt.state = 'connected';
            attempt.verified = true;
            attempt.completed_at = Date.now();
            return this._peerInfo(normalizedRemote, true);
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
          this.verifiedPeers.add(normalizedRemote);
          attempt.state = 'connected';
          attempt.verified = true;
          attempt.completed_at = Date.now();
          return this._peerInfo(normalizedRemote, true);
        } catch (error) {
          attempt.error = error?.message ?? String(error);
          probed.add(connection);
          if (this.healthTimeoutMs > 0 && this.verifiedPeers.has(normalizedRemote)) {
            const health = this.connectionHealth.get(connection);
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
        return this._peerInfo(normalizedRemote, true);
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
    if (!connection || this.pairedConnections.has(connection)) return;
    const mux = this._muxForConnection(connection);
    if (!mux) return;
    this.pairedConnections.add(connection);
    connection.on('error', (error) => {
      this.connectionErrors.set(connection, error);
    });
    this._prepareHealthChannel(connection, mux);
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
    connection.on('close', () => {
      const remote = this._remoteKey(connection);
      this._dropConnection(connection);
      this._dropHealthConnection(connection);
      this._rejoinExplicitPeer(remote);
    });
  }

  _rejoinExplicitPeer(remote) {
    if (
      !remote
      || !this.explicitPeers.has(remote)
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
      timer: null,
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
    if (this._healthIsFresh(health)) {
      health.unhealthySince = 0;
    } else if (
      explicitlyPinned
      && this.verifiedPeers.has(remote)
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
    this.healthNonce = (this.healthNonce + 1) % Number.MAX_SAFE_INTEGER;
    return `${Date.now().toString(36)}-${this.healthNonce.toString(36)}`;
  }

  _sendHealthPing(health, waiter = null) {
    if (!health?.opened || !health.message) return false;
    const nonce = this._nextHealthNonce();
    if (waiter) health.waiters.set(nonce, waiter);
    try {
      health.message.send({ t: 'ping', n: nonce });
      return nonce;
    } catch (error) {
      if (waiter) health.waiters.delete(nonce);
      if (waiter?.timer) clearTimeout(waiter.timer);
      waiter?.reject(error);
      return false;
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
    health.lastAckAt = Date.now();
    health.unhealthySince = 0;
    const remote = this._remoteKey(health.connection);
    if (remote) this.verifiedPeers.add(remote);
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

  _peerInfo(remote, connected) {
    return {
      remote,
      connected,
      direct: connected === true,
      relayed: false,
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
      receiveLimiter: this._newLimiter(),
      drainWaiters: new Set(),
      closed: false,
    };
    const message = channel.addMessage({
      encoding: boundedJsonEncoding(this.maxFrameBytes, 'Direct session frame'),
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
    if (!this._checkRate(session.receiveLimiter, frameBytes)) {
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
        const result = this.onFrame({
          session_id: session.sessionId,
          channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
          protocol: SESSION_PROTOCOL,
          remote: session.remote,
          direct: true,
          relayed: false,
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

  _newLimiter() {
    return {
      tokens: this.rateBurstBytes,
      lastRefill: Date.now(),
    };
  }

  _checkRate(limiter, bytes) {
    if (this.rateBytesPerSecond <= 0) return true;
    const now = Date.now();
    const elapsedMs = now - limiter.lastRefill;
    if (elapsedMs > 0) {
      const refill = (elapsedMs / 1000) * this.rateBytesPerSecond;
      limiter.tokens = Math.min(this.rateBurstBytes, limiter.tokens + refill);
      limiter.lastRefill = now;
    }
    if (bytes > limiter.tokens) return false;
    limiter.tokens -= bytes;
    return true;
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
    const transportError = this.connectionErrors.get(session.connection) ?? null;
    const closeReason = error?.message ?? 'Direct session closed.';
    this._rejectDrainWaiters(session, error);
    const key = sessionKey(session.remote, session.sessionId);
    if (this.sessions.get(key) === session) this.sessions.delete(key);
    if (emitClose && this.onClose) {
      try {
        const result = this.onClose({
          session_id: session.sessionId,
          channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
          protocol: SESSION_PROTOCOL,
          remote: session.remote,
          direct: true,
          relayed: false,
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
      session_id: session.sessionId,
      channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
      protocol: SESSION_PROTOCOL,
      remote: session.remote,
      direct: true,
      relayed: false,
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
