import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemFeature, { requestIdFor } from '../features/mayhem/index.js';
import { createServer, submitMayhemFeature } from '../src/rpc.js';

const adminKey = 'aa'.repeat(32);
const providerKey = 'bb'.repeat(32);
const otherKey = 'cc'.repeat(32);

const viewFor = (state) => ({
  async get(key) {
    return state.has(key) ? { value: state.get(key) } : null;
  },
});

const peerFor = (publicKey, { writable = false } = {}) => {
  const state = new Map([['admin', adminKey]]);
  const appended = [];
  const peer = {
    wallet: {
      publicKey,
      sign(message) {
        const suffix = String(message).length.toString(16).padStart(4, '0');
        return `${publicKey}${suffix}`.padEnd(128, '0').slice(0, 128);
      },
    },
    base: {
      writable,
      view: viewFor(state),
      async append(op) {
        appended.push(op);
        const hash = op.value.dispatch.hash;
        state.set(`fr/${hash}`, {
          type: 'feature_result',
          status: 'applied',
          ok: true,
          result: { ok: true, op: op.value.dispatch.value.op },
        });
      },
    },
    protocol: { instance: { features: {} } },
  };
  return { peer, state, appended };
};

const connect = (leftPeer, leftFeature, rightPeer, rightFeature) => {
  leftPeer.sidechannel = {
    started: true,
    verifyPayload(payload, expectedKey) {
      return payload.from === expectedKey && payload.sig === `signed:${payload.from}`;
    },
    broadcast(channel, message) {
      const payload = {
        from: leftPeer.wallet.publicKey,
        message,
        sig: `signed:${leftPeer.wallet.publicKey}`,
      };
      queueMicrotask(() => rightFeature.handleSidechannelMessage(channel, payload));
      return true;
    },
  };
};

const consentValue = (sender = providerKey) => ({
  op: 'consent',
  sender,
  ver: 1,
  hash: 'rules-hash',
  sig: 'dd'.repeat(64),
});

test('read-only participant relays a signed feature to the sole admin writer', async () => {
  const participant = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  const participantFeature = new MayhemFeature(participant.peer, { timeoutMs: 1_000, retryMs: 100 });
  const writerFeature = new MayhemFeature(writer.peer, { timeoutMs: 1_000, retryMs: 100 });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const key = `consent/${providerKey}/1/rules-hash`;
  const value = consentValue();
  const server = createServer(participant.peer);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  let result;
  try {
    const response = await fetch(`http://127.0.0.1:${address.port}/v1/contract/feature`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ feature: 'mayhem', key, value }),
    });
    assert.equal(response.status, 200);
    result = await response.json();
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }

  assert.equal(result.ok, true);
  assert.equal(result.relayed, true);
  assert.equal(writer.appended.length, 1);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended[0].value.dispatch.address, adminKey);
  assert.deepEqual(writer.appended[0].value.dispatch.value, value);
  assert.equal(writer.appended[0].value.dispatch.nonce, requestIdFor('mayhem', key, value));
});

test('admin writer deduplicates a retried relay request by deterministic request id', async () => {
  const writer = peerFor(adminKey, { writable: true });
  const writerFeature = new MayhemFeature(writer.peer, {});
  writerFeature.key = 'mayhem';
  writer.peer.sidechannel = {
    started: true,
    verifyPayload: () => true,
    broadcast: () => true,
  };
  const key = `consent/${providerKey}/1/rules-hash`;
  const value = consentValue();
  const requestId = requestIdFor('mayhem', key, value);
  const payload = {
    from: providerKey,
    sig: `signed:${providerKey}`,
    message: {
      control: 'mayhem_feature_request',
      version: 1,
      request_id: requestId,
      feature: 'mayhem',
      key,
      value,
    },
  };

  await writerFeature.handleSidechannelMessage('0000intercom', payload);
  await writerFeature.handleSidechannelMessage('0000intercom', payload);

  assert.equal(writer.appended.length, 1);
});

