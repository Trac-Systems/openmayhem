import assert from 'node:assert/strict';
import { Duplex } from 'node:stream';
import test from 'node:test';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import NoiseSecretStream from '@hyperswarm/secret-stream';

import InferenceRelay, {
  canonicalHeartbeatJson,
  heartbeatEnvelope,
  heartbeatRoom,
} from '../features/inference-relay/index.js';

const relayKey = '11'.repeat(32);
const enclaveKey = '33'.repeat(32);
const roomId = '44'.repeat(16);
const otherRoomId = '55'.repeat(16);
const room = `mx/room/${roomId}`;
const otherRoom = `mx/room/${otherRoomId}`;
const providerWallet = new PeerWallet();
await providerWallet.ready;
await providerWallet.generateKeyPair();
const providerKey = b4a.toString(providerWallet.publicKey, 'hex');

const heartbeat = (ts, overrides = {}) => {
  const value = {
    t: 'hb',
    v: 1,
    contract_version: 16,
    provider: providerKey,
    enclave_id: enclaveKey,
    model_id: 'Cactus-Compute/needle',
    room_id: roomId,
    identity_anchor: '66'.repeat(32),
    accepting_new: true,
    sat: 0,
    slots: { active: 0, active_requests: 0, max: 1 },
    q: { free_slots: 1, engine_backlog: 0, est_wait_ms: 0 },
    perf: { tok_s: 10, tok_s_source: 'measured', ttft_ms: 100 },
    price_ver: 1,
    min_ask_au: '0',
    caps: {
      tools: true,
      json: true,
      ctx: 1024,
      vision: false,
      served_modalities: ['text'],
      served_specialities: {},
      modality_capacity: {},
    },
    att: { epoch: 1, head: '77'.repeat(32) },
    ts,
    nonce: ts.toString(16).padStart(64, '0'),
    transport_peer: '88'.repeat(32),
    ...overrides,
  };
  value.sig = b4a.toString(
    providerWallet.sign(b4a.from(canonicalHeartbeatJson(value, true), 'utf8')),
    'hex'
  );
  return value;
};

const mockPeer = () => ({
  wallet: { publicKey: 'aa'.repeat(32) },
  swarm: {
    connections: new Set(),
    joined: [],
    joinPeer(key) {
      this.joined.push(Buffer.from(key).toString('hex'));
    },
  },
});

const authenticatedPair = async (leftKeyPair, rightKeyPair) => {
  let leftRaw = null;
  let rightRaw = null;
  leftRaw = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      rightRaw.push(Buffer.from(chunk));
      callback();
    },
  });
  rightRaw = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      leftRaw.push(Buffer.from(chunk));
      callback();
    },
  });
  const left = new NoiseSecretStream(true, leftRaw, { keyPair: leftKeyPair });
  const right = new NoiseSecretStream(false, rightRaw, { keyPair: rightKeyPair });
  left.on('error', () => {});
  right.on('error', () => {});
  await Promise.all([left.opened, right.opened]);
  return { left, right };
};

const waitFor = async (predicate, timeoutMs = 1_000) => {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error('Timed out waiting for relay proof.');
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
};

const syntheticRelayClient = (remote = 'bb'.repeat(32), opened = true) => {
  const sent = [];
  return {
    state: {
      remote,
      opened,
      blocked: false,
      heartbeatDisabled: false,
      message: {
        send(frame) {
          sent.push(frame);
          return true;
        },
      },
      subscriptions: new Set(),
      pendingControls: new Map(),
      pending: new Map(),
      pendingBytes: 0,
      pendingPeerUsage: new Map(),
    },
    sent,
  };
};

