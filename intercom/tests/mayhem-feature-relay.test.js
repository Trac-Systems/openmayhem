import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import sodium from 'sodium-universal';
import { blake3 } from '@tracsystems/blake3';
import { keccak256 } from 'ethereum-cryptography/keccak';
import { secp256k1 } from 'ethereum-cryptography/secp256k1';
import PeerWallet from 'trac-wallet';
import {
  CONTRACT_VERSION,
  adminContractTxDigest,
  providerPayoutBindingMessage,
  providerPayoutTargetBindingMessage,
} from '../contract/contract.js';
import MayhemFeature, {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_MESSAGE_BYTES,
  requestIdFor,
  serviceRequestIdFor,
  serviceSigningMessage,
  stripeConnectRelinkConsentMessage,
} from '../features/mayhem/index.js';
import {
  adminWriterDiagnostics,
  createServer,
  getMayhemStatus,
  getStatePrefix,
  loadPrivateInternalAuthSecret,
  requestStripeCheckout,
  requestStripeConnect,
  submitMayhemFeature,
} from '../src/rpc.js';

const adminKey = 'aa'.repeat(32);
const providerKey = 'bb'.repeat(32);
const otherKey = 'cc'.repeat(32);
const adminTxContext = Object.freeze({
  contract_version: CONTRACT_VERSION,
  msb_bootstrap: '22'.repeat(32),
  network_id: 918,
  subnet_bootstrap: '11'.repeat(32),
});

test('private Stripe auth secret loader uses Windows ACLs and rejects symlinks', () => {
  const secret = '11'.repeat(32);
  const pathModule = { resolve: (value) => `resolved:${value}` };
  const regularFs = {
    lstatSync: () => ({
      mode: 0o100666,
      isFile: () => true,
      isSymbolicLink: () => false,
    }),
    readFileSync: () => `${secret}\n`,
  };
  assert.equal(loadPrivateInternalAuthSecret({
    fsModule: regularFs,
    pathModule,
    secretPath: 'auth.secret',
    platform: 'win32',
  }), secret);
  assert.throws(() => loadPrivateInternalAuthSecret({
    fsModule: regularFs,
    pathModule,
    secretPath: 'auth.secret',
    platform: 'linux',
  }), /group\/world accessible/);
  assert.throws(() => loadPrivateInternalAuthSecret({
    fsModule: {
      ...regularFs,
      lstatSync: () => ({
        mode: 0o100600,
        isFile: () => true,
        isSymbolicLink: () => true,
      }),
    },
    pathModule,
    secretPath: 'auth.secret',
    platform: 'win32',
  }), /non-symlink/);
});

const fakeSignature = (publicKey, message) => {
  const suffix = String(message).length.toString(16).padStart(4, '0');
  return `${publicKey}${suffix}`.padEnd(128, '0').slice(0, 128);
};

const signEthereumPersonalMessage = (privateKey, message) => {
  const body = b4a.from(message, 'utf8');
  const prefix = b4a.from(`\x19Ethereum Signed Message:\n${body.length}`, 'utf8');
  const signature = secp256k1.sign(
    keccak256(b4a.concat([prefix, body])),
    privateKey,
    { lowS: true }
  );
  const bytes = b4a.alloc(65);
  bytes.set(signature.toCompactRawBytes(), 0);
  bytes[64] = signature.recovery + 27;
  return `0x${b4a.toString(bytes, 'hex')}`;
};

const viewFor = (state) => ({
  async get(key) {
    return state.has(key) ? { value: state.get(key) } : null;
  },
});

