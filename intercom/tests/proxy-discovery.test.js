import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import b4a from 'b4a';
import { CONTRACT_VERSION } from '../contract/contract.js';
import { withProxyDiscoveryWrites, PROXY_CATALOG_PREFIX as CATALOG, PROXY_CATALOG_INDEX_PREFIX as INDEX } from '../contract/proxy-discovery.js';
import { createProxyDiscovery, normalizeProxyDiscoveryQuery, PROXY_DISCOVERY_MAX_PAGE_BYTES } from '../features/mayhem/proxy-discovery.js';
import { makeIdentity } from './helpers/contract.js';
import { proxyContractFixture } from './helpers/proxy.js';
import MayhemFeature from '../features/mayhem/index.js';
import { createServer, discoverProxyCatalog } from '../src/rpc.js';

const hex = value => value.toString(16).padStart(64, '0');
const clone = value => JSON.parse(JSON.stringify(value));
const market = (id, family = 'other', type = 'llm') => ({ id, family: type, model: { family_id: family }, label: `Model ${id}` });
const project = changes => withProxyDiscoveryWrites(changes.map(([key, value]) => ({ key, value })));

// Real local signed Hyperbee storage for pagination/scale. The base authority shim
// is only this read-model fixture; actual Autobase + signed Protomux authority is
// separately tested in proxy-canonical-view and mayhem-feature-protomux.
async function fixture(t, options = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-proxy-discovery-'));
  const identity = await makeIdentity();
  const store = new Corestore(root);
  const view = new Hyperbee(store.get({ name: 'catalog' }), { keyEncoding: 'utf-8', valueEncoding: 'json', extension: false });
  await view.ready();
  const peer = { base: { writable: true, isIndexer: true, key: b4a.from(hex(1), 'hex'), view, _applyState: { view } },
    wallet: { publicKey: identity.publicKey, sign: bytes => identity.wallet.sign(bytes), verify: (...args) => identity.wallet.verify(...args) },
    config: { bootstrap: hex(1) }, msbClient: { networkId: 918, bootstrapHex: hex(2) } };
  let clock = 100_000;
  const run = createProxyDiscovery(peer, CONTRACT_VERSION, { now: () => clock, ...options });
  const write = async entries => {
    const batch = view.batch();
    try {
      for (const entry of entries) {
        const [key, value] = Array.isArray(entry) ? entry : [entry.key, entry.value];
        if (value === null) await batch.del(key); else await batch.put(key, value);
      }
      await batch.flush();
    } finally { await batch.close(); }
  };
  await write([['admin', identity.publicKey], ['epoch/apply/state', { epoch: 100 }]]);
  t.after(async () => { await view.close(); await store.close(); fs.rmSync(root, { recursive: true, force: true }); });
  return { peer, view, write, identity, run, tick: ms => { clock += ms; },
    request: query => run({ requester: identity.publicKey, request_nonce: hex(3), query }) };
}

