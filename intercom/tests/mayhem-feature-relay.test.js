import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import sodium from 'sodium-universal';
import MayhemFeature, {
  MAYHEM_RELAY_CHANNEL,
  MAYHEM_RELAY_MAX_MESSAGE_BYTES,
  requestIdFor,
  serviceRequestIdFor,
  serviceSigningMessage,
} from '../features/mayhem/index.js';
import {
  adminWriterDiagnostics,
  createServer,
  requestStripeCheckout,
  requestStripeConnect,
  submitMayhemFeature,
} from '../src/rpc.js';

const adminKey = 'aa'.repeat(32);
const providerKey = 'bb'.repeat(32);
const otherKey = 'cc'.repeat(32);

const fakeSignature = (publicKey, message) => {
  const suffix = String(message).length.toString(16).padStart(4, '0');
  return `${publicKey}${suffix}`.padEnd(128, '0').slice(0, 128);
};

const viewFor = (state) => ({
  async get(key) {
    return state.has(key) ? { value: state.get(key) } : null;
  },
});

const peerFor = (publicKey, { writable = false } = {}) => {
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

test('16KB relay cap fits a maximal canonical spend-reserve envelope', () => {
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
    op: 'spend_reserve',
    contract_version: 11,
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
  const key = `spend/reserve/${voucher.session_id}`;
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