const peerFor = (publicKey, { writable = false, bootstrap = '11'.repeat(32) } = {}) => {
  const state = new Map([['admin', adminKey]]);
  const appended = [];
  const flushes = [];
  const peer = {
    wallet: {
      publicKey,
      sign(message) {
        return fakeSignature(publicKey, message);
      },
      verify(signature, message, signer) {
        return signature === fakeSignature(signer, message);
      },
    },
    base: {
      key: b4a.from(bootstrap, 'hex'),
      writable,
      view: viewFor(state),
      async append(op) {
        if (op === null) {
          flushes.push(true);
          return;
        }
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
    config: {
      bootstrap: b4a.from(bootstrap, 'hex'),
    },
    msbClient: {
      bootstrapHex: adminTxContext.msb_bootstrap,
      networkId: adminTxContext.network_id,
    },
    protocol: { instance: { features: {} } },
  };
  return { peer, state, appended, flushes };
};

const connect = (leftPeer, leftFeature, rightPeer, rightFeature) => {
  leftPeer.sidechannel = {
    started: true,
    connectDirectPeer: async () => true,
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

const stripeCheckoutValue = (who = providerKey) => ({
  who,
  au: '1000000000000000000',
  currency: 'usd',
  locale: 'en',
  success_url: 'https://stripe.com',
  cancel_url: 'https://stripe.com',
  idempotency_key: 'checkout-test-1',
  request_nonce: '34'.repeat(32),
});

const stripeConnectValue = (provider = providerKey) => ({
  provider,
  country: 'DE',
  request_nonce: '12'.repeat(32),
});

const stripeConnectRelinkValue = (provider = providerKey, overrides = {}) => {
  const unsigned = {
    provider,
    source_provider: otherKey,
    account_id: 'acct_test_provider',
    context_revision: '78'.repeat(32),
    country: 'DE',
    request_nonce: '56'.repeat(32),
    consent_expires_at: Math.floor(Date.now() / 1_000) + 300,
    ...overrides,
  };
  return {
    ...unsigned,
    source_consent_signature: fakeSignature(
      unsigned.source_provider,
      stripeConnectRelinkConsentMessage(unsigned)
    ),
  };
};

const signedServiceValue = (signer, service, payload, transport = signer.wallet.publicKey) => {
  const actor = payload.who || payload.provider;
  const envelope = {
    actor,
    admin: adminKey,
    payload,
    signing_version: 1,
    transport,
  };
  return {
    ...envelope,
    signature: signer.wallet.sign(serviceSigningMessage(service, envelope)),
  };
};

const signedAdminTxValue = async (
  preparedCommand,
  { nonce = '56'.repeat(32), sim = false, context = adminTxContext } = {}
) => {
  const unsigned = {
    op: 'admin_contract_tx',
    prepared_command: preparedCommand,
    address: adminKey,
    context,
    nonce,
    sim,
  };
  const tx = await adminContractTxDigest(unsigned);
  return {
    ...unsigned,
    tx,
    signature: fakeSignature(adminKey, b4a.from(tx, 'hex')),
  };
};

test('service signing message has one cross-runtime canonical form', () => {
  const payload = {
    provider: providerKey,
    request_nonce: '12'.repeat(32),
  };
  assert.equal(
    serviceSigningMessage('stripe_connect_status', {
      actor: providerKey,
      admin: adminKey,
      payload,
      transport: otherKey,
    }),
    `{"actor":"${providerKey}","admin":"${adminKey}","domain":"mayhem-service-request","payload":{"provider":"${providerKey}","request_nonce":"${'12'.repeat(32)}"},"service":"stripe_connect_status","signing_version":1,"transport":"${otherKey}"}`
  );
});

test('Stripe relink source consent has one cross-runtime canonical form', () => {
  assert.equal(
    stripeConnectRelinkConsentMessage({
      provider: providerKey,
      source_provider: otherKey,
      account_id: 'acct_test_provider',
      context_revision: '78'.repeat(32),
      country: 'DE',
      request_nonce: '56'.repeat(32),
      consent_expires_at: 1_900_000_000,
    }),
    `{"account_id":"acct_test_provider","consent_expires_at":1900000000,"context_revision":"${'78'.repeat(32)}","country":"DE","domain":"mayhem-stripe-connect-relink-consent-v1","request_nonce":"${'56'.repeat(32)}","signing_version":1,"source_provider":"${otherKey}","target_provider":"${providerKey}"}`
  );
});

test('service signing message has one cross-runtime Ed25519 signature', () => {
  const seed = b4a.alloc(32, 1);
  const publicKey = b4a.alloc(sodium.crypto_sign_PUBLICKEYBYTES);
  const secretKey = b4a.alloc(sodium.crypto_sign_SECRETKEYBYTES);
  sodium.crypto_sign_seed_keypair(publicKey, secretKey, seed);
  const actor = b4a.toString(publicKey, 'hex');
  assert.equal(actor, '8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c');
  const message = serviceSigningMessage('stripe_connect_status', {
    actor,
    admin: adminKey,
    payload: { provider: actor, request_nonce: '12'.repeat(32) },
    transport: otherKey,
  });
  const signature = b4a.alloc(sodium.crypto_sign_BYTES);
  sodium.crypto_sign_detached(signature, b4a.from(message), secretKey);
  assert.equal(
    b4a.toString(signature, 'hex'),
    '75532c9727d07be74033c992d1ef38cda19553e4250a6d40d91f267c072f202361fd5860615bc731e271e048a612b4996eb60eefca8ccd2abce1057c1ce68e0b'
  );
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
  assert.equal(writer.flushes.length, 1);
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

  await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload);
  await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload);

  assert.equal(writer.appended.length, 1);
});

test('admin writer bounds concurrent remote relay work and returns retryable busy', async () => {
  const writer = peerFor(adminKey, { writable: true });
  const replies = [];
  const writerFeature = new MayhemFeature(writer.peer, { processedInFlightMax: 1 });
  writerFeature.key = 'mayhem';
  writerFeature.processed.set('already-running', {
    at: Date.now(),
    pending: true,
    promise: new Promise(() => {}),
  });
  writer.peer.sidechannel = {
    started: true,
    verifyPayload: () => true,
    broadcast(_channel, message) {
      replies.push(message);
      return true;
    },
  };
  const key = `consent/${providerKey}/1/rules-hash`;
  const value = consentValue();
  const payload = {
    from: providerKey,
    sig: `signed:${providerKey}`,
    message: {
      control: 'mayhem_feature_request',
      version: 1,
      request_id: requestIdFor('mayhem', key, value),
      feature: 'mayhem',
      key,
      value,
    },
  };

  await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload);

  assert.equal(writer.appended.length, 0);
  assert.equal(replies.length, 1);
  assert.equal(replies[0].response.status, 'rejected');
  assert.match(replies[0].response.message, /busy/);
});

test('admin writer retries a previously rejected deterministic relay request', async () => {
  const participant = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  let attempts = 0;
  writer.peer.base.append = async (op) => {
    if (op === null) {
      writer.flushes.push(true);
      return;
    }
    writer.appended.push(op);
    attempts += 1;
    const hash = op.value.dispatch.hash;
    writer.state.set(
      `fr/${hash}`,
      attempts === 1
        ? {
            type: 'feature_result',
            status: 'rejected',
            ok: false,
            result: null,
            error: { message: 'Consent required for rules version 1.' },
          }
        : {
            type: 'feature_result',
            status: 'applied',
            ok: true,
            result: { ok: true, op: op.value.dispatch.value.op },
            error: null,
          }
    );
  };
  const participantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writer.peer, { timeoutMs: 1_000, retryMs: 100 });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const key = `consent/${providerKey}/1/rules-hash`;
  const value = consentValue();
  const first = await submitMayhemFeature(participant.peer, { feature: 'mayhem', key, value });
  const second = await submitMayhemFeature(participant.peer, { feature: 'mayhem', key, value });

  assert.equal(first.ok, false);
  assert.equal(second.ok, true);
  assert.equal(writer.appended.length, 2);
  assert.equal(
    writer.appended[0].value.dispatch.hash,
    writer.appended[1].value.dispatch.hash
  );
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

test('participant retries through the direct admin relay channel until acknowledged', async () => {
  const participant = peerFor(providerKey);
  const events = [];
  participant.peer.sidechannel = {
    started: true,
    async connectDirectPeer(remote, channel, waitMs) {
      events.push(['connect', remote, channel, waitMs]);
      return true;
    },
    broadcast() {
      events.push(['broadcast']);
      return false;
    },
  };
  const feature = new MayhemFeature(participant.peer, { timeoutMs: 25, retryMs: 5 });
  feature.key = 'mayhem';

  const result = await feature.relay(`consent/${providerKey}/1/rules-hash`, consentValue());

  assert.equal(result.ok, false);
  assert.ok(events.length >= 4);
  for (let index = 0; index < events.length; index += 2) {
    assert.deepEqual(events[index], ['connect', adminKey, MAYHEM_RELAY_CHANNEL, 25]);
    assert.deepEqual(events[index + 1], ['broadcast']);
  }
});

test('participant does not broadcast when the canonical admin channel is unavailable', async () => {
  const participant = peerFor(providerKey);
  let broadcasts = 0;
  participant.peer.sidechannel = {
    started: true,
    connectDirectPeer: async () => false,
    broadcast() {
      broadcasts += 1;
      return true;
    },
  };
  const feature = new MayhemFeature(participant.peer, { timeoutMs: 10 });
  feature.key = 'mayhem';

  const result = await feature.relay(
    `consent/${providerKey}/1/rules-hash`,
    consentValue()
  );

  assert.equal(result.ok, false);
  assert.match(result.message, /direct channel to the canonical admin/);
  assert.equal(broadcasts, 0);
});

test('participant relay has no implicit wall-clock deadline and stops cleanly', async () => {
  const participant = peerFor(providerKey);
  participant.peer.sidechannel = {
    started: true,
    connectDirectPeer: async () => false,
    broadcast: () => false,
  };
  const feature = new MayhemFeature(participant.peer, { retryMs: 5 });
  feature.key = 'mayhem';
  const relaying = feature.relay(
    `consent/${providerKey}/1/rules-hash`,
    consentValue()
  );

  const state = await Promise.race([
    relaying.then(() => 'settled'),
    new Promise((resolve) => setTimeout(() => resolve('pending'), 30)),
  ]);
  assert.equal(state, 'pending');

  await feature.stop();
  const result = await relaying;
  assert.equal(result.ok, false);
  assert.match(result.message, /relay stopped/);
});

test('participant reconnects the direct admin channel while a relay is pending', async () => {
  const participant = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  const participantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 500,
    retryMs: 10,
  });
  const writerFeature = new MayhemFeature(writer.peer, { timeoutMs: 500, retryMs: 10 });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';

  let connectAttempts = 0;
  participant.peer.sidechannel = {
    started: true,
    async connectDirectPeer() {
      connectAttempts += 1;
      return connectAttempts >= 2;
    },
    verifyPayload(payload, expectedKey) {
      return payload.from === expectedKey && payload.sig === `signed:${payload.from}`;
    },
    broadcast(channel, message) {
      const payload = {
        from: providerKey,
        message,
        sig: `signed:${providerKey}`,
      };
      queueMicrotask(() => writerFeature.handleSidechannelMessage(channel, payload));
      return true;
    },
  };
  writer.peer.sidechannel = {
    started: true,
    verifyPayload(payload, expectedKey) {
      return payload.from === expectedKey && payload.sig === `signed:${payload.from}`;
    },
    broadcast(channel, message) {
      const payload = {
        from: adminKey,
        message,
        sig: `signed:${adminKey}`,
      };
      queueMicrotask(() => participantFeature.handleSidechannelMessage(channel, payload));
      return true;
    },
  };

  const key = `consent/${providerKey}/1/rules-hash`;
  const result = await participantFeature.relay(key, consentValue());

  assert.equal(result.ok, true);
  assert.ok(connectAttempts >= 2);
  assert.equal(writer.appended.length, 1);
});

