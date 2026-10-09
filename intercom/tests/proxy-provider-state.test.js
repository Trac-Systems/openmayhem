import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import b4a from 'b4a';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import PeerWallet from 'trac-wallet';
import { CONTRACT_VERSION } from '../contract/contract.js';
import MayhemFeature from '../features/mayhem/index.js';
import { createProxyCanonicalSnapshot } from '../features/mayhem/proxy-canonical-view.js';
import { readProxyProviderState, validateProxyProviderStateRequest, PROXY_PROVIDER_STATE_SERVICE } from '../features/mayhem/proxy-provider-state.js';
import { proxyOperationDigest } from '../contract/proxy-protocol.js';
import { proxyContractFixture } from './helpers/proxy.js';
import { createServer, requestProxyProviderState } from '../src/rpc.js';

const h = n => n.toString(16).padStart(64, '0');
const hex = value => b4a.toString(value, 'hex');
async function fixture(t, admitted = true) {
  const f = await proxyContractFixture();
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'proxy-provider-state-'));
  const store = new Corestore(directory);
  const base = new Autobase(store, null, { ackInterval: 0, valueEncoding: 'json',
    open: views => new Hyperbee(views.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
    apply: async (nodes, view) => {
      const batch = view.batch();
      try { for (const node of nodes) for (const [key, value] of node.value.entries) {
        if (value === null) await batch.del(key); else await batch.put(key, value);
      } await batch.flush(); } finally { await batch.close(); }
    } });
  await base.ready();
  t.after(async () => { await base.close(); await store.close(); fs.rmSync(directory, { recursive: true, force: true }); });
  for (const value of [f.network, f.context, f.config]) value.subnet_bootstrap = hex(base.key);
  f.peer.config.bootstrap = hex(base.key); f.peer.base = base;
  await f.storage.put('proxy/v1/config', f.config);
  const envelope = await f.create();
  if (admitted) assert.equal((await f.submit(envelope)).ok, true);
  await base.append({ entries: [...f.storage.values] });
  await base.update();
  f.peer.wallet = { publicKey: f.admin.publicKey, sign: bytes => hex(f.admin.wallet.sign(b4a.from(bytes))),
    verify: (signature, bytes, key) => PeerWallet.verify(b4a.from(signature, 'hex'), b4a.from(bytes), b4a.from(key, 'hex')) };
  const snapshot = createProxyCanonicalSnapshot(f.peer, CONTRACT_VERSION);
  const request = { provider_pubkey: f.provider.publicKey, requester: f.provider.publicKey,
    initial_operation_digest: await proxyOperationDigest(envelope.intent), request_nonce: h(11) };
  const write = async entries => { await base.append({ entries }); await base.update(); };
  return { ...f, base, snapshot, request, write };
}

test('reads real signed canonical provider facts with bounded exact keys and no writes/private fee evidence', async t => {
  const f = await fixture(t), keys = [], before = f.base.local.length;
  const response = await readProxyProviderState({ request: f.request, withCanonicalSnapshot: body => f.snapshot(s => body({ ...s,
    read: key => { keys.push(key); return s.read(key); } })) });
  assert.equal(response.provider.sequence, 1);
  assert.equal(response.provider.operation_digest, f.request.initial_operation_digest);
  assert.equal(response.fee_policy_hash, f.config.fee_policy_hash);
  assert.equal(response.provider_revoked, false); assert.equal(response.admission_revoked, false);
  assert.equal(keys.length, 5); assert.equal(new Set(keys).size, 5);
  assert.ok(keys.every(key => key.startsWith('proxy/v1/')));
  for (const privateField of ['accepted_amount', 'invoice_commitment', 'evidence_commitment', 'issuer_pubkey', 'private_key', 'payment_required']) {
    assert.ok(!JSON.stringify(response).includes(privateField), privateField);
  }
  assert.equal(f.base.local.length, before);
});

test('canonical absence, disabled policy and revocation remain distinct without claiming unpaid', async t => {
  const f = await fixture(t, false);
  let response = await readProxyProviderState({ request: f.request, withCanonicalSnapshot: f.snapshot });
  assert.equal(response.provider, null); assert.equal(response.registry_enabled, true);
  assert.equal(response.admission_revoked, false); assert.equal(response.payment_required, undefined);
  await f.write([['proxy/v1/config', { ...f.config, enabled: false }], [`proxy/v1/provider-revoked/${f.provider.publicKey}`, { revision: 1, reason_hash: h(33) }]]);
  response = await readProxyProviderState({ request: f.request, withCanonicalSnapshot: f.snapshot });
  assert.equal(response.provider, null); assert.equal(response.registry_enabled, false); assert.equal(response.provider_revoked, true);
});

