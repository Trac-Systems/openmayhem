import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';
import c from '../../node_modules/compact-encoding/index.js';
import Protomux from 'protomux';

const SESSION_PROTOCOL = 'mx/s';
const SESSION_CHANNEL_PREFIX = 'mx/s/';
const DEFAULT_MAX_FRAME_BYTES = 256 * 1024;
const DEFAULT_RATE_BYTES_PER_SECOND = 1_000_000;
const DEFAULT_RATE_BURST_BYTES = 1_000_000;

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

class DirectSession extends Feature {
  constructor(peer, config = {}) {
    super(peer, config);
    this.key = 'direct-session';
    this.started = false;
    this.debug = config.debug === true;
    this.maxFrameBytes = Number.isSafeInteger(config.maxFrameBytes)
      ? config.maxFrameBytes
      : DEFAULT_MAX_FRAME_BYTES;
    this.rateBytesPerSecond = DEFAULT_RATE_BYTES_PER_SECOND;
    this.rateBurstBytes = DEFAULT_RATE_BURST_BYTES;
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
      rateBytesPerSecond: this.rateBytesPerSecond,
      rateBurstBytes: this.rateBurstBytes,
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
    const connection = this._findConnection(normalizedRemote);
    if (!connection) throw new Error(`No direct connection to ${normalizedRemote}.`);
    const session = this._ensureSession(connection, normalizedSession);
    const opened = await session.channel.fullyOpened();
    if (!opened) throw new Error(`Session ${normalizedSession} was not opened.`);
    return this._sessionInfo(session);
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
    const ok = record.message.send(frame);
    if (!ok) throw new Error(`Session ${session.session_id} send failed.`);
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
    if (!connection || this.pairedConnections.has(connection)) return;
    const mux = this._muxForConnection(connection);
    if (!mux) return;
    this.pairedConnections.add(connection);
    mux.pair({ protocol: SESSION_PROTOCOL }, (id) => {
      const sessionId = id ? b4a.toString(id, 'hex') : null;
      if (!normalizeSessionId(sessionId)) return;
      this._ensureSession(connection, sessionId);
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
    const channel = mux.createChannel({
      protocol: SESSION_PROTOCOL,
      id,
      onopen: () => {
        if (this.debug) console.log(`[direct-session:${sessionId}] open ${remote}`);
      },
      onclose: () => {
        if (this.debug) console.log(`[direct-session:${sessionId}] close ${remote}`);
        this.sessions.delete(key);
      },
    });
    if (!channel) throw new Error(`Session ${sessionId} could not be opened.`);

    const record = {
      sessionId,
      remote,
      connection,
      channel,
      message: null,
      sendLimiter: this._newLimiter(),
      receiveLimiter: this._newLimiter(),
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
    if (this.onFrame) {
      this.onFrame({
        session_id: session.sessionId,
        channel: `${SESSION_CHANNEL_PREFIX}${session.sessionId}`,
        protocol: SESSION_PROTOCOL,
        remote: session.remote,
        direct: true,
        relayed: false,
        frame,
      });
    }
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