test('relay accepts only the dedicated channel and drops oversized feature envelopes', async () => {
  const writer = peerFor(adminKey, { writable: true });
  const writerFeature = new MayhemFeature(writer.peer, { maxMessageBytes: 512 });
  writerFeature.key = 'mayhem';
  writer.peer.sidechannel = {
    started: true,
    verifyPayload: () => true,
    broadcast: () => true,
  };
  const key = `consent/${providerKey}/1/rules-hash`;
  const value = consentValue();
  const message = {
    control: 'mayhem_feature_request',
    version: 1,
    request_id: requestIdFor('mayhem', key, value),
    feature: 'mayhem',
    key,
    value,
  };
  const payload = { from: providerKey, sig: `signed:${providerKey}`, message };

  assert.equal(await writerFeature.handleSidechannelMessage('0000intercom', payload), false);
  assert.equal(writer.appended.length, 0);
  assert.equal(await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload), true);
  assert.equal(writer.appended.length, 1);

  const oversizedValue = { ...value, padding: 'x'.repeat(1_000) };
  const oversizedPayload = {
    from: providerKey,
    sig: `signed:${providerKey}`,
    message: {
      ...message,
      request_id: requestIdFor('mayhem', key, oversizedValue),
      value: oversizedValue,
    },
  };
  assert.equal(
    await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, oversizedPayload),
    true
  );
  assert.equal(writer.appended.length, 1);
});

