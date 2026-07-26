import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';
import Protomux from 'protomux';
import PeerWallet from 'trac-wallet';
import { boundedJsonEncoding } from '../bounded-json.js';

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
const DEFAULT_INBOUND_RELEASE_GRACE_MS = 1_000;
const HEARTBEAT_RELAY_PROTOCOL = 'mx/hb-relay/1';
const DEFAULT_MAX_HEARTBEAT_BYTES = 256 * 1024;
const DEFAULT_MAX_HEARTBEAT_ROOMS_PER_CLIENT = 4096;
const DEFAULT_MAX_HEARTBEAT_ENTRIES = 16_384;
const DEFAULT_MAX_PENDING_HEARTBEATS_PER_CLIENT = 4096;
const HEARTBEAT_RELAY_FRAME_OVERHEAD_BYTES = 1024;
const HEARTBEAT_ROOM_PATTERN = /^mx\/room\/([0-9a-f]{32})$/;
const HEX_32_PATTERN = /^[0-9a-fA-F]{64}$/;
const HEX_64_PATTERN = /^[0-9a-fA-F]{128}$/;

const heartbeatRoom = (value) => {
  const channel = String(value || '').trim();
  return HEARTBEAT_ROOM_PATTERN.test(channel) ? channel : null;
};

const canonicalHeartbeatJson = (value, omitSignature = false) => {
  if (value === null) return 'null';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw new Error('Heartbeat contains a non-finite number.');
    return Number.isInteger(value) ? BigInt(value).toString() : JSON.stringify(value);
  }
  if (typeof value === 'string') return JSON.stringify(value);
  if (Array.isArray(value)) {
    return `[${value.map((child) => canonicalHeartbeatJson(child)).join(',')}]`;
  }
  if (!value || typeof value !== 'object') {
    throw new Error('Heartbeat contains a non-JSON value.');
  }
  const keys = Object.keys(value)
    .filter((key) => !(omitSignature && key === 'sig'))
    .sort();
  return `{${keys.map((key) => (
    `${JSON.stringify(key)}:${canonicalHeartbeatJson(value[key])}`
  )).join(',')}}`;
};

const heartbeatSignatureValid = (value) => {
  try {
    const signature = b4a.from(value.sig, 'hex');
    const provider = b4a.from(value.provider, 'hex');
    const payload = b4a.from(canonicalHeartbeatJson(value, true), 'utf8');
    return PeerWallet.verify(signature, payload, provider) === true;
  } catch (_error) {
    return false;
  }
};