test('real registry/policy application projects public indexes atomically without fee evidence or native changes', async () => {
  const f = await proxyContractFixture();
  assert.equal((await f.submit(await f.create())).ok, true);
  const id = f.membership.market_id;
  const memberKey = `${CATALOG}memberships/${id}/${f.provider.publicKey}`;
  assert.deepEqual(await f.read(`${CATALOG}markets/${id}`), f.market);
  assert.equal((await f.read(`${INDEX}type-family/llm/other/${id}`)).key, `${CATALOG}markets/${id}`);
  assert.equal((await f.read(memberKey)).active, true);
  const provider = await f.read(`${CATALOG}providers/${f.provider.publicKey}`);
  assert.deepEqual(Object.keys(provider).sort(), ['active_memberships', 'admission_id', 'provider_pubkey', 'sequence']);
  assert.equal((await f.submit(await f.envelope({ kind: 'set_offer', offer: f.offer }))).ok, true);
  const offers = [...f.storage.values].filter(([key]) => key.startsWith(`${CATALOG}offers/`));
  assert.equal(offers.length, 1);
  const offerKey = offers[0][0];
  assert.equal((await f.read(offerKey)).active, true);
  assert.equal((await f.submit(await f.envelope({ kind: 'withdraw_offer', market_id: id,
    endpoint: f.offer.endpoint, ctx_bracket: f.offer.ctx_bracket, outcome_class: f.offer.outcome_class, revision: 2 }))).ok, true);
  assert.equal((await f.read(offerKey)).active, false);
  assert.equal((await f.read(offerKey)).revision, 2);
  assert.equal((await f.submit(await f.envelope({ kind: 'leave_market', market_id: id, revision: 2 }))).ok, true);
  assert.equal((await f.read(memberKey)).active, false);
  assert.equal((await f.policy({ kind: 'set_status', scope: 'provider', id: f.provider.publicKey, revoked: true, reason_hash: hex(5) })).ok, true);
  assert.equal((await f.read(`${CATALOG}provider_status/${f.provider.publicKey}`)).reason_hash, hex(5));
  assert.equal((await f.policy({ kind: 'set_status', scope: 'provider', id: f.provider.publicKey, revoked: false, reason_hash: hex(5) })).ok, true);
  assert.equal(await f.read(`${CATALOG}provider_status/${f.provider.publicKey}`), null);
  assert.deepEqual(await f.read('payout/epoch/542'), { status: 'prepared', native: true });
  assert.deepEqual(await f.read('bal/existing-customer'), { fiat: '10', tnk: '20', tap: '30' });
});

test('snapshot paging has no duplicate/skipped rows while catalog changes; delta includes additions, updates and deletion', async t => {
  const f = await fixture(t);
  await f.write(project(Array.from({ length: 13 }, (_, n) => [`proxy/v1/market/${hex(n + 1)}`, market(n + 1)])));
  const query = { kind: 'markets', limit: 3 };
  let page = await f.request(query);
  const length = f.view.core.length;
  const proof = page.proof;
  const results = [...page.entries];
  assert.equal(page.checkpoint, null);
  await f.write([[`${CATALOG}markets/${hex(2)}`, { label: 'Updated' }],
    [`${CATALOG}markets/${hex(8)}`, null], [`${CATALOG}markets/${hex(99)}`, { label: 'Added' }], ['native/receipt', { doNotScan: true }]]);
  while (page.next_cursor) {
    page = await f.request({ ...query, cursor: page.next_cursor });
    assert.deepEqual(page.proof, proof);
    results.push(...page.entries);
  }
  assert.deepEqual(results.map(row => row.value.id), Array.from({ length: 13 }, (_, n) => n + 1));
  assert.ok(page.checkpoint);
  const changedLength = f.view.core.length;
  const delta = await f.request({ ...query, since: page.checkpoint });
  assert.equal(delta.mode, 'changes');
  assert.equal(delta.truncated, false);
  assert.deepEqual(delta.entries, [{ key: `${CATALOG}markets/${hex(2)}`, value: { label: 'Updated' } },
    { key: `${CATALOG}markets/${hex(8)}`, value: null }, { key: `${CATALOG}markets/${hex(99)}`, value: { label: 'Added' } }]);
  assert.ok(changedLength > length);
  assert.equal(f.view.core.length, changedLength, 'browsing never appends ledger work');
  await f.write([['native/another-receipt', 1]]);
  const empty = await f.request({ ...query, since: delta.checkpoint });
  assert.deepEqual(empty.entries, []);
  assert.equal((await f.request({ kind: 'markets', lookup: hex(99) })).entries[0].value.label, 'Added');
});

