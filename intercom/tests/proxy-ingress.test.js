import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import { Protocol } from 'trac-peer';
import MayhemProtocol, { mayhemFeatureParticipant } from '../contract/protocol.js';
import { assertProxyPublicationNotPaid, proxyRegistryFeatureKey } from '../contract/proxy-protocol.js';
import { proxyPolicyFeatureKey } from '../contract/proxy-policy.js';
import MayhemFeature, { participantFor } from '../features/mayhem/index.js';
import { submitMayhemFeature } from '../src/rpc.js';
import { MemoryStorage, executeFeature } from './helpers/contract.js';
import { proxyContractFixture } from './helpers/proxy.js';

const clone = value => JSON.parse(JSON.stringify(value));

for (const op of ['proxy_registry', 'proxy_policy']) {
  test(`no paid/prepared/aliased ${op} operation reaches MSB transport`, async t => {
    let sent = 0;
    t.mock.method(Protocol.prototype, 'broadcastTransaction', async () => { sent++; });
    const protocol = new MayhemProtocol({}, {}, {});
    const value = { op };
    const forms = [value, { type: op, value: {} }, { type: 'mayhem_feature', value },
      { type: 'registerProvider', value }, { type: 'tx', value: { dispatch: { type: 'mayhem_feature', value } } },
      { op: 'admin_contract_tx', prepared_command: { type: 'mayhem_feature', value } }];
    for (const command of forms) {
      assert.throws(() => protocol.mapTxCommand(JSON.stringify(command)), /admitted feature/);
      await assert.rejects(protocol.preparePaidTransaction(command), /admitted feature/);
      await assert.rejects(protocol.broadcastTransaction(command), /admitted feature/);
      await assert.rejects(protocol.broadcastPreparedTransaction({ dispatch: command, surrogate: {} }), /admitted feature/);
    }
    assert.equal(sent, 0);
  });
}

test('native paid dispatch and persisted historical bytes are unchanged', async t => {
  const sent = [];
  t.mock.method(Protocol.prototype, 'broadcastTransaction', async (...args) => { sent.push(args); return 'native'; });
  const protocol = new MayhemProtocol({}, {}, {});
  const native = { type: 'setRules', value: { op: 'set_rules', rules: {} } };
  assert.equal(await protocol.broadcastTransaction(native), 'native');
  assert.deepEqual(sent[0][0], protocol.versionedTransactionObject(native));
  const historical = { ...native, value: { ...native.value, contract_version: 27 } };
  const surrogate = { tx: 'persisted-original-transaction' };
  assert.equal(await protocol.broadcastPreparedTransaction({ dispatch: historical, surrogate }), 'native');
  assert.strictEqual(sent[1][0], historical);
  assert.strictEqual(sent[1][2], surrogate);
  assert.doesNotThrow(() => assertProxyPublicationNotPaid({ type: 'setRules',
    value: { op: 'set_rules', examples: [{ op: 'proxy_registry' }] } }));
});

