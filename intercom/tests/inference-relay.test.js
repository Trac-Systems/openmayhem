import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import b4a from 'b4a';
import InferenceRelay, {
  BoundedPairingMap,
  normalizeRelayKeys,
} from '../features/inference-relay/index.js';

const localKey = '11'.repeat(32);
const relayA = '22'.repeat(32);
const relayB = '33'.repeat(32);
const remote = '44'.repeat(32);
const remoteB = '55'.repeat(32);

class MockSwarm extends EventEmitter {
  constructor({ relayOutcomes = [] } = {}) {
    super();
    this.keyPair = { publicKey: b4a.from(localKey, 'hex') };
    this.relayAttempts = [];
    this.relayOutcomes = relayOutcomes.slice();
    this.dht = {
      randomized: false,
      connect: (remotePublicKey, options) => {
        const connection = new EventEmitter();
        connection.remotePublicKey = remotePublicKey;
        connection.destroyed = false;
        connection.closed = false;
        connection.destroy = (error) => {
          if (connection.closed) return;
          connection.error = error;
          connection.destroyed = true;
          connection.closed = true;
          connection.emit('close');
        };
        this.relayAttempts.push({ connection, remotePublicKey, options });
        const outcome = this.relayOutcomes.shift() || 'open';
        queueMicrotask(() => {
          if (outcome === 'open') connection.emit('open');
          if (outcome === 'error') connection.emit('error', new Error('relay unavailable'));
          if (outcome === 'close') connection.emit('close');
        });
        return connection;
      },
    };
    this.connections = new Set();
    const tracked = new Map();
    this._allConnections = {
      get: (key) => tracked.get(b4a.isBuffer(key) ? b4a.toString(key, 'hex') : key),
      add: (connection) => tracked.set(
        b4a.toString(connection.remotePublicKey, 'hex'),
        connection
      ),
      delete: (connection) => {
        const key = b4a.toString(connection.remotePublicKey, 'hex');
        if (tracked.get(key) === connection) tracked.delete(key);
      },
    };
    this.peers = new Map();
    this.relayThrough = null;
    this.server = new EventEmitter();
    this.server.firewall = () => false;
    this.server._relayConnection = () => {};
  }
}

const mockPeer = (swarm = new MockSwarm()) => ({
  wallet: { publicKey: localKey },
  swarm,
  directSession: { sessions: new Map() },
});

test('relay keys are canonical, unique, and never include the local peer', () => {
  assert.deepEqual(
    normalizeRelayKeys([relayA, relayA.toUpperCase(), localKey, 'bad', relayB], localKey),
    [relayA, relayB]
  );
});

test('targeted relay connections rotate official peers without changing global swarm routing', async () => {
  const swarm = new MockSwarm();
  const original = () => null;
  swarm.relayThrough = original;
  const directSession = {
    explicitPeers: new Set(),
    suspendReconnect: () => {},
    resumeReconnect: () => {},
  };
  const relay = new InferenceRelay(mockPeer(swarm), { relays: [relayA, relayB] });
  await relay.start();

  relay.requestRelay(remote);
  relay.requestRelay(remoteB);
  const first = await relay._connectViaRelay(directSession, remote);
  const second = await relay._connectViaRelay(directSession, remoteB);
  assert.equal(first.relay, relayA);
  assert.equal(second.relay, relayB);
  assert.equal(b4a.toString(swarm.relayAttempts[0].remotePublicKey, 'hex'), remote);
  assert.equal(b4a.toString(swarm.relayAttempts[0].options.relayThrough, 'hex'), relayA);
  assert.equal(b4a.toString(swarm.relayAttempts[1].remotePublicKey, 'hex'), remoteB);
  assert.equal(b4a.toString(swarm.relayAttempts[1].options.relayThrough, 'hex'), relayB);
  assert.equal(swarm.relayThrough, original);
  assert.equal(relay.stats().counters.relay_selections, 2);

  await relay.stop();
  assert.equal(swarm.relayThrough, original);
});

test('targeted fallback rotates past an offline official relay', async () => {
  const swarm = new MockSwarm({ relayOutcomes: ['error', 'open'] });
  const directSession = {
    explicitPeers: new Set(),
    suspendReconnect: () => {},
    resumeReconnect: () => {},
  };
  const relay = new InferenceRelay(mockPeer(swarm), {
    relays: [relayA, relayB],
    relayWaitMs: 100,
  });
  await relay.start();

  relay.requestRelay(remote);
  const connected = await relay._connectViaRelay(directSession, remote);

  assert.equal(connected.relayed, true);
  assert.equal(connected.relay, relayB);
  assert.equal(swarm.relayAttempts.length, 2);
  assert.equal(b4a.toString(swarm.relayAttempts[0].options.relayThrough, 'hex'), relayA);
  assert.equal(b4a.toString(swarm.relayAttempts[1].options.relayThrough, 'hex'), relayB);
  assert.equal(swarm.relayAttempts[0].connection.destroyed, true);
  assert.equal(relay.stats().counters.relay_selections, 2);

  await relay.stop();
});