test('16KB relay cap fits a maximal canonical targeted spend-reserve envelope', () => {
  const participant = peerFor(providerKey);
  const feature = new MayhemFeature(participant.peer, {});
  feature.key = 'mayhem';
  const lockedRateMap = Array.from({ length: 16 }, (_, index) => ({
    unit: `unit_${String(index).padStart(2, '0')}_${'x'.repeat(53)}`,
    per_unit_au: '99999999999999999999999999999999999999',
    granularity: Number.MAX_SAFE_INTEGER,
  }));
  const voucher = {
    session_id: '11'.repeat(32),
    billing_id: '66'.repeat(32),
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    rail: 'tap',
    enclave_id: '22'.repeat(32),
    price_ver: Number.MAX_SAFE_INTEGER,
    locked_rate_map: lockedRateMap,
    locked_per_req_au: '99999999999999999999999999999999999999',
    locked_min_session_au: '99999999999999999999999999999999999999',
    served_ctx: Number.MAX_SAFE_INTEGER,
    required_modalities: ['text'],
    ctx_bracket: 'z'.repeat(64),
    ctx_bracket_table_ver: Number.MAX_SAFE_INTEGER,
    max_spend_au: '99999999999999999999999999999999999999',
    checkpoint_every: { tokens: Number.MAX_SAFE_INTEGER, ms: Number.MAX_SAFE_INTEGER },
    user_sig: '33'.repeat(64),
  };
  const value = {
    op: 'spend_reserve_targeted',
    payout_revision: '77'.repeat(32),
    contract_version: 13,
    session_id: voucher.session_id,
    epoch: Number.MAX_SAFE_INTEGER,
    at: Number.MAX_SAFE_INTEGER,
    rail: 'tap',
    user: '44'.repeat(32),
    provider: providerKey,
    enclave_id: voucher.enclave_id,
    price_ver: voucher.price_ver,
    rules_ver: Number.MAX_SAFE_INTEGER,
    served_ctx: voucher.served_ctx,
    required_modalities: voucher.required_modalities,
    ctx_bracket: voucher.ctx_bracket,
    ctx_bracket_table_ver: voucher.ctx_bracket_table_ver,
    max_spend_au: voucher.max_spend_au,
    voucher,
    provider_sig: '55'.repeat(64),
  };
  const key = `hold/targeted/tap/${value.user}/${value.epoch}/${voucher.session_id}/${'88'.repeat(32)}`;
  const message = {
    control: 'mayhem_feature_request',
    version: 1,
    request_id: requestIdFor('mayhem', key, value),
    feature: 'mayhem',
    key,
    value,
  };

  const measuredBytes = feature.relayMessageBytes(message);
  assert.ok(measuredBytes < MAYHEM_RELAY_MAX_MESSAGE_BYTES);
  assert.ok(measuredBytes > 4_000);
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

test('read-only provider relays a verified payout binding only to the sole admin writer', async () => {
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

  const target = PeerWallet.encodeBech32mSafe('trac', b4a.from(providerKey, 'hex'));
  const intent = {
    op: 'bind_provider_payout',
    network: 'mainnet',
    admin: adminKey,
    bootstrap: '11'.repeat(32),
    context_revision: '22'.repeat(32),
    provider: providerKey,
    rail: 'tnk',
    currency: null,
    chain_id: null,
    target,
    target_wallet: providerKey,
    target_signature: null,
    previous_revision: null,
    payment_config_version: 7,
    nonce: '55'.repeat(32),
    expires_after_epoch: 10,
  };
  intent.target_signature = fakeSignature(
    providerKey,
    b4a.from(providerPayoutTargetBindingMessage(intent))
  );
  const value = {
    op: 'bind_provider_payout',
    intent,
    provider_signature: fakeSignature(
      providerKey,
      b4a.from(providerPayoutBindingMessage(intent))
    ),
  };
  const revision = b4a.toString(
    await blake3(b4a.from(providerPayoutBindingMessage(intent))),
    'hex'
  );
  const key = `payout/binding/tnk/${providerKey}/${revision}`;
  writer.state.set('payments/current', {
    rails: ['fiat', 'tap', 'tnk'],
    tap: { chain_id: 1 },
    tnk: { network: 'mainnet' },
    ver: 7,
    set_by: adminKey,
    set_by_role: 'admin',
  });
  writer.state.set('payout/context/current', {
    payment_config_version: 7,
    revision: intent.context_revision,
  });
  writer.state.set(`payout/context/7/${intent.context_revision}`, {
    revision: intent.context_revision,
    network: 'mainnet',
    admin: adminKey,
    bootstrap: '11'.repeat(32),
    payment_config_version: 7,
    published_by: adminKey,
    published_by_role: 'admin',
  });
  writer.state.set(`prov/${providerKey}`, {
    provider: providerKey,
    status: 'active',
    accepted_rails: ['tnk'],
  });
  const result = await submitMayhemFeature(participant.peer, { feature: 'mayhem', key, value });

  assert.equal(result.ok, true);
  assert.equal(result.relayed, true);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 1);
  assert.equal(writer.appended[0].value.dispatch.address, adminKey);
  assert.deepEqual(writer.appended[0].value.dispatch.value, value);

  writer.state.set(`payout/nonce/${providerKey}/${intent.nonce}`, {
    provider: providerKey,
    rail: 'tnk',
    nonce: intent.nonce,
    revision,
  });
  writer.state.set(key, {
    type: 'provider_payout_binding',
    provider: providerKey,
    rail: 'tnk',
    revision,
    nonce: intent.nonce,
    provider_signature: value.provider_signature,
    target_signature: intent.target_signature,
  });
  const restartedParticipantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const restartedWriterFeature = new MayhemFeature(writer.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  restartedParticipantFeature.key = 'mayhem';
  restartedWriterFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = restartedParticipantFeature;
  writer.peer.protocol.instance.features.mayhem = restartedWriterFeature;
  connect(participant.peer, restartedParticipantFeature, writer.peer, restartedWriterFeature);
  connect(writer.peer, restartedWriterFeature, participant.peer, restartedParticipantFeature);

  const redelivered = await submitMayhemFeature(
    participant.peer,
    { feature: 'mayhem', key, value }
  );
  assert.equal(redelivered.ok, true);
  assert.equal(redelivered.relayed, true);
  assert.equal(writer.appended.length, 1);

  const rejected = await restartedParticipantFeature.relay(key, {
    ...value,
    intent: {
      ...intent,
      rail: 'fiat',
      currency: 'usd',
      target: 'acct_unverified',
      target_wallet: null,
      target_signature: null,
    },
  });
  assert.equal(rejected.ok, false);
  assert.match(rejected.message, /failed relay verification/);
  assert.equal(writer.appended.length, 1);
});

test('fiat payout relay keeps the active account admissible during verified rotation', async () => {
  const participant = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  const participantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writer.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const activeTarget = 'acct_active';
  const futureTarget = 'acct_future';
  const contextRevision = '22'.repeat(32);
  const intent = {
    op: 'bind_provider_payout',
    network: 'mainnet',
    admin: adminKey,
    bootstrap: '11'.repeat(32),
    context_revision: contextRevision,
    provider: providerKey,
    rail: 'fiat',
    currency: 'usd',
    chain_id: null,
    target: activeTarget,
    target_wallet: null,
    target_signature: null,
    previous_revision: null,
    payment_config_version: 7,
    nonce: '77'.repeat(32),
    expires_after_epoch: 10,
  };
  const value = {
    op: 'bind_provider_payout',
    intent,
    provider_signature: fakeSignature(
      providerKey,
      b4a.from(providerPayoutBindingMessage(intent))
    ),
  };
  const revision = b4a.toString(
    await blake3(b4a.from(providerPayoutBindingMessage(intent))),
    'hex'
  );
  const key = `payout/binding/fiat/${providerKey}/${revision}`;
  writer.state.set('payments/current', {
    rails: ['fiat', 'tap', 'tnk'],
    fiat: { currencies: ['usd', 'eur'] },
    tap: { chain_id: 1 },
    tnk: { network: 'mainnet' },
    ver: 7,
    set_by: adminKey,
    set_by_role: 'admin',
  });
  writer.state.set('payout/context/current', {
    payment_config_version: 7,
    revision: contextRevision,
  });
  writer.state.set(`payout/context/7/${contextRevision}`, {
    revision: contextRevision,
    network: 'mainnet',
    admin: adminKey,
    bootstrap: '11'.repeat(32),
    payment_config_version: 7,
    published_by: adminKey,
    published_by_role: 'admin',
  });
  writer.state.set(`prov/${providerKey}`, {
    provider: providerKey,
    status: 'active',
    accepted_rails: ['fiat'],
  });
  const activeRevision = '88'.repeat(32);
  const activeRecordKey =
    `payout/stripe-verified/${providerKey}/${activeRevision}`;
  const activeRecord = {
    type: 'stripe_payout_verification',
    revision: activeRevision,
    provider: providerKey,
    target: activeTarget,
    currency: 'usd',
    processor_revision: '99'.repeat(32),
    context_revision: contextRevision,
    payment_config_version: 7,
    details_submitted: true,
    payouts_enabled: true,
    transfers_enabled: true,
    verified_by: adminKey,
    verified_by_role: 'admin',
  };
  writer.state.set(activeRecordKey, activeRecord);
  writer.state.set(
    `payout/stripe-verified/target/${providerKey}/${activeTarget}`,
    {
      provider: providerKey,
      target: activeTarget,
      revision: activeRevision,
      processor_revision: activeRecord.processor_revision,
      record_key: activeRecordKey,
    }
  );
  writer.state.set(`payout/stripe-verified/current/${providerKey}`, {
    provider: providerKey,
    target: futureTarget,
    revision: 'aa'.repeat(32),
    processor_revision: 'ab'.repeat(32),
    record_key: `payout/stripe-verified/${providerKey}/${'aa'.repeat(32)}`,
  });

  const accepted = await submitMayhemFeature(
    participant.peer,
    { feature: 'mayhem', key, value }
  );
  assert.equal(accepted.ok, true);
  assert.equal(accepted.relayed, true);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 1);
});

test('read-only provider relay verifies an exact TAP wallet co-signature', async () => {
  const participant = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  const participantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writer.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const targetPrivateKey = b4a.alloc(32, 7);
  const targetPublicKey = secp256k1.getPublicKey(targetPrivateKey, false);
  const target = `0x${b4a.toString(
    keccak256(targetPublicKey.subarray(1)).subarray(12),
    'hex'
  )}`;
  const intent = {
    op: 'bind_provider_payout',
    network: 'mainnet',
    admin: adminKey,
    bootstrap: '11'.repeat(32),
    context_revision: '22'.repeat(32),
    provider: providerKey,
    rail: 'tap',
    currency: null,
    chain_id: 1,
    target,
    target_wallet: null,
    target_signature: null,
    previous_revision: null,
    payment_config_version: 7,
    nonce: '66'.repeat(32),
    expires_after_epoch: 10,
  };
  intent.target_signature = signEthereumPersonalMessage(
    targetPrivateKey,
    providerPayoutTargetBindingMessage(intent)
  );
  const value = {
    op: 'bind_provider_payout',
    intent,
    provider_signature: fakeSignature(
      providerKey,
      b4a.from(providerPayoutBindingMessage(intent))
    ),
  };
  const revision = b4a.toString(
    await blake3(b4a.from(providerPayoutBindingMessage(intent))),
    'hex'
  );
  const key = `payout/binding/tap/${providerKey}/${revision}`;
  writer.state.set('payments/current', {
    rails: ['fiat', 'tap', 'tnk'],
    tap: { chain_id: 1 },
    tnk: { network: 'mainnet' },
    ver: 7,
    set_by: adminKey,
    set_by_role: 'admin',
  });
  writer.state.set('payout/context/current', {
    payment_config_version: 7,
    revision: intent.context_revision,
  });
  writer.state.set(`payout/context/7/${intent.context_revision}`, {
    revision: intent.context_revision,
    network: 'mainnet',
    admin: adminKey,
    bootstrap: '11'.repeat(32),
    payment_config_version: 7,
    published_by: adminKey,
    published_by_role: 'admin',
  });
  writer.state.set(`prov/${providerKey}`, {
    provider: providerKey,
    status: 'active',
    accepted_rails: ['tap'],
  });

  const accepted = await submitMayhemFeature(
    participant.peer,
    { feature: 'mayhem', key, value }
  );
  assert.equal(accepted.ok, true);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 1);

  const substituted = structuredClone(value);
  substituted.intent.chain_id = 61_000;
  substituted.provider_signature = fakeSignature(
    providerKey,
    b4a.from(providerPayoutBindingMessage(substituted.intent))
  );
  const substitutedRevision = b4a.toString(
    await blake3(b4a.from(providerPayoutBindingMessage(substituted.intent))),
    'hex'
  );
  const rejected = await participantFeature.relay(
    `payout/binding/tap/${providerKey}/${substitutedRevision}`,
    substituted
  );
  assert.equal(rejected.ok, false);
  assert.match(rejected.message, /failed relay verification/);
  assert.equal(writer.appended.length, 1);
});

