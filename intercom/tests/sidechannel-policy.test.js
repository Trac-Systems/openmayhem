import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import Sidechannel from '../features/sidechannel/index.js';
import {
  decodedJsonByteLength,
  decodedJsonWasRejected,
} from '../features/bounded-json.js';
import {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_MESSAGE_BYTES,
  MAYHEM_RELAY_POW_EXEMPT_CONTROLS,
} from '../features/mayhem/index.js';

const entryChannel = '0000intercom';
const sessionChannel = `mx/s/${'11'.repeat(32)}`;

const peer = {
  wallet: {
    publicKey: 'aa'.repeat(32),
    sign: () => 'bb'.repeat(64),
  },
};

const makeSignedPeer = async () => {
  const wallet = new PeerWallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  return { wallet };
};

const captureIncoming = (sidechannel, connection, channelName) => {
  let incoming = null;
  const channel = {
    opened: true,
    addMessage({ onmessage }) {
      incoming = onmessage;
      return { send: () => true };
    },
    open() {},
    close() {},
    fullyOpened: async () => true,
  };
  connection.userData = {
    pair() {},
    createChannel: () => channel,
  };
  sidechannel._openChannelForConnection(
    connection,
    sidechannel.channels.get(channelName)
  );
  return () => incoming;
};

test('relay PoW and size cap are isolated from entry and session channels', () => {
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel, MAYHEM_RELAY_CHANNEL, sessionChannel],
    entryChannel,
    maxMessageBytes: 1_000_000,
    maxMessageBytesByChannel: {
      [MAYHEM_RELAY_CHANNEL]: MAYHEM_RELAY_MAX_MESSAGE_BYTES,
    },
    powEnabled: true,
    powDifficulty: 8,
    powRequiredChannels: [MAYHEM_RELAY_CHANNEL],
  });

  assert.equal(sidechannel._powRequired(MAYHEM_RELAY_CHANNEL), true);
  assert.equal(sidechannel._powRequired(entryChannel), false);
  assert.equal(sidechannel._powRequired(sessionChannel), false);
  assert.equal(sidechannel._relayPolicyAllows(MAYHEM_RELAY_CHANNEL), true);
  assert.equal(sidechannel._relayPolicyAllows(entryChannel), false);
  assert.equal(sidechannel._relayPolicyAllows(sessionChannel), false);
  assert.equal(sidechannel.relayTtl, 1);
  assert.equal(sidechannel._maxMessageBytes(MAYHEM_RELAY_CHANNEL), 16_384);
  assert.equal(sidechannel._maxMessageBytes(entryChannel), 1_000_000);
  assert.equal(sidechannel._maxMessageBytes(sessionChannel), 1_000_000);
  assert.equal(sidechannel.rateBytesPerSecond, 64_000);
  assert.equal(sidechannel.rateBurstBytes, 256_000);
  assert.equal(sidechannel.maxStrikes, 3);
  assert.equal(sidechannel.blockMs, 30_000);

  const relayPayload = sidechannel._buildPayload(MAYHEM_RELAY_CHANNEL, { control: 'test' });
  assert.equal(sidechannel._checkPow(relayPayload, MAYHEM_RELAY_CHANNEL), true);
  assert.ok(Number.isInteger(relayPayload.pow.nonce));
  assert.equal(sidechannel._buildPayload(entryChannel, { control: 'test' }).pow, undefined);
  assert.equal(sidechannel._buildPayload(sessionChannel, { control: 'test' }).pow, undefined);

  assert.equal(
    sidechannel.broadcast(MAYHEM_RELAY_CHANNEL, { data: 'x'.repeat(20_000) }),
    false
  );
  assert.equal(sidechannel.broadcast(sessionChannel, { data: 'x'.repeat(20_000) }), true);
});