test('relay rejects admin operations before network send', async () => {
  const participant = peerFor(providerKey);
  const feature = new MayhemFeature(participant.peer, {});
  feature.key = 'mayhem';
  let broadcasts = 0;
  participant.peer.sidechannel = {
    started: true,
    broadcast() {
      broadcasts += 1;
      return true;
    },
  };

  await assert.rejects(
    feature.relay('rate/tnk/1', { op: 'rate_oracle', at: 1 }),
    /Invalid relayed feature operation/
  );
  assert.equal(broadcasts, 0);
});

test('transport peer may relay an intent signed by an explicitly selected participant key', async () => {
  const participant = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  const participantFeature = new MayhemFeature(participant.peer, { timeoutMs: 1_000, retryMs: 100 });
  const writerFeature = new MayhemFeature(writer.peer, { timeoutMs: 1_000, retryMs: 100 });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const key = `consent/${otherKey}/1/rules-hash`;
  const value = consentValue(otherKey);
  const result = await submitMayhemFeature(participant.peer, { feature: 'mayhem', key, value });

  assert.equal(result.ok, true);
  assert.equal(writer.appended.length, 1);
  assert.equal(writer.appended[0].value.dispatch.value.sender, otherKey);
  assert.equal(writer.appended[0].value.dispatch.address, adminKey);
});

test('read-only provider relays a signed rails preference through the existing lifecycle envelope', async () => {
  const participant = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  const participantFeature = new MayhemFeature(participant.peer, { timeoutMs: 1_000, retryMs: 100 });
  const writerFeature = new MayhemFeature(writer.peer, { timeoutMs: 1_000, retryMs: 100 });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const intent = {
    op: 'set_provider_rails',
    provider: providerKey,
    rails: ['tap'],
    nonce: '11'.repeat(32),
  };
  const key = `intent/provider/${providerKey}/set_provider_rails/${'22'.repeat(32)}`;
  const value = { op: 'provider_lifecycle', intent, sig: '33'.repeat(64) };
  const result = await submitMayhemFeature(participant.peer, { feature: 'mayhem', key, value });

  assert.equal(result.ok, true);
  assert.equal(result.relayed, true);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 1);
  assert.equal(writer.appended[0].value.dispatch.address, adminKey);
  assert.deepEqual(writer.appended[0].value.dispatch.value, value);
});

test('admin-writer RPC keeps the local append path', async () => {
  const writer = peerFor(adminKey, { writable: true });
  let appendCalls = 0;
  writer.peer.protocol.instance.features.mayhem = {
    async append() {
      appendCalls += 1;
      const hash = 'ef'.repeat(64);
      writer.state.set(`fr/${hash}`, { status: 'applied', ok: true, result: { ok: true } });
      return { hash };
    },
  };

  const result = await submitMayhemFeature(writer.peer, {
    feature: 'mayhem',
    key: 'admin/local',
    value: { op: 'admin_only' },
  });

  assert.equal(result.ok, true);
  assert.equal(result.relayed, undefined);
  assert.equal(appendCalls, 1);
});

test('an admin-added cross-signing writer still relays participant features to the admin appender', async () => {
  const crossSigner = peerFor(otherKey, { writable: true });
  let appendCalls = 0;
  let relayCalls = 0;
  crossSigner.peer.protocol.instance.features.mayhem = {
    async append() {
      appendCalls += 1;
    },
    async relay(key, value) {
      relayCalls += 1;
      return { ok: true, relayed: true, key, value };
    },
  };

  const key = `consent/${otherKey}/1/rules-hash`;
  const result = await submitMayhemFeature(crossSigner.peer, {
    feature: 'mayhem',
    key,
    value: consentValue(otherKey),
  });

  assert.equal(result.ok, true);
  assert.equal(result.relayed, true);
  assert.equal(relayCalls, 1);
  assert.equal(appendCalls, 0);
});