test('all official relays offline fails once within the total bound before session admission', async () => {
  const swarm = new MockSwarm({ relayOutcomes: ['error', 'error'] });
  let directAttempts = 0;
  let admittedSessions = 0;
  const directSession = {
    explicitPeers: new Set(),
    suspendReconnect() {},
    resumeReconnect() {},
    async connectPeer() {
      directAttempts += 1;
      throw new Error('direct unavailable');
    },
    sessionOpen() {
      admittedSessions += 1;
    },
  };
  const relay = new InferenceRelay(mockPeer(swarm), {
    relays: [relayA, relayB],
    directWaitMs: 10,
    relayWaitMs: 100,
  });
  await relay.start();

  const startedAt = Date.now();
  await assert.rejects(
    relay.connectPeer(directSession, remote, 1_000),
    /Direct connection failed.*official relay fallback failed/
  );
  const elapsedMs = Date.now() - startedAt;

  assert.equal(directAttempts, 1);
  assert.equal(admittedSessions, 0);
  assert.equal(swarm.relayAttempts.length, 2);
  assert.ok(elapsedMs < 250, `relay failure exceeded its total bound: ${elapsedMs} ms`);
  assert.equal(relay.forcedPeers.has(remote), false);
  assert.equal(relay.relayConnections.has(remote), false);
  assert.equal(relay.relayConnects.has(remote), false);

  await relay.stop();
});

test('failed relay-server startup restores the peer transport selector', async () => {
  const swarm = new MockSwarm();
  const original = () => null;
  swarm.relayThrough = original;
  const relay = new InferenceRelay(mockPeer(swarm), {
    relays: [relayA],
    serve: true,
    RelayServer: {},
  });

  await assert.rejects(relay.start(), /Server is unavailable/);
  assert.equal(relay.started, false);
  assert.equal(relay.server, null);
  assert.equal(swarm.relayThrough, original);
  assert.equal(swarm.listenerCount('connection'), 0);
});

test('requestRelay marks only Mayhem session state and never mutates Hyperswarm routing', async () => {
  const swarm = new MockSwarm();
  const peerInfo = { forceRelaying: false, attempts: 9 };
  swarm.peers.set(remote, peerInfo);
  const relay = new InferenceRelay(mockPeer(swarm), { relays: [relayA] });
  await relay.start();

  assert.equal(relay.requestRelay(remote), true);
  assert.equal(relay.requestRelay(remote), true);
  assert.equal(peerInfo.forceRelaying, false);
  assert.equal(peerInfo.attempts, 9);
  assert.equal(relay.forcedPeers.has(remote), true);
  assert.equal(relay.stats().counters.forced_fallbacks, 1);
  assert.deepEqual(relay.connectionTransport({ remotePublicKey: b4a.from(remote, 'hex') }), {
    direct: true,
    relayed: false,
    relay: null,
  });
  relay.clearRelayRequest(remote);
  assert.equal(peerInfo.forceRelaying, false);
  assert.equal(relay.forcedPeers.has(remote), false);

  await relay.stop();
});

test('relay release waits for the final active session then restores direct-first', async () => {
  const swarm = new MockSwarm();
  const connection = new EventEmitter();
  connection.remotePublicKey = b4a.from(remote, 'hex');
  connection.destroyed = false;
  connection.closed = false;
  connection.destroy = () => {
    connection.destroyed = true;
    connection.closed = true;
    connection.emit('close');
  };
  swarm.connections.add(connection);
  swarm.peers.set(remote, { forceRelaying: true, attempts: 0 });
  const peer = mockPeer(swarm);
  peer.directSession.sessions.set('one', { remote, closed: false });
  const relay = new InferenceRelay(peer, { relays: [relayA] });
  relay.forcedPeers.add(remote);
  relay.connectionRoutes.set(connection, { relayed: true, relay: relayA });

  assert.equal(relay.releaseRelay(remote), true);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(relay.forcedPeers.has(remote), true);
  assert.equal(connection.destroyed, false);

  peer.directSession.sessions.clear();
  assert.equal(relay.releaseRelay(remote), true);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(relay.forcedPeers.has(remote), false);
  assert.equal(connection.destroyed, true);
  assert.equal(relay.stats().counters.relay_releases, 1);
});