test('Mayhem control envelopes bypass relay PoW but generic relay traffic still pays it', () => {
  const sidechannel = new Sidechannel(peer, {
    channels: [MAYHEM_RELAY_CHANNEL],
    maxMessageBytesByChannel: {
      [MAYHEM_RELAY_CHANNEL]: MAYHEM_RELAY_MAX_MESSAGE_BYTES,
    },
    powEnabled: true,
    powDifficulty: 8,
    powRequiredChannels: [MAYHEM_RELAY_CHANNEL],
    powExemptControlsByChannel: {
      [MAYHEM_RELAY_CHANNEL]: MAYHEM_RELAY_POW_EXEMPT_CONTROLS,
    },
  });

  const mayhemPayload = sidechannel._buildPayload(MAYHEM_RELAY_CHANNEL, {
    control: 'mayhem_feature_request',
    request_id: '11'.repeat(32),
  });
  assert.equal(mayhemPayload.pow, undefined);
  assert.equal(sidechannel._checkPow(mayhemPayload, MAYHEM_RELAY_CHANNEL), true);

  const genericPayload = sidechannel._buildPayload(MAYHEM_RELAY_CHANNEL, {
    control: 'wire_probe',
  });
  assert.ok(Number.isInteger(genericPayload.pow?.nonce));
  assert.equal(sidechannel._checkPow(genericPayload, MAYHEM_RELAY_CHANNEL), true);
});

test('a rejected remote message handler is contained and later messages still run', async () => {
  const senderPeer = await makeSignedPeer();
  const sender = new Sidechannel(senderPeer, {
    channels: [entryChannel],
    entryChannel,
  });
  let calls = 0;
  const connection = {
    remotePublicKey: senderPeer.wallet.publicKey,
  };
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel],
    entryChannel,
    relayEnabled: false,
    onMessage: async () => {
      calls += 1;
      if (calls === 1) throw new Error('injected poisoned message');
    },
  });
  const incomingRef = captureIncoming(sidechannel, connection, entryChannel);
  const incoming = incomingRef();
  assert.equal(typeof incoming, 'function');

  const originalError = console.error;
  console.error = () => {};
  try {
    assert.doesNotThrow(() => incoming(sender._buildPayload(entryChannel, { n: 1 })));
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.doesNotThrow(() => incoming(sender._buildPayload(entryChannel, { n: 2 })));
    await new Promise((resolve) => setTimeout(resolve, 0));
  } finally {
    console.error = originalError;
  }
  assert.equal(calls, 2);
});

test('generic dispatch rejects unsigned and spoofed senders but accepts the signed direct path', async () => {
  const senderPeer = await makeSignedPeer();
  const senderKey = b4a.toString(senderPeer.wallet.publicKey, 'hex');
  const sender = new Sidechannel(senderPeer, {
    channels: [entryChannel],
    entryChannel,
  });
  const received = [];
  const receiver = new Sidechannel(peer, {
    channels: [entryChannel],
    entryChannel,
    relayEnabled: false,
    onMessage: (_channel, payload) => received.push(payload),
  });
  const connection = { remotePublicKey: senderPeer.wallet.publicKey };
  const incoming = captureIncoming(receiver, connection, entryChannel)();

  incoming({
    type: 'sidechannel',
    id: 'unsigned',
    channel: entryChannel,
    from: senderKey,
    origin: senderKey,
    message: { text: 'unsigned' },
    ts: Date.now(),
    ttl: 1,
  });
  const spoofed = sender._buildPayload(entryChannel, { text: 'spoofed' });
  spoofed.from = 'ff'.repeat(32);
  spoofed.origin = spoofed.from;
  incoming(spoofed);
  const valid = sender._buildPayload(entryChannel, { text: 'signed' });
  incoming(valid);
  const wrongChannel = sender._buildPayload(entryChannel, { text: 'cross-channel' });
  wrongChannel.channel = 'other-channel';
  incoming(wrongChannel);
  const oversizedId = sender._buildPayload(entryChannel, { text: 'large-id' });
  oversizedId.id = 'x'.repeat(257);
  incoming(oversizedId);

  assert.equal(received.length, 1);
  assert.equal(received[0].id, valid.id);
  assert.equal(receiver.verifyPayload(valid, senderKey), true);
  assert.equal(receiver.relayCounters.unauthenticated_drops, 2);
});

