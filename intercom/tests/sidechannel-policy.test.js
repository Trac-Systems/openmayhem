import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import Sidechannel from '../features/sidechannel/index.js';
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
