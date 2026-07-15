import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';

const DEFAULT_DIRECT_WAIT_MS = 15_000;
const DEFAULT_RELAY_WAIT_MS = 45_000;
const DEFAULT_MAX_CLIENTS = 256;
const DEFAULT_MAX_PENDING_PAIRS = 512;
const DEFAULT_MAX_ACTIVE_PAIRS = 128;
const DEFAULT_MAX_BYTES_PER_LINK = 512 * 1024 * 1024;
const DEFAULT_RATE_BYTES_PER_SECOND = 32 * 1024 * 1024;
const DEFAULT_RATE_BURST_BYTES = 64 * 1024 * 1024;
const DEFAULT_IDLE_TIMEOUT_MS = 120_000;
const DEFAULT_SWEEP_INTERVAL_MS = 1_000;

const normalizeKey = (value) => {
  const key = b4a.isBuffer(value)
    ? b4a.toString(value, 'hex')
    : String(value || '').trim().toLowerCase();
  return /^[0-9a-f]{64}$/.test(key) ? key : null;
};

const normalizeRelayKeys = (values, localKey = null) => {
  const seen = new Set();
  const relays = [];
  for (const value of Array.isArray(values) ? values : []) {
    const key = normalizeKey(value);
    if (!key || key === localKey || seen.has(key)) continue;
    seen.add(key);
    relays.push(key);
  }
  return relays;
};

const positiveInteger = (value, fallback, minimum = 1) => (
  Number.isSafeInteger(value) && value >= minimum ? value : fallback
);

const streamByteCount = (stream) => (
  Math.max(0, Number(stream?.bytesReceived) || 0)
  + Math.max(0, Number(stream?.bytesTransmitted) || 0)
);

class BoundedPairingMap extends Map {
  constructor(limit, onReject) {
    super();
    this.limit = limit;
    this.onReject = onReject;
  }

  set(key, value) {
    if (!this.has(key) && this.size >= this.limit) {
      this.onReject(key, value);
      return this;
    }
    return super.set(key, value);
  }
}

class InferenceRelay extends Feature {
  constructor(peer, config = {}) {
    super(peer, config);
    this.key = 'inference-relay';
    this.started = false;
    this.serve = config.serve === true;
    this.debug = config.debug === true;
    this.forceRelay = config.forceRelay === true;
    this.RelayServer = config.RelayServer || null;
    this.localKey = normalizeKey(peer?.wallet?.publicKey)
      || normalizeKey(peer?.swarm?.keyPair?.publicKey);
    this.relays = normalizeRelayKeys(config.relays, this.localKey);
    this.directWaitMs = positiveInteger(config.directWaitMs, DEFAULT_DIRECT_WAIT_MS);
    this.relayWaitMs = positiveInteger(config.relayWaitMs, DEFAULT_RELAY_WAIT_MS);
    this.maxClients = positiveInteger(config.maxClients, DEFAULT_MAX_CLIENTS);
    this.maxPendingPairs = positiveInteger(
      config.maxPendingPairs,
      DEFAULT_MAX_PENDING_PAIRS
    );
    this.maxActivePairs = positiveInteger(config.maxActivePairs, DEFAULT_MAX_ACTIVE_PAIRS);
    this.maxBytesPerLink = positiveInteger(
      config.maxBytesPerLink,
      DEFAULT_MAX_BYTES_PER_LINK
    );
    this.rateBytesPerSecond = positiveInteger(
      config.rateBytesPerSecond,
      DEFAULT_RATE_BYTES_PER_SECOND
    );
    this.rateBurstBytes = Math.max(
      this.rateBytesPerSecond,
      positiveInteger(config.rateBurstBytes, DEFAULT_RATE_BURST_BYTES)
    );
    this.idleTimeoutMs = positiveInteger(config.idleTimeoutMs, DEFAULT_IDLE_TIMEOUT_MS);
    this.sweepIntervalMs = positiveInteger(
      config.sweepIntervalMs,
      DEFAULT_SWEEP_INTERVAL_MS,
      50
    );

    this.server = null;
    this.sessions = new Map();
    this.acceptedConnections = new WeakSet();
    this.streams = new Map();
    this.outboundRelayConnections = new Set();
    this.relayConnections = new Map();
    this.relayConnects = new Map();
    this.connectionRoutes = new WeakMap();
    this.forcedPeers = new Set();
    this.relayCursor = 0;
    this.sweepTimer = null;
    this.onConnection = (connection, peerInfo) => this._handleConnection(connection, peerInfo);
    this.counters = {
      relay_selections: 0,
      forced_fallbacks: 0,
      relay_releases: 0,
      clients_rejected: 0,
      pending_pairs_rejected: 0,
      active_pairs_rejected: 0,
      links_rate_limited: 0,
      links_byte_limited: 0,
      links_idle_closed: 0,
      clients_idle_closed: 0,
      stream_errors: 0,
      bytes_observed: 0,
    };
    this.lastSweepAt = null;
  }