test('room heartbeat relay accepts only canonical bounded signed envelope shapes', () => {
  const valid = heartbeat(1_700_000_000_000);
  assert.equal(heartbeatRoom(room), room);
  assert.equal(heartbeatRoom('mx/room/*'), null);
  assert.equal(heartbeatRoom(`mx/room/${'AB'.repeat(16)}`), null);
  assert.equal(heartbeatEnvelope(room, valid)?.heartbeat, valid);
  assert.equal(heartbeatEnvelope(otherRoom, valid), null);
  assert.equal(heartbeatEnvelope(room, { ...valid, sig: 'not-a-signature' }), null);
  assert.equal(
    heartbeatEnvelope(room, heartbeat(valid.ts, { model_id: 'x'.repeat(4096) }), 1024),
    null
  );
});

test('relay cache rejection never suppresses a valid direct or local heartbeat', () => {
  const now = 1_700_000_000_050;
  const peer = mockPeer();
  const relay = new InferenceRelay(peer, {
    relays: [relayKey],
    heartbeatNow: () => now,
    maxHeartbeatCacheBytes: 1,
    maxHeartbeatBytesPerPeer: 1,
  });
  const direct = heartbeat(now);
  const local = heartbeat(now + 1, { enclave_id: '31'.repeat(32) });

  assert.deepEqual(
    relay.observeSidechannelHeartbeat(room, { message: direct, origin: providerKey }),
    { heartbeat: true, emit: true }
  );
  assert.deepEqual(
    relay.observeSidechannelHeartbeat(room, { message: local, origin: 'local' }),
    { heartbeat: true, emit: true }
  );
  assert.equal(relay.heartbeatCache.size, 0);
  assert.equal(relay.counters.heartbeat_peer_quota_rejected, 2);

  const coalescingRelay = new InferenceRelay(mockPeer(), {
    heartbeatNow: () => now,
  });
  assert.deepEqual(
    coalescingRelay.observeSidechannelHeartbeat(room, { message: direct, origin: providerKey }),
    { heartbeat: true, emit: true }
  );
  assert.deepEqual(
    coalescingRelay.observeSidechannelHeartbeat(room, { message: direct, origin: providerKey }),
    { heartbeat: true, emit: false }
  );
});

test('room heartbeat relay fans out by explicit room and coalesces each route to newest', () => {
  let now = 1_700_000_000_100;
  const relay = new InferenceRelay(mockPeer(), {
    serve: true,
    maxHeartbeatEntries: 8,
    maxPendingHeartbeatsPerClient: 4,
    heartbeatNow: () => now,
  });
  const first = syntheticRelayClient();
  const second = syntheticRelayClient('cc'.repeat(32));
  const unrelated = syntheticRelayClient('dd'.repeat(32));
  const slow = syntheticRelayClient('de'.repeat(32), false);
  relay.heartbeatConnectionStates.add(first.state);
  relay.heartbeatConnectionStates.add(second.state);
  relay.heartbeatConnectionStates.add(unrelated.state);
  relay.heartbeatConnectionStates.add(slow.state);

  assert.equal(
    relay._handleHeartbeatRelayFrame(first.state, { t: 'hb.subscribe', v: 1, room }),
    true
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(second.state, { t: 'hb.subscribe', v: 1, room }),
    true
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      unrelated.state,
      { t: 'hb.subscribe', v: 1, room: otherRoom }
    ),
    true
  );
  slow.state.subscriptions.add(room);

  const publisherKey = 'ee'.repeat(32);
  const publisher = syntheticRelayClient(publisherKey).state;
  const current = heartbeat(1_700_000_000_100, { transport_peer: publisherKey });
  const stale = heartbeat(1_700_000_000_099, { transport_peer: publisherKey });
  const newest = heartbeat(1_700_000_000_101, { transport_peer: publisherKey });
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      {
        t: 'hb.publish',
        v: 1,
        room,
        heartbeat: heartbeat(1_700_000_000_098),
      }
    ),
    false
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      { t: 'hb.publish', v: 1, room, heartbeat: current }
    ),
    true
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      { t: 'hb.publish', v: 1, room, heartbeat: stale }
    ),
    false
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      { t: 'hb.publish', v: 1, room, heartbeat: newest }
    ),
    true
  );

  assert.deepEqual(first.sent.map((frame) => frame.heartbeat.ts), [
    current.ts,
    newest.ts,
  ]);
  assert.deepEqual(second.sent.map((frame) => frame.heartbeat.ts), [
    current.ts,
    newest.ts,
  ]);
  assert.deepEqual(unrelated.sent, []);
  assert.equal(slow.state.pending.size, 1);
  now = newest.ts;
  slow.state.opened = true;
  relay._flushHeartbeatState(slow.state);
  assert.deepEqual(slow.sent.map((frame) => frame.heartbeat.ts), [newest.ts]);
  assert.equal(relay.heartbeatCache.size, 1);
});

