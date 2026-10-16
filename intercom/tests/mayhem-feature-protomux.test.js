import assert from 'node:assert/strict';
import { Duplex } from 'node:stream';
import test from 'node:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import b4a from 'b4a';
import Protomux from 'protomux';
import PeerWallet from 'trac-wallet';
import MayhemFeature, {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_PAYLOAD_BYTES,
  MAYHEM_RELAY_POW_EXEMPT_CONTROLS,
  serviceSigningMessage,
} from '../features/mayhem/index.js';
import Sidechannel from '../features/sidechannel/index.js';
import { CONTRACT_VERSION } from '../contract/contract.js';
import { proxyRegistryFeatureKey } from '../contract/proxy-protocol.js';
import { proxyContractFixture } from './helpers/proxy.js';
import { createProxyCanonicalSnapshot } from '../features/mayhem/proxy-canonical-view.js';
import { installProxyPublicationController } from '../features/mayhem/proxy-publication-transport.js';
import { discoverProxyCatalog } from '../src/rpc.js';

class HexWallet extends PeerWallet {
  get publicKey() {
    const key = super.publicKey;
    return key ? b4a.toString(key, 'hex') : null;
  }

  sign(message) {
    const body = b4a.isBuffer(message) ? message : b4a.from(String(message));
    return b4a.toString(super.sign(body), 'hex');
  }

  verify(signature, message, publicKey = this.publicKey) {
    const sig = b4a.isBuffer(signature) ? signature : b4a.from(String(signature), 'hex');
    const body = b4a.isBuffer(message) ? message : b4a.from(String(message));
    const key = b4a.isBuffer(publicKey) ? publicKey : b4a.from(String(publicKey), 'hex');
    return PeerWallet.verify(sig, body, key);
  }
}

const makeWallet = async () => {
  const wallet = new HexWallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  return wallet;
};

const memoryDuplexPair = () => {
  let left;
  let right;
  left = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      right.push(Buffer.from(chunk));
      callback();
    },
    final(callback) {
      right.push(null);
      callback();
    },
  });
  right = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      left.push(Buffer.from(chunk));
      callback();
    },
    final(callback) {
      left.push(null);
      callback();
    },
  });
  return [left, right];
};

const waitFor = async (predicate, timeoutMs = 1_000) => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail('condition did not become true before timeout');
};

const peerFor = (wallet, admin, writable) => {
  const state = new Map([['admin', admin]]);
  return {
    wallet,
    state,
    base: {
      writable,
      view: {
        async get(key) {
          return state.has(key) ? { value: state.get(key) } : null;
        },
      },
    },
    protocol: {
      instance: {
        features: {},
        featMaxBytes: () => 256 * 1024,
      },
    },
  };
};

const sidechannelConfig = (onMessage, { relayEnabled = false } = {}) => ({
  channels: [MAYHEM_RELAY_CHANNEL],
  welcomeRequired: false,
  relayEnabled,
  maxMessageBytesByChannel: {
    [MAYHEM_RELAY_CHANNEL]: MAYHEM_RELAY_MAX_PAYLOAD_BYTES,
  },
  powEnabled: true,
  powDifficulty: 8,
  powRequiredChannels: [MAYHEM_RELAY_CHANNEL],
  powExemptControlsByChannel: {
    [MAYHEM_RELAY_CHANNEL]: MAYHEM_RELAY_POW_EXEMPT_CONTROLS,
  },
  onMessage,
});

const connectSidechannels = (leftPeer, left, rightPeer, right) => {
  const [leftConnection, rightConnection] = memoryDuplexPair();
  leftConnection.remotePublicKey = b4a.from(rightPeer.wallet.publicKey, 'hex');
  rightConnection.remotePublicKey = b4a.from(leftPeer.wallet.publicKey, 'hex');
  leftConnection.userData = Protomux.from(leftConnection);
  rightConnection.userData = Protomux.from(rightConnection);
  leftPeer.swarm.connections.add(leftConnection);
  rightPeer.swarm.connections.add(rightConnection);
  left._prepareConnection(leftConnection);
  right._prepareConnection(rightConnection);
  return [leftConnection, rightConnection];
};