test('non-admin read-only transport relays a valid admin-signed tx without gaining writer authority', async () => {
  const workstation = peerFor(otherKey);
  const writer = peerFor(adminKey, { writable: true });
  const workstationFeature = new MayhemFeature(workstation.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writer.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  workstationFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  workstation.peer.protocol.instance.features.mayhem = workstationFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(workstation.peer, workstationFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, workstation.peer, workstationFeature);

  const preparedCommand = {
    type: 'setParams',
    value: {
      zeta: 'preserve-first',
      op: 'set_params',
      submitted_at: 90_000,
      effective_at: 93_600,
      values: { fee_bps: 1_500 },
      alpha: 'preserve-last',
    },
  };
  const value = await signedAdminTxValue(preparedCommand);
  const tx = value.tx;
  const key = `admin/contract-tx/${tx}`;
  const server = createServer(workstation.peer);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  let first;
  try {
    const response = await fetch(`http://127.0.0.1:${address.port}/v1/contract/feature`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ feature: 'mayhem', key, value }),
    });
    assert.equal(response.status, 200);
    first = await response.json();
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
  const replay = await submitMayhemFeature(workstation.peer, {
    feature: 'mayhem',
    key,
    value,
  });

  assert.equal(first.ok, true);
  assert.equal(first.status, 'applied');
  assert.equal(first.relayed, true);
  assert.deepEqual(replay.result, first.result);
  assert.equal(
    JSON.stringify(writer.appended[0].value.dispatch.value.prepared_command),
    JSON.stringify(value.prepared_command),
    'prepared admin command key order must survive the relay unchanged'
  );
  assert.equal(workstation.appended.length, 0);
  assert.equal(writer.appended.length, 1);
  assert.equal(workstation.peer.base.writable, false);

  const forgedValue = await signedAdminTxValue(
    preparedCommand,
    { nonce: 'de'.repeat(32) }
  );
  const forged = await submitMayhemFeature(workstation.peer, {
    feature: 'mayhem',
    key: `admin/contract-tx/${forgedValue.tx}`,
    value: {
      ...forgedValue,
      signature: fakeSignature(otherKey, b4a.from(forgedValue.tx, 'hex')),
    },
  });
  assert.equal(forged.ok, false);
  assert.match(forged.message, /invalid admin contract transaction signature/i);
  assert.equal(writer.appended.length, 1);
  assert.equal(workstation.peer.base.writable, false);

  const wrongNetwork = await signedAdminTxValue(preparedCommand, {
    nonce: 'df'.repeat(32),
    context: {
      ...adminTxContext,
      subnet_bootstrap: '99'.repeat(32),
    },
  });
  const crossStoreReplay = await submitMayhemFeature(workstation.peer, {
    feature: 'mayhem',
    key: `admin/contract-tx/${wrongNetwork.tx}`,
    value: wrongNetwork,
  });
  assert.equal(crossStoreReplay.ok, false);
  assert.match(crossStoreReplay.message, /context does not match/i);
  assert.equal(writer.appended.length, 1);
  assert.equal(workstation.peer.base.writable, false);
});

