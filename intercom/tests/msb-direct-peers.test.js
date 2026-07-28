import assert from 'node:assert/strict';
import test from 'node:test';

import {
  openMsbWithDirectPeers,
  parseCanonicalMsbDirectPeers,
} from '../src/msb-direct-peers.js';

const peerA = 'ab'.repeat(32);
const peerBUpper = 'CD'.repeat(32);

test('canonical MSB direct peers are bounded, validated, normalized, and deduplicated', () => {
  assert.deepEqual(
    parseCanonicalMsbDirectPeers(`${peerA},${peerBUpper},${peerA.toUpperCase()}`),
    [peerA, peerBUpper.toLowerCase()]
  );
  assert.deepEqual(parseCanonicalMsbDirectPeers([peerA, peerA.toUpperCase()]), [peerA]);
  assert.throws(
    () => parseCanonicalMsbDirectPeers('not-a-public-key'),
    /at most 16 32-byte hexadecimal/
  );
  assert.throws(
    () => parseCanonicalMsbDirectPeers(Array.from({ length: 17 }, () => peerA)),
    /at most 16 32-byte hexadecimal/
  );
});

test('canonical peers connect while MainSettlementBus.ready is still opening', async () => {
  let releaseReady;
  const readyGate = new Promise((resolve) => {
    releaseReady = resolve;
  });
  const events = [];
  const msb = {
    network: null,
    async ready() {
      events.push('ready-start');
      this.network = {
        swarm: {},
        validatorConnectionManager: { connected: () => false },
        async tryConnect(peer, type) {
          events.push(`connect:${peer}:${type}`);
          releaseReady();
        },
      };
      await readyGate;
      events.push('ready-end');
    },
  };

  await openMsbWithDirectPeers(msb, {
    directPeers: [peerA],
    timeoutSeconds: 1,
    sleepFn: async () => {},
  });
  assert.deepEqual(events, [
    'ready-start',
    `connect:${peerA}:validator`,
    'ready-end',
  ]);
});

test('opening deadline also bounds a stalled direct connection', async () => {
  let fireDeadline;
  const never = new Promise(() => {});
  const msb = {
    network: null,
    async ready() {
      this.network = {
        swarm: {},
        validatorConnectionManager: { connected: () => false },
        tryConnect: async () => await never,
      };
    },
  };
  const opening = openMsbWithDirectPeers(msb, {
    directPeers: [peerA],
    timeoutSeconds: 7,
    sleepFn: async () => {},
    setTimeoutFn(callback) {
      fireDeadline = callback;
      return 1;
    },
    clearTimeoutFn() {},
  });
  await Promise.resolve();
  await Promise.resolve();
  fireDeadline();
  await assert.rejects(
    opening,
    /opening\/direct-peer connection timed out after 7 seconds/
  );
});

test('normal no-direct opening does not require a network swarm', async () => {
  let readyCalls = 0;
  const msb = {
    async ready() {
      readyCalls += 1;
    },
  };
  await openMsbWithDirectPeers(msb, {
    timeoutSeconds: 1,
  });
  assert.equal(readyCalls, 1);
});
