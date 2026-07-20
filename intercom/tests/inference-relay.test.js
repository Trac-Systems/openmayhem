import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { Duplex } from 'node:stream';
import test from 'node:test';
import b4a from 'b4a';
import NoiseSecretStream from '../node_modules/@hyperswarm/secret-stream/index.js';
import DirectSession from '../features/direct-session/index.js';
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

const nextTurn = () => new Promise((resolve) => setImmediate(resolve));

const streamBytes = (stream) => (
  Math.max(0, Number(stream?.bytesReceived) || 0)
  + Math.max(0, Number(stream?.bytesTransmitted) || 0)
);

const memoryDuplexPair = () => {
  const writes = { left: [], right: [] };
  let left = null;
  let right = null;
  left = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      const data = Buffer.from(chunk);
      writes.left.push(data.byteLength);
      right.push(data);
      callback();
    },
  });
  right = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      const data = Buffer.from(chunk);
      writes.right.push(data.byteLength);
      left.push(data);
      callback();
    },
  });
  return { left, right, writes };
};

const authenticatedDuplexPair = async (rawPair, leftKeyPair, rightKeyPair) => {
  const rawLeft = rawPair.left;
  const rawRight = rawPair.right;
  const left = new NoiseSecretStream(true, rawLeft, { keyPair: leftKeyPair });
  const right = new NoiseSecretStream(false, rawRight, { keyPair: rightKeyPair });
  left.on('error', () => {});
  right.on('error', () => {});
  const [leftOpened, rightOpened] = await Promise.all([left.opened, right.opened]);
  assert.equal(leftOpened, true);
  assert.equal(rightOpened, true);
  assert.deepEqual(left.remotePublicKey, rightKeyPair.publicKey);
  assert.deepEqual(right.remotePublicKey, leftKeyPair.publicKey);
  assert.ok(left.handshakeHash.byteLength > 0);
  assert.deepEqual(left.handshakeHash, right.handshakeHash);
  return {
    ...rawPair,
    left,
    right,
    rawLeft,
    rawRight,
  };
};

const boundedRelayDuplexPair = (relay) => {
  let nextStreamId = 1;
  relay.peer.swarm.dht.createRawStream = () => {
    const stream = new EventEmitter();
    stream.id = nextStreamId++;
    stream.bytesReceived = 0;
    stream.bytesTransmitted = 0;
    stream.destroyed = false;
    stream.destroy = (error = null) => {
      if (stream.destroyed) return;
      stream.destroyed = true;
      stream.error = error;
      if (error) stream.emit('error', error);
      stream.emit('close');
    };
    return stream;
  };

  const legs = [
    relay._createBoundedStream({ side: 'left' }),
    relay._createBoundedStream({ side: 'right' }),
  ];
  const writes = { left: [], right: [] };
  let left = null;
  let right = null;
  let closing = false;

  const closeConnections = (error) => {
    if (closing) return;
    closing = true;
    if (!left.destroyed) left.destroy(error || undefined);
    if (!right.destroyed) right.destroy(error || undefined);
  };
  for (const leg of legs) {
    leg.on('close', () => closeConnections(leg.error));
  }

  const forward = (source, destination, peer, side, chunk, callback) => {
    if (source.destroyed || destination.destroyed || peer.destroyed) {
      callback(source.error || destination.error || new Error('Relay link is closed.'));
      return;
    }
    const data = Buffer.from(chunk);
    source.bytesReceived += data.byteLength;
    destination.bytesTransmitted += data.byteLength;
    writes[side].push(data.byteLength);
    peer.push(data);
    callback();
  };

  left = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      forward(legs[0], legs[1], right, 'left', chunk, callback);
    },
  });
  right = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      forward(legs[1], legs[0], left, 'right', chunk, callback);
    },
  });
  left.on('error', () => {});
  right.on('error', () => {});

  return {
    left,
    right,
    legs,
    writes,
  };
};