test('read-only user relays a dual-signed TAP account binding to the admin appender', async () => {
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

  const key = `tap_account/${providerKey}/${'44'.repeat(32)}`;
  const value = {
    op: 'tap_account_bind',
    user: providerKey,
    ethereum_address: `0x${'55'.repeat(20)}`,
    chain_id: 1,
    pool_address: `0x${'66'.repeat(20)}`,
    user_sig: '77'.repeat(64),
    ethereum_sig: `0x${'88'.repeat(65)}`,
  };
  const result = await submitMayhemFeature(participant.peer, { feature: 'mayhem', key, value });

  assert.equal(result.ok, true);
  assert.equal(result.relayed, true);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 1);
  assert.equal(writer.appended[0].value.dispatch.address, adminKey);
  assert.deepEqual(writer.appended[0].value.dispatch.value, value);
});

test('read-only transport relays a wallet-signed Stripe checkout without appending', async () => {
  const participant = peerFor(otherKey);
  const signer = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  let serviceCalls = 0;
  const participantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writer.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
    async serviceHandler(service, value) {
      serviceCalls += 1;
      assert.equal(service, 'stripe_checkout');
      assert.deepEqual(value, stripeCheckoutValue());
      return {
        ok: true,
        rail: 'fiat',
        processor_rail: 'stripe',
        checkout_session: {
          id: 'cs_live_test',
          url: 'https://checkout.stripe.com/c/pay/cs_live_test',
        },
      };
    },
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const checkout = stripeCheckoutValue();
  const result = await requestStripeCheckout(
    participant.peer,
    signedServiceValue(signer.peer, 'stripe_checkout', checkout, otherKey)
  );

  assert.equal(result.ok, true);
  assert.equal(result.relayed, true);
  assert.equal(result.checkout_session.url, 'https://checkout.stripe.com/c/pay/cs_live_test');
  assert.equal(serviceCalls, 1);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 0);
  assert.equal(writer.flushes.length, 0);
});

test('read-only transport relays wallet-signed Stripe Connect onboarding without appending', async () => {
  const participant = peerFor(otherKey);
  const signer = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  let serviceCalls = 0;
  const participantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writer.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
    async serviceHandler(service, value) {
      serviceCalls += 1;
      assert.equal(service, 'stripe_connect_onboard');
      assert.deepEqual(value, stripeConnectValue());
      return {
        ok: true,
        rail: 'fiat',
        processor_rail: 'stripe',
        provider: providerKey,
        account: { id: 'acct_test_provider', ready: false },
        onboarding: { url: 'https://connect.stripe.com/setup/test' },
      };
    },
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const result = await requestStripeConnect(
    participant.peer,
    'stripe_connect_onboard',
    signedServiceValue(signer.peer, 'stripe_connect_onboard', stripeConnectValue(), otherKey)
  );

  assert.equal(result.ok, true);
  assert.equal(result.relayed, true);
  assert.equal(result.onboarding.url, 'https://connect.stripe.com/setup/test');
  assert.equal(serviceCalls, 1);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 0);
  assert.equal(writer.flushes.length, 0);
});