test('relay egress is one hop, gated, and bounded per node and authenticated author', async () => {
  const relayPeer = await makeSignedPeer();
  const firstPeer = await makeSignedPeer();
  const secondPeer = await makeSignedPeer();
  const relayKey = b4a.toString(relayPeer.wallet.publicKey, 'hex');
  const channelName = MAYHEM_RELAY_CHANNEL;
  const senderConfig = {
    channels: [channelName],
    powEnabled: true,
    powDifficulty: 1,
    powRequiredChannels: [channelName],
  };
  const firstSender = new Sidechannel(firstPeer, senderConfig);
  const secondSender = new Sidechannel(secondPeer, senderConfig);
  const relay = new Sidechannel(relayPeer, {
    ...senderConfig,
    relayRateBytesPerSecond: 1,
    relaySourceRateBytesPerSecond: 1,
  });
  const firstPayload = firstSender._buildPayload(channelName, { text: 'bounded relay' });
  const relayedBytes = b4a.byteLength(JSON.stringify({
    ...firstPayload,
    ttl: 0,
    relayedBy: relayKey,
  }), 'utf8');
  relay.relayBurstBytes = relayedBytes * 2;
  relay.relayLimiter.tokens = relay.relayBurstBytes;
  relay.relaySourceBurstBytes = relayedBytes;

  const sent = [];
  for (const key of ['66', '77']) {
    const connection = { remotePublicKey: b4a.from(key.repeat(32), 'hex') };
    relay.connections.set(connection, new Map([[
      channelName,
      { message: { send: (payload) => sent.push(payload) } },
    ]]));
  }
  const firstConnection = { remotePublicKey: firstPeer.wallet.publicKey };
  relay._relay(channelName, firstPayload, firstConnection);
  assert.equal(sent.length, 1);
  assert.equal(sent[0].ttl, 0);
  assert.equal(relay.verifyPayload(sent[0]), true);
  assert.equal(relay.relayCounters.source_budget_drops, 1);

  const secondPayload = secondSender._buildPayload(channelName, { text: 'bounded relay' });
  const secondConnection = { remotePublicKey: secondPeer.wallet.publicKey };
  relay._relay(channelName, secondPayload, secondConnection);
  assert.equal(sent.length, 2);
  assert.equal(relay.relayCounters.node_budget_drops, 1);
  assert.equal(relay.relayCounters.messages, 2);
  assert.equal(relay.relayCounters.bytes, relayedBytes * 2);

  let relayedDispatches = 0;
  let downstreamSends = 0;
  const recipientPeer = await makeSignedPeer();
  const recipient = new Sidechannel(recipientPeer, {
    ...senderConfig,
    welcomeRequired: false,
    onMessage: () => { relayedDispatches += 1; },
  });
  const downstream = { remotePublicKey: b4a.from('88'.repeat(32), 'hex') };
  recipient.connections.set(downstream, new Map([[
    channelName,
    { message: { send: () => { downstreamSends += 1; } } },
  ]]));
  const relayConnection = { remotePublicKey: relayPeer.wallet.publicKey };
  const incoming = captureIncoming(recipient, relayConnection, channelName)();
  incoming(sent[0]);

  assert.equal(relayedDispatches, 1);
  assert.equal(downstreamSends, 0);
  assert.equal(recipient.verifyPayload(sent[0]), true);
});

test('seen purge scans bounded state instead of assuming timestamp insertion order', () => {
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel],
    entryChannel,
    seenTtlMs: 100,
    maxSeen: 3,
  });
  sidechannel.seen.set('fresh-first', 1_000);
  sidechannel.seen.set('stale-middle', 100);
  sidechannel.seen.set('fresh-last', 1_001);

  sidechannel._purgeSeen(1_050);
  assert.deepEqual(Array.from(sidechannel.seen.keys()), ['fresh-first', 'fresh-last']);

  sidechannel.seen.set('oldest-by-time', 900);
  sidechannel._rememberSeen('newest', 1_050);
  assert.equal(sidechannel.seen.has('oldest-by-time'), false);
  assert.equal(sidechannel.seen.has('fresh-first'), true);
  assert.equal(sidechannel.seen.has('fresh-last'), true);
  assert.equal(sidechannel.seen.has('newest'), true);
});

test('required DHT bootstrap failure rejects sidechannel startup for supervisor recovery', async () => {
  const startupPeer = {
    ...peer,
    swarm: {
      dht: {
        async fullyBootstrapped() {
          throw new Error('injected DHT bootstrap failure');
        },
      },
      connections: [],
      on() {},
      join() {},
      async flush() {},
    },
  };
  const sidechannel = new Sidechannel(startupPeer, {
    channels: [entryChannel],
    entryChannel,
  });

  await assert.rejects(sidechannel.start(), /injected DHT bootstrap failure/);
  assert.equal(sidechannel.started, false);
});