test('refuses foreign requests, broken entitlement ownership, missing policy and canonical drift', async t => {
  const f = await fixture(t);
  for (const change of [{ requester: h(88) }, { initial_operation_digest: 'bad' }, { invoice: h(1) }, { request_nonce: null }]) {
    assert.throws(() => validateProxyProviderStateRequest({ ...f.request, ...change }));
  }
  const initial = await readProxyProviderState({ request: f.request, withCanonicalSnapshot: f.snapshot });
  const id = initial.provider.entitlement_id;
  await f.write([[`proxy/v1/admission-revoked/${id}`, { revision: 1, reason_hash: h(3) }]]);
  assert.equal((await readProxyProviderState({ request: f.request, withCanonicalSnapshot: f.snapshot })).admission_revoked, true);
  await assert.rejects(readProxyProviderState({ request: f.request, withCanonicalSnapshot: body => f.snapshot(async snapshot => {
    let checks = 0;
    return body({ ...snapshot, assertCurrent: async () => {
      if (++checks === 2) await f.write([['proxy/v1/config', { ...f.config, enabled: false }]]);
      await snapshot.assertCurrent();
    } });
  }) }), /registry state changed/);
  await f.write([[`proxy/v1/admission-used/entitlement/${id}`, { provider_pubkey: h(55), entitlement_id: id }]]);
  await assert.rejects(readProxyProviderState({ request: f.request, withCanonicalSnapshot: f.snapshot }), /ownership differs/);
  await f.write([['proxy/v1/config', null]]);
  await assert.rejects(readProxyProviderState({ request: f.request, withCanonicalSnapshot: f.snapshot }), /not configured/);
});

test('uses four independent bounded read permits and releases only completed work', async t => {
  const f = await fixture(t);
  let release;
  const wait = new Promise(resolve => { release = resolve; });
  const blocked = async body => { await wait; return f.snapshot(body); };
  const reads = Array.from({ length: 4 }, () => readProxyProviderState({ request: f.request, withCanonicalSnapshot: blocked }));
  await assert.rejects(readProxyProviderState({ request: f.request, withCanonicalSnapshot: blocked }), /read capacity is busy/);
  release(); await Promise.all(reads);
  assert.equal((await readProxyProviderState({ request: f.request, withCanonicalSnapshot: blocked })).provider.sequence, 1);
});

test('expired reads retain their permits while underlying snapshot work is still pending', async t => {
  const callbacks = [];
  t.mock.method(globalThis, 'setTimeout', callback => { callbacks.push(callback); return 0; });
  t.mock.method(globalThis, 'clearTimeout', () => {});
  let release;
  const wait = new Promise(resolve => { release = resolve; });
  const blocked = async () => { await wait; throw new Error('snapshot cleanup finished'); };
  const request = { provider_pubkey: h(1), requester: h(1), initial_operation_digest: h(2), request_nonce: h(3) };
  const reads = Array.from({ length: 4 }, () => readProxyProviderState({ request, withCanonicalSnapshot: blocked }));
  const results = Promise.all(reads.map(read => assert.rejects(read, /observation expired/)));
  callbacks.forEach(callback => callback()); await results;
  await assert.rejects(readProxyProviderState({ request, withCanonicalSnapshot: blocked }), /read capacity is busy/);
  release(); await new Promise(resolve => setImmediate(resolve));
  await assert.rejects(readProxyProviderState({ request, withCanonicalSnapshot: blocked }), /snapshot cleanup finished/);
});

test('authenticated service/RPC binds provider, network and fresh nonces; old peers and stale replies fail closed', async t => {
  const f = await fixture(t), admin = new MayhemFeature(f.peer, {});
  admin.withProxyCanonicalSnapshot = f.snapshot;
  const peer = { ...f.peer, wallet: { ...f.peer.wallet, publicKey: f.provider.publicKey,
    sign: bytes => hex(f.provider.wallet.sign(b4a.from(bytes))) }, base: { writable: false, view: f.base.view } };
  const client = new MayhemFeature(peer, {});
  t.after(() => { admin.stop(); client.stop(); });
  let previous, replay = false;
  const nonces = [];
  client.requestService = async (service, envelope) => {
    assert.equal(service, PROXY_PROVIDER_STATE_SERVICE);
    const verify = value => admin._verifyServiceRequest(service, value, { admin: f.admin.publicKey, transport: f.provider.publicKey });
    assert.equal(verify({ ...envelope, payload: { ...envelope.payload, provider_pubkey: h(7) } }), null);
    const authorization = verify(envelope); assert.ok(authorization);
    nonces.push(authorization.payload.request_nonce);
    if (replay) return structuredClone(previous);
    previous = await admin._handleService(service, authorization.payload, authorization);
    return structuredClone(previous);
  };
  peer.protocol = { instance: { features: { mayhem: client } } };
  const { requester, ...query } = f.request;
  const server = createServer(peer);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => server.close(resolve)));
  const before = f.base.local.length;
  const response = await fetch(`http://127.0.0.1:${server.address().port}/v1/proxy/provider-state`, {
    method: 'POST', body: JSON.stringify(query), headers: { 'content-type': 'application/json' },
  });
  assert.equal(response.status, 200); assert.equal((await response.json()).provider.sequence, 1);
  await requestProxyProviderState(peer, query);
  assert.equal(new Set(nonces).size, 2); assert.ok(nonces.every(n => n !== query.request_nonce));
  replay = true; await assert.rejects(requestProxyProviderState(peer, query), /does not match/);
  await assert.rejects(requestProxyProviderState({}, query), /not ready/);
  await assert.rejects(requestProxyProviderState(peer, { ...query, requester }), /Invalid/);
  assert.equal(f.base.local.length, before);
});
