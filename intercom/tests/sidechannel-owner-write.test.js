// Regression test for the owner-write-only welcome-frame content-injection bug.
//
// On an owner-write-only channel, auth and welcome control frames are accepted so
// listeners can authorize without write access. They must not be dispatched as
// application content.

import test from 'node:test';
import assert from 'node:assert/strict';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import Sidechannel from '../features/sidechannel/index.js';

const stableStringify = (value) => {
  if (value === null || value === undefined) return 'null';
  if (typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(',')}]`;
  const keys = Object.keys(value).sort();
  return `{${keys.map((k) => `${JSON.stringify(k)}:${stableStringify(value[k])}`).join(',')}}`;
};

async function makeWallet() {
  const w = new PeerWallet({});
  await w.ready;
  if (!w.secretKey) {
    await w.generateKeyPair(null, null);
    await w.ready;
  }
  return w;
}

const peerStub = (wallet) => ({ wallet });
const toHex = (sig) => (typeof sig === 'string' ? sig : b4a.toString(sig, 'hex')).toLowerCase();

const CH = 'sig-owner-write';
const scConfig = (extra) => ({
  channels: [CH],
  ownerWriteChannels: [CH],
  powEnabled: false,
  inviteRequired: false,
  welcomeRequired: true,
  rateBytesPerSecond: 0,
  ...extra,
});

function fakeConn(remotePubHex) {
  const captured = {};
  const channel = {
    addMessage({ onmessage }) {
      captured.onmessage = onmessage;
      return { send() {} };
    },
    open() {},
    fullyOpened() {
      return new Promise(() => {});
    },
    close() {},
  };
  const mux = {
    createChannel() {
      return channel;
    },
    pair() {},
  };
  const connection = { userData: mux, remotePublicKey: b4a.from(remotePubHex, 'hex') };
  return { connection, captured };
}

function makeOwnerWelcome(ownerSc, ownerWallet, ownerPubHex) {
  const wpayload = {
    channel: CH,
    ownerPubKey: ownerPubHex,
    text: 'welcome',
    issuedAt: Date.now(),
    version: 1,
  };
  const normalized = ownerSc._normalizeWelcomePayload(wpayload);
  const sig = ownerWallet.sign(b4a.from(stableStringify(normalized)));
  return { payload: wpayload, sig: toHex(sig) };
}

test('owner-write-only sidechannel rejects welcome-frame content injection', async (t) => {
  const ownerWallet = await makeWallet();
  const recvWallet = await makeWallet();
  const attackerWallet = await makeWallet();
  const ownerPub = b4a.toString(ownerWallet.publicKey, 'hex');
  const ownerKeys = { [CH]: ownerPub };

  const ownerSc = new Sidechannel(peerStub(ownerWallet), scConfig({ ownerKeys }));
  const attackerSc = new Sidechannel(peerStub(attackerWallet), scConfig({ ownerKeys }));
  const dispatched = [];
  const recvSc = new Sidechannel(
    peerStub(recvWallet),
    scConfig({ ownerKeys, onMessage: (_name, payload) => dispatched.push(payload) })
  );

  const { connection, captured } = fakeConn(ownerPub);
  recvSc._openChannelForConnection(connection, { name: CH, protocol: `sidechannel/${CH}` });
  t.after(() => {
    const record = recvSc.connections.get(connection)?.get(CH);
    if (record?.openTimer) clearTimeout(record.openTimer);
    try {
      record?.channel?.close?.();
    } catch (_error) {}
  });
  assert.equal(typeof captured.onmessage, 'function', 'message handler captured');

  const welcome = makeOwnerWelcome(ownerSc, ownerWallet, ownerPub);
  assert.equal(recvSc._verifyWelcome(welcome, CH, connection), true, 'test-built welcome verifies');
  recvSc.welcomedChannels.delete(CH);

  const legitWelcome = ownerSc._buildPayload(CH, { control: 'welcome', welcome });
  captured.onmessage(legitWelcome);
  assert.equal(recvSc._isWelcomed(CH), true, 'legit owner welcome established access');
  assert.equal(dispatched.length, 0, 'welcome frame not delivered as content');

  const ownerContent = ownerSc._buildPayload(CH, 'legit-signal');
  captured.onmessage(ownerContent);
  assert.equal(dispatched.length, 1, 'owner content delivered');
  assert.equal(dispatched[0].message, 'legit-signal');

  const attack = attackerSc._buildPayload(CH, {
    control: 'welcome',
    welcome,
    text: 'INJECTED',
    evil: true,
  });
  attack.from = ownerPub;
  attack.origin = ownerPub;
  captured.onmessage(attack);
  assert.equal(dispatched.length, 1, 'welcome-frame content injection is not dispatched');

  const spoof = attackerSc._buildPayload(CH, 'spoofed-signal');
  spoof.from = ownerPub;
  spoof.origin = ownerPub;
  captured.onmessage(spoof);
  assert.equal(dispatched.length, 1, 'spoofed non-owner content rejected');
});