  async start() {
    if (this.started) return;
    const swarm = this.peer?.swarm;
    if (!swarm?.dht || !swarm?.keyPair) {
      throw new Error('InferenceRelay requires an initialized peer swarm.');
    }
    try {
      if (this.serve) {
        if (!this.RelayServer) {
          const relayModule = await import('blind-relay');
          this.RelayServer = relayModule.Server || relayModule.default?.Server;
        }
        if (typeof this.RelayServer !== 'function') {
          throw new Error('blind-relay Server is unavailable.');
        }
        this.server = new this.RelayServer({
          createStream: (options) => this._createBoundedStream(options),
        });
        // blind-relay 1.6.1 intentionally exposes no admission callback. Replacing
        // its pending-token Map contains allocation at the configured limit.
        const relayServer = this.server;
        this.server._pairing = new BoundedPairingMap(this.maxPendingPairs, (_key, pair) => {
          this.counters.pending_pairs_rejected += 1;
          setTimeout(() => {
            if (relayServer.stats?.pairings?.pending > 0) {
              relayServer.stats.pairings.pending -= 1;
            }
            const offender = pair?.links?.find((link) => link?.session)?.session;
            offender?.destroy?.(new Error('Inference relay pending-pair limit reached.'));
          }, 0);
        });
        swarm.on('connection', this.onConnection);
        for (const connection of swarm.connections || []) {
          this._handleConnection(connection, null);
        }
        this.sweepTimer = setInterval(() => this._sweep(), this.sweepIntervalMs);
      } else {
        swarm.on('connection', this.onConnection);
      }
      this.started = true;
    } catch (error) {
      if (this.sweepTimer) clearInterval(this.sweepTimer);
      this.sweepTimer = null;
      swarm.off?.('connection', this.onConnection);
      this.server = null;
      throw error;
    }
  }

  async stop() {
    if (!this.started) return;
    this.started = false;
    if (this.sweepTimer) clearInterval(this.sweepTimer);
    this.sweepTimer = null;
    this.peer?.swarm?.off?.('connection', this.onConnection);
    for (const connection of this.outboundRelayConnections) {
      connection.on?.('error', () => {});
      connection.destroy?.(new Error('Inference relay stopped.'));
    }
    this.outboundRelayConnections.clear();
    this.relayConnections.clear();
    this.relayConnects.clear();
    for (const state of this.streams.values()) {
      state.stream.on('error', () => {});
      state.stream.destroy(new Error('Inference relay stopped.'));
    }
    this.streams.clear();
    if (this.server) {
      for (const session of this.server.sessions) {
        session.destroy?.(new Error('Inference relay stopped.'));
      }
      await this.server.close();
    }
    this.server = null;
    this.sessions.clear();
  }

  hasFallback() {
    return this.relays.length > 0;
  }

