import assert from 'node:assert/strict';
import { Duplex } from 'node:stream';
import test from 'node:test';
import b4a from 'b4a';
import Protomux from 'protomux';
import PeerWallet from 'trac-wallet';
import MayhemFeature, {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_PAYLOAD_BYTES,
  MAYHEM_RELAY_POW_EXEMPT_CONTROLS,
} from '../features/mayhem/index.js';
import Sidechannel from '../features/sidechannel/index.js';

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

test('five concurrent provider records return through real direct and relayed Protomux paths', async () => {
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

  const participantSidechannel = new Sidechannel(
    participantPeer,
    sidechannelConfig((channel, payload) => (
      participantFeature.handleSidechannelMessage(channel, payload)
    ), { relayEnabled: true })
  );
  const writerSidechannel = new Sidechannel(
    writerPeer,
    sidechannelConfig((channel, payload) => (
      writerFeature.handleSidechannelMessage(channel, payload)
    ), { relayEnabled: true })
  );
  const relaySidechannel = new Sidechannel(
    relayPeer,
    sidechannelConfig(() => {}, { relayEnabled: true })
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