// Retain the previous sender's signed TTL-one broadcast wire behavior. Receive
// encoding, signature validation and Mayhem feature code are unchanged.
const useLegacyBroadcast = (sidechannel) => {
  sidechannel.broadcast = function (channel, message) {
    const entry = this.channels.get(channel);
    const payload = this._buildPayload(channel, message);
    if (!entry || !this._payloadWithinBounds(channel, payload)) return false;
    this._sendBroadcastPayload(entry, payload, false);
    return true;
  };
};

for (const legacy of [null, 'participant', 'writer']) {
  test(`signed service survives cold reconnect and lost result/ACK with ${legacy ?? 'no'} legacy sender`, async t => {
    const [participantWallet, writerWallet, observerWallet, actorWallet] = await Promise.all(
      Array.from({ length: 4 }, makeWallet)
    );
    const admin = writerWallet.publicKey;
    const participant = peerFor(participantWallet, admin, false);
    const writer = peerFor(writerWallet, admin, true);
    const observer = peerFor(observerWallet, admin, false);
    for (const peer of [participant, writer, observer]) {
      peer.swarm = { connections: new Set(), joinPeer() {} };
    }
    let handled = 0;
    let receivedResults = 0;
    let receivedAcks = 0;
    let directConnections = [];
    let reconnects = 0;
    let connectTimer = null;
    const connections = [];
    const received = [];
    const unrelated = [];
    const participantFeature = new MayhemFeature(participant, { timeoutMs: 1500, retryMs: 50 });
    const writerFeature = new MayhemFeature(writer, {
      resultRetryMs: 20, resultRetryMax: 4, resultConnectGraceMs: 5, connectTimeoutMs: 250,
      serviceHandler: async (service, value, authorization) => {
        assert.equal(service, 'stripe_connect_status');
        assert.equal(authorization.actor, actorWallet.publicKey);
        assert.equal(authorization.transport, participantWallet.publicKey);
        assert.equal(value.provider, actorWallet.publicKey);
        handled += 1;
        // Lose the direct session after the request was authenticated. The
        // existing result owner must reconnect without broadcasting a result.
        for (const [index, connection] of directConnections.entries()) {
          [participant, writer][index].swarm.connections.delete(connection);
          connection.destroy();
        }
        directConnections = [];
        await new Promise(resolve => setImmediate(resolve));
        return { ok: true, status: 'applied', provider: value.provider };
      },
    });
    participantFeature.key = writerFeature.key = 'mayhem';
    participant.sidechannel = new Sidechannel(participant, {
      ...sidechannelConfig((channel, payload) => {
        received.push(payload);
        if (payload.message.control === 'mayhem_service_result' && ++receivedResults === 1) return;
        return participantFeature.handleSidechannelMessage(channel, payload);
      }, { relayEnabled: true }),
      directConnectPollMs: 5,
    });
    writer.sidechannel = new Sidechannel(writer, {
      ...sidechannelConfig((channel, payload) => {
        received.push(payload);
        if (payload.message.control === 'mayhem_service_result_ack' && ++receivedAcks === 1) return;
        return writerFeature.handleSidechannelMessage(channel, payload);
      }, { relayEnabled: true }),
      directConnectPollMs: 5,
    });
    observer.sidechannel = new Sidechannel(observer,
      sidechannelConfig((_channel, payload) => unrelated.push(payload), { relayEnabled: true }));
    for (const peer of [participant, writer, observer]) peer.sidechannel.started = true;
    if (legacy) useLegacyBroadcast({ participant, writer }[legacy].sidechannel);
    t.after(async () => {
      if (connectTimer !== null) clearTimeout(connectTimer);
      await participantFeature.stop();
      await writerFeature.stop();
      for (const connection of connections) connection.destroy();
      for (const peer of [participant, writer, observer]) await peer.sidechannel.stop();
    });
    connections.push(
      ...connectSidechannels(participant, participant.sidechannel, observer, observer.sidechannel),
      ...connectSidechannels(writer, writer.sidechannel, observer, observer.sidechannel)
    );
    await waitFor(() => (
      participant.sidechannel._directPeerChannelReady(observerWallet.publicKey, MAYHEM_RELAY_CHANNEL)
      && writer.sidechannel._directPeerChannelReady(observerWallet.publicKey, MAYHEM_RELAY_CHANNEL)
    ));
    const connectWhenRequested = () => {
      if (connectTimer !== null || directConnections.length > 0) return;
      connectTimer = setTimeout(() => {
        connectTimer = null;
        reconnects += 1;
        directConnections = connectSidechannels(participant, participant.sidechannel, writer, writer.sidechannel);
        connections.push(...directConnections);
      }, 30);
    };
    participant.swarm.joinPeer = writer.swarm.joinPeer = connectWhenRequested;
    const service = 'stripe_connect_status';
    const envelope = {
      actor: actorWallet.publicKey, admin, transport: participantWallet.publicKey, signing_version: 1,
      payload: { provider: actorWallet.publicKey, request_nonce: '12'.repeat(32) },
    };
    envelope.signature = actorWallet.sign(serviceSigningMessage(service, envelope));
    const result = await participantFeature.requestService(service, envelope);
    assert.equal(result.ok, true);
    assert.equal(result.provider, actorWallet.publicKey);
    await waitFor(() => writerFeature.serviceProcessed.values().next().value?.acked === true);
    assert.equal(handled, 1, 'lost transport and ACKs cannot repeat the service work');
    assert.equal(reconnects, 2, 'both initial request and result used the existing cold-connect owner');
    assert.ok(receivedResults >= 3, 'lost result and lost ACK each require another identical result');
    assert.ok(receivedAcks >= 2);
    const resultDigests = new Set(received.filter(payload =>
      payload.message.control === 'mayhem_service_result').map(payload => payload.message.result_digest));
    assert.equal(resultDigests.size, 1);
    const legacyKey = legacy ? { participant, writer }[legacy].wallet.publicKey : null;
    for (const payload of received) {
      assert.equal(participant.sidechannel.verifyPayload(payload), true);
      if (payload.from === legacyKey) assert.ok(payload.ttl === 0 || payload.ttl === 1);
      else assert.equal(payload.ttl, 0);
    }
    assert.ok(unrelated.every(payload => payload.from === legacyKey), 'updated senders never fan out addressed controls');
    if (legacy) assert.ok(unrelated.some(payload => payload.ttl === 1), 'legacy senders retain their existing fanout behavior');
    else assert.equal(unrelated.length, 0);
  });
}

