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
      message: {
        send(frame) {
          sent.push(frame);
          return true;
        },
      },
      subscriptions: new Set(),
      pendingControls: new Map(),
      pending: new Map(),
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

test('room heartbeat relay fans out by explicit room and coalesces each route to newest', () => {
  const relay = new InferenceRelay(mockPeer(), {
    serve: true,
    maxHeartbeatEntries: 8,
    maxPendingHeartbeatsPerClient: 4,
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
  slow.state.opened = true;
  relay._flushHeartbeatState(slow.state);
  assert.deepEqual(slow.sent.map((frame) => frame.heartbeat.ts), [newest.ts]);
  assert.equal(relay.heartbeatCache.size, 1);
});

test('relay fallback continues after direct heartbeat loss without becoming authoritative', () => {
  const peer = mockPeer();
  const consumer = new InferenceRelay(peer, { relays: [relayKey] });
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
  const helper = new InferenceRelay(helperPeer, { serve: true });
  const provider = new InferenceRelay(providerPeer, { relays: [officialRelay] });
  const buyer = new InferenceRelay(buyerPeer, { relays: [officialRelay] });
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