const payloadFrames = (requestId, mode, startIndex) => {
  const payloads = [
    { modelClass: 'text', contentType: 'text/plain', byte: '74' },
    { modelClass: 'image', contentType: 'image/png', byte: '89' },
    { modelClass: 'audio', contentType: 'audio/wav', byte: '52' },
    { modelClass: 'video', contentType: 'video/mp4', byte: '00' },
    { modelClass: 'embedding', contentType: 'application/json', byte: '65' },
  ];
  let index = startIndex;
  return payloads.flatMap(({ modelClass, contentType, byte }, payloadIndex) => (
    [0, 1].map((part) => {
      const frameIndex = index++;
      if (modelClass === 'text') {
        return {
          t: 's.delta',
          rid: requestId,
          i: frameIndex,
          d: `${mode}-text-${part}-`.repeat(12),
          model_class: modelClass,
          wire_mode: mode,
          wire_part: part,
          fin: null,
        };
      }
      if (modelClass === 'embedding') {
        return {
          t: 's.delta_chunk',
          v: 1,
          rid: requestId,
          i: frameIndex,
          field: 'embeddings',
          payload_id: byte.repeat(32),
          chunk: {
            index: part,
            count: 2,
            values: Array.from(
              { length: 24 },
              (_, valueIndex) => (payloadIndex + part + valueIndex) / 100
            ),
          },
          model_class: modelClass,
          wire_mode: mode,
          wire_part: part,
        };
      }
      return {
        t: 's.delta',
        rid: requestId,
        i: frameIndex,
        d: '',
        model_class: modelClass,
        wire_mode: mode,
        wire_part: part,
        artifact: {
          id: `${mode}-${modelClass}`,
          content_type: contentType,
          encoding: 'hex',
          offset: part * 96,
          len: 96,
          total_len: 192,
          blake3: byte.repeat(32),
          data: byte.repeat(96),
          final: part === 1,
        },
        fin: null,
      };
    })
  ));
};

const sendFrames = async (directSession, record, frames, coalesced) => {
  if (!coalesced) {
    for (const frame of frames) {
      await directSession.send(record.remote, record.sessionId, frame);
      await nextTurn();
    }
    return;
  }
  const mux = record.connection.userData;
  mux.cork();
  try {
    for (const frame of frames) {
      directSession._validateFrame(frame);
      assert.equal(record.message.send(frame), true);
    }
  } finally {
    mux.uncork();
  }
  await nextTurn();
};