test('pending pairing allocation stays bounded without throwing through the event loop', () => {
  let rejected = 0;
  let rejectedPair = null;
  const pairs = new BoundedPairingMap(2, (_key, pair) => {
    rejected += 1;
    rejectedPair = pair;
  });
  pairs.set('a', {});
  pairs.set('b', {});
  const third = {};
  assert.doesNotThrow(() => pairs.set('c', third));
  assert.equal(pairs.size, 2);
  assert.equal(pairs.has('c'), false);
  assert.equal(rejected, 1);
  assert.equal(rejectedPair, third);
});

test('relay stream byte cap destroys only the offending link', () => {
  const swarm = new MockSwarm();
  const streams = [];
  swarm.dht.createRawStream = () => {
    const stream = new EventEmitter();
    stream.bytesReceived = 0;
    stream.bytesTransmitted = 0;
    stream.destroyed = false;
    stream.destroy = (error) => {
      stream.destroyed = true;
      stream.error = error;
      stream.emit('close');
    };
    streams.push(stream);
    return stream;
  };
  const relay = new InferenceRelay(mockPeer(swarm), {
    maxBytesPerLink: 10,
    rateBytesPerSecond: 1_000_000,
    rateBurstBytes: 1_000_000,
  });
  const first = relay._createBoundedStream({});
  const second = relay._createBoundedStream({});
  first.bytesReceived = 11;
  relay._sweep();

  assert.equal(first.destroyed, true);
  assert.match(first.error.message, /byte limit/);
  assert.equal(second.destroyed, false);
  assert.equal(relay.stats().counters.links_byte_limited, 1);
});

test('idle cleanup closes only inactive relay control sessions', () => {
  const relay = new InferenceRelay(mockPeer(), { idleTimeoutMs: 50 });
  let idleDestroyed = false;
  let activeDestroyed = false;
  const idleSession = {
    _pairing: new Set(),
    _links: new Map(),
    destroy: () => { idleDestroyed = true; },
  };
  const activeSession = {
    _pairing: new Set(['pending']),
    _links: new Map(),
    destroy: () => { activeDestroyed = true; },
  };
  relay.sessions.set(idleSession, {
    session: idleSession,
    connection: {},
    last_activity_at: Date.now() - 1_000,
    last_bytes: 0,
  });
  relay.sessions.set(activeSession, {
    session: activeSession,
    connection: {},
    last_activity_at: Date.now() - 1_000,
    last_bytes: 0,
  });

  relay._sweep();
  assert.equal(idleDestroyed, true);
  assert.equal(activeDestroyed, false);
  assert.equal(relay.stats().counters.clients_idle_closed, 1);
});

test('relay policy retries peer connection before any session frame exists', async () => {
  const waits = [];
  let attempts = 0;
  const directSession = {
    explicitPeers: new Set(),
    suspendReconnect() {},
    resumeReconnect() {},
    async connectPeer(_remote, waitMs) {
      waits.push(waitMs);
      attempts += 1;
      if (attempts === 1) throw new Error('direct unavailable');
      return { remote, connected: true, direct: false, relayed: true, relay: relayA };
    },
  };
  const swarm = new MockSwarm();
  const peerInfo = { forceRelaying: false, attempts: 2 };
  swarm.peers.set(remote, peerInfo);
  const relay = new InferenceRelay(mockPeer(swarm), {
    relays: [relayA],
    directWaitMs: 100,
    relayWaitMs: 500,
  });

  const connected = await relay.connectPeer(directSession, remote, 1_000);
  assert.deepEqual(waits, [100]);
  assert.equal(connected.relayed, true);
  assert.equal(connected.relay, relayA);
  assert.equal(attempts, 1);
  assert.equal(peerInfo.forceRelaying, false);
  assert.equal(swarm.relayAttempts.length, 1);
  assert.equal(b4a.toString(swarm.relayAttempts[0].remotePublicKey, 'hex'), remote);
});

test('relay policy leaves a successful direct transport alone', async () => {
  const swarm = new MockSwarm();
  const peerInfo = { forceRelaying: false, attempts: 2 };
  swarm.peers.set(remote, peerInfo);
  const directSession = {
    connectPeer: async () => ({ remote, connected: true, direct: true, relayed: false }),
  };
  const relay = new InferenceRelay(mockPeer(swarm), {
    relays: [relayA],
    directWaitMs: 100,
    relayWaitMs: 500,
  });

  const connected = await relay.connectPeer(directSession, remote, 1_000);
  assert.equal(connected.direct, true);
  assert.equal(peerInfo.forceRelaying, false);
  assert.equal(relay.stats().counters.forced_fallbacks, 0);
});

