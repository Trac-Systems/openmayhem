import test from 'node:test';
import assert from 'node:assert/strict';
import { createAdmissionMsbReader, validateAdmissionMsbSnapshot } from '../features/mayhem/proxy-admission-msb.js';
import { createTnkDiscoveryFixture, testTnkAddress } from './helpers/proxy-admission-tnk-fixture.mjs';
import { scanTnkSignedPage, verifyTnkObservedTransfer } from '../scripts/proxy-admission-tnk.mjs';

const hash = 'ab'.repeat(32), destination = testTnkAddress('07'.repeat(32));
const context = { network_id: '1', msb_bootstrap: hash };
async function fixture(t) {
  const f = await createTnkDiscoveryFixture({ hash, destination }); t.after(() => f.close()); return f;
}
const options = proof => ({ frontier: proof.signed_length, canonicalProof: proof,
  finality: 1, timeoutSeconds: 1, addressPrefix: 'testtrac', signal: AbortSignal.timeout(1000) });

test('authority reads a real signed identity without writes and accepts an online non-indexer reader', async t => {
  const f = await fixture(t), before = f.view.core.length, proof = await f.frontier();
  assert.equal(proof.signed_length, before);
  assert.equal(proof.tree_hash, (await f.view.core.treeHash(before)).toString('hex'));
  assert.equal(proof.view_key, f.view.core.key.toString('hex')); assert.equal(f.view.core.length, before);
  assert.equal(validateAdmissionMsbSnapshot(proof, context), proof);
  f.msb.state.isIndexer = () => false;
  await assert.rejects(f.frontier(), /source unavailable/);
  f.msb.network = { validatorConnectionManager: { connectionCount: () => 1 } };
  assert.equal((await f.frontier()).tree_hash, proof.tree_hash);
  for (const change of [{ network_id: '2' }, { msb_bootstrap: 'cd'.repeat(32) }, { signed_length: 0 },
    { fork: -1 }, { tree_hash: 'bad' }, { observed_at_ms: Date.now() - 20000 },
    { observed_at_ms: Date.now() + 20000 }, { extra: true }]) {
    assert.throws(() => validateAdmissionMsbSnapshot({ ...proof, ...change }, context));
  }
});

test('authority fences source changes while hashing and refuses foreign configured network', async () => {
  for (const fault of ['network', 'bootstrap', 'key', 'fork', 'base', 'offline', 'length']) {
    let online = true, length = 10, closed = 0;
    const core = { key: Buffer.alloc(32, 1), fork: 0 };
    const msb = { config: { networkId: '1', bootstrap: Buffer.from(hash, 'hex') },
      state: { isIndexer: () => online, getSignedLength: () => length, base: null } };
    const view = { core, checkout: () => ({ core: { treeHash: async () => {
      if (fault === 'network') msb.config.networkId = '2';
      if (fault === 'bootstrap') msb.config.bootstrap = Buffer.alloc(32, 3);
      if (fault === 'key') core.key = Buffer.alloc(32, 4);
      if (fault === 'fork') core.fork++;
      if (fault === 'base') msb.state.base = { view };
      if (fault === 'offline') online = false;
      if (fault === 'length') length = 9;
      return Buffer.alloc(32, 2);
    } }, async close() { closed++; } }) };
    msb.state.base = { view };
    await assert.rejects(createAdmissionMsbReader(msb)(context), /source changed/); assert.equal(closed, 1);
  }
  const foreign = { config: { networkId: '2', bootstrap: Buffer.from(hash, 'hex') },
    state: { base: { view: { core: { key: Buffer.alloc(32, 1), fork: 0 }, checkout() { throw Error('must not read'); } } },
      getSignedLength: () => 10, isIndexer: () => true } };
  await assert.rejects(createAdmissionMsbReader(foreign)(context), /source unavailable/);
});

test('timed-out source reads retain one of four permits until actual cleanup; retries cannot flood disk work', async t => {
  const timers = []; t.mock.method(globalThis, 'setTimeout', fn => { timers.push(fn); return 0; });
  t.mock.method(globalThis, 'clearTimeout', () => {});
  const pending = []; let opens = 0, closes = 0;
  const view = { core: { key: Buffer.alloc(32, 1), fork: 0 }, checkout() {
    opens++;
    return { core: { treeHash: () => new Promise(resolve => pending.push(resolve)) }, async close() { closes++; } };
  } };
  const read = createAdmissionMsbReader({ config: { networkId: '1', bootstrap: Buffer.from(hash, 'hex') },
    state: { base: { view }, getSignedLength: () => 10, isIndexer: () => true } });
  const jobs = Array.from({ length: 4 }, () => read(context));
  const rejected = Promise.all(jobs.map(job => assert.rejects(job, /deadline/)));
  await new Promise(resolve => setImmediate(resolve)); timers.forEach(fn => fn()); await rejected;
  for (let i = 0; i < 10; i++) await assert.rejects(read(context), /capacity/);
  assert.equal(opens, 4); assert.equal(closes, 4);
  pending.forEach(resolve => resolve(Buffer.alloc(32, 2)));
  await new Promise(resolve => setImmediate(resolve));
  const next = read(context); await new Promise(resolve => setImmediate(resolve));
  pending[4](Buffer.alloc(32, 2)); assert.equal((await next).signed_length, 10); assert.equal(opens, 5);
});