test('room heartbeat relay strictly bounds cached and pending payload bytes', () => {
  const now = 1_700_000_000_500;
  const publisherKey = 'a1'.repeat(32);
  const firstValue = heartbeat(now, {
    enclave_id: '01'.repeat(32),
    transport_peer: publisherKey,
  });
  const payloadBytes = heartbeatEnvelope(room, firstValue).bytes;
  const relay = new InferenceRelay(mockPeer(), {
    serve: true,
    heartbeatNow: () => now,
    maxHeartbeatEntries: 8,
    maxHeartbeatEntriesPerPeer: 8,
    maxPendingHeartbeatsPerClient: 8,
    maxHeartbeatCacheBytes: payloadBytes * 2,
    maxPendingHeartbeatBytes: payloadBytes,
    maxHeartbeatResidentBytes: payloadBytes * 3,
    maxHeartbeatBytesPerPeer: payloadBytes * 4,
    maxPendingHeartbeatBytesPerClient: payloadBytes * 2,
    maxPendingHeartbeatBytesPerPeer: payloadBytes * 2,
  });
  const slow = syntheticRelayClient('a2'.repeat(32), false);
  slow.state.subscriptions.add(room);
  relay.heartbeatConnectionStates.add(slow.state);
  const publisher = syntheticRelayClient(publisherKey).state;
  const secondPublisherKey = 'a5'.repeat(32);
  const secondPublisher = syntheticRelayClient(secondPublisherKey).state;
  const thirdPublisherKey = 'a6'.repeat(32);
  const thirdPublisher = syntheticRelayClient(thirdPublisherKey).state;

  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      { t: 'hb.publish', v: 1, room, heartbeat: firstValue }
    ),
    true
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      secondPublisher,
      {
        t: 'hb.publish',
        v: 1,
        room,
        heartbeat: heartbeat(now + 1, {
          enclave_id: '02'.repeat(32),
          transport_peer: secondPublisherKey,
        }),
      }
    ),
    true
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      thirdPublisher,
      {
        t: 'hb.publish',
        v: 1,
        room,
        heartbeat: heartbeat(now + 2, {
          enclave_id: '03'.repeat(32),
          transport_peer: thirdPublisherKey,
        }),
      }
    ),
    false
  );

  assert.equal(relay.heartbeatCache.size, 2);
  assert.equal(slow.state.pending.size, 0);
  assert.equal(slow.state.heartbeatDisabled, true);
  assert.ok(relay.heartbeatCacheBytes <= relay.maxHeartbeatCacheBytes);
  assert.ok(relay.heartbeatPendingBytes <= relay.maxPendingHeartbeatBytes);
  assert.ok(
    relay.heartbeatCacheBytes + relay.heartbeatPendingBytes
      <= relay.maxHeartbeatResidentBytes
  );
  assert.ok(relay.counters.heartbeat_pending_bytes_rejected >= 1);
  assert.ok(relay.counters.heartbeat_cache_bytes_rejected >= 1);
  assert.equal(relay.counters.heartbeat_slow_consumers_isolated, 1);
});