test('relay fallback owns the tracked connection until release and blocks a late direct race', async () => {
  const swarm = new MockSwarm();
  const directAttempt = new EventEmitter();
  directAttempt.remotePublicKey = b4a.from(remote, 'hex');
  directAttempt.destroyed = false;
  directAttempt.destroy = (error) => {
    directAttempt.error = error;
    directAttempt.destroyed = true;
    directAttempt.emit('close');
  };
  swarm._allConnections.add(directAttempt);
  const suspended = [];
  const resumed = [];
  const directSession = {
    explicitPeers: new Set([remote]),
    suspendReconnect: (peer) => suspended.push(peer),
    resumeReconnect: (peer) => {
      assert.equal(swarm._allConnections.get(remote), undefined);
      assert.equal(swarm.connections.has(relayConnection), false);
      resumed.push(peer);
    },
  };
  const relay = new InferenceRelay(mockPeer(swarm), { relays: [relayA] });
  await relay.start();

  relay.requestRelay(remote);
  const connected = await relay._connectViaRelay(directSession, remote);
  const relayConnection = swarm.relayAttempts[0].connection;

  assert.equal(connected.relayed, true);
  assert.equal(directAttempt.destroyed, true);
  assert.match(directAttempt.error.message, /superseded/);
  assert.deepEqual(suspended, [remote]);
  assert.equal(swarm._allConnections.get(remote), relayConnection);
  assert.equal(swarm.connections.has(relayConnection), true);

  relay.releaseRelay(remote);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(swarm._allConnections.get(remote), undefined);
  assert.deepEqual(resumed, [remote]);
  await relay.stop();
});

test('official inbound relay suppresses provider reconnect until the relay ownership ends', async () => {
  const swarm = new MockSwarm();
  const reconnecting = [];
  swarm.peers.set(remote, {
    reconnect: (enabled) => reconnecting.push(enabled),
  });
  const suspended = [];
  const resumed = [];
  const peer = mockPeer(swarm);
  peer.directSession = {
    sessions: new Map(),
    suspendReconnect: (key) => suspended.push(key),
    resumeReconnect: (key) => resumed.push(key),
  };
  const relay = new InferenceRelay(peer, {
    relays: [relayA],
    inboundReleaseGraceMs: 1,
  });
  await relay.start();

  const directAttempt = new EventEmitter();
  directAttempt.remotePublicKey = b4a.from(remote, 'hex');
  directAttempt.destroyed = false;
  directAttempt.destroy = (error) => {
    directAttempt.destroyed = true;
    directAttempt.error = error;
  };
  swarm._allConnections.add(directAttempt);
  swarm.connections.add(directAttempt);

  assert.equal(swarm.server.firewall(
    b4a.from(remote, 'hex'),
    { relayThrough: { publicKey: b4a.from(relayA, 'hex') } }
  ), false);
  const unrelatedDirect = new EventEmitter();
  unrelatedDirect.remotePublicKey = b4a.from(remote, 'hex');
  unrelatedDirect.rawStream = {};
  swarm.server.emit('connection', unrelatedDirect);
  assert.equal(relay.connectionTransport(unrelatedDirect).relayed, false);

  const relayRawStream = {};
  swarm.server._relayConnection(
    { rawStream: relayRawStream },
    null,
    { relayThrough: { publicKey: b4a.from(relayA, 'hex') } },
    null
  );
  const connection = new EventEmitter();
  connection.remotePublicKey = b4a.from(remote, 'hex');
  connection.rawStream = relayRawStream;
  swarm.server.emit('connection', connection);

  assert.equal(relay.connectionTransport(connection).relayed, true);
  assert.equal(relay.connectionTransport(connection).relay, relayA);
  assert.equal(directAttempt.destroyed, true);
  assert.match(directAttempt.error.message, /inbound relay/);
  assert.equal(swarm.connections.has(directAttempt), false);
  assert.equal(swarm._allConnections.get(remote), undefined);
  assert.deepEqual(reconnecting, [false, false]);
  assert.deepEqual(suspended, [remote, remote]);

  connection.emit('close');
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.deepEqual(reconnecting, [false, false, true]);
  assert.deepEqual(resumed, [remote]);
  await relay.stop();
});