async function fixture({ snapshot = true, participant = false } = {}) {
  const f = await proxyContractFixture();
  let appended = 0;
  let forwarded = 0;
  let checks = 0;
  let invalidateAt = 0;
  const wallet = participant ? f.provider.wallet : f.admin.wallet;
  const publicKey = participant ? f.provider.publicKey : f.admin.publicKey;
  const peer = { ...f.peer, wallet: { ...f.peer.wallet, publicKey,
    sign: bytes => wallet.sign(b4a.from(bytes)).toString('hex') },
    base: { writable: !participant, view: f.storage,
      append: async operation => {
        appended++;
        const dispatch = operation.value.dispatch;
        const result = await executeFeature(f.contract, f.storage, 'mayhem_feature', dispatch.key, dispatch.value, f.admin.publicKey);
        await f.storage.put(`fr/${dispatch.hash}`, result instanceof Error
          ? { ok: false, status: 'rejected', error: { message: result.message } }
          : { ok: true, status: 'applied', result });
      } },
    protocol: { instance: { generateNonce: () => 'a'.repeat(64), featMaxBytes: () => 64000, features: {} } },
    sidechannel: { started: true, broadcast: () => { forwarded++; } },
  };
  const config = { resultTimeoutMs: 20,
    withProxyCanonicalSnapshot: snapshot ? async body => {
      const pinned = MemoryStorage.fromSnapshotBytes(f.storage.snapshotBytes());
      return await body({ context: clone(f.context),
        proof: { view_key: '1'.repeat(64), tree_hash: '2'.repeat(64), fork: 0, signed_length: 10 },
        read: async key => (await pinned.get(key))?.value ?? null,
        assertCurrent: async () => {
          checks++;
          if (invalidateAt && checks >= invalidateAt) throw new Error('canonical snapshot changed');
        } });
    } : null };
  const feature = new MayhemFeature(peer, config);
  feature.key = 'mayhem';
  peer.protocol.instance.features.mayhem = feature;
  if (participant) {
    const writer = new MayhemFeature({ ...peer,
      wallet: { ...f.peer.wallet, publicKey: f.admin.publicKey },
      base: { ...peer.base, writable: true } }, config);
    feature.requestService = async (service, envelope) => {
      const authorization = writer._verifyServiceRequest(service, envelope, { admin: f.admin.publicKey, transport: publicKey });
      assert.ok(authorization, 'preflight request must be authenticated');
      return await writer._handleService(service, authorization.payload, authorization);
    };
  }
  return { ...f, peer, feature, get appended() { return appended; }, get forwarded() { return forwarded; },
    expireAfterFirstCheck: () => { invalidateAt = checks + 2; } };
}

for (const ingress of ['submit', 'record', 'append', 'rpc', 'relayed']) {
  test(`unpaid registration through ${ingress} causes zero ledger writes`, async () => {
    const f = await fixture();
    const value = await f.create();
    value.admission = null;
    const key = await proxyRegistryFeatureKey(value);
    const before = f.storage.snapshotBytes();
    const run = ingress === 'rpc'
      ? () => submitMayhemFeature(f.peer, { key, value })
      : ingress === 'relayed'
        ? () => f.feature._applyRelayed(key, value, 'transport-test')
        : () => f.feature[ingress](key, value);
    await assert.rejects(run, /admission/);
    assert.equal(f.appended, 0);
    assert.equal(f.storage.snapshotBytes(), before);
  });
}

test('writer refuses missing canonical configuration, stale snapshots and forged signatures before append', async () => {
  for (const kind of ['unconfigured', 'stale', 'forged']) {
    const f = await fixture({ snapshot: kind !== 'unconfigured' });
    const value = await f.create();
    if (kind === 'stale') f.expireAfterFirstCheck();
    if (kind === 'forged') value.provider_signature = '0'.repeat(128);
    await assert.rejects(f.feature.submit(await proxyRegistryFeatureKey(value), value));
    assert.equal(f.appended, 0, kind);
  }
});

test('an admitted real signed registration applies once and exact retries do not append', async () => {
  const f = await fixture();
  const value = await f.create();
  const key = await proxyRegistryFeatureKey(value);
  assert.equal((await submitMayhemFeature(f.peer, { key, value })).ok, true);
  for (const method of ['submit', 'record', 'append']) {
    const reply = await f.feature[method](key, clone(value));
    assert.equal(reply.ok, true);
    assert.equal(reply.duplicate, true);
    assert.equal(reply.accepted, false);
  }
  assert.equal(f.appended, 1);
  assert.deepEqual(await f.read('payout/epoch/542'), { status: 'prepared', native: true });
});

test('participant refuses unpaid/unconfigured/stale operations before relay forwarding', async () => {
  for (const kind of ['unpaid', 'unconfigured', 'stale']) {
    const f = await fixture({ participant: true, snapshot: kind !== 'unconfigured' });
    const value = await f.create();
    if (kind === 'unpaid') value.admission = null;
    if (kind === 'stale') f.expireAfterFirstCheck();
    assert.equal(participantFor(value), f.provider.publicKey);
    assert.equal(mayhemFeatureParticipant(value), f.provider.publicKey);
    await assert.rejects(f.feature.relay(await proxyRegistryFeatureKey(value), value));
    assert.equal(f.forwarded, 0, kind);
    assert.equal(f.appended, 0, kind);
  }
});