test('slow subscriber is isolated without starving healthy subscribers in any iteration order', () => {
  const now = 1_700_000_002_500;
  const publisherKey = 'a7'.repeat(32);
  const first = heartbeat(now, {
    enclave_id: '07'.repeat(32),
    transport_peer: publisherKey,
  });
  const second = heartbeat(now + 1, {
    enclave_id: '08'.repeat(32),
    transport_peer: publisherKey,
  });
  const payloadBytes = Math.max(
    heartbeatEnvelope(room, first).bytes,
    heartbeatEnvelope(room, second).bytes
  );
  const relay = new InferenceRelay(mockPeer(), {
    serve: true,
    heartbeatNow: () => now,
    maxHeartbeatCacheBytes: payloadBytes * 4,
    maxPendingHeartbeatBytes: payloadBytes * 8,
    maxHeartbeatResidentBytes: payloadBytes * 12,
    maxHeartbeatBytesPerPeer: payloadBytes * 4,
    maxPendingHeartbeatBytesPerClient: payloadBytes * 4,
    maxPendingHeartbeatBytesPerPeer: payloadBytes,
  });
  const slow = syntheticRelayClient('a8'.repeat(32), false);
  const healthy = [
    syntheticRelayClient('a9'.repeat(32), false),
    syntheticRelayClient('aa'.repeat(32), false),
    syntheticRelayClient('ab'.repeat(32), false),
  ];
  slow.state.subscriptions.add(room);
  relay.heartbeatConnectionStates.add(slow.state);
  for (const subscriber of healthy) {
    subscriber.state.opened = true;
    subscriber.state.subscriptions.add(room);
    relay.heartbeatConnectionStates.add(subscriber.state);
  }

  assert.equal(
    relay._handleHeartbeatRelayFrame(
      syntheticRelayClient(publisherKey).state,
      { t: 'hb.publish', v: 1, room, heartbeat: first }
    ),
    true
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      syntheticRelayClient(publisherKey).state,
      { t: 'hb.publish', v: 1, room, heartbeat: second }
    ),
    true
  );
  for (const subscriber of healthy) {
    assert.deepEqual(
      subscriber.sent.map((frame) => frame.heartbeat.enclave_id),
      [first.enclave_id, second.enclave_id]
    );
  }
  assert.equal(slow.state.heartbeatDisabled, true);
  assert.equal(relay.heartbeatConnectionStates.has(slow.state), false);
  assert.equal(slow.state.pending.size, 0);
  assert.equal(relay.heartbeatPendingBytes, 0);
  assert.equal(relay.heartbeatPendingPeerUsage.has(publisherKey), false);
  assert.ok(relay.counters.heartbeat_peer_quota_rejected >= 1);
  assert.equal(relay.counters.heartbeat_slow_consumers_isolated, 1);
});