const heartbeatEnvelope = (channel, value, maxBytes = DEFAULT_MAX_HEARTBEAT_BYTES) => {
  const room = heartbeatRoom(channel);
  if (!room || !value || typeof value !== 'object' || Array.isArray(value)) return null;
  let bytes = 0;
  try {
    bytes = b4a.byteLength(JSON.stringify(value), 'utf8');
  } catch (_error) {
    return null;
  }
  if (bytes <= 0 || bytes > maxBytes) return null;
  if (
    value.t !== 'hb'
    || !Number.isSafeInteger(value.v)
    || value.v < 1
    || !Number.isSafeInteger(value.contract_version)
    || value.contract_version < 1
    || typeof value.provider !== 'string'
    || !HEX_32_PATTERN.test(value.provider)
    || typeof value.enclave_id !== 'string'
    || !HEX_32_PATTERN.test(value.enclave_id)
    || typeof value.room_id !== 'string'
    || room !== `mx/room/${value.room_id.toLowerCase()}`
    || typeof value.model_id !== 'string'
    || value.model_id.length === 0
    || typeof value.accepting_new !== 'boolean'
    || !Number.isFinite(value.sat)
    || !value.slots
    || typeof value.slots !== 'object'
    || Array.isArray(value.slots)
    || !value.q
    || typeof value.q !== 'object'
    || Array.isArray(value.q)
    || !value.perf
    || typeof value.perf !== 'object'
    || Array.isArray(value.perf)
    || !value.caps
    || typeof value.caps !== 'object'
    || Array.isArray(value.caps)
    || !value.att
    || typeof value.att !== 'object'
    || Array.isArray(value.att)
    || !Number.isSafeInteger(value.ts)
    || value.ts < 0
    || typeof value.nonce !== 'string'
    || !HEX_32_PATTERN.test(value.nonce)
    || typeof value.sig !== 'string'
    || !HEX_64_PATTERN.test(value.sig)
    || (
      value.transport_peer !== undefined
      && (
        typeof value.transport_peer !== 'string'
        || !HEX_32_PATTERN.test(value.transport_peer)
      )
    )
  ) {
    return null;
  }
  if (!heartbeatSignatureValid(value)) return null;
  return {
    bytes,
    channel: room,
    key: [
      value.provider.toLowerCase(),
      value.enclave_id.toLowerCase(),
      value.room_id.toLowerCase(),
    ].join(':'),
    ts: value.ts,
    heartbeat: value,
  };
};

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
    this.inboundReleaseGraceMs = positiveInteger(
      config.inboundReleaseGraceMs,
      DEFAULT_INBOUND_RELEASE_GRACE_MS
    );
    this.maxHeartbeatBytes = positiveInteger(
      config.maxHeartbeatBytes,
      DEFAULT_MAX_HEARTBEAT_BYTES
    );
    this.maxHeartbeatRoomsPerClient = positiveInteger(
      config.maxHeartbeatRoomsPerClient,
      DEFAULT_MAX_HEARTBEAT_ROOMS_PER_CLIENT
    );
    this.maxHeartbeatEntries = positiveInteger(
      config.maxHeartbeatEntries,
      DEFAULT_MAX_HEARTBEAT_ENTRIES
    );
    this.maxPendingHeartbeatsPerClient = positiveInteger(
      config.maxPendingHeartbeatsPerClient,
      DEFAULT_MAX_PENDING_HEARTBEATS_PER_CLIENT
    );

    this.server = null;
    this.sessions = new Map();
    this.acceptedConnections = new WeakSet();
    this.streams = new Map();
    this.outboundRelayConnections = new Set();
    this.relayConnections = new Map();
    this.relayConnects = new Map();
    this.relayOwners = new Map();
    this.connectionRoutes = new WeakMap();
    this.forcedPeers = new Set();
    this.inboundRelays = new Map();
    this.inboundRelayStreams = new WeakMap();
    this.originalServerFirewall = null;
    this.wrappedServerFirewall = null;
    this.originalServerRelayConnection = null;
    this.wrappedServerRelayConnection = null;
    this.originalServerEmit = null;
    this.wrappedServerEmit = null;
    this.relayCursor = 0;
    this.sweepTimer = null;
    this.heartbeatSink = null;
    this.heartbeatConnectionStates = new Set();
    this.heartbeatStateByConnection = new WeakMap();
    this.heartbeatRoomRefs = new Map();
    this.heartbeatCache = new Map();
    this.heartbeatJoinedRelays = new Set();
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
      heartbeat_frames_rejected: 0,
      heartbeat_clients_rejected: 0,
      heartbeat_coalesced: 0,
      heartbeat_delivered: 0,
      heartbeat_pending_evicted: 0,
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
      this._installInboundRelayTracking(swarm);
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
      this._removeInboundRelayTracking();
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
    this._removeInboundRelayTracking();
    for (const connection of this.outboundRelayConnections) {
      const remote = normalizeKey(connection?.remotePublicKey);
      const owner = remote ? this.relayOwners.get(remote) || this.peer?.directSession : null;
      owner?.clearPreferredConnection?.(remote, connection);
      connection.on?.('error', () => {});
      connection.destroy?.(new Error('Inference relay stopped.'));
    }
    this.outboundRelayConnections.clear();
    this.relayConnections.clear();
    this.relayConnects.clear();
    this.relayOwners.clear();
    this.forcedPeers.clear();
    for (const state of this.heartbeatConnectionStates) {
      try {
        state.channel?.close?.();
      } catch (_error) {}
      state.pending.clear();
      state.subscriptions.clear();
    }
    this.heartbeatConnectionStates.clear();
    this.heartbeatRoomRefs.clear();
    this.heartbeatCache.clear();
    this.heartbeatJoinedRelays.clear();
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

  setHeartbeatSink(handler) {
    this.heartbeatSink = typeof handler === 'function' ? handler : null;
  }

  subscribeHeartbeatRoom(channel) {
    const room = heartbeatRoom(channel);
    if (!room || !this.hasFallback()) return false;
    const current = this.heartbeatRoomRefs.get(room) || 0;
    if (current === 0 && this.heartbeatRoomRefs.size >= this.maxHeartbeatRoomsPerClient) {
      return false;
    }
    this.heartbeatRoomRefs.set(room, current + 1);
    if (current === 0) {
      this._ensureHeartbeatRelayConnections();
      this._sendHeartbeatControlToRelays({ t: 'hb.subscribe', v: 1, room });
    }
    return true;
  }

  unsubscribeHeartbeatRoom(channel) {
    const room = heartbeatRoom(channel);
    if (!room) return false;
    const current = this.heartbeatRoomRefs.get(room) || 0;
    if (current <= 0) return false;
    if (current === 1) {
      this.heartbeatRoomRefs.delete(room);
      this._sendHeartbeatControlToRelays({ t: 'hb.unsubscribe', v: 1, room });
    } else {
      this.heartbeatRoomRefs.set(room, current - 1);
    }
    return true;
  }

  observeSidechannelHeartbeat(channel, payload) {
    const heartbeat = payload?.message ?? payload;
    const envelope = heartbeatEnvelope(channel, heartbeat, this.maxHeartbeatBytes);
    if (!envelope) return { heartbeat: false, emit: true };
    const locallyPublished = payload?.origin === 'local';
    const newest = this._rememberHeartbeat(envelope, locallyPublished);
    if (locallyPublished) {
      this._ensureHeartbeatRelayConnections();
      this._sendHeartbeatControlToRelays({
        t: 'hb.publish',
        v: 1,
        room: envelope.channel,
        heartbeat: envelope.heartbeat,
      });
    }
    if (!newest) return { heartbeat: true, emit: false };
    if (this.serve) this._fanoutHeartbeat(envelope);
    return { heartbeat: true, emit: true };
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
    this.relayOwners.delete(key);
  }

  releaseRelay(remote) {
    const key = normalizeKey(remote);
    if (!key || !this.forcedPeers.has(key)) return false;
    setTimeout(() => {
      const stillActive = Array.from(this.peer?.directSession?.sessions?.values?.() || [])
        .some((session) => session.remote === key && session.closed !== true);
      if (stillActive) return;
      if (!this.forcedPeers.has(key)) return;
      const directSession = this.relayOwners.get(key) || this.peer?.directSession;
      const ownedConnection = this.relayConnections.get(key);
      this.forcedPeers.delete(key);
      this.relayOwners.delete(key);
      if (!ownedConnection) {
        this.counters.relay_releases += 1;
        return;
      }
      directSession?.clearPreferredConnection?.(key, ownedConnection);
      this.outboundRelayConnections.delete(ownedConnection);
      if (this.relayConnections.get(key) === ownedConnection) {
        this.relayConnections.delete(key);
      }
      ownedConnection.on?.('error', () => {});
      ownedConnection.destroy?.(
        new Error('Relay session complete; returning to direct-first mode.')
      );
      this.counters.relay_releases += 1;
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
        heartbeat_bytes: this.maxHeartbeatBytes,
        heartbeat_rooms_per_client: this.maxHeartbeatRoomsPerClient,
        heartbeat_entries: this.maxHeartbeatEntries,
        pending_heartbeats_per_client: this.maxPendingHeartbeatsPerClient,
      },
      clients: this.sessions.size,
      links: this.streams.size,
      last_sweep_at: this.lastSweepAt,
      heartbeat_rooms: Array.from(this.heartbeatRoomRefs.keys()).sort(),
      heartbeat_entries: this.heartbeatCache.size,
      heartbeat_clients: this.heartbeatConnectionStates.size,
      relay: relayStats ? {
        sessions: { ...relayStats.sessions },
        pairings: { ...relayStats.pairings },
        streams: { ...relayStats.streams },
      } : null,
      counters: { ...this.counters },
    };
  }

  _ensureHeartbeatRelayConnections() {
    const swarm = this.peer?.swarm;
    if (!swarm || typeof swarm.joinPeer !== 'function') return;
    for (const relay of this.relays) {
      let connected = false;
      for (const connection of swarm.connections || []) {
        if (normalizeKey(connection?.remotePublicKey) !== relay) continue;
        connected = true;
        const state = this._prepareHeartbeatConnection(connection);
        if (state) this._openHeartbeatChannel(state);
      }
      if (connected || this.heartbeatJoinedRelays.has(relay)) continue;
      try {
        swarm.joinPeer(b4a.from(relay, 'hex'));
        this.heartbeatJoinedRelays.add(relay);
      } catch (error) {
        if (this.debug) {
          console.error(
            `[inference-relay] heartbeat relay join ${relay} failed:`,
            error?.message ?? error
          );
        }
      }
    }
  }

  _prepareHeartbeatConnection(connection) {
    if (!connection) return null;
    const existing = this.heartbeatStateByConnection.get(connection);
    if (existing) return existing;
    const remote = normalizeKey(connection.remotePublicKey);
    if (!remote) return null;
    if (!this.serve && !this.relays.includes(remote)) return null;
    if (
      this.serve
      && !this.relays.includes(remote)
      && this.heartbeatConnectionStates.size >= this.maxClients
    ) {
      this.counters.heartbeat_clients_rejected += 1;
      return null;
    }
    let mux = null;
    try {
      mux = Protomux.from(connection);
      connection.userData = mux;
    } catch (_error) {
      return null;
    }
    const state = {
      connection,
      remote,
      mux,
      channel: null,
      message: null,
      opened: false,
      blocked: false,
      subscriptions: new Set(),
      pendingControls: new Map(),
      pending: new Map(),
    };
    this.heartbeatStateByConnection.set(connection, state);
    this.heartbeatConnectionStates.add(state);
    mux.pair?.({ protocol: HEARTBEAT_RELAY_PROTOCOL }, () => {
      this._openHeartbeatChannel(state);
    });
    connection.once?.('close', () => {
      state.opened = false;
      state.pending.clear();
      state.pendingControls.clear();
      state.subscriptions.clear();
      this.heartbeatConnectionStates.delete(state);
      if (this.relays.includes(state.remote)) {
        this.heartbeatJoinedRelays.delete(state.remote);
        const needsRelay = this.heartbeatRoomRefs.size > 0
          || Array.from(this.heartbeatCache.values()).some((record) => record.publishable);
        if (this.started && needsRelay) {
          setTimeout(() => {
            if (this.started) this._ensureHeartbeatRelayConnections();
          }, 0);
        }
      }
    });
    if (this.relays.includes(remote)) this._openHeartbeatChannel(state);
    return state;
  }

  _openHeartbeatChannel(state) {
    if (
      !state
      || state.channel
      || state.connection?.destroyed === true
      || state.connection?.closed === true
    ) {
      return;
    }
    let channel = null;
    try {
      channel = state.mux.createChannel({
        protocol: HEARTBEAT_RELAY_PROTOCOL,
        onopen: () => {
          state.opened = true;
          state.blocked = false;
          if (this.relays.includes(state.remote)) {
            for (const room of this.heartbeatRoomRefs.keys()) {
              this._queueHeartbeatControl(state, { t: 'hb.subscribe', v: 1, room });
            }
            for (const record of this.heartbeatCache.values()) {
              if (!record.publishable) continue;
              this._queueHeartbeatFrame(state, {
                t: 'hb.publish',
                v: 1,
                room: record.envelope.channel,
                heartbeat: record.envelope.heartbeat,
              }, record.envelope);
            }
          }
          this._flushHeartbeatState(state);
        },
        onclose: () => {
          state.opened = false;
          state.blocked = false;
          state.channel = null;
          state.message = null;
        },
        ondrain: () => {
          state.blocked = false;
          this._flushHeartbeatState(state);
        },
      });
      if (!channel) return;
      state.channel = channel;
      state.message = channel.addMessage({
        encoding: boundedJsonEncoding(
          this.maxHeartbeatBytes + HEARTBEAT_RELAY_FRAME_OVERHEAD_BYTES,
          'Room heartbeat relay frame',
          { maxStringBytes: this.maxHeartbeatBytes }
        ),
        onmessage: (frame) => this._handleHeartbeatRelayFrame(state, frame),
      });
      channel.open();
    } catch (error) {
      state.channel = null;
      state.message = null;
      if (this.debug) {
        console.error(
          `[inference-relay] heartbeat channel ${state.remote} failed:`,
          error?.message ?? error
        );
      }
    }
  }

  _sendHeartbeatControlToRelays(frame) {
    for (const state of this.heartbeatConnectionStates) {
      if (!this.relays.includes(state.remote)) continue;
      if (frame?.t === 'hb.publish') {
        const envelope = heartbeatEnvelope(
          frame.room,
          frame.heartbeat,
          this.maxHeartbeatBytes
        );
        if (envelope) this._queueHeartbeatFrame(state, frame, envelope);
      } else {
        this._queueHeartbeatControl(state, frame);
      }
    }
  }

  _queueHeartbeatControl(state, frame) {
    const room = heartbeatRoom(frame?.room);
    if (!state || !room) return false;
    state.pendingControls.set(room, { ...frame, room });
    this._flushHeartbeatState(state);
    return true;
  }

  _sendHeartbeatControlToState(state, frame) {
    if (!state?.opened || !state.message || state.blocked) return false;
    try {
      const writable = state.message.send(frame);
      if (writable === false) state.blocked = true;
      return true;
    } catch (_error) {
      return false;
    }
  }

  _queueHeartbeatFrame(state, frame, envelope) {
    if (!state || !envelope) return false;
    const existing = state.pending.get(envelope.key);
    if (existing && existing.envelope.ts >= envelope.ts) {
      this.counters.heartbeat_coalesced += 1;
      return false;
    }
    if (existing) state.pending.delete(envelope.key);
    while (
      !state.pending.has(envelope.key)
      && state.pending.size >= this.maxPendingHeartbeatsPerClient
    ) {
      const oldest = state.pending.keys().next().value;
      if (oldest === undefined) break;
      state.pending.delete(oldest);
      this.counters.heartbeat_pending_evicted += 1;
    }
    state.pending.set(envelope.key, { frame, envelope });
    this._flushHeartbeatState(state);
    return true;
  }

  _flushHeartbeatState(state) {
    if (!state?.opened || !state.message || state.blocked) return;
    while (state.pendingControls.size > 0 && !state.blocked) {
      const [room, frame] = state.pendingControls.entries().next().value;
      if (!this._sendHeartbeatControlToState(state, frame)) return;
      state.pendingControls.delete(room);
    }
    while (state.pending.size > 0 && !state.blocked) {
      const [key, record] = state.pending.entries().next().value;
      if (!this._sendHeartbeatControlToState(state, record.frame)) return;
      state.pending.delete(key);
      this.counters.heartbeat_delivered += 1;
    }
  }

  _handleHeartbeatRelayFrame(state, frame) {
    if (!frame || typeof frame !== 'object' || Array.isArray(frame) || frame.v !== 1) {
      this.counters.heartbeat_frames_rejected += 1;
      return false;
    }
    const room = heartbeatRoom(frame.room);
    if (!room) {
      this.counters.heartbeat_frames_rejected += 1;
      return false;
    }
    if (frame.t === 'hb.subscribe') {
      if (!this.serve) {
        this.counters.heartbeat_frames_rejected += 1;
        return false;
      }
      if (
        !state.subscriptions.has(room)
        && state.subscriptions.size >= this.maxHeartbeatRoomsPerClient
      ) {
        this.counters.heartbeat_frames_rejected += 1;
        return false;
      }
      state.subscriptions.add(room);
      for (const record of this.heartbeatCache.values()) {
        if (record.envelope.channel !== room) continue;
        this._queueHeartbeatFrame(state, {
          t: 'hb.event',
          v: 1,
          room,
          heartbeat: record.envelope.heartbeat,
        }, record.envelope);
      }
      return true;
    }
    if (frame.t === 'hb.unsubscribe') {
      if (!this.serve) {
        this.counters.heartbeat_frames_rejected += 1;
        return false;
      }
      state.subscriptions.delete(room);
      for (const [key, record] of state.pending) {
        if (record.envelope.channel === room) state.pending.delete(key);
      }
      return true;
    }

    const envelope = heartbeatEnvelope(room, frame.heartbeat, this.maxHeartbeatBytes);
    if (!envelope) {
      this.counters.heartbeat_frames_rejected += 1;
      return false;
    }
    if (frame.t === 'hb.publish') {
      if (
        !this.serve
        || envelope.heartbeat.transport_peer?.toLowerCase() !== state.remote
        || !this._rememberHeartbeat(envelope, false)
      ) {
        this.counters.heartbeat_frames_rejected += 1;
        return false;
      }
      this._fanoutHeartbeat(envelope);
      return true;
    }
    if (frame.t === 'hb.event') {
      if (
        !this.relays.includes(state.remote)
        || !this.heartbeatRoomRefs.has(room)
        || !this._rememberHeartbeat(envelope, false)
      ) {
        return false;
      }
      this.heartbeatSink?.(room, envelope.heartbeat, {
        relay: state.remote,
        authoritative: false,
      });
      return true;
    }
    this.counters.heartbeat_frames_rejected += 1;
    return false;
  }

  _rememberHeartbeat(envelope, publishable = false) {
    const current = this.heartbeatCache.get(envelope.key);
    if (current && current.envelope.ts >= envelope.ts) {
      if (publishable) current.publishable = true;
      this.counters.heartbeat_coalesced += 1;
      return false;
    }
    if (current) this.heartbeatCache.delete(envelope.key);
    while (
      !this.heartbeatCache.has(envelope.key)
      && this.heartbeatCache.size >= this.maxHeartbeatEntries
    ) {
      const oldest = this.heartbeatCache.keys().next().value;
      if (oldest === undefined) break;
      this.heartbeatCache.delete(oldest);
    }
    this.heartbeatCache.set(envelope.key, { envelope, publishable });
    return true;
  }

  _fanoutHeartbeat(envelope) {
    for (const state of this.heartbeatConnectionStates) {
      if (!state.subscriptions.has(envelope.channel)) continue;
      this._queueHeartbeatFrame(state, {
        t: 'hb.event',
        v: 1,
        room: envelope.channel,
        heartbeat: envelope.heartbeat,
      }, envelope);
    }
  }

  _handleConnection(connection, peerInfo) {
    if (!connection) return;
    this._prepareHeartbeatConnection(connection);
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
    this.relayOwners.set(remote, directSession);
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
    this.connectionRoutes.set(connection, { relayed: true, relay, authoritative: false });
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

    const deadline = Date.now() + waitMs;
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
        if (connection.opened === true) {
          finish();
          return;
        }
        timer = setTimeout(
          () => finish(new Error(`Relay connection timed out after ${waitMs} ms.`)),
          waitMs
        );
        connection.once?.('open', onOpen);
        connection.once?.('error', onError);
        connection.once?.('close', onClose);
      });
      if (
        typeof directSession?.proveConnection !== 'function'
        || typeof directSession?.preferConnection !== 'function'
      ) {
        throw new Error('Direct session relay handover support is unavailable.');
      }
      const remaining = Math.max(1, deadline - Date.now());
      await directSession.proveConnection(connection, remaining);
    } catch (error) {
      this.outboundRelayConnections.delete(connection);
      directSession?._dropHealthConnection?.(connection);
      connection.destroy?.(error);
      throw error;
    }

    const previous = this.relayConnections.get(remote) ?? null;
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
      directSession?.clearPreferredConnection?.(remote, connection);
      if (this.relayConnections.get(remote) === connection) {
        this.relayConnections.delete(remote);
      }
    });
    try {
      this.connectionRoutes.set(connection, { relayed: true, relay, authoritative: true });
      directSession.preferConnection(remote, connection);
    } catch (error) {
      this.outboundRelayConnections.delete(connection);
      directSession.clearPreferredConnection?.(remote, connection);
      directSession._dropHealthConnection?.(connection);
      connection.destroy?.(error);
      throw error;
    }
    directSession.explicitPeers?.add?.(remote);
    this.relayConnections.set(remote, connection);
    if (previous && previous !== connection) {
      this.outboundRelayConnections.delete(previous);
      previous.on?.('error', () => {});
      previous.destroy?.(new Error('Replaced by a proven official relay connection.'));
    }
    return {
      remote,
      connected: true,
      direct: false,
      relayed: true,
      relay,
    };
  }

  _installInboundRelayTracking(swarm) {
    if (!this.hasFallback()) return;
    const server = swarm?.server;
    if (
      !server
      || typeof server.firewall !== 'function'
      || typeof server._relayConnection !== 'function'
      || typeof server.emit !== 'function'
    ) {
      throw new Error('Peer transport cannot track official inbound relay ownership.');
    }
    this.originalServerFirewall = server.firewall;
    this.wrappedServerFirewall = (remotePublicKey, payload, address) => {
      const blocked = this.originalServerFirewall(remotePublicKey, payload, address);
      if (blocked === false) this._trackInboundRelayHandshake(remotePublicKey, payload);
      return blocked;
    };
    server.firewall = this.wrappedServerFirewall;
    this.originalServerRelayConnection = server._relayConnection;
    this.wrappedServerRelayConnection = (handshake, relayThrough, payload, noise) => {
      const relay = normalizeKey(relayThrough)
        || normalizeKey(payload?.relayThrough?.publicKey);
      if (relay && this.relays.includes(relay) && handshake?.rawStream) {
        this.inboundRelayStreams.set(handshake.rawStream, { relay });
      }
      return this.originalServerRelayConnection.call(
        server,
        handshake,
        relayThrough,
        payload,
        noise
      );
    };
    server._relayConnection = this.wrappedServerRelayConnection;
    this.originalServerEmit = server.emit;
    this.wrappedServerEmit = (event, ...args) => {
      if (event === 'connection' && this._claimInboundRelay(args[0])) return true;
      return this.originalServerEmit.call(server, event, ...args);
    };
    server.emit = this.wrappedServerEmit;
  }

  _removeInboundRelayTracking() {
    const server = this.peer?.swarm?.server;
    if (server) {
      if (server.emit === this.wrappedServerEmit && this.originalServerEmit) {
        server.emit = this.originalServerEmit;
      }
      if (server.firewall === this.wrappedServerFirewall && this.originalServerFirewall) {
        server.firewall = this.originalServerFirewall;
      }
      if (
        server._relayConnection === this.wrappedServerRelayConnection
        && this.originalServerRelayConnection
      ) {
        server._relayConnection = this.originalServerRelayConnection;
      }
    }
    this.originalServerFirewall = null;
    this.wrappedServerFirewall = null;
    this.originalServerRelayConnection = null;
    this.wrappedServerRelayConnection = null;
    this.originalServerEmit = null;
    this.wrappedServerEmit = null;
    for (const [remote, state] of this.inboundRelays) {
      for (const pending of state.pending) clearTimeout(pending.timer);
      if (state.releaseTimer) clearTimeout(state.releaseTimer);
      for (const timer of state.retireTimers.values()) clearTimeout(timer);
      for (const connection of state.connections) {
        this.peer?.directSession?.clearPreferredConnection?.(remote, connection);
        connection.on?.('error', () => {});
        connection.destroy?.(new Error('Inference relay stopped.'));
      }
    }
    this.inboundRelays.clear();
  }

  _trackInboundRelayHandshake(remotePublicKey, payload) {
    const remote = normalizeKey(remotePublicKey);
    const relay = normalizeKey(payload?.relayThrough?.publicKey);
    if (!remote || !relay || !this.relays.includes(relay)) return false;
    const state = this._inboundRelayState(remote);
    if (state.releaseTimer) {
      clearTimeout(state.releaseTimer);
      state.releaseTimer = null;
    }
    const pending = { relay, timer: null };
    pending.timer = setTimeout(() => {
      const index = state.pending.indexOf(pending);
      if (index !== -1) state.pending.splice(index, 1);
      this._scheduleInboundReconnect(remote, state);
    }, this.relayWaitMs);
    pending.timer.unref?.();
    state.pending.push(pending);
    return true;
  }

  _claimInboundRelay(connection) {
    const remote = normalizeKey(connection?.remotePublicKey);
    const streamRoute = this.inboundRelayStreams.get(connection?.rawStream);
    const state = remote ? this.inboundRelays.get(remote) : null;
    if (!state || !streamRoute || state.pending.length === 0) return false;
    const pendingIndex = state.pending.findIndex(
      (candidate) => candidate.relay === streamRoute.relay
    );
    if (pendingIndex === -1) return false;
    const [pending] = state.pending.splice(pendingIndex, 1);
    clearTimeout(pending.timer);
    if (state.candidate?.destroyed === true || state.candidate?.closed === true) {
      state.candidate = null;
    }
    if (state.candidate || state.retiring.size > 0) {
      connection.on?.('error', () => {});
      connection.destroy?.(new Error('Duplicate official inbound relay connection.'));
      return true;
    }
    state.candidate = connection;
    state.connections.add(connection);
    this.connectionRoutes.set(connection, {
      relayed: true,
      relay: pending.relay,
      authoritative: false,
    });
    connection.once?.('close', () => {
      this.peer?.directSession?.clearPreferredConnection?.(remote, connection);
      if (state.active === connection) state.active = null;
      if (state.candidate === connection) state.candidate = null;
      state.retiring.delete(connection);
      const retireTimer = state.retireTimers.get(connection);
      if (retireTimer) clearTimeout(retireTimer);
      state.retireTimers.delete(connection);
      state.connections.delete(connection);
      this._scheduleInboundReconnect(remote, state);
    });
    void this._admitInboundRelay(remote, state, connection, pending.relay);
    return true;
  }

  async _admitInboundRelay(remote, state, connection, relay) {
    const directSession = this.peer?.directSession;
    try {
      if (
        typeof directSession?.proveConnection !== 'function'
        || typeof directSession?.preferConnection !== 'function'
      ) {
        throw new Error('Direct session relay handover support is unavailable.');
      }
      await directSession.proveConnection(connection, this.relayWaitMs);
      if (connection.destroyed === true || connection.closed === true) {
        throw new Error('Inbound relay connection closed during admission.');
      }
      this.connectionRoutes.set(connection, {
        relayed: true,
        relay,
        authoritative: true,
      });
      const previous = state.active;
      directSession.preferConnection(remote, connection);
      state.active = connection;
      if (state.candidate === connection) state.candidate = null;
      if (previous && previous !== connection) {
        this._retireInboundConnection(remote, state, previous, directSession);
      }
    } catch (error) {
      if (state.active === connection) state.active = null;
      if (state.candidate === connection) state.candidate = null;
      state.connections.delete(connection);
      directSession?._dropHealthConnection?.(connection);
      connection.on?.('error', () => {});
      connection.destroy?.(error);
      this._scheduleInboundReconnect(remote, state);
    }
  }

  _inboundRelayState(remote) {
    let state = this.inboundRelays.get(remote);
    if (state) return state;
    state = {
      pending: [],
      connections: new Set(),
      active: null,
      candidate: null,
      retiring: new Set(),
      retireTimers: new Map(),
      releaseTimer: null,
    };
    this.inboundRelays.set(remote, state);
    return state;
  }

  _retireInboundConnection(remote, state, connection, directSession) {
    const hasActiveSessions = () => Array.from(directSession?.sessions?.values?.() || [])
      .some((session) => session.connection === connection && session.closed !== true);
    const retire = () => {
      state.retireTimers.delete(connection);
      if (connection.destroyed === true || connection.closed === true) {
        state.retiring.delete(connection);
        state.connections.delete(connection);
        this._scheduleInboundReconnect(remote, state);
        return;
      }
      if (hasActiveSessions()) {
        const timer = setTimeout(retire, this.inboundReleaseGraceMs);
        timer.unref?.();
        state.retireTimers.set(connection, timer);
        return;
      }
      state.retiring.delete(connection);
      state.connections.delete(connection);
      connection.on?.('error', () => {});
      connection.destroy?.(new Error('Replaced by a proven official relay connection.'));
    };
    state.retiring.add(connection);
    retire();
  }

  _scheduleInboundReconnect(remote, state) {
    if (state.pending.length > 0 || state.connections.size > 0 || state.releaseTimer) return;
    state.releaseTimer = setTimeout(() => {
      state.releaseTimer = null;
      if (state.pending.length > 0 || state.connections.size > 0) return;
      this.inboundRelays.delete(remote);
    }, this.inboundReleaseGraceMs);
    state.releaseTimer.unref?.();
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
  HEARTBEAT_RELAY_PROTOCOL,
  canonicalHeartbeatJson,
  heartbeatEnvelope,
  heartbeatRoom,
  normalizeRelayKeys,
};
export default InferenceRelay;
