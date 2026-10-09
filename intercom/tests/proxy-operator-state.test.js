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
import { readProxyOperatorState, validateProxyOperatorStateRequest, PROXY_OPERATOR_STATE_SERVICE } from '../features/mayhem/proxy-operator-state.js';
import { proxyContractFixture } from './helpers/proxy.js';
import { execute, signProviderKyb } from './helpers/contract.js';
import { createServer, requestProxyOperatorState } from '../src/rpc.js';
const h = n => n.toString(16).padStart(64, '0');
const hex = value => b4a.toString(value, 'hex');

async function fixture(t) {
  const f = await proxyContractFixture();
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'proxy-operator-state-'));
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
  f.peer.wallet = { publicKey: f.admin.publicKey, sign: bytes => hex(f.admin.wallet.sign(b4a.from(bytes))),
    verify: (signature, bytes, key) => PeerWallet.verify(b4a.from(signature, 'hex'), b4a.from(bytes), b4a.from(key, 'hex')) };
  // Native enrollment is an explicit fixture. Verification itself uses the real
  // canonical admin command and signature, then a real signed Autobase view.
  await f.storage.put(`prov/${f.provider.publicKey}`, { provider: f.provider.publicKey, status: 'active',
    kyb: { status: 'verified', label: 'provider supplied summary is not evidence' } });
  const unsigned = { op: 'set_provider_kyb', provider: f.provider.publicKey, legal_name: 'Local Test Operator',
    jurisdiction: 'DE', proof_hash: h(9), kyb_ref: 'LOCAL-TEST-ONLY', verified_at: 1_788_000_000, schema_version: 1 };
  const signed = { ...unsigned, admin_sig: signProviderKyb(f.admin.wallet, unsigned) };
  const write = async entries => { await base.append({ entries }); await base.update(); };
  await write([...f.storage.values]);
  const snapshot = createProxyCanonicalSnapshot(f.peer, CONTRACT_VERSION);
  const request = { provider_pubkey: f.provider.publicKey, requester: f.issuer.publicKey, request_nonce: h(11) };
  const command = async (method, value, actor = f.admin.publicKey) => {
    const result = await execute(f.contract, f.storage, method, value, actor, 77);
    if (result?.ok) await write([...f.storage.values]);
    return result;
  };
  return { ...f, base, snapshot, request, write, signed, command };
}

test('exact canonical operator reads require real admin KYB and never expose legal fields or mutate state', async t => {
  const f = await fixture(t), read = () => readProxyOperatorState({ request: f.request, withCanonicalSnapshot: f.snapshot });
  assert.equal((await read()).operator.status, 'not_verified');
  assert.notEqual((await f.command('setProviderKyb', f.signed, f.provider.publicKey)).ok, true);
  assert.notEqual((await f.command('setProviderKyb', { ...f.signed, admin_sig: '0'.repeat(128) })).ok, true);
  assert.equal((await read()).operator.status, 'not_verified');
  assert.equal((await f.command('setProviderKyb', f.signed)).ok, true);
  const before = f.base.local.length, keys = [];
  const response = await readProxyOperatorState({ request: f.request, withCanonicalSnapshot: (body, options) => f.snapshot(s => body({ ...s,
    read: key => { keys.push(key); return s.read(key); } }), options) });
  assert.deepEqual(response.operator, { status: 'verified', proof_hash: h(9) });
  assert.deepEqual(keys, [`prov/${f.provider.publicKey}`, `kyb/${f.provider.publicKey}`]);
  for (const forbidden of ['legal_name', 'jurisdiction', 'kyb_ref', 'admin_sig', 'LOCAL-TEST-ONLY', 'private_key']) {
    assert.equal(JSON.stringify(response).includes(forbidden), false, forbidden);
  }
  assert.equal(f.base.local.length, before);
  assert.equal((await f.command('revokeProviderKyb', { op: 'revoke_provider_kyb', provider: f.provider.publicKey })).ok, true);
  assert.deepEqual((await read()).operator, { status: 'revoked', proof_hash: null });
  await f.write([[`prov/${f.provider.publicKey}`, { provider: f.provider.publicKey, status: 'banned' }]]);
  assert.equal((await read()).operator.status, 'inactive');
  await f.write([[`prov/${f.provider.publicKey}`, null]]);
  assert.equal((await read()).operator.status, 'not_registered');
});