test('heartbeat accounting stays exact across owner replacement and cleanup paths', () => {
  let now = 1_700_000_002_800;
  const firstOwner = 'ac'.repeat(32);
  const secondOwner = 'ad'.repeat(32);
  const relay = new InferenceRelay(mockPeer(), {
    serve: true,
    heartbeatNow: () => now,
    heartbeatFreshnessMs: 1_000,
    maxHeartbeatRetentionMs: 100,
  });
  const slow = syntheticRelayClient('ae'.repeat(32), false);
  slow.state.subscriptions.add(room);
  relay.heartbeatConnectionStates.add(slow.state);
  const first = heartbeat(now, {
    transport_peer: firstOwner,
    model_id: 'first',
  });
  const firstEnvelope = heartbeatEnvelope(room, first);

  assert.equal(
    relay._handleHeartbeatRelayFrame(
      syntheticRelayClient(firstOwner).state,
      { t: 'hb.publish', v: 1, room, heartbeat: first }
    ),
    true
  );
  assert.equal(relay.heartbeatCacheBytes, firstEnvelope.bytes);
  assert.equal(relay.heartbeatPendingBytes, firstEnvelope.bytes);
  assert.deepEqual(relay.heartbeatCachePeerUsage.get(firstOwner), {
    entries: 1,
    bytes: firstEnvelope.bytes,
  });
  assert.deepEqual(relay.heartbeatPendingPeerUsage.get(firstOwner), {
    entries: 1,
    bytes: firstEnvelope.bytes,
  });

  const replacement = heartbeat(now + 1, {
    transport_peer: secondOwner,
    model_id: 'replacement-with-a-different-byte-length',
  });
  const replacementEnvelope = heartbeatEnvelope(room, replacement);
  assert.equal(replacementEnvelope.key, firstEnvelope.key);
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      syntheticRelayClient(secondOwner).state,
      { t: 'hb.publish', v: 1, room, heartbeat: replacement }
    ),
    true
  );
  assert.equal(relay.heartbeatCacheBytes, replacementEnvelope.bytes);
  assert.equal(relay.heartbeatPendingBytes, replacementEnvelope.bytes);
  assert.equal(relay.heartbeatCachePeerUsage.has(firstOwner), false);
  assert.equal(relay.heartbeatPendingPeerUsage.has(firstOwner), false);
  assert.deepEqual(relay.heartbeatCachePeerUsage.get(secondOwner), {
    entries: 1,
    bytes: replacementEnvelope.bytes,
  });
  assert.deepEqual(relay.heartbeatPendingPeerUsage.get(secondOwner), {
    entries: 1,
    bytes: replacementEnvelope.bytes,
  });

  assert.equal(
    relay._handleHeartbeatRelayFrame(
      slow.state,
      { t: 'hb.unsubscribe', v: 1, room }
    ),
    true
  );
  assert.equal(slow.state.pendingBytes, 0);
  assert.equal(relay.heartbeatPendingBytes, 0);
  assert.equal(relay.heartbeatPendingPeerUsage.size, 0);

  now += 100;
  relay._sweep();
  assert.equal(relay.heartbeatCache.size, 0);
  assert.equal(relay.heartbeatCacheBytes, 0);
  assert.equal(relay.heartbeatCachePeerUsage.size, 0);
});

test('room heartbeat relay expires signed heartbeats by freshness and local retention', () => {
  let now = 1_700_000_003_000;
  const publisherKey = 'a3'.repeat(32);
  const relay = new InferenceRelay(mockPeer(), {
    serve: true,
    heartbeatNow: () => now,
    heartbeatFreshnessMs: 1_000,
    maxHeartbeatRetentionMs: 250,
    maxHeartbeatFutureSkewMs: 50,
  });
  const slow = syntheticRelayClient('a4'.repeat(32), false);
  slow.state.subscriptions.add(room);
  relay.heartbeatConnectionStates.add(slow.state);
  const publisher = syntheticRelayClient(publisherKey).state;
  const value = heartbeat(now - 100, { transport_peer: publisherKey });
  const key = heartbeatEnvelope(room, value).key;

  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      { t: 'hb.publish', v: 1, room, heartbeat: value }
    ),
    true
  );
  assert.equal(relay.heartbeatCache.get(key).expiresAt, now + 250);
  assert.equal(slow.state.pending.get(key).expiresAt, now + 250);

  now += 249;
  relay._sweep();
  assert.equal(relay.heartbeatCache.has(key), true);
  assert.equal(slow.state.pending.has(key), true);

  now += 1;
  relay._sweep();
  assert.equal(relay.heartbeatCache.has(key), false);
  assert.equal(slow.state.pending.has(key), false);
  assert.equal(relay.heartbeatCacheBytes, 0);
  assert.equal(relay.heartbeatPendingBytes, 0);
  assert.equal(relay.counters.heartbeat_cache_expired, 1);
  assert.equal(relay.counters.heartbeat_pending_expired, 1);

  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      {
        t: 'hb.publish',
        v: 1,
        room,
        heartbeat: heartbeat(now + 51, {
          enclave_id: '04'.repeat(32),
          transport_peer: publisherKey,
        }),
      }
    ),
    false
  );
  assert.equal(
    relay._handleHeartbeatRelayFrame(
      publisher,
      {
        t: 'hb.publish',
        v: 1,
        room,
        heartbeat: heartbeat(now - 1_000, {
          enclave_id: '05'.repeat(32),
          transport_peer: publisherKey,
        }),
      }
    ),
    false
  );
  assert.equal(relay.heartbeatCache.size, 0);
});

