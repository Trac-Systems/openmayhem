import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';
import c from '../../node_modules/compact-encoding/index.js';
import Protomux from 'protomux';

const SESSION_PROTOCOL = 'mx/s';
const SESSION_CHANNEL_PREFIX = 'mx/s/';
const DEFAULT_MAX_FRAME_BYTES = 256 * 1024;
const DEFAULT_RATE_BYTES_PER_SECOND = 1_000_000;
const DEFAULT_RATE_BURST_BYTES = 1_000_000;
const DEFAULT_SEND_DRAIN_TIMEOUT_MS = 0;
const DEFAULT_CONNECT_MAX_WAIT_MS = 120_000;
const DEFAULT_CONNECT_POLL_MS = 100;

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
    this.rateBurstBytes = safeIntegerOr(config.rateBurstBytes, DEFAULT_RATE_BURST_BYTES);
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
        const opened = await existing.channel.fullyOpened();
        if (!opened) throw new Error(`Session ${normalizedSession} was not opened.`);
      }
      return this._sessionInfo(existing);
    }
    const connection = this._findConnection(normalizedRemote);
    if (!connection) throw new Error(`No direct connection to ${normalizedRemote}.`);
    const session = this._ensureSession(connection, normalizedSession);
    if (session.channel?.opened !== true) {
      const opened = await session.channel.fullyOpened();
      if (!opened) throw new Error(`Session ${normalizedSession} was not opened.`);
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
    if (!this._checkRate(record.sendLimiter, frameBytes)) {
      throw new Error(`Session ${session.session_id} send rate limit exceeded.`);
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
    if (!ok) await this._waitForDrain(record);
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
    try {
      session.channel?.close?.();
    } catch (_e) {}
    this.sessions.delete(key);
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
        if (record) this._rejectDrainWaiters(record, new Error(`Session ${sessionId} closed.`));
        this.sessions.delete(key);
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
    };
    const message = channel.addMessage({
      encoding: c.json,
      onmessage: (frame) => this._handleFrame(record, frame),
    });
    record.message = message;
    this.sessions.set(key, record);
    channel.open();
    return record;
  }

  _handleFrame(session, frame) {
    if (!this._validateFrame(frame, false)) return;
    const frameBytes = this._frameBytes(frame);
    if (!this._checkRate(session.receiveLimiter, frameBytes)) {
      if (this.debug) console.log(`[direct-session:${session.sessionId}] drop (rate limit)`);
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
    if (typeof frame.t !== 'string' || frame.t.length === 0) {
      return fail('Session frame missing t.');
    }
    let size = 0;
    try {
      size = b4a.byteLength(JSON.stringify(frame), 'utf8');
    } catch (_e) {
      return fail('Session frame is not serializable.');
    }
    if (size > this.maxFrameBytes) {
      return fail(`Session frame is too large (${size} > ${this.maxFrameBytes}).`);
    }
    return true;
  }

  _frameBytes(frame) {
    return b4a.byteLength(JSON.stringify(frame), 'utf8');
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

  _waitForDrain(session) {
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
    for (const [key, session] of this.sessions.entries()) {
      if (session.connection === connection || (remote && session.remote === remote)) {
        this.sessions.delete(key);
      }
    }
  }
}

export default DirectSession;