const relayedSessionHarness = async ({
  relayConfig = {},
  buyerConfig = {},
  providerConfig = {},
} = {}) => {
  const buyerKeyPair = NoiseSecretStream.keyPair(b4a.alloc(32, 0x66));
  const providerKeyPair = NoiseSecretStream.keyPair(b4a.alloc(32, 0x77));
  const buyerKey = b4a.toString(buyerKeyPair.publicKey, 'hex');
  const providerKey = b4a.toString(providerKeyPair.publicKey, 'hex');
  const relaySwarm = new MockSwarm();
  const boundedRelay = new InferenceRelay(mockPeer(relaySwarm), {
    maxBytesPerLink: 16 * 1024 * 1024,
    rateBytesPerSecond: 16 * 1024 * 1024,
    rateBurstBytes: 16 * 1024 * 1024,
    ...relayConfig,
  });
  const relayPair = await authenticatedDuplexPair(
    boundedRelayDuplexPair(boundedRelay),
    buyerKeyPair,
    providerKeyPair
  );

  const buyerSwarm = {
    connections: new Set([relayPair.left]),
    peers: new Map(),
    joinPeer() {},
    leavePeer() {},
  };
  const providerSwarm = {
    connections: new Set([relayPair.right]),
    peers: new Map(),
    joinPeer() {},
    leavePeer() {},
  };
  const buyerPeer = {
    wallet: { publicKey: buyerKey },
    swarm: buyerSwarm,
    directSession: null,
  };
  const providerPeer = {
    wallet: { publicKey: providerKey },
    swarm: providerSwarm,
    directSession: null,
  };
  const buyerRoutes = new InferenceRelay(buyerPeer, { relays: [relayA] });
  const providerRoutes = new InferenceRelay(providerPeer, { relays: [relayA] });
  buyerRoutes._handleConnection(relayPair.left, {
    inferenceRelay: true,
    relay: relayA,
  });
  providerRoutes._handleConnection(relayPair.right, {
    inferenceRelay: true,
    relay: relayA,
  });

  const buyerFrames = [];
  const buyerCloses = [];
  const providerFrames = [];
  const providerCloses = [];
  const commonConfig = {
    maxFrameBytes: 4096,
    maxStringBytes: 4096,
    rateBytesPerSecond: 0,
    rateBurstBytes: 4096,
    receiveBatchHeadroomBytes: 64 * 1024,
  };
  const buyer = new DirectSession(buyerPeer, {
    ...commonConfig,
    ...buyerConfig,
    transportInfo: (connection, key) => buyerRoutes.connectionTransport(connection, key),
    onFrame: (event) => buyerFrames.push(event),
    onClose: (event) => buyerCloses.push(event),
  });
  const provider = new DirectSession(providerPeer, {
    ...commonConfig,
    ...providerConfig,
    transportInfo: (connection, key) => providerRoutes.connectionTransport(connection, key),
    onFrame: (event) => providerFrames.push(event),
    onClose: (event) => providerCloses.push(event),
  });
  buyerPeer.directSession = buyer;
  providerPeer.directSession = provider;
  buyer._prepareConnection(relayPair.left);
  provider._prepareConnection(relayPair.right);

  const connections = [
    { feature: buyer, connection: relayPair.left },
    { feature: provider, connection: relayPair.right },
  ];

  const open = async (sessionId) => {
    await buyer.open(providerKey, sessionId);
    await nextTurn();
    const buyerRecord = buyer.sessions.get(`${providerKey}:${sessionId}`);
    const providerRecord = provider.sessions.get(`${buyerKey}:${sessionId}`);
    assert.ok(buyerRecord, `buyer session ${sessionId}`);
    assert.ok(providerRecord, `provider session ${sessionId}`);
    return { buyerRecord, providerRecord };
  };

  const replaceWithDirect = async (sessionId) => {
    const directPair = await authenticatedDuplexPair(
      memoryDuplexPair(),
      buyerKeyPair,
      providerKeyPair
    );
    buyerSwarm.connections.add(directPair.left);
    providerSwarm.connections.add(directPair.right);
    buyerRoutes._handleConnection(directPair.left, null);
    providerRoutes._handleConnection(directPair.right, null);
    buyer._prepareConnection(directPair.left);
    provider._prepareConnection(directPair.right);
    connections.push(
      { feature: buyer, connection: directPair.left },
      { feature: provider, connection: directPair.right }
    );

    const buyerRecord = buyer._ensureSession(directPair.left, sessionId);
    await buyerRecord.channel.fullyOpened();
    await nextTurn();
    const providerRecord = provider.sessions.get(`${buyerKey}:${sessionId}`);
    assert.ok(providerRecord, `replacement provider session ${sessionId}`);
    assert.equal(providerRecord.connection, directPair.right);
    return { buyerRecord, providerRecord, directPair };
  };

  const cleanup = () => {
    for (const { feature, connection } of connections) {
      feature._dropHealthConnection(connection);
      if (!connection.destroyed) connection.destroy();
    }
    for (const leg of relayPair.legs) {
      if (!leg.destroyed) leg.destroy();
    }
  };

  return {
    boundedRelay,
    buyer,
    buyerCloses,
    buyerFrames,
    buyerKey,
    buyerRoutes,
    cleanup,
    open,
    provider,
    providerCloses,
    providerFrames,
    providerKey,
    providerRoutes,
    relayPair,
    replaceWithDirect,
  };
};

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

test('relay stream rate budget accepts a bounded burst after refill', (t) => {
  let now = 10_000;
  t.mock.method(Date, 'now', () => now);
  const swarm = new MockSwarm();
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
    return stream;
  };
  const relay = new InferenceRelay(mockPeer(swarm), {
    maxBytesPerLink: 10_000,
    rateBytesPerSecond: 100,
    rateBurstBytes: 200,
  });
  const stream = relay._createBoundedStream({});

  stream.bytesReceived = 200;
  relay._sweep();
  assert.equal(stream.destroyed, false);

  now += 2_000;
  stream.bytesReceived += 200;
  relay._sweep();

  assert.equal(stream.destroyed, false);
  assert.equal(relay.stats().counters.links_rate_limited, 0);
  assert.equal(relay.stats().counters.bytes_observed, 400);
});