test('attacker heartbeat churn cannot evict another peer route', () => {
  const now = 1_700_000_004_000;
  const legitimateKey = 'b1'.repeat(32);
  const attackerKey = 'b2'.repeat(32);
  const legitimate = heartbeat(now, {
    enclave_id: '10'.repeat(32),
    transport_peer: legitimateKey,
  });
  const payloadBytes = heartbeatEnvelope(room, legitimate).bytes;
  const relay = new InferenceRelay(mockPeer(), {
    serve: true,
    heartbeatNow: () => now,
    maxHeartbeatEntries: 3,
    maxHeartbeatEntriesPerPeer: 2,
    maxPendingHeartbeatsPerClient: 3,
    maxHeartbeatCacheBytes: payloadBytes * 4,
    maxPendingHeartbeatBytes: payloadBytes * 4,
    maxHeartbeatResidentBytes: payloadBytes * 8,
    maxHeartbeatBytesPerPeer: payloadBytes * 2,
    maxPendingHeartbeatBytesPerClient: payloadBytes * 4,
    maxPendingHeartbeatBytesPerPeer: payloadBytes * 2,
  });
  const slow = syntheticRelayClient('b3'.repeat(32), false);
  slow.state.subscriptions.add(room);
  relay.heartbeatConnectionStates.add(slow.state);
  const legitimatePublisher = syntheticRelayClient(legitimateKey).state;
  const attacker = syntheticRelayClient(attackerKey).state;
  const legitimateEnvelope = heartbeatEnvelope(room, legitimate);

  assert.equal(
    relay._handleHeartbeatRelayFrame(
      legitimatePublisher,
      { t: 'hb.publish', v: 1, room, heartbeat: legitimate }
    ),
    true
  );
  for (let index = 0; index < 12; index += 1) {
    relay._handleHeartbeatRelayFrame(
      attacker,
      {
        t: 'hb.publish',
        v: 1,
        room,
        heartbeat: heartbeat(now + index, {
          enclave_id: index.toString(16).padStart(64, '0'),
          transport_peer: attackerKey,
        }),
      }
    );
  }

  assert.equal(relay.heartbeatCache.has(legitimateEnvelope.key), true);
  assert.equal(slow.state.pending.has(legitimateEnvelope.key), true);
  assert.equal(relay.heartbeatCache.size, 3);
  assert.equal(slow.state.pending.size, 3);
  assert.equal(relay.heartbeatCachePeerUsage.get(attackerKey).entries, 2);
  assert.equal(slow.state.pendingPeerUsage.get(attackerKey).entries, 2);
  assert.ok(relay.counters.heartbeat_peer_quota_rejected >= 10);
});

test('relay fallback continues after direct heartbeat loss without becoming authoritative', () => {
  const peer = mockPeer();
  const consumer = new InferenceRelay(peer, {
    relays: [relayKey],
    heartbeatNow: () => 1_700_000_001_001,
  });
  const delivered = [];
  consumer.setHeartbeatSink((channel, value, metadata) => {
    delivered.push({ channel, value, metadata });
  });
  assert.equal(consumer.subscribeHeartbeatRoom(room), true);
  assert.deepEqual(peer.swarm.joined, [relayKey]);

  const relayClient = syntheticRelayClient(relayKey).state;
  const direct = heartbeat(1_700_000_001_000);
  assert.deepEqual(
    consumer.observeSidechannelHeartbeat(room, {
      message: direct,
      origin: providerKey,
    }),
    { heartbeat: true, emit: true }
  );
  assert.equal(
    consumer._handleHeartbeatRelayFrame(
      relayClient,
      { t: 'hb.event', v: 1, room, heartbeat: direct }
    ),
    false
  );
  assert.deepEqual(delivered, []);

  const fallback = heartbeat(1_700_000_001_001);
  assert.equal(
    consumer._handleHeartbeatRelayFrame(
      relayClient,
      { t: 'hb.event', v: 1, room, heartbeat: fallback }
    ),
    true
  );
  assert.equal(delivered.length, 1);
  assert.equal(delivered[0].channel, room);
  assert.equal(delivered[0].value, fallback);
  assert.deepEqual(delivered[0].metadata, {
    relay: relayKey,
    authoritative: false,
  });
});