test('delta pagination stays on both snapshots and exposes a checkpoint only after all pages', async t => {
  const f = await fixture(t);
  const original = Hyperbee.prototype.createDiffStream;
  const history = Hyperbee.prototype.createHistoryStream;
  Hyperbee.prototype.createHistoryStream = () => { throw new Error('history scan is forbidden'); };
  Hyperbee.prototype.createDiffStream = function (right, options) {
    assert.ok((options.gte ?? options.gt).startsWith(CATALOG));
    assert.equal(options.lt, `${CATALOG}\xff`);
    assert.equal(options.limit, 3, 'only one bounded lookahead');
    return original.call(this, right, options);
  };
  t.after(() => { Hyperbee.prototype.createDiffStream = original; Hyperbee.prototype.createHistoryStream = history; });
  const query = { kind: 'catalog', limit: 2 };
  const initial = await f.request(query);
  await f.write(project(Array.from({ length: 7 }, (_, n) => [`proxy/v1/market/${hex(n + 1)}`, market(n + 1)])));
  let page = await f.request({ ...query, since: initial.checkpoint });
  const proof = page.proof;
  const entries = [...page.entries];
  await f.write(project([[`proxy/v1/market/${hex(99)}`, market(99)]]));
  while (page.next_cursor) {
    assert.equal(page.checkpoint, null);
    page = await f.request({ ...query, cursor: page.next_cursor });
    assert.deepEqual(page.proof, proof);
    assert.deepEqual(page.base_proof, initial.proof);
    entries.push(...page.entries);
  }
  assert.equal(entries.length, 7);
  const next = await f.request({ ...query, since: page.checkpoint });
  assert.deepEqual(next.entries.map(row => row.value.id), [99]);
});

test('family/type and provider filters use indexes, with membership changes visible in filtered deltas', async t => {
  const f = await fixture(t);
  await f.write(project([[`proxy/v1/market/${hex(1)}`, market(1, 'other', 'llm')],
    [`proxy/v1/market/${hex(2)}`, market(2, 'other', 'decisions')],
    [`proxy/v1/market/${hex(3)}`, market(3, 'gpt', 'llm')],
    [`proxy/v1/membership/${hex(1)}/${hex(9)}`, { active: true, revision: 1, offer_slots: 0 }],
    [`proxy/v1/membership/${hex(2)}/${hex(10)}`, { active: true, revision: 1, offer_slots: 0 }]]));
  assert.deepEqual((await f.request({ kind: 'markets', filter: { endpoint_family: 'llm', family_id: 'other' } })).entries.map(row => row.value.id), [1]);
  const query = { kind: 'memberships', filter: { provider_pubkey: hex(9) } };
  const before = await f.request(query);
  assert.equal(before.entries.length, 1);
  await f.write(project([[`proxy/v1/membership/${hex(1)}/${hex(9)}`, { active: false, revision: 2, offer_slots: 0 }]]));
  const after = await f.request({ ...query, since: before.checkpoint });
  assert.equal(after.entries.length, 1);
  assert.equal(after.entries[0].value.active, false);
  assert.equal((await f.request({ kind: 'memberships', filter: { market_id: hex(1), provider_pubkey: hex(9) } })).entries.length, 1);
});

