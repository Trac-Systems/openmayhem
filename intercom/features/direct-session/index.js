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
const DEFAULT_MAX_FRAME_BYTES = 256 * 1024;
const DEFAULT_RATE_BYTES_PER_SECOND = 1_000_000;
const DEFAULT_RATE_BURST_BYTES = 1_000_000;
const DEFAULT_SEND_DRAIN_TIMEOUT_MS = 0;
const DEFAULT_CONNECT_MAX_WAIT_MS = 120_000;
const DEFAULT_CONNECT_POLL_MS = 100;
const DEFAULT_OPEN_MAX_WAIT_MS = 120_000;
const DEFAULT_MAX_SESSIONS = 1024;
const DEFAULT_MAX_SESSIONS_PER_CONNECTION = 128;
const MAX_FRAME_TYPE_BYTES = 64;

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
    this.maxSessions = safeIntegerOr(config.maxSessions, DEFAULT_MAX_SESSIONS, { min: 1 });
    this.maxSessionsPerConnection = safeIntegerOr(
      config.maxSessionsPerConnection,
      DEFAULT_MAX_SESSIONS_PER_CONNECTION,
      { min: 1 }
    );
    this.sessions = new Map();
    this.pairedConnections = new WeakSet();
    this.onFrame = typeof config.onFrame === 'function' ? config.onFrame : null;
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
    this.peer.swarm.joinPeer(b4a.from(normalizedRemote, 'hex'));
    const existing = this._findConnection(normalizedRemote);
    if (existing) {
      this._prepareConnection(existing);
      return this._peerInfo(normalizedRemote, true);
    }
    const maxWaitMs = Math.max(1, Math.min(Number(waitMs) || 10_000, this.connectMaxWaitMs));
    const deadline = Date.now() + maxWaitMs;
    while (Date.now() < deadline) {
      const connection = this._findConnection(normalizedRemote);
      if (connection) {
        this._prepareConnection(connection);
        return this._peerInfo(normalizedRemote, true);
      }
      await new Promise((resolve) => setTimeout(resolve, this.connectPollMs));
    }
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
    const ok = record.message.send(frame);
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
    connection.on('close', () => this._dropConnection(connection));
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
    if (!this.peer?.swarm?.connections) return null;
    for (const connection of this.peer.swarm.connections) {
      if (this._remoteKey(connection) === remote) return connection;
    }
    return null;
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
    if (existing) return existing;
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

  _closeRecord(session, error, closeChannel) {
    if (!session || session.closed) return;
    session.closed = true;
    this._rejectDrainWaiters(session, error);
    const key = sessionKey(session.remote, session.sessionId);
    if (this.sessions.get(key) === session) this.sessions.delete(key);
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
      if (session.connection === connection || (remote && session.remote === remote)) {
        this._closeRecord(session, new Error(`Connection to ${session.remote} closed.`), false);
      }
    }
  }
}

export default DirectSession;