test('batch room join performs one swarm flush for every registered room', async () => {
  const joined = [];
  let flushes = 0;
  const batchPeer = {
    ...peer,
    swarm: {
      join: (topic) => joined.push(b4a.toString(topic, 'hex')),
      flush: async () => { flushes += 1; },
    },
  };
  const sidechannel = new Sidechannel(batchPeer, {
    channels: [entryChannel],
    entryChannel,
  });
  sidechannel.started = true;
  const rooms = Array.from(
    { length: 256 },
    (_, index) => `mx/room/${index.toString(16).padStart(32, '0')}`,
  );

  assert.deepEqual(await sidechannel.addChannels(rooms), rooms);
  assert.equal(joined.length, rooms.length);
  assert.equal(flushes, 1);
  assert.ok(rooms.every((room) => sidechannel.channels.has(room)));
});

test('sidechannel decoder drops oversized JSON before parsing it', () => {
  let encoding = null;
  const channel = {
    opened: true,
    addMessage(options) {
      encoding = options.encoding;
      return { send: () => true };
    },
    open() {},
    close() {},
    fullyOpened: async () => true,
  };
  const connection = {
    remotePublicKey: b4a.from('dd'.repeat(32), 'hex'),
    userData: {
      pair() {},
      createChannel: () => channel,
    },
  };
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel],
    entryChannel,
    maxMessageBytes: 64,
  });
  sidechannel._openChannelForConnection(connection, sidechannel.channels.get(entryChannel));

  assert.ok(encoding);
  const state = { buffer: b4a.concat([b4a.from([65]), b4a.alloc(65)]), start: 0, end: 66 };
  const decoded = encoding.decode(state);
  assert.equal(decodedJsonWasRejected(decoded), true);
  assert.equal(decodedJsonByteLength(decoded), 65);
  assert.equal(state.start, 66);
});

test('sidechannel decoder rejects an individually oversized UTF-8 string within the byte budget', () => {
  const text = JSON.stringify({ value: 'é'.repeat(17) });
  const encoded = b4a.from(text);
  const encoding = {
    buffer: b4a.concat([b4a.from([encoded.length]), encoded]),
    start: 0,
    end: encoded.length + 1,
  };
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel],
    entryChannel,
    maxMessageBytes: 1_000,
    maxStringBytes: 32,
  });
  let boundedEncoding = null;
  const connection = {
    remotePublicKey: b4a.from('dd'.repeat(32), 'hex'),
    userData: {
      pair() {},
      createChannel: () => ({
        opened: true,
        addMessage(options) {
          boundedEncoding = options.encoding;
          return { send: () => true };
        },
        open() {},
        close() {},
        fullyOpened: async () => true,
      }),
    },
  };
  sidechannel._openChannelForConnection(connection, sidechannel.channels.get(entryChannel));

  const decoded = boundedEncoding.decode(encoding);
  assert.equal(decodedJsonWasRejected(decoded), true);
  assert.equal(decodedJsonByteLength(decoded), encoded.length);
});

test('sidechannel bounds channel names/count and reclaims limiter state under connection churn', () => {
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel],
    entryChannel,
    maxChannels: 2,
    maxChannelNameBytes: 32,
  });
  assert.ok(sidechannel._registerChannel('room-one'));
  assert.equal(sidechannel._registerChannel('room-two'), null);
  assert.equal(sidechannel._registerChannel('x'.repeat(33)), null);

  for (let index = 0; index < 1000; index += 1) {
    const connection = { remotePublicKey: b4a.alloc(32, index % 255) };
    sidechannel._getLimiter(connection);
    sidechannel._dropConnection(connection);
  }
  assert.equal(sidechannel.rateLimits.size, 0);
  assert.equal(sidechannel.connections.size, 0);

  sidechannel.maxRelaySources = 2;
  sidechannel._relaySourceLimiter('aa'.repeat(32), 1);
  sidechannel._relaySourceLimiter('bb'.repeat(32), 2);
  sidechannel._relaySourceLimiter('cc'.repeat(32), 3);
  assert.equal(sidechannel.relaySourceLimits.size, 2);
  assert.equal(sidechannel.relaySourceLimits.has('aa'.repeat(32)), false);
});