test('three low-footprint peers deliver a heartbeat when provider-to-buyer direct path is absent', async () => {
  const relayKeyPair = NoiseSecretStream.keyPair(b4a.alloc(32, 0x10));
  const providerKeyPair = NoiseSecretStream.keyPair(b4a.alloc(32, 0x20));
  const buyerKeyPair = NoiseSecretStream.keyPair(b4a.alloc(32, 0x30));
  const officialRelay = b4a.toString(relayKeyPair.publicKey, 'hex');
  const providerPair = await authenticatedPair(providerKeyPair, relayKeyPair);
  const buyerPair = await authenticatedPair(buyerKeyPair, relayKeyPair);

  const helperPeer = {
    wallet: { publicKey: officialRelay },
    swarm: { connections: new Set([providerPair.right, buyerPair.right]) },
  };
  const providerPeer = {
    wallet: { publicKey: b4a.toString(providerKeyPair.publicKey, 'hex') },
    swarm: { connections: new Set([providerPair.left]), joinPeer() {} },
  };
  const buyerPeer = {
    wallet: { publicKey: b4a.toString(buyerKeyPair.publicKey, 'hex') },
    swarm: { connections: new Set([buyerPair.left]), joinPeer() {} },
  };
  const relayNow = () => 1_700_000_002_000;
  const helper = new InferenceRelay(helperPeer, {
    serve: true,
    heartbeatNow: relayNow,
  });
  const provider = new InferenceRelay(providerPeer, {
    relays: [officialRelay],
    heartbeatNow: relayNow,
  });
  const buyer = new InferenceRelay(buyerPeer, {
    relays: [officialRelay],
    heartbeatNow: relayNow,
  });
  const delivered = [];
  buyer.setHeartbeatSink((channel, value, metadata) => {
    delivered.push({ channel, value, metadata });
  });

  try {
    helper._prepareHeartbeatConnection(providerPair.right);
    helper._prepareHeartbeatConnection(buyerPair.right);
    provider._prepareHeartbeatConnection(providerPair.left);
    buyer._prepareHeartbeatConnection(buyerPair.left);
    assert.equal(buyer.subscribeHeartbeatRoom(room), true);
    const value = heartbeat(1_700_000_002_000, {
      transport_peer: providerPeer.wallet.publicKey,
    });
    assert.deepEqual(
      provider.observeSidechannelHeartbeat(room, { message: value, origin: 'local' }),
      { heartbeat: true, emit: true }
    );
    await waitFor(() => delivered.length === 1);
    assert.equal(delivered[0].channel, room);
    assert.deepEqual(delivered[0].value, value);
    assert.deepEqual(delivered[0].metadata, {
      relay: officialRelay,
      authoritative: false,
    });
  } finally {
    providerPair.left.destroy();
    providerPair.right.destroy();
    buyerPair.left.destroy();
    buyerPair.right.destroy();
  }
});

test('relay subscriptions require exact room channels and retain reference counts', () => {
  const peer = mockPeer();
  const consumer = new InferenceRelay(peer, { relays: [relayKey] });

  assert.equal(consumer.subscribeHeartbeatRoom('mx/room/*'), false);
  assert.equal(consumer.subscribeHeartbeatRoom('0000mayhem-relay'), false);
  assert.equal(consumer.subscribeHeartbeatRoom(room), true);
  assert.equal(consumer.subscribeHeartbeatRoom(room), true);
  assert.equal(consumer.heartbeatRoomRefs.get(room), 2);
  assert.equal(consumer.unsubscribeHeartbeatRoom(room), true);
  assert.equal(consumer.heartbeatRoomRefs.get(room), 1);
  assert.equal(consumer.unsubscribeHeartbeatRoom(room), true);
  assert.equal(consumer.heartbeatRoomRefs.has(room), false);
});