test('relay stream rate budget closes only sustained over-budget traffic', (t) => {
  let now = 20_000;
  t.mock.method(Date, 'now', () => now);
  const swarm = new MockSwarm();
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
    return stream;
  };
  const relay = new InferenceRelay(mockPeer(swarm), {
    maxBytesPerLink: 10_000,
    rateBytesPerSecond: 100,
    rateBurstBytes: 200,
  });
  const offender = relay._createBoundedStream({});
  const healthy = relay._createBoundedStream({});

  offender.bytesReceived = 200;
  healthy.bytesReceived = 100;
  relay._sweep();
  now += 1_000;
  offender.bytesReceived += 101;
  healthy.bytesReceived += 100;
  relay._sweep();

  assert.equal(offender.destroyed, true);
  assert.match(offender.error.message, /rate limit/);
  assert.equal(healthy.destroyed, false);
  assert.equal(relay.stats().counters.links_rate_limited, 1);
});

test('bounded relay carries authenticated payload classes across relay-to-direct without replay', async () => {
  const harness = await relayedSessionHarness();
  const matrixSessionId = '88'.repeat(32);
  const requestId = 'relay-boundary-request';

  try {
    const relayed = await harness.open(matrixSessionId);
    assert.deepEqual(harness.buyer._sessionInfo(relayed.buyerRecord), {
      session_id: matrixSessionId,
      channel: `mx/s/${matrixSessionId}`,
      protocol: 'mx/s',
      remote: harness.providerKey,
      direct: false,
      relayed: true,
      relay: relayA,
      opened: true,
    });
    assert.equal(relayed.providerRecord.remote, harness.buyerKey);

    let nextIndex = 0;
    const expectedFrames = [];
    const relayFragmented = payloadFrames(requestId, 'relay-fragmented', nextIndex);
    nextIndex += relayFragmented.length;
    const fragmentedWritesBefore = harness.relayPair.writes.right.length;
    await sendFrames(harness.provider, relayed.providerRecord, relayFragmented, false);
    expectedFrames.push(...relayFragmented);
    assert.ok(
      harness.relayPair.writes.right.length - fragmentedWritesBefore
        >= relayFragmented.length
    );

    const relayCoalesced = payloadFrames(requestId, 'relay-coalesced', nextIndex);
    nextIndex += relayCoalesced.length;
    const coalescedWritesBefore = harness.relayPair.writes.right.length;
    await sendFrames(harness.provider, relayed.providerRecord, relayCoalesced, true);
    expectedFrames.push(...relayCoalesced);
    assert.equal(harness.relayPair.writes.right.length - coalescedWritesBefore, 1);

    const partialReceipt = {
      t: 's.receipt',
      v: 1,
      session_id: matrixSessionId,
      seq: 1,
      receipt: {
        body: {
          session_id: matrixSessionId,
          seq: 1,
          final_receipt: false,
        },
      },
    };
    await harness.provider.send(
      harness.buyerKey,
      matrixSessionId,
      partialReceipt
    );
    await nextTurn();
    expectedFrames.push(partialReceipt);
    const partialReceiptAck = {
      t: 's.receipt_ack',
      v: 1,
      session_id: matrixSessionId,
      seq: 1,
      user_sig: 'a1'.repeat(64),
    };
    await harness.buyer.send(
      harness.providerKey,
      matrixSessionId,
      partialReceiptAck
    );
    await nextTurn();

    harness.boundedRelay._sweep();
    const relayBytesAtBoundary = harness.boundedRelay.stats().counters.bytes_observed;
    assert.ok(
      relayBytesAtBoundary
        > relayFragmented.reduce(
          (total, frame) => total + harness.buyer._frameBytes(frame),
          0
        )
    );
    assert.equal(harness.relayPair.legs.every((leg) => !leg.destroyed), true);

    const direct = await harness.replaceWithDirect(matrixSessionId);
    assert.equal(relayed.buyerRecord.closed, true);
    assert.equal(relayed.providerRecord.closed, true);
    assert.equal(relayed.buyerRecord.channel.closed, true);
    assert.equal(relayed.providerRecord.channel.closed, true);
    assert.deepEqual(harness.buyer._sessionInfo(direct.buyerRecord), {
      session_id: matrixSessionId,
      channel: `mx/s/${matrixSessionId}`,
      protocol: 'mx/s',
      remote: harness.providerKey,
      direct: true,
      relayed: false,
      relay: null,
      opened: true,
    });
    harness.boundedRelay._sweep();
    const relayBytesAfterRelease = harness.boundedRelay.stats().counters.bytes_observed;
    assert.ok(relayBytesAfterRelease >= relayBytesAtBoundary);

    const directFragmented = payloadFrames(requestId, 'direct-fragmented', nextIndex);
    nextIndex += directFragmented.length;
    const directFragmentedWritesBefore = direct.directPair.writes.right.length;
    await sendFrames(harness.provider, direct.providerRecord, directFragmented, false);
    expectedFrames.push(...directFragmented);
    assert.ok(
      direct.directPair.writes.right.length - directFragmentedWritesBefore
        >= directFragmented.length
    );

    const directCoalesced = payloadFrames(requestId, 'direct-coalesced', nextIndex);
    nextIndex += directCoalesced.length;
    const finalDelta = {
      t: 's.delta',
      rid: requestId,
      i: nextIndex++,
      d: '',
      fin: 'stop',
      model_class: 'text',
    };
    const directCoalescedBatch = [...directCoalesced, finalDelta];
    const directCoalescedWritesBefore = direct.directPair.writes.right.length;
    await sendFrames(
      harness.provider,
      direct.providerRecord,
      directCoalescedBatch,
      true
    );
    expectedFrames.push(...directCoalescedBatch);
    assert.equal(
      direct.directPair.writes.right.length - directCoalescedWritesBefore,
      1
    );

    const finalReceipt = {
      t: 's.receipt',
      v: 1,
      session_id: matrixSessionId,
      seq: 2,
      receipt: {
        body: {
          session_id: matrixSessionId,
          seq: 2,
          final_receipt: true,
        },
      },
    };
    await harness.provider.send(harness.buyerKey, matrixSessionId, finalReceipt);
    await nextTurn();
    expectedFrames.push(finalReceipt);
    const finalReceiptAck = {
      t: 's.receipt_ack',
      v: 1,
      session_id: matrixSessionId,
      seq: 2,
      user_sig: 'a2'.repeat(64),
    };
    await harness.buyer.send(
      harness.providerKey,
      matrixSessionId,
      finalReceiptAck
    );
    await nextTurn();

    harness.boundedRelay._sweep();
    assert.equal(
      harness.boundedRelay.stats().counters.bytes_observed,
      relayBytesAfterRelease,
      'direct traffic must not accrue relay bytes'
    );
    assert.deepEqual(
      harness.buyerFrames.map(({ frame }) => frame),
      expectedFrames
    );
    const deltaIndices = harness.buyerFrames
      .map(({ frame }) => frame)
      .filter((frame) => Number.isSafeInteger(frame.i))
      .map((frame) => frame.i);
    assert.deepEqual(
      deltaIndices,
      Array.from({ length: nextIndex }, (_, index) => index),
      'replacement must neither reorder nor replay provider deltas'
    );
    assert.equal(new Set(deltaIndices).size, deltaIndices.length);

    const receipts = harness.buyerFrames
      .map(({ frame }) => frame)
      .filter((frame) => frame.t === 's.receipt');
    assert.equal(receipts.length, 2);
    assert.equal(
      receipts.filter((frame) => frame.receipt.body.final_receipt === true).length,
      1
    );
    assert.deepEqual(
      harness.providerFrames.map(({ frame }) => frame),
      [partialReceiptAck, finalReceiptAck]
    );
    assert.equal(harness.providerFrames[0].relayed, true);
    assert.equal(harness.providerFrames[1].direct, true);
    assert.equal(
      harness.providerFrames.filter(({ frame }) => frame.seq === 2).length,
      1
    );
    assert.equal(
      harness.buyerFrames
        .slice(0, relayFragmented.length + relayCoalesced.length + 1)
        .every((event) => event.relayed && !event.direct && event.relay === relayA),
      true
    );
    assert.equal(
      harness.buyerFrames
        .slice(relayFragmented.length + relayCoalesced.length + 1)
        .every((event) => event.direct && !event.relayed && event.relay === null),
      true
    );
    assert.equal(harness.boundedRelay.stats().counters.links_byte_limited, 0);
    assert.equal(harness.boundedRelay.stats().counters.links_rate_limited, 0);
  } finally {
    harness.cleanup();
  }
});