test('read-only transport relays dual-provider-signed Stripe Connect relink consent once', async () => {
  const participant = peerFor(otherKey);
  const signer = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  const relinkValue = stripeConnectRelinkValue();
  const relinkRequest = signedServiceValue(
    signer.peer,
    'stripe_connect_relink',
    relinkValue,
    otherKey
  );
  let serviceCalls = 0;
  const participantFeature = new MayhemFeature(participant.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
  });
  const writerFeature = new MayhemFeature(writer.peer, {
    timeoutMs: 1_000,
    retryMs: 100,
    async serviceHandler(service, value, authorization) {
      serviceCalls += 1;
      assert.equal(service, 'stripe_connect_relink');
      assert.deepEqual(value, relinkValue);
      assert.equal(authorization.actor, providerKey);
      assert.equal(authorization.transport, otherKey);
      assert.equal(authorization.signature, relinkRequest.signature);
      return {
        ok: true,
        rail: 'fiat',
        processor_rail: 'stripe',
        provider: providerKey,
        source_provider: otherKey,
        status: 'consent_required',
        account: null,
        onboarding: { url: 'https://connect.stripe.com/oauth/authorize?state=test' },
      };
    },
  });
  participantFeature.key = 'mayhem';
  writerFeature.key = 'mayhem';
  participant.peer.protocol.instance.features.mayhem = participantFeature;
  writer.peer.protocol.instance.features.mayhem = writerFeature;
  connect(participant.peer, participantFeature, writer.peer, writerFeature);
  connect(writer.peer, writerFeature, participant.peer, participantFeature);

  const result = await requestStripeConnect(
    participant.peer,
    'stripe_connect_relink',
    relinkRequest
  );
  const replay = await requestStripeConnect(
    participant.peer,
    'stripe_connect_relink',
    relinkRequest
  );

  assert.equal(result.ok, true);
  assert.deepEqual(replay, result);
  assert.equal(result.relayed, true);
  assert.equal(result.status, 'consent_required');
  assert.match(result.onboarding.url, /^https:\/\/connect\.stripe\.com\//);
  assert.equal(serviceCalls, 1);
  assert.equal(participant.appended.length, 0);
  assert.equal(writer.appended.length, 0);
});

test('Stripe relink rejects stale, source-forged, target-forged, and substituted consent locally', async () => {
  const participant = peerFor(otherKey);
  const target = peerFor(providerKey);
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

  const valid = stripeConnectRelinkValue();
  const forged = {
    ...valid,
    source_consent_signature: fakeSignature(providerKey, stripeConnectRelinkConsentMessage(valid)),
  };
  const substituted = {
    ...valid,
    account_id: 'acct_substituted_longer',
  };
  const stale = stripeConnectRelinkValue(providerKey, {
    consent_expires_at: Math.floor(Date.now() / 1_000) - 1,
  });

  for (const payload of [forged, substituted, stale]) {
    await assert.rejects(
      feature.requestService(
        'stripe_connect_relink',
        signedServiceValue(target.peer, 'stripe_connect_relink', payload, otherKey)
      ),
      /Invalid Mayhem service request signature/
    );
  }
  await assert.rejects(
    feature.requestService(
      'stripe_connect_relink',
      signedServiceValue(participant.peer, 'stripe_connect_relink', valid, otherKey)
    ),
    /Invalid Mayhem service request signature/
  );
  assert.equal(broadcasts, 0);
});

test('Stripe Connect service relay rejects a forged provider signature locally', async () => {
  const participant = peerFor(providerKey);
  const participantFeature = new MayhemFeature(participant.peer, {});
  participantFeature.key = 'mayhem';
  let broadcasts = 0;
  participant.peer.sidechannel = {
    started: true,
    broadcast() {
      broadcasts += 1;
      return true;
    },
  };

  await assert.rejects(
    participantFeature.requestService(
      'stripe_connect_onboard',
      signedServiceValue(participant.peer, 'stripe_connect_onboard', stripeConnectValue(otherKey))
    ),
    /Invalid Mayhem service request signature/
  );
  assert.equal(broadcasts, 0);
});

test('Stripe service relay rejects a signed request replayed through another transport', async () => {
  const signer = peerFor(providerKey);
  const attacker = peerFor(otherKey);
  const attackerFeature = new MayhemFeature(attacker.peer, {});
  attackerFeature.key = 'mayhem';
  let broadcasts = 0;
  attacker.peer.sidechannel = {
    started: true,
    broadcast: () => {
      broadcasts += 1;
      return true;
    },
  };

  const value = signedServiceValue(
    signer.peer,
    'stripe_connect_onboard',
    stripeConnectValue(),
    providerKey
  );
  await assert.rejects(
    attackerFeature.requestService('stripe_connect_onboard', value),
    /Invalid Mayhem service request signature/
  );
  assert.equal(broadcasts, 0);
});

test('Stripe service relay binds the request identity and deduplicates retries', async () => {
  const participant = peerFor(providerKey);
  const participantFeature = new MayhemFeature(participant.peer, {});
  participantFeature.key = 'mayhem';
  let broadcasts = 0;
  participant.peer.sidechannel = {
    started: true,
    broadcast() {
      broadcasts += 1;
      return true;
    },
  };
  await assert.rejects(
    participantFeature.requestService(
      'stripe_checkout',
      signedServiceValue(participant.peer, 'stripe_checkout', stripeCheckoutValue(otherKey))
    ),
    /Invalid Mayhem service request signature/
  );
  assert.equal(broadcasts, 0);

  const writer = peerFor(adminKey, { writable: true });
  let serviceCalls = 0;
  const writerFeature = new MayhemFeature(writer.peer, {
    async serviceHandler() {
      serviceCalls += 1;
      return { ok: true, checkout_session: { url: 'https://checkout.stripe.com/c/pay/test' } };
    },
  });
  writerFeature.key = 'mayhem';
  writer.peer.sidechannel = {
    started: true,
    verifyPayload: () => true,
    broadcast: () => true,
  };
  const value = signedServiceValue(
    participant.peer,
    'stripe_checkout',
    stripeCheckoutValue(),
    otherKey
  );
  const requestId = serviceRequestIdFor('stripe_checkout', value);
  const payload = {
    from: otherKey,
    sig: `signed:${otherKey}`,
    message: {
      control: 'mayhem_service_request',
      version: 1,
      request_id: requestId,
      service: 'stripe_checkout',
      value,
    },
  };

  await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload);
  await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload);
  assert.equal(serviceCalls, 1);
  assert.equal(writer.appended.length, 0);
});