test('cursor signatures, expiry, query/network binding and snapshot identity fail closed', async t => {
  const f = await fixture(t, { pageMaxAgeMs: 1000 });
  await f.write(project([[`proxy/v1/market/${hex(1)}`, market(1)], [`proxy/v1/market/${hex(2)}`, market(2)]]));
  const query = { kind: 'markets', limit: 1 };
  const first = await f.request(query);
  await assert.rejects(f.request({ ...query, limit: 2, cursor: first.next_cursor }), /does not match/);
  const altered = first.next_cursor.slice(0, -1) + (first.next_cursor.endsWith('0') ? '1' : '0');
  await assert.rejects(f.request({ ...query, cursor: altered }), /does not match/);
  const networkId = f.peer.msbClient.networkId;
  f.peer.msbClient.networkId++;
  await assert.rejects(f.request({ ...query, cursor: first.next_cursor }), /does not match/);
  f.peer.msbClient.networkId = networkId;
  f.peer.base.isIndexer = false;
  await assert.rejects(f.request(query), /indexer is not ready/);
  f.peer.base.isIndexer = true;
  // A valid old token cannot cross to a different signed view, even with the same
  // signer/bootstrap. This exercises the proof binding, not just signature edits.
  const replacement = await fixture(t);
  await replacement.write([['admin', f.identity.publicKey]]);
  const originalBase = f.peer.base;
  f.peer.base = { ...originalBase, view: replacement.view, _applyState: { view: replacement.view } };
  try { await assert.rejects(f.request({ ...query, cursor: first.next_cursor }), error => error.code === 'proxy_cursor_invalidated'); }
  finally { f.peer.base = originalBase; }
  f.tick(1001);
  await assert.rejects(f.request({ ...query, cursor: first.next_cursor }), error => error.code === 'proxy_cursor_expired');
  assert.equal((await f.request(query)).ok, true, 'expiry permits fresh traversal');
});

test('byte and record bounds paginate large rows without dropping the first over-budget record', async t => {
  const f = await fixture(t);
  await f.write(Array.from({ length: 25 }, (_, n) => [`${CATALOG}markets/${hex(n + 1)}`, { id: n, description: 'x'.repeat(16000) }]));
  const query = { kind: 'markets', limit: 100 };
  let page = await f.request(query);
  const ids = [];
  do {
    assert.ok(b4a.byteLength(JSON.stringify(page.entries)) < PROXY_DISCOVERY_MAX_PAGE_BYTES + 1024);
    ids.push(...page.entries.map(row => row.value.id));
    if (!page.next_cursor) break;
    page = await f.request({ ...query, cursor: page.next_cursor });
  } while (true);
  assert.deepEqual(ids, Array.from({ length: 25 }, (_, n) => n));
});

test('request validation refuses arbitrary state keys, excessive pages and ambiguous filters', () => {
  for (const query of [{ kind: 'markets', prefix: 'bal/' }, { kind: 'catalog', lookup: '../bal/customer' },
    { kind: 'markets', limit: 101 }, { kind: 'markets', limit: 0 }, { kind: 'offers', lookup: '../secret' },
    { kind: 'markets', filter: { provider_pubkey: hex(1) } }, { kind: 'offers', filter: { family_id: 'gpt' } },
    { kind: 'families', lookup: 'family/../secret' }, { kind: 'network', lookup: 'secret' }]) {
    assert.throws(() => normalizeProxyDiscoveryQuery(query));
  }
});

test('discovery concurrency is bounded independently and failed requests release the allowance', async t => {
  const f = await fixture(t, { maxInFlight: 1 });
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const original = f.view.core.treeHash.bind(f.view.core);
  f.view.core.treeHash = async (...args) => { await gate; return original(...args); };
  const pending = f.request({ kind: 'catalog' });
  await assert.rejects(f.request({ kind: 'catalog' }), error => error.code === 'proxy_discovery_busy');
  release();
  await pending;
  f.view.core.treeHash = original;
  await assert.rejects(f.request({ kind: 'catalog', lookup: 'bad' }));
  assert.equal((await f.request({ kind: 'catalog' })).ok, true);
});