test('bounded relay aggregate and rate limits observe actual DirectSession frames', async (t) => {
  await t.test('aggregate bytes', async () => {
    const harness = await relayedSessionHarness();
    const aggregateSessionId = '89'.repeat(32);
    try {
      const { providerRecord } = await harness.open(aggregateSessionId);
      harness.boundedRelay._sweep();
      harness.boundedRelay.maxBytesPerLink = Math.max(
        ...harness.relayPair.legs.map((leg) => streamBytes(leg))
      );
      const frame = {
        t: 's.delta',
        rid: 'aggregate-limit',
        i: 0,
        d: 'aggregate-limit-frame'.repeat(32),
      };

      await sendFrames(harness.provider, providerRecord, [frame], false);
      assert.deepEqual(harness.buyerFrames.map((event) => event.frame), [frame]);
      harness.boundedRelay._sweep();

      assert.equal(harness.boundedRelay.stats().counters.links_byte_limited, 2);
      assert.equal(harness.boundedRelay.stats().counters.links_rate_limited, 0);
      assert.equal(harness.relayPair.legs.every((leg) => leg.destroyed), true);
      assert.ok(
        harness.boundedRelay.stats().counters.bytes_observed
          >= harness.buyer._frameBytes(frame)
      );
    } finally {
      harness.cleanup();
    }
  });

  await t.test('rate bytes', async () => {
    const harness = await relayedSessionHarness();
    const rateSessionId = '8a'.repeat(32);
    try {
      const { providerRecord } = await harness.open(rateSessionId);
      harness.boundedRelay._sweep();
      harness.boundedRelay.rateBytesPerSecond = 1;
      harness.boundedRelay.rateBurstBytes = 1;
      for (const state of harness.boundedRelay.streams.values()) {
        state.tokens = 1;
        state.last_checked_at = Date.now() + 60_000;
      }
      const frame = {
        t: 's.delta',
        rid: 'rate-limit',
        i: 0,
        d: 'rate-limit-frame'.repeat(32),
      };

      await sendFrames(harness.provider, providerRecord, [frame], false);
      assert.deepEqual(harness.buyerFrames.map((event) => event.frame), [frame]);
      harness.boundedRelay._sweep();

      assert.equal(harness.boundedRelay.stats().counters.links_rate_limited, 2);
      assert.equal(harness.boundedRelay.stats().counters.links_byte_limited, 0);
      assert.equal(harness.relayPair.legs.every((leg) => leg.destroyed), true);
    } finally {
      harness.cleanup();
    }
  });
});