  async connectPeer(directSession, remote, waitMs) {
    if (!directSession?.connectPeer) {
      throw new Error('Direct session feature is unavailable.');
    }
    if (!this.hasFallback()) return directSession.connectPeer(remote, waitMs);
    if (this.forceRelay) {
      if (!this.requestRelay(remote)) {
        throw new Error('No official inference relay is configured.');
      }
      try {
        return await this._connectViaRelay(directSession, remote);
      } catch (error) {
        this.clearRelayRequest(remote);
        throw error;
      }
    }

    const directWaitMs = Math.max(1, Math.min(waitMs, this.directWaitMs));
    try {
      return await directSession.connectPeer(remote, directWaitMs);
    } catch (directError) {
      if (!this.requestRelay(remote)) throw directError;
      try {
        return await this._connectViaRelay(directSession, remote);
      } catch (relayError) {
        this.clearRelayRequest(remote);
        throw new Error(
          `Direct connection failed (${directError?.message ?? directError}); `
          + `official relay fallback failed (${relayError?.message ?? relayError}).`
        );
      }
    }
  }

  requestRelay(remote) {
    const key = normalizeKey(remote);
    if (!key || !this.hasFallback()) return false;
    if (!this.forcedPeers.has(key)) {
      this.forcedPeers.add(key);
      this.counters.forced_fallbacks += 1;
    }
    return true;
  }

  clearRelayRequest(remote) {
    const key = normalizeKey(remote);
    if (!key) return;
    this.forcedPeers.delete(key);
  }

  releaseRelay(remote) {
    const key = normalizeKey(remote);
    if (!key || !this.forcedPeers.has(key)) return false;
    setTimeout(() => {
      const stillActive = Array.from(this.peer?.directSession?.sessions?.values?.() || [])
        .some((session) => session.remote === key && session.closed !== true);
      if (stillActive) return;
      if (!this.forcedPeers.has(key)) return;
      this.clearRelayRequest(key);
      this.counters.relay_releases += 1;
      for (const connection of this.peer?.swarm?.connections || []) {
        if (normalizeKey(connection?.remotePublicKey) !== key) continue;
        if (this.connectionRoutes.get(connection)?.relayed !== true) continue;
        connection.on?.('error', () => {});
        connection.destroy?.(new Error('Relay session complete; returning to direct-first mode.'));
      }
    }, 0);
    return true;
  }

  connectionTransport(connection, remote = null) {
    const route = this.connectionRoutes.get(connection);
    const relayed = route?.relayed === true;
    return {
      direct: !relayed,
      relayed,
      relay: relayed ? route?.relay ?? null : null,
    };
  }

  stats() {
    const relayStats = this.server?.stats ?? null;
    return {
      started: this.started,
      serving: this.serve,
      healthy: this.started
        && (!this.serve || this.sessions.size < this.maxClients),
      configured_relays: this.relays.slice(),
      direct_first: this.forceRelay !== true,
      force_relay: this.forceRelay,
      direct_wait_ms: this.directWaitMs,
      relay_wait_ms: this.relayWaitMs,
      limits: {
        clients: this.maxClients,
        pending_pairs: this.maxPendingPairs,
        active_pairs: this.maxActivePairs,
        bytes_per_link: this.maxBytesPerLink,
        bytes_per_second: this.rateBytesPerSecond,
        burst_bytes: this.rateBurstBytes,
        idle_timeout_ms: this.idleTimeoutMs,
      },
      clients: this.sessions.size,
      links: this.streams.size,
      last_sweep_at: this.lastSweepAt,
      relay: relayStats ? {
        sessions: { ...relayStats.sessions },
        pairings: { ...relayStats.pairings },
        streams: { ...relayStats.streams },
      } : null,
      counters: { ...this.counters },
    };
  }