test('feature result and ACK cross the real bidirectional Sidechannel Protomux session', async () => {
  const [participantWallet, writerWallet] = await Promise.all([makeWallet(), makeWallet()]);
  const admin = writerWallet.publicKey;
  const participantPeer = peerFor(participantWallet, admin, false);
  const writerPeer = peerFor(writerWallet, admin, true);
  const participantFeature = new MayhemFeature(participantPeer, {
    timeoutMs: 750,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writerPeer, {
    resultRetryMs: 100,
    resultRetryMax: 3,
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participantPeer.protocol.instance.features.mayhem = participantFeature;
  writerPeer.protocol.instance.features.mayhem = writerFeature;

  writerFeature._applyRelayed = async () => ({
    ok: true,
    accepted: true,
    status: 'applied',
    result: { ok: true, status: 'applied' },
  });

  let participantMessages = 0;
  let writerMessages = 0;
  const participantSidechannel = new Sidechannel(
    participantPeer,
    sidechannelConfig((channel, payload) => {
      participantMessages += 1;
      return participantFeature.handleSidechannelMessage(channel, payload);
    })
  );
  const writerSidechannel = new Sidechannel(
    writerPeer,
    sidechannelConfig((channel, payload) => {
      writerMessages += 1;
      return writerFeature.handleSidechannelMessage(channel, payload);
    })
  );
  participantPeer.sidechannel = participantSidechannel;
  writerPeer.sidechannel = writerSidechannel;

  const [participantConnection, writerConnection] = memoryDuplexPair();
  participantConnection.remotePublicKey = b4a.from(writerWallet.publicKey, 'hex');
  writerConnection.remotePublicKey = b4a.from(participantWallet.publicKey, 'hex');
  participantConnection.userData = Protomux.from(participantConnection);
  writerConnection.userData = Protomux.from(writerConnection);
  participantPeer.swarm = {
    connections: new Set([participantConnection]),
    joinPeer() {},
  };
  writerPeer.swarm = {
    connections: new Set([writerConnection]),
    joinPeer() {},
  };
  participantSidechannel.started = true;
  writerSidechannel.started = true;
  participantSidechannel._prepareConnection(participantConnection);
  writerSidechannel._prepareConnection(writerConnection);

  await waitFor(() => (
    participantSidechannel._directPeerChannelReady(admin, MAYHEM_RELAY_CHANNEL) &&
    writerSidechannel._directPeerChannelReady(
      participantWallet.publicKey,
      MAYHEM_RELAY_CHANNEL
    )
  ));

  const result = await participantFeature.relay(
    `consent/${participantWallet.publicKey}/1/rules-hash`,
    {
      op: 'consent',
      sender: participantWallet.publicKey,
      ver: 1,
      hash: 'rules-hash',
      sig: 'dd'.repeat(64),
    }
  );

  assert.equal(result.ok, true);
  assert.equal(result.status, 'applied');
  assert.equal(writerMessages, 2);
  assert.equal(participantMessages, 1);
  const cached = writerFeature.processed.values().next().value;
  assert.equal(cached.acked, true);
  assert.equal(cached.deliveryStarted, false);

  await participantFeature.stop();
  await writerFeature.stop();
  participantConnection.destroy();
  writerConnection.destroy();
});

test('five concurrent provider records keep results and ACKs off the unrelated Protomux peer', async () => {
  const [participantWallet, writerWallet, relayWallet] = await Promise.all([
    makeWallet(),
    makeWallet(),
    makeWallet(),
  ]);
  const admin = writerWallet.publicKey;
  const participantPeer = peerFor(participantWallet, admin, false);
  const writerPeer = peerFor(writerWallet, admin, true);
  const relayPeer = peerFor(relayWallet, admin, false);
  for (const peer of [participantPeer, writerPeer, relayPeer]) {
    peer.swarm = { connections: new Set(), joinPeer() {} };
  }

  const participantFeature = new MayhemFeature(participantPeer, {
    timeoutMs: 1_500,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writerPeer, {
    resultRetryMs: 100,
    resultRetryMax: 3,
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participantPeer.protocol.instance.features.mayhem = participantFeature;
  writerPeer.protocol.instance.features.mayhem = writerFeature;

  const applied = [];
  writerFeature._applyRelayed = async (key, value) => {
    applied.push(key);
    await new Promise((resolve) => setTimeout(resolve, 10));
    return {
      ok: true,
      accepted: true,
      status: 'applied',
      feature: 'mayhem',
      key,
      hash: '91'.repeat(32),
      result_key: `fr/${'92'.repeat(32)}`,
      result: {
        ok: true,
        op: 'recordUsageReceipt',
        epoch: 170,
        billing_epoch: 170,
        billing_id: value.receipt.body.billing_id,
        billing_attempt: 0,
        receipt_seq: value.receipt.body.seq,
        receipt_hash: '93'.repeat(32),
        idempotent: false,
      },
    };
  };

  const addressed = [];
  const unrelated = [];
  const participantSidechannel = new Sidechannel(
    participantPeer,
    sidechannelConfig((channel, payload) => {
      if (payload.message.control === 'mayhem_feature_result') addressed.push(payload);
      return participantFeature.handleSidechannelMessage(channel, payload);
    }, { relayEnabled: true })
  );
  const writerSidechannel = new Sidechannel(
    writerPeer,
    sidechannelConfig((channel, payload) => {
      if (payload.message.control === 'mayhem_feature_result_ack') addressed.push(payload);
      return writerFeature.handleSidechannelMessage(channel, payload);
    }, { relayEnabled: true })
  );
  const relaySidechannel = new Sidechannel(
    relayPeer,
    sidechannelConfig((_channel, payload) => unrelated.push(payload), { relayEnabled: true })
  );
  participantPeer.sidechannel = participantSidechannel;
  writerPeer.sidechannel = writerSidechannel;
  relayPeer.sidechannel = relaySidechannel;
  for (const sidechannel of [participantSidechannel, writerSidechannel, relaySidechannel]) {
    sidechannel.started = true;
  }

  const connections = [
    ...connectSidechannels(participantPeer, participantSidechannel, writerPeer, writerSidechannel),
    ...connectSidechannels(participantPeer, participantSidechannel, relayPeer, relaySidechannel),
    ...connectSidechannels(writerPeer, writerSidechannel, relayPeer, relaySidechannel),
  ];
  await waitFor(() => (
    participantSidechannel._directPeerChannelReady(admin, MAYHEM_RELAY_CHANNEL) &&
    writerSidechannel._directPeerChannelReady(participantWallet.publicKey, MAYHEM_RELAY_CHANNEL) &&
    participantSidechannel._directPeerChannelReady(relayWallet.publicKey, MAYHEM_RELAY_CHANNEL) &&
    relaySidechannel._directPeerChannelReady(participantWallet.publicKey, MAYHEM_RELAY_CHANNEL) &&
    writerSidechannel._directPeerChannelReady(relayWallet.publicKey, MAYHEM_RELAY_CHANNEL) &&
    relaySidechannel._directPeerChannelReady(admin, MAYHEM_RELAY_CHANNEL)
  ));

  const provider = 'ab'.repeat(32);
  const requests = Array.from({ length: 5 }, (_, index) => {
    const billingId = (index + 1).toString(16).padStart(64, '0');
    const value = {
      op: 'record_usage_receipt',
      contract_version: 21,
      epoch: 170,
      payout_revision: '94'.repeat(32),
      receipt: {
        body: {
          provider,
          billing_id: billingId,
          seq: index + 1,
          locked_rate_map: [
            { unit: 'input_token', per_unit_au: '50000000000', granularity: 1_000 },
            { unit: 'output_token', per_unit_au: '350000000000', granularity: 1_000 },
          ],
        },
        enclave_sig: '95'.repeat(64),
        enclave_pubkey: '96'.repeat(32),
        user_sig: '97'.repeat(64),
      },
      provider_sig: '98'.repeat(64),
    };
    return participantFeature.relay(
      `receipt/submit/170/${billingId}/0/${index + 1}/${'99'.repeat(32)}`,
      value
    );
  });
  const results = await Promise.all(requests);

  assert.equal(applied.length, 5);
  assert.deepEqual(results.map((result) => result.status), Array(5).fill('applied'));
  assert.equal(participantFeature.pending.size, 0);
  assert.equal(writerFeature.processed.size, 5);
  for (const cached of writerFeature.processed.values()) {
    assert.equal(cached.acked, true);
    assert.equal(cached.deliveryStarted, false);
  }
  assert.equal(addressed.length, 10);
  for (const payload of addressed) {
    assert.equal(payload.ttl, 0);
    assert.equal(participantSidechannel.verifyPayload(payload), true);
  }
  assert.equal(unrelated.length, 5, 'only the five unaddressed feature requests retain gossip');
  assert.ok(unrelated.every(payload => payload.message.control === 'mayhem_feature_request'));

  await participantFeature.stop();
  await writerFeature.stop();
  for (const connection of connections) connection.destroy();
});

test('a canonical result returns over real Protomux without a synthetic ACK append', async () => {
  const [participantWallet, writerWallet] = await Promise.all([makeWallet(), makeWallet()]);
  const admin = writerWallet.publicKey;
  const participantPeer = peerFor(participantWallet, admin, false);
  const writerPeer = peerFor(writerWallet, admin, true);
  participantPeer.swarm = { connections: new Set(), joinPeer() {} };
  writerPeer.swarm = { connections: new Set(), joinPeer() {} };

  const participantFeature = new MayhemFeature(participantPeer, {
    timeoutMs: 1_000,
    retryMs: 200,
  });
  const writerFeature = new MayhemFeature(writerPeer, {
    resultRetryMs: 100,
    resultRetryMax: 3,
    resultPollMs: 5,
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';

  let appendCount = 0;
  let canonicalResultKey = null;
  writerPeer.base.append = async (operation) => {
    appendCount += 1;
    assert.equal(appendCount, 1, 'submit must append exactly one feature operation');
    canonicalResultKey = `fr/${operation.value.dispatch.hash}`;
    writerPeer.state.set(canonicalResultKey, {
      status: 'applied',
      ok: true,
      result: { ok: true, op: 'consent' },
    });
  };

  let participantResults = 0;
  let writerAcks = 0;
  const participantSidechannel = new Sidechannel(
    participantPeer,
    sidechannelConfig((channel, payload) => {
      if (payload.message?.control === 'mayhem_feature_result') participantResults += 1;
      return participantFeature.handleSidechannelMessage(channel, payload);
    })
  );
  const writerSidechannel = new Sidechannel(
    writerPeer,
    sidechannelConfig((channel, payload) => {
      if (payload.message?.control === 'mayhem_feature_result_ack') writerAcks += 1;
      return writerFeature.handleSidechannelMessage(channel, payload);
    })
  );
  participantPeer.sidechannel = participantSidechannel;
  writerPeer.sidechannel = writerSidechannel;
  participantSidechannel.started = true;
  writerSidechannel.started = true;
  const connections = connectSidechannels(
    participantPeer,
    participantSidechannel,
    writerPeer,
    writerSidechannel
  );
  await waitFor(() => (
    participantSidechannel._directPeerChannelReady(admin, MAYHEM_RELAY_CHANNEL) &&
    writerSidechannel._directPeerChannelReady(participantWallet.publicKey, MAYHEM_RELAY_CHANNEL)
  ));

  const request = participantFeature.relay(
    `consent/${participantWallet.publicKey}/1/rules-hash`,
    {
      op: 'consent',
      sender: participantWallet.publicKey,
      ver: 1,
      hash: 'rules-hash',
      sig: 'dd'.repeat(64),
    }
  );
  await waitFor(() => appendCount === 1 && canonicalResultKey !== null);
  assert.equal(writerPeer.state.get(canonicalResultKey).status, 'applied');
  assert.equal(
    writerSidechannel._directPeerChannelReady(
      participantWallet.publicKey,
      MAYHEM_RELAY_CHANNEL
    ),
    true
  );

  const result = await request;
  assert.equal(result.ok, true);
  assert.equal(result.status, 'applied');
  assert.equal(participantResults, 1);
  assert.equal(writerAcks, 1);
  assert.equal(writerFeature.processed.values().next().value.pending, false);

  await participantFeature.stop();
  await writerFeature.stop();
  for (const connection of connections) connection.destroy();
});

test('proxy preflight and publication cross signed Protomux into the actual canonical registry view', async t => {
  const f = await proxyContractFixture();
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-proxy-relay-'));
  const store = new Corestore(root);
  let featureAppends = 0;
  const base = new Autobase(store, null, { ackInterval: 0, valueEncoding: 'json',
    open: views => new Hyperbee(views.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
    apply: async (nodes, view) => {
      const batch = view.batch();
      try {
        for (const node of nodes) {
          const operation = node.value;
          if (operation?.type === 'test_seed') {
            for (const [key, value] of operation.entries) await batch.put(key, value);
          } else if (operation?.type === 'feature') {
            featureAppends++;
            await f.contract.execute(operation, batch);
          }
        }
        await batch.flush();
      } finally { await batch.close(); }
    } });
  let writerFeature;
  let providerFeature;
  let proxyController;
  let connections = [];
  t.after(async () => {
    if (writerFeature) await writerFeature.stop();
    if (providerFeature) await providerFeature.stop();
    for (const connection of connections) connection.destroy();
    await base.close(); await proxyController?.close(); await store.close(); fs.rmSync(root, { recursive: true, force: true });
  });
  await base.ready();
  const bootstrap = b4a.toString(base.key, 'hex');
  f.network.subnet_bootstrap = bootstrap;
  f.context.subnet_bootstrap = bootstrap;
  f.peer.config.bootstrap = bootstrap;
  f.config.subnet_bootstrap = bootstrap;
  await f.storage.put('proxy/v1/config', f.config);
  await base.append({ type: 'test_seed', entries: [...f.storage.values.entries()] });
  await base.update();

  const adaptWallet = (identity) => ({
    publicKey: identity.publicKey,
    sign: message => b4a.toString(identity.wallet.sign(b4a.isBuffer(message) ? message : b4a.from(String(message))), 'hex'),
    verify: (signature, message, key) => PeerWallet.verify(
      b4a.isBuffer(signature) ? signature : b4a.from(signature, 'hex'),
      b4a.isBuffer(message) ? message : b4a.from(String(message)),
      b4a.isBuffer(key) ? key : b4a.from(key, 'hex')),
  });
  const writer = f.peer;
  writer.base = base;
  writer.wallet = adaptWallet(f.admin);
  writer.protocol = { instance: { features: {}, featMaxBytes: () => 64000 } };
  writer.contract = { instance: f.contract };
  writer.swarm = { connections: new Set(), joinPeer() {} };
  const provider = peerFor(adaptWallet(f.provider), f.admin.publicKey, false);
  provider.config = { ...writer.config };
  provider.msbClient = { ...writer.msbClient };
  provider.swarm = { connections: new Set(), joinPeer() {} };
  // The provider deliberately has no proxy registry state. Its only local
  // authority hint is admin; admission must come from the signed indexer service.
  writerFeature = new MayhemFeature(writer, { resultTimeoutMs: 1000, resultRetryMs: 50,
    withProxyCanonicalSnapshot: createProxyCanonicalSnapshot(writer, CONTRACT_VERSION) });
  providerFeature = new MayhemFeature(provider, { timeoutMs: 2000, retryMs: 50 });
  writerFeature.key = 'mayhem'; providerFeature.key = 'mayhem';
  proxyController = await installProxyPublicationController(writerFeature, {
    directory: path.join(root, 'proxy-publications'), contractVersion: CONTRACT_VERSION,
  });
  const preflights = new Set();
  const forwarded = new Set();
  const serviceFrames = [];
  writer.sidechannel = new Sidechannel(writer, sidechannelConfig((channel, payload) => {
    if (payload.message.control.startsWith('mayhem_service_')) serviceFrames.push(payload);
    if (payload.message.control === 'mayhem_service_request') preflights.add(payload.message.request_id);
    if (payload.message.control === 'mayhem_feature_request') forwarded.add(payload.message.request_id);
    return writerFeature.handleSidechannelMessage(channel, payload);
  }));
  provider.sidechannel = new Sidechannel(provider, sidechannelConfig((channel, payload) => {
    if (payload.message.control.startsWith('mayhem_service_')) serviceFrames.push(payload);
    return providerFeature.handleSidechannelMessage(channel, payload);
  }));
  writer.sidechannel.started = true; provider.sidechannel.started = true;
  connections = connectSidechannels(provider, provider.sidechannel, writer, writer.sidechannel);
  await waitFor(() => provider.sidechannel._directPeerChannelReady(f.admin.publicKey, MAYHEM_RELAY_CHANNEL));

  const envelope = await f.create();
  const unpaid = structuredClone(envelope); unpaid.admission = null;
  await assert.rejects(providerFeature.relay(await proxyRegistryFeatureKey(unpaid), unpaid), /admission/);
  assert.equal(featureAppends, 0);
  assert.equal(forwarded.size, 0, 'failed preflight must not forward a ledger publication');
  const key = await proxyRegistryFeatureKey(envelope);
  const first = await providerFeature.relay(key, envelope);
  assert.equal(first.ok, true);
  assert.equal(featureAppends, 1);
  const again = await providerFeature.relay(key, envelope);
  assert.equal(again.duplicate, true);
  assert.equal(featureAppends, 1);
  assert.equal(forwarded.size, 1);
  assert.equal(preflights.size, 3, 'each invocation gets a fresh signed preflight, including retries');
  assert.equal((await base.view.get(`proxy/v1/provider/${f.provider.publicKey}`)).value.sequence, 1);
  assert.deepEqual((await base.view.get('payout/epoch/542')).value, { status: 'prepared', native: true });
  const signedLength = base.view.core.signedLength;
  const discovery = await providerFeature.discoverProxyCatalog({ kind: 'markets', filter: { family_id: 'other' } });
  assert.equal(discovery.entries.length, 1);
  assert.deepEqual(discovery.entries[0].value, f.market);
  assert.equal(discovery.proof.signed_length, signedLength);
  assert.equal(discovery.relayed, true, 'actual authenticated remote service response');
  provider.protocol = { instance: { features: { mayhem: providerFeature } } };
  const rpcDiscovery = await discoverProxyCatalog(provider, { query: { kind: 'markets', filter: { family_id: 'other' } } });
  assert.equal(Object.hasOwn(rpcDiscovery, 'relayed'), false);
  assert.equal(Object.hasOwn(rpcDiscovery, 'request_id'), false);
  assert.deepEqual(rpcDiscovery.entries, discovery.entries);
  assert.deepEqual(rpcDiscovery.proof, discovery.proof);
  assert.equal(featureAppends, 1, 'authenticated discovery cannot append a transaction');
  assert.equal(base.view.core.signedLength, signedLength);
  const againDiscovery = await providerFeature.discoverProxyCatalog({ kind: 'markets', filter: { family_id: 'other' }, since: discovery.checkpoint });
  assert.deepEqual(againDiscovery.entries, []);
  assert.notEqual(againDiscovery.request_nonce, discovery.request_nonce, 'a fresh read is not a cached old challenge');
  await assert.rejects(providerFeature.discoverProxyCatalog({ kind: 'markets', prefix: 'bal/' }), /Invalid/);
  await assert.rejects(providerFeature.proxyFinancialState({
    accepted_terms: 'dd'.repeat(32), request_nonce: 'ee'.repeat(32),
  }), { code: 'proxy_financial_pending' });
  assert.equal(featureAppends, 1, 'financial observation cannot append a transaction');
  assert.equal(base.view.core.signedLength, signedLength);
  assert.ok(serviceFrames.some(payload => payload.message.service === 'proxy_financial_state'));
  assert.ok(serviceFrames.some(payload => payload.message.service === 'proxy_discovery'));
  for (const payload of serviceFrames) {
    assert.equal(payload.ttl, 0);
    assert.equal(provider.sidechannel.verifyPayload(payload), true);
  }
});
