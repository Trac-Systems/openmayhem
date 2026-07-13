import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import Sidechannel from '../features/sidechannel/index.js';
import {
  decodedJsonByteLength,
  decodedJsonWasRejected,
} from '../features/bounded-json.js';
import {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_MESSAGE_BYTES,
} from '../features/mayhem/index.js';

const entryChannel = '0000intercom';
const sessionChannel = `mx/s/${'11'.repeat(32)}`;

const peer = {
  wallet: {
    publicKey: 'aa'.repeat(32),
    sign: () => 'bb'.repeat(64),
  },
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

test('a rejected remote message handler is contained and later messages still run', async () => {
  let incoming;
  let calls = 0;
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
  const connection = {
    remotePublicKey: b4a.from('cc'.repeat(32), 'hex'),
    userData: {
      pair() {},
      createChannel: () => channel,
    },
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
  sidechannel._openChannelForConnection(connection, sidechannel.channels.get(entryChannel));
  assert.equal(typeof incoming, 'function');

  const originalError = console.error;
  console.error = () => {};
  try {
    assert.doesNotThrow(() => incoming({ id: 'first', from: 'cc'.repeat(32), message: { n: 1 } }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.doesNotThrow(() => incoming({ id: 'second', from: 'cc'.repeat(32), message: { n: 2 } }));
    await new Promise((resolve) => setTimeout(resolve, 0));
  } finally {
    console.error = originalError;
  }
  assert.equal(calls, 2);
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