  _handleConnection(connection, peerInfo) {
    if (!connection) return;
    const remote = normalizeKey(connection.remotePublicKey);
    const route = this.connectionRoutes.get(connection);
    if (!route) {
      const relayed = peerInfo?.inferenceRelay === true;
      this.connectionRoutes.set(connection, {
        relayed,
        relay: relayed ? peerInfo.relay ?? null : null,
      });
    }
    if (!this.serve || this.acceptedConnections.has(connection)) return;
    this.acceptedConnections.add(connection);
    if (this.sessions.size >= this.maxClients) {
      this.counters.clients_rejected += 1;
      return;
    }
    if (!b4a.isBuffer(connection.remotePublicKey)
      || connection.remotePublicKey.byteLength !== 32) {
      this.counters.clients_rejected += 1;
      return;
    }
    const session = this.server.accept(connection, { id: connection.remotePublicKey });
    const state = {
      session,
      connection,
      remote,
      accepted_at: Date.now(),
      last_activity_at: Date.now(),
      matched_pairs: 0,
      last_bytes: streamByteCount(connection),
      closing: false,
    };
    this.sessions.set(session, state);
    session.on('open', () => {
      state.last_activity_at = Date.now();
    });
    session.on('pair', () => {
      state.last_activity_at = Date.now();
      state.matched_pairs += 1;
    });
    session.on('error', (error) => {
      this.counters.stream_errors += 1;
      if (this.debug) {
        console.error('[inference-relay] client session error:', error?.message ?? error);
      }
    });
    session.on('close', () => this.sessions.delete(session));
  }

  async _connectViaRelay(directSession, remote) {
    const key = normalizeKey(remote);
    if (!key) throw new Error('Invalid remote peer key.');
    const existing = this.relayConnections.get(key);
    if (existing && existing.destroyed !== true && existing.closed !== true) {
      return {
        remote: key,
        connected: true,
        ...this.connectionTransport(existing, key),
      };
    }
    const inFlight = this.relayConnects.get(key);
    if (inFlight) return inFlight;
    const connecting = this._connectViaRelayUnchecked(directSession, key)
      .finally(() => this.relayConnects.delete(key));
    this.relayConnects.set(key, connecting);
    return connecting;
  }

  async _connectViaRelayUnchecked(directSession, remote) {
    const deadline = Date.now() + this.relayWaitMs;
    let lastError = null;
    for (let index = 0; index < this.relays.length; index += 1) {
      const relay = this.relays[(this.relayCursor + index) % this.relays.length];
      const relaysLeft = this.relays.length - index;
      const remaining = Math.max(1, deadline - Date.now());
      const attemptWaitMs = Math.max(1, Math.floor(remaining / relaysLeft));
      try {
        const connected = await this._openRelayConnection(
          directSession,
          remote,
          relay,
          attemptWaitMs
        );
        this.relayCursor = (this.relays.indexOf(relay) + 1) % this.relays.length;
        return connected;
      } catch (error) {
        lastError = error;
      }
      if (Date.now() >= deadline) break;
    }
    throw lastError || new Error(`No official relay could connect to ${remote}.`);
  }

  async _openRelayConnection(directSession, remote, relay, waitMs) {
    const swarm = this.peer?.swarm;
    if (!swarm?.dht?.connect || !swarm?.keyPair) {
      throw new Error('Peer DHT does not support targeted relay connections.');
    }
    this.counters.relay_selections += 1;
    const connection = swarm.dht.connect(b4a.from(remote, 'hex'), {
      keyPair: swarm.keyPair,
      relayThrough: b4a.from(relay, 'hex'),
    });
    this.outboundRelayConnections.add(connection);
    this.connectionRoutes.set(connection, { relayed: true, relay });
    let connectionError = null;
    connection.on?.('error', (error) => {
      connectionError = error;
      if (this.debug) {
        console.error(
          `[inference-relay] connection ${remote} through ${relay} failed:`,
          error?.code || error?.message || error
        );
      }
    });

    try {
      await new Promise((resolve, reject) => {
        let settled = false;
        let timer = null;
        const finish = (error = null) => {
          if (settled) return;
          settled = true;
          if (timer) clearTimeout(timer);
          connection.off?.('open', onOpen);
          connection.off?.('error', onError);
          connection.off?.('close', onClose);
          if (error) reject(error);
          else resolve();
        };
        const onOpen = () => finish();
        const onError = (error) => finish(error);
        const onClose = () => finish(new Error('Relay connection closed before opening.'));
        timer = setTimeout(
          () => finish(new Error(`Relay connection timed out after ${waitMs} ms.`)),
          waitMs
        );
        connection.once?.('open', onOpen);
        connection.once?.('error', onError);
        connection.once?.('close', onClose);
      });
    } catch (error) {
      this.outboundRelayConnections.delete(connection);
      connection.destroy?.(error);
      throw error;
    }

    this.relayConnections.set(remote, connection);
    directSession?.explicitPeers?.add?.(remote);
    swarm.connections?.add?.(connection);
    connection.once?.('close', () => {
      if (this.debug) {
        const reason = connectionError?.code
          || connectionError?.message
          || (connection.destroyed === true ? 'destroyed' : 'closed');
        console.log(
          `[inference-relay] connection ${remote} through ${relay} closed: ${reason}`
        );
      }
      this.outboundRelayConnections.delete(connection);
      swarm.connections?.delete?.(connection);
      if (this.relayConnections.get(remote) === connection) {
        this.relayConnections.delete(remote);
      }
      swarm.emit?.('update');
    });
    swarm.emit?.('connection', connection, {
      forceRelaying: true,
      inferenceRelay: true,
      relay,
    });
    swarm.emit?.('update');
    return {
      remote,
      connected: true,
      direct: false,
      relayed: true,
      relay,
    };
  }