test('signed proxy policy cannot bypass admission via an admin transaction wrapper', async () => {
  const f = await fixture();
  const wrapped = { op: 'admin_contract_tx', prepared_command: { type: 'mayhem_feature', value: { op: 'proxy_policy' } } };
  const result = await f.feature.submit('admin/contract-tx/not-admitted', wrapped);
  assert.equal(result.accepted, false);
  assert.match(result.message, /admitted feature/);
  assert.equal(f.appended, 0);
});

test('proxy policy rejects foreign context and stale revision before append', async () => {
  const f = await fixture();
  const head = await f.read('proxy/v1/policy-head');
  for (const kind of ['context', 'revision']) {
    const value = { op: 'proxy_policy', context: { ...f.network }, revision: head.revision + 1,
      action: { kind: 'set_family', family_id: 'future', label: 'Future', enabled: true } };
    if (kind === 'context') value.context.msb_bootstrap = '0'.repeat(64);
    if (kind === 'revision') value.revision++;
    await assert.rejects(f.feature.submit(await proxyPolicyFeatureKey(value), value));
    assert.equal(f.appended, 0, kind);
  }
});

test('admin taxonomy policy applies through the gate and its exact retry does not append', async () => {
  const f = await fixture();
  const head = await f.read('proxy/v1/policy-head');
  const value = { op: 'proxy_policy', context: { ...f.network }, revision: head.revision + 1,
    action: { kind: 'set_family', family_id: 'new_future_family', label: 'New family', enabled: true } };
  const key = await proxyPolicyFeatureKey(value);
  assert.equal((await f.feature.submit(key, value)).ok, true);
  assert.equal((await f.feature.submit(key, clone(value))).duplicate, true);
  assert.deepEqual(await f.read('proxy/v1/family/new_future_family'), { enabled: true, label: 'New family' });
  assert.equal(f.appended, 1);
});

test('provider forwarding uses the validated signed operation and never a write plan', async () => {
  const f = await fixture({ participant: true });
  const value = await f.create();
  const key = await proxyRegistryFeatureKey(value);
  const forwarded = [];
  f.feature._relayUntilAcknowledged = async request => { forwarded.push(request); return { ok: true }; };
  assert.equal((await f.feature.relay(key, value)).ok, true);
  assert.equal(forwarded.length, 1);
  assert.deepEqual(forwarded[0].message.value, value);
  assert.deepEqual(Object.keys(forwarded[0].message.value).sort(), ['admission', 'intent', 'op', 'provider_signature']);
  assert.equal(f.appended, 0);
});

test('provider rejects mismatched challenge, operation, network and view evidence before forwarding', async () => {
  for (const change of [
    reply => { reply.request_nonce = '0'.repeat(64); },
    reply => { reply.feature_key = 'another-operation'; },
    reply => { reply.context.msb_bootstrap = '0'.repeat(64); },
    reply => { reply.proof.signed_length = 0; },
    reply => { reply.proof.tree_hash = null; },
  ]) {
    const f = await fixture({ participant: true });
    const request = f.feature.requestService.bind(f.feature);
    f.feature.requestService = async (...args) => { const reply = await request(...args); change(reply); return reply; };
    const envelope = await f.create();
    await assert.rejects(f.feature.relay(await proxyRegistryFeatureKey(envelope), envelope), /does not match/);
    assert.equal(f.forwarded, 0);
  }
});

test('slow preflight cannot authorize a later publication or impose an inference timeout', async t => {
  const f = await fixture({ participant: true });
  let now = 1000;
  t.mock.method(Date, 'now', () => now);
  const request = f.feature.requestService.bind(f.feature);
  f.feature.requestService = async (...args) => { const reply = await request(...args); now += 15001; return reply; };
  const envelope = await f.create();
  await assert.rejects(f.feature.relay(await proxyRegistryFeatureKey(envelope), envelope), /preflight expired/);
  assert.equal(f.forwarded, 0);
});