test('foreign records, unsupported schemas and mid-read revocation are unavailable rather than verified', async t => {
  const f = await fixture(t);
  assert.equal((await f.command('setProviderKyb', f.signed)).ok, true);
  const kyb = (await f.storage.get(`kyb/${f.provider.publicKey}`)).value;
  for (const altered of [{ ...kyb, provider: h(123) }, { ...kyb, verified_by_role: 'provider' }, { ...kyb, schema_version: 0 }]) {
    await f.write([[`kyb/${f.provider.publicKey}`, altered]]);
    await assert.rejects(readProxyOperatorState({ request: f.request, withCanonicalSnapshot: f.snapshot }), /canonical KYB/);
  }
  await f.write([[`kyb/${f.provider.publicKey}`, kyb]]);
  await f.write([[`prov/${f.provider.publicKey}`, { provider: f.provider.publicKey, status: 'future_unknown' }]]);
  await assert.rejects(readProxyOperatorState({ request: f.request, withCanonicalSnapshot: f.snapshot }), /provider status is unknown/);
  await f.write([[`prov/${f.provider.publicKey}`, { provider: f.provider.publicKey, status: 'active' }]]);
  await assert.rejects(readProxyOperatorState({ request: f.request, withCanonicalSnapshot: (body, options) => f.snapshot(s => {
    let checks = 0;
    return body({ ...s, assertCurrent: async () => {
      if (++checks === 2) await f.write([[`kyb/${f.provider.publicKey}`, { ...kyb, status: 'revoked' }]]);
      await s.assertCurrent();
    } });
  }, options) }), /registry state changed/);
  for (const change of [{ provider_pubkey: 'bad' }, { raw_documents: [] }, { requester: null }])
    assert.throws(() => validateProxyOperatorStateRequest({ ...f.request, ...change }));
  await assert.rejects(f.snapshot(s => s.read(`kyb/${f.provider.publicKey}`)), /invalid registry read key/);
  await assert.rejects(f.snapshot(s => s.read('bal/private'), { operator: true }), /invalid registry read key/);
});

test('four bounded reads retain permits through timeout until canonical work completes', async t => {
  const callbacks = [];
  t.mock.method(globalThis, 'setTimeout', callback => { callbacks.push(callback); return 0; });
  t.mock.method(globalThis, 'clearTimeout', () => {});
  let release;
  const wait = new Promise(resolve => { release = resolve; });
  const blocked = async () => { await wait; throw new Error('cleanup complete'); };
  const request = { provider_pubkey: h(1), requester: h(2), request_nonce: h(3) };
  const reads = Array.from({ length: 4 }, () => readProxyOperatorState({ request, withCanonicalSnapshot: blocked }));
  await assert.rejects(readProxyOperatorState({ request, withCanonicalSnapshot: blocked }), /read capacity is busy/);
  const results = Promise.all(reads.map(read => assert.rejects(read, /observation expired/)));
  callbacks.forEach(callback => callback()); await results;
  await assert.rejects(readProxyOperatorState({ request, withCanonicalSnapshot: blocked }), /read capacity is busy/);
  release(); await new Promise(resolve => setImmediate(resolve));
  await assert.rejects(readProxyOperatorState({ request, withCanonicalSnapshot: blocked }), /cleanup complete/);
});

test('real loopback RPC authenticates fresh provider/network nonces; old peers and replay fail closed', async t => {
  const f = await fixture(t), admin = new MayhemFeature(f.peer, {});
  assert.equal((await f.command('setProviderKyb', f.signed)).ok, true);
  admin.withProxyCanonicalSnapshot = f.snapshot;
  const peer = { ...f.peer, wallet: { ...f.peer.wallet, publicKey: f.issuer.publicKey,
    sign: bytes => hex(f.issuer.wallet.sign(b4a.from(bytes))) }, base: { writable: false, view: f.base.view } };
  const client = new MayhemFeature(peer, {});
  t.after(() => { admin.stop(); client.stop(); });
  let previous, replay = false;
  const nonces = [];
  client.requestService = async (service, envelope) => {
    assert.equal(service, PROXY_OPERATOR_STATE_SERVICE);
    const verify = value => admin._verifyServiceRequest(service, value, { admin: f.admin.publicKey, transport: f.issuer.publicKey });
    assert.equal(verify({ ...envelope, payload: { ...envelope.payload, provider_pubkey: h(7) } }), null);
    const authorization = verify(envelope); assert.ok(authorization);
    nonces.push(authorization.payload.request_nonce);
    if (replay) return structuredClone(previous);
    previous = await admin._handleService(service, authorization.payload, authorization);
    return structuredClone(previous);
  };
  peer.protocol = { instance: { features: { mayhem: client } } };
  const { requester, ...query } = f.request;
  const server = createServer(peer); await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => server.close(resolve)));
  const before = f.base.local.length;
  const response = await fetch(`http://127.0.0.1:${server.address().port}/v1/proxy/operator-state`, {
    method: 'POST', body: JSON.stringify(query), headers: { 'content-type': 'application/json' },
  });
  assert.equal(response.status, 200); assert.equal(response.headers.get('cache-control'), 'no-store');
  assert.equal((await response.json()).operator.status, 'verified');
  await requestProxyOperatorState(peer, query);
  assert.equal(new Set(nonces).size, 2); assert.ok(nonces.every(n => n !== query.request_nonce));
  replay = true; await assert.rejects(requestProxyOperatorState(peer, query), /does not match/);
  await assert.rejects(requestProxyOperatorState({}, query), /not ready/);
  assert.equal(f.base.local.length, before);
});