test('poisoned relay service request is rejected locally and a retry still runs', async () => {
  const signer = peerFor(providerKey);
  const writer = peerFor(adminKey, { writable: true });
  let serviceCalls = 0;
  const writerFeature = new MayhemFeature(writer.peer, {
    async serviceHandler() {
      serviceCalls += 1;
      if (serviceCalls === 1) throw new Error('injected relay poison');
      return { ok: true, checkout_session: { url: 'https://checkout.stripe.com/c/pay/retry' } };
    },
  });
  writerFeature.key = 'mayhem';
  const responses = [];
  writer.peer.sidechannel = {
    started: true,
    verifyPayload: () => true,
    broadcast: (_channel, message) => {
      responses.push(message.response);
      return true;
    },
  };
  const value = signedServiceValue(signer.peer, 'stripe_checkout', stripeCheckoutValue());
  const requestId = serviceRequestIdFor('stripe_checkout', value);
  const payload = {
    from: providerKey,
    sig: `signed:${providerKey}`,
    message: {
      control: 'mayhem_service_request',
      version: 1,
      request_id: requestId,
      service: 'stripe_checkout',
      value,
    },
  };

  await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload);
  await writerFeature.handleSidechannelMessage(MAYHEM_RELAY_CHANNEL, payload);

  assert.equal(serviceCalls, 2);
  assert.equal(responses[0].ok, false);
  assert.match(responses[0].message, /injected relay poison/);
  assert.equal(responses[1].ok, true);
});

test('admin-writer RPC keeps the local append path', async () => {
  const writer = peerFor(adminKey, { writable: true });
  let submitCalls = 0;
  writer.peer.protocol.instance.features.mayhem = {
    async submit() {
      submitCalls += 1;
      return { status: 'applied', ok: true, result: { ok: true } };
    },
  };

  const result = await submitMayhemFeature(writer.peer, {
    feature: 'mayhem',
    key: 'admin/local',
    value: { op: 'admin_only' },
  });

  assert.equal(result.ok, true);
  assert.equal(result.relayed, undefined);
  assert.equal(submitCalls, 1);
  assert.equal(writer.flushes.length, 0);
});

test('admin writer diagnostics expose transition state without key material', () => {
  const writer = peerFor(adminKey, { writable: true });
  writer.peer.base.opened = true;
  writer.peer.base.isIndexer = true;
  writer.peer.base.signedLength = 12;
  writer.peer.base.length = 13;
  writer.peer.base.local = { length: 4 };
  writer.peer.base._caughtup = true;
  writer.peer.base._appending = [{ op: 'pending' }];
  writer.peer.base._appended = 3;
  writer.peer.base.localWriter = {
    isRemoved: false,
    isActiveIndexer: true,
    length: 4,
    available: 4,
    seenLength: 4,
    idle: () => true,
    flushed: () => true,
    core: {
      length: 4,
      writable: true,
      opened: true,
      core: { upgrading: false },
    },
  };
  writer.peer.base._applyState = {
    opened: true,
    applying: false,
    indexedLength: 12,
    isLocalIndexer: () => true,
    isLocalPendingIndexer: () => false,
    system: {
      indexers: [{ length: 4 }],
      pendingIndexers: [],
      indexerUpdate: false,
    },
    view: {
      core: {
        writable: true,
        opened: true,
        length: 10,
        contiguousLength: 10,
        signedLength: 9,
        core: { upgrading: false },
      },
    },
    views: [{
      name: 'view',
      mappedIndex: 0,
      length: 9,
      core: {
        writable: true,
        opened: true,
        length: 10,
        contiguousLength: 10,
        signedLength: 9,
        core: { upgrading: false },
      },
    }],
  };
  writer.peer.base.view = {
    core: {
      writable: false,
      opened: true,
      closing: false,
      length: 9,
      contiguousLength: 8,
      fork: 0,
      signedLength: 9,
      core: { upgrading: true },
    },
  };
  writer.peer.contract = { instance: { _mayhemApplyStage: 'rate:key:hash' } };

  const report = adminWriterDiagnostics(writer.peer);
  assert.equal(report.contract.apply_stage, 'rate:key:hash');
  assert.equal(report.base.writable, true);
  assert.equal(report.base.appending_count, 1);
  assert.equal(report.view.writable, false);
  assert.equal(report.view.contiguous_length, 8);
  assert.equal(report.view.upgrading, true);
  assert.equal(report.apply_state.view.writable, true);
  assert.equal(report.apply_state.view.contiguous_length, 10);
  assert.equal(report.apply_state.views[0].core_length, 10);
  assert.equal(report.local_writer.active_indexer, true);
  assert.deepEqual(report.apply_state.indexer_lengths, [4]);
  assert.doesNotMatch(JSON.stringify(report), new RegExp(adminKey));
});

test('Mayhem-owned RPC adapters expose canonical status and bounded prefix reads', async () => {
  const entries = [
    { key: 'price/a', value: { current: 1 } },
    { key: 'price/b', value: { current: 2 } },
  ];
  const peer = {
    wallet: { publicKey: adminKey },
    writerLocalKey: otherKey,
    config: {},
    base: {
      writable: true,
      isIndexer: true,
      key: b4a.from('11'.repeat(32), 'hex'),
      view: {
        core: { signedLength: 12, length: 13 },
        async get(key) {
          if (key === 'admin') return { value: adminKey };
          if (key === 'chat_status') return { value: 'enabled' };
          return null;
        },
        createReadStream({ gte, lt, limit }) {
          assert.equal(gte, 'price/');
          assert.equal(lt, 'price/\xff');
          assert.equal(limit, 2);
          return (async function* () {
            yield* entries;
          })();
        },
      },
    },
    msbClient: {
      bootstrapHex: '22'.repeat(32),
      networkId: 918,
      pubKeyHexToAddress: () => 'trac1admin',
      getSignedLength: () => 99,
      getConnectedValidatorsCount: () => 2,
    },
  };
  const status = await getMayhemStatus(peer, {
    subnetBootstrapHex: '33'.repeat(32),
    subnetChannelUtf8: '0000mayhem',
    peerDhtBootstrap: ['peer.example:1'],
    msbChannel: '0000msb',
    msbDhtBootstrap: ['msb.example:2'],
  });
  assert.equal(status.peer.subnetBootstrapHex, '33'.repeat(32));
  assert.equal(status.peer.subnetChannelUtf8, '0000mayhem');
  assert.equal(status.peer.admin, adminKey);
  assert.equal(status.msb.connectedValidators, 2);
  assert.deepEqual(status.msb.dhtBootstrap, ['msb.example:2']);

  assert.deepEqual(
    await getStatePrefix(peer, 'price/', { confirmed: false, limit: 2 }),
    {
      prefix: 'price/',
      confirmed: false,
      values: entries,
    }
  );
  await assert.rejects(
    getStatePrefix(peer, 'price/', { confirmed: false, limit: 1001 }),
    /integer from 1 to 1000/
  );
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