test('discovery and verification match the authoritative prefix, even when local state is ahead', async t => {
  const f = await fixture(t), initial = await f.frontier();
  await f.view.put('later/1', Buffer.from('1')); await f.view.put('later/2', Buffer.from('2'));
  const current = await f.frontier(); await f.view.put('later/3', Buffer.from('3'));
  const page = await scanTnkSignedPage(f.msb, { ...options(initial), from: 0 });
  assert.equal(page.transfers.length, 1); assert.equal(page.next_cursor, String(initial.signed_length));
  // Extra local entries must not promote a transaction past authoritative finality.
  await assert.rejects(verifyTnkObservedTransfer(f.msb, { transaction_hash: hash, destination }, options(initial)), /awaiting_finality/);
  const receipt = await verifyTnkObservedTransfer(f.msb, { transaction_hash: hash, destination }, options(current));
  assert.equal(receipt.finalized, true); assert.equal(receipt.tokenAmountBaseUnits, 9n);
  for (const change of [{ view_key: '01'.repeat(32) }, { tree_hash: '02'.repeat(32) },
    { signed_length: current.signed_length + 1 }, { network_id: '2' }, { msb_bootstrap: '03'.repeat(32) },
    { observed_at_ms: Date.now() - 20000 }]) {
    const bad = { ...current, ...change }, opts = { ...options(current), canonicalProof: bad };
    await assert.rejects(scanTnkSignedPage(f.msb, { ...opts, from: 0 }));
    await assert.rejects(verifyTnkObservedTransfer(f.msb, { transaction_hash: hash, destination }, opts));
  }
});

test('independent reader rebuild counters do not reject the same canonical Merkle prefix', async t => {
  const f = await fixture(t);
  await f.view.put('later/1', Buffer.from('1')); await f.view.put('later/2', Buffer.from('2'));
  const previous = { ...await f.frontier(), fork: 17 };
  assert.notEqual(f.view.core.fork, previous.fork);
  const page = await scanTnkSignedPage(f.msb, { ...options(previous), from: 0 });
  assert.equal(page.transfers.length, 1);
  const receipt = await verifyTnkObservedTransfer(f.msb, { transaction_hash: hash, destination }, options(previous));
  assert.equal(receipt.finalized, true); assert.equal(receipt.tokenAmountBaseUnits, 9n);
  await f.view.put('later/3', Buffer.from('3'));
  const current = { ...await f.frontier(), fork: 18 };
  const next = await scanTnkSignedPage(f.msb, { ...options(current), from: previous.signed_length, previousSnapshot: previous });
  assert.equal(next.next_cursor, String(current.signed_length));
  await assert.rejects(scanTnkSignedPage(f.msb, { ...options(current), from: previous.signed_length,
    previousSnapshot: { ...previous, tree_hash: 'ab'.repeat(32) } }), /prefix hash differs/);
  // An authority rebuild is safe only when its new view still proves the exact
  // retained signed prefix. A changed counter alone must not strand discovery.
  const rebuilt = await scanTnkSignedPage(f.msb, { ...options(current), from: previous.signed_length,
    previousSnapshot: { ...previous, fork: 16 } });
  assert.equal(rebuilt.next_cursor, String(current.signed_length));
});

test('verification never accepts a changed local fork during its exact-key read', async t => {
  const f = await fixture(t);
  await f.view.put('later/1', Buffer.from('1')); await f.view.put('later/2', Buffer.from('2'));
  const proof = await f.frontier(), real = f.view.core;
  const core = { key: real.key, fork: real.fork, treeHash: length => real.treeHash(length) };
  f.msb.state.base.view = { core, checkout(length) {
    const snapshot = f.view.checkout(length);
    return { async get(key) { const value = await snapshot.get(key); core.fork++; return value; }, close: () => snapshot.close() };
  } };
  await assert.rejects(verifyTnkObservedTransfer(f.msb, { transaction_hash: hash, destination }, options(proof)), /snapshot changed/);
});

test('continued discovery verifies the retained Merkle prefix inside the newer canonical view', async t => {
  const f = await fixture(t), previous = await f.frontier();
  await f.view.put('later/1', Buffer.from('1')); await f.view.put('later/2', Buffer.from('2'));
  const current = await f.frontier();
  const scan = older => scanTnkSignedPage(f.msb, { ...options(current), from: previous.signed_length, previousSnapshot: older });
  const page = await scan(previous); assert.equal(page.next_cursor, String(current.signed_length));
  assert.equal(page.transfers.length, 0);
  for (const change of [{ tree_hash: '01'.repeat(32) }, { view_key: '02'.repeat(32) },
    { network_id: '2' }, { signed_length: current.signed_length + 1 }]) {
    await assert.rejects(scan({ ...previous, ...change }));
  }
  // Retained evidence may be old; it is the newly authenticated authority
  // response that must be fresh. Do not strand discovery after downtime.
  await scan({ ...previous, observed_at_ms: previous.observed_at_ms - 100000 });
});