  _createBoundedStream(options) {
    const stream = this.peer.swarm.dht.createRawStream(options);
    stream.on('error', () => {});
    if (this.streams.size >= this.maxActivePairs * 2) {
      this.counters.active_pairs_rejected += 1;
      queueMicrotask(() => stream.destroy(new Error('Inference relay capacity reached.')));
      return stream;
    }
    const now = Date.now();
    const state = {
      stream,
      created_at: now,
      last_activity_at: now,
      last_checked_at: now,
      last_bytes: 0,
      tokens: this.rateBurstBytes,
      closing: false,
    };
    this.streams.set(stream, state);
    stream.on('close', () => this.streams.delete(stream));
    return stream;
  }

  _sweep() {
    const now = Date.now();
    this.lastSweepAt = now;
    for (const state of this.sessions.values()) {
      if (state.closing) continue;
      const bytes = streamByteCount(state.connection);
      if (bytes > state.last_bytes) {
        state.last_bytes = bytes;
        state.last_activity_at = now;
      }
      const pending = Number(state.session?._pairing?.size) || 0;
      const active = Number(state.session?._links?.size) || 0;
      if (pending === 0 && active === 0 && now - state.last_activity_at > this.idleTimeoutMs) {
        state.closing = true;
        this.counters.clients_idle_closed += 1;
        state.session.destroy?.(new Error('Inference relay client idle timeout.'));
      }
    }
    for (const state of this.streams.values()) {
      if (state.closing) continue;
      const bytes = streamByteCount(state.stream);
      const delta = Math.max(0, bytes - state.last_bytes);
      const elapsedMs = Math.max(1, now - state.last_checked_at);
      state.tokens = Math.min(
        this.rateBurstBytes,
        state.tokens + (elapsedMs / 1000) * this.rateBytesPerSecond
      );
      state.last_checked_at = now;
      state.last_bytes = bytes;
      this.counters.bytes_observed += delta;
      if (delta > 0) state.last_activity_at = now;

      if (bytes > this.maxBytesPerLink) {
        state.closing = true;
        this.counters.links_byte_limited += 1;
        state.stream.destroy(new Error('Inference relay byte limit reached.'));
        continue;
      }
      if (delta > state.tokens) {
        state.closing = true;
        this.counters.links_rate_limited += 1;
        state.stream.destroy(new Error('Inference relay rate limit reached.'));
        continue;
      }
      state.tokens -= delta;
      if (now - state.last_activity_at > this.idleTimeoutMs) {
        state.closing = true;
        this.counters.links_idle_closed += 1;
        state.stream.destroy(new Error('Inference relay link idle timeout.'));
      }
    }
  }
}

export {
  BoundedPairingMap,
  normalizeRelayKeys,
};
export default InferenceRelay;