test('local RPC signs read-only discovery and rejects extra request fields', async t => {
  const f = await fixture(t);
  await f.write(project([[`proxy/v1/market/${hex(1)}`, market(1)]]));
  const feature = new MayhemFeature(f.peer, {}); feature.key = 'mayhem';
  feature.proxyDiscovery = f.run; // Controlled clock; actual signed service path.
  f.peer.protocol = { instance: { features: { mayhem: feature } } };
  assert.equal((await discoverProxyCatalog(f.peer, { query: { kind: 'markets' } })).entries.length, 1);
  await assert.rejects(discoverProxyCatalog(f.peer, { query: { kind: 'markets' }, admin: hex(9) }), /Invalid/);
  const server = createServer(f.peer);
  t.after(async () => { await feature.stop(); await new Promise(resolve => server.close(resolve)); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const response = await fetch(`http://127.0.0.1:${server.address().port}/v1/proxy/discovery`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ query: { kind: 'markets' } }) });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).entries[0].value.id, 1);
  await f.write(project([[`proxy/v1/market/${hex(2)}`, market(2)]]));
  const first = await feature.discoverProxyCatalog({ kind: 'markets', limit: 1 });
  f.tick(15 * 60_000 + 1);
  const expired = await fetch(`http://127.0.0.1:${server.address().port}/v1/proxy/discovery`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ query: { kind: 'markets', limit: 1, cursor: first.next_cursor } }) });
  assert.equal(expired.status, 409);
  assert.equal((await expired.json()).code, 'proxy_cursor_expired');
  feature.proxyDiscovery = async () => { throw new Error('test-only backend path and credential detail'); };
  await assert.rejects(feature.discoverProxyCatalog({ kind: 'markets' }), error => {
    assert.equal(error.code, 'proxy_discovery_unavailable');
    assert.equal(error.message, 'Proxy discovery is temporarily unavailable.');
    return true;
  });
});

test('read-only discovery relay times out and releases pending work without changing native deadlines', async t => {
  const f = await fixture(t);
  const requester = await makeIdentity();
  const peer = { ...f.peer, wallet: { publicKey: requester.publicKey, sign: bytes => requester.wallet.sign(bytes),
    verify: (...args) => requester.wallet.verify(...args) }, base: { ...f.peer.base, writable: false },
    sidechannel: { started: true, connectDirectPeer: async () => false, broadcast: () => false } };
  const feature = new MayhemFeature(peer, { timeoutMs: 0, proxyDiscoveryTimeoutMs: 25, retryMs: 5 });
  t.after(() => feature.stop());
  await Promise.all([
    assert.rejects(feature.discoverProxyCatalog({ kind: 'markets' }), error => error.code === 'proxy_discovery_unavailable'),
    new Promise(resolve => setTimeout(resolve, 50)),
  ]);
  assert.equal(feature.servicePending.size, 0);
  assert.equal(feature.timeoutMs, 0, 'native service timeout remains unchanged');
});

test('100,000 synthetic offers remain reachable through indexed provider pages and direct lookup', async t => {
  const f = await fixture(t);
  const total = 100_000;
  // Synthetic public records exercise storage/query scale, not model/payment proof.
  const batch = f.view.batch();
  try {
    for (let n = 1; n <= total; n++) {
      const provider = hex(n % 100);
      const id = `${hex(n)}/${provider}/${hex(7)}`;
      const key = `${CATALOG}offers/${id}`;
      await batch.put(key, { revision: 1, active: true, offer: { synthetic_id: n } });
      await batch.put(`${INDEX}offers-provider/${provider}/${hex(n)}/${hex(7)}`, { key, stamp: [1, 0, true] });
    }
    await batch.flush();
  } finally { await batch.close(); }
  const query = { kind: 'offers', filter: { provider_pubkey: hex(0) }, limit: 100 };
  let page = await f.request(query);
  const ids = [];
  let count = 0;
  do {
    count++;
    assert.ok(page.entries.length <= 100);
    ids.push(...page.entries.map(row => row.value.offer.synthetic_id));
    if (!page.next_cursor) break;
    page = await f.request({ ...query, cursor: page.next_cursor });
  } while (true);
  assert.equal(count, 10);
  assert.deepEqual(ids, Array.from({ length: 1000 }, (_, n) => (n + 1) * 100));
  const last = await f.request({ kind: 'offers', lookup: `${hex(total)}/${hex(0)}/${hex(7)}` });
  assert.equal(last.entries[0].value.offer.synthetic_id, total);
  t.diagnostic('100,000 synthetic offers: 1,000 matching provider rows traversed in ten bounded pages; last offer read directly.');
});