test('sidechannel does not send from a stale fully-opened callback after channel removal', async () => {
  const room = 'room-race';
  let resolveOpened = null;
  let sends = 0;
  const transportChannel = {
    opened: false,
    close() {},
    fullyOpened: () => new Promise((resolve) => { resolveOpened = resolve; }),
  };
  const record = {
    channel: transportChannel,
    message: { send: () => { sends += 1; } },
  };
  const connection = { remotePublicKey: b4a.from('ee'.repeat(32), 'hex') };
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel, room],
    entryChannel,
  });
  sidechannel.connections.set(connection, new Map([[room, record]]));

  assert.equal(sidechannel.broadcast(room, { text: 'must not escape after leave' }), true);
  await sidechannel.removeChannel(room);
  resolveOpened(true);
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(sends, 0);
  assert.equal(sidechannel.channels.has(room), false);
});

test('fast PoW mining stays byte-identical to the canonical pow base', () => {
  const sidechannel = new Sidechannel(peer, {
    channels: [entryChannel, MAYHEM_RELAY_CHANNEL],
    entryChannel,
    powEnabled: true,
    powDifficulty: 8,
    powRequiredChannels: [MAYHEM_RELAY_CHANNEL],
  });
  const payload = {
    id: 'cc'.repeat(16),
    channel: MAYHEM_RELAY_CHANNEL,
    from: 'aa'.repeat(32),
    origin: null,
    ts: 1783899999999,
    // Adversarial message: contains the literal `"nonce":0` and unsorted keys so a
    // sloppy prefix/suffix split would produce a different mining base.
    message: {
      zebra: [1, { b: 2, a: '"nonce":0' }],
      control: 'mx_relay_request_v1',
      value: { op: 'spend_reserve', provider: 'ee'.repeat(32), nested: { nonce: 0 } },
    },
  };
  sidechannel._attachPow(payload);
  assert.equal(typeof payload.pow?.nonce, 'number');
  // The receiver-side check recomputes the canonical `_powBase`; passing it proves
  // the mined nonce came from the identical byte stream.
  assert.equal(sidechannel._checkPow(payload, MAYHEM_RELAY_CHANNEL), true);
});

test('relay PoW mining yields while preserving deterministic validation', async () => {
  let resolveSent = null;
  const sent = new Promise((resolve) => {
    resolveSent = resolve;
  });
  const connection = { remotePublicKey: b4a.from('dd'.repeat(32), 'hex') };
  const sidechannel = new Sidechannel(peer, {
    channels: [MAYHEM_RELAY_CHANNEL],
    powEnabled: true,
    powDifficulty: 10,
    powRequiredChannels: [MAYHEM_RELAY_CHANNEL],
    powYieldEvery: 32,
  });
  sidechannel.connections.set(connection, new Map([[
    MAYHEM_RELAY_CHANNEL,
    {
      channel: { opened: true },
      message: { send: resolveSent },
    },
  ]]));

  let sentAlready = false;
  let timerProgressedBeforeSend = false;
  const heartbeat = setTimeout(() => {
    timerProgressedBeforeSend = !sentAlready;
  }, 0);

  assert.equal(
    sidechannel.broadcast(MAYHEM_RELAY_CHANNEL, { control: 'heartbeat-proof' }),
    true
  );
  let timeout = null;
  const payload = await Promise.race([
    sent,
    new Promise((_, reject) => {
      timeout = setTimeout(
        () => reject(new Error('cooperative PoW send timed out')),
        2_000
      );
    }),
  ]);
  clearTimeout(timeout);
  sentAlready = true;
  clearTimeout(heartbeat);

  assert.equal(timerProgressedBeforeSend, true);
  assert.equal(payload.pow.difficulty, 10);
  assert.equal(sidechannel._checkPow(payload, MAYHEM_RELAY_CHANNEL), true);

  const canonical = { ...payload };
  delete canonical.pow;
  delete canonical.sig;
  sidechannel._attachPow(canonical);
  assert.equal(payload.pow.nonce, canonical.pow.nonce);
});