test('buyer-local receive limiting does not penalize the provider relay link', async () => {
  const harness = await relayedSessionHarness({
    buyerConfig: {
      rateBytesPerSecond: 1,
      rateBurstBytes: 4096,
      receiveBatchHeadroomBytes: 8192,
    },
  });
  const limitedSessionId = '8b'.repeat(32);
  const healthySessionId = '8c'.repeat(32);

  try {
    const limited = await harness.open(limitedSessionId);
    const healthy = await harness.open(healthySessionId);
    limited.buyerRecord.receiveLimiter.tokens = 0;
    limited.buyerRecord.receiveLimiter.lastRefill = Date.now() + 60_000;
    const rejectedLocally = {
      t: 's.delta',
      rid: 'buyer-local-limit',
      i: 0,
      d: 'buyer-local-overflow',
    };
    const healthyFrame = {
      t: 's.delta',
      rid: 'provider-still-healthy',
      i: 0,
      d: 'provider connection remains usable',
    };

    await harness.provider.send(
      harness.buyerKey,
      limitedSessionId,
      rejectedLocally
    );
    await nextTurn();
    assert.equal(limited.buyerRecord.closed, true);
    assert.equal(
      harness.buyerCloses.some(
        (event) => event.session_id === limitedSessionId
          && event.locally_initiated === true
          && /receive rate/.test(event.reason)
      ),
      true
    );

    await harness.provider.send(
      harness.buyerKey,
      healthySessionId,
      healthyFrame
    );
    await nextTurn();
    harness.boundedRelay._sweep();

    assert.deepEqual(
      harness.buyerFrames.map(({ session_id, frame }) => ({ session_id, frame })),
      [{ session_id: healthySessionId, frame: healthyFrame }]
    );
    assert.equal(healthy.buyerRecord.closed, false);
    assert.equal(healthy.providerRecord.closed, false);
    assert.equal(harness.relayPair.legs.every((leg) => !leg.destroyed), true);
    assert.equal(harness.boundedRelay.stats().counters.links_rate_limited, 0);
    assert.equal(harness.boundedRelay.stats().counters.links_byte_limited, 0);
  } finally {
    harness.cleanup();
  }
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
