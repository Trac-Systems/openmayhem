import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import PeerWallet from 'trac-wallet';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { bigIntTo16ByteBuffer } from 'trac-msb/src/utils/amountSerialization.js';
import { OperationType } from 'trac-msb/src/utils/constants.js';
import { verifyTnkObservedTransfer, tnkVerifier } from '../scripts/proxy-admission-worker.mjs';
import { scanTnkSignedPage } from '../scripts/proxy-admission-tnk.mjs';

const hash = 'ab'.repeat(32), other = 'cd'.repeat(32), addressPrefix = 'testtrac';
const destination = PeerWallet.encodeBech32mSafe(addressPrefix, Buffer.alloc(32, 7));
const sender = PeerWallet.encodeBech32mSafe(addressPrefix, Buffer.alloc(32, 8));
const encoded = (tx = hash, amount = 9n, to = destination) => safeEncodeApplyOperation({ type: OperationType.TRANSFER,
  address: Buffer.from(sender), tro: { tx: Buffer.from(tx, 'hex'), txv: Buffer.alloc(32, 1), in: Buffer.alloc(32, 2),
    to: Buffer.from(to), am: bigIntTo16ByteBuffer(amount), is: Buffer.alloc(64, 3) } });
const options = frontier => ({ frontier, finality: 3, timeoutSeconds: 1, addressPrefix, signal: AbortSignal.timeout(5000) });
const intent = { transaction_hash: hash, destination };

async function fixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'proxy-admission-exact-'));
  let store, view;
  const open = async () => { store = new Corestore(root); view = new Hyperbee(store.get({ name: 'signed' }),
    { keyEncoding: 'utf-8', valueEncoding: 'binary', extension: false }); await view.ready(); };
  await open();
  t.after(async () => { await view.close(); await store.close(); await fs.rm(root, { recursive: true, force: true }); });
  return { get view() { return view; }, async reopen() { await view.close(); await store.close(); await open(); } };
}

test('discovery pages all signed positions without moving reads, skips unrelated entries and excludes unsigned/newer writes', async t => {
  const f = await fixture(t);
  const hashes = [];
  for (let n = 0; n < 38; n++) {
    const key = n.toString(16).padStart(64, '0'); hashes.push(key);
    await f.view.put(key, encoded(key));
    if (n % 5 === 0) await f.view.put(`metadata/${n}`, Buffer.from('not a transfer'));
  }
  const frontier = f.view.core.signedLength;
  await f.view.put(hash, encoded(hash)); // Outside the authoritative boundary.
  let historyReads = 0, maximum = 0;
  const base = { core: f.view.core, checkout(length) {
    assert.equal(length, frontier); const snapshot = f.view.checkout(length);
    return { createHistoryStream(options) {
      historyReads++; maximum = Math.max(maximum, options.lt - options.gte);
      assert.equal(options.limit, 16); return snapshot.createHistoryStream(options);
    }, close: () => snapshot.close() };
  } };
  const msb = { state: { base: { view: base }, getSignedLength: () => f.view.core.signedLength },
    getTxHashes() { throw Error('moving scan'); }, getTxDetails() { throw Error('moving payload'); } };
  let from = 0; const found = [];
  while (from < frontier) {
    const p = await scanTnkSignedPage(msb, { from, frontier, signal: AbortSignal.timeout(1000), addressPrefix });
    found.push(...p.transfers.map(x => x.transaction_hash)); from = Number(p.next_cursor);
    assert.equal(p.proof.signed_length, frontier); assert.equal(p.proof.view_key, f.view.core.key.toString('hex'));
    assert.equal(p.proof.tree_hash, (await f.view.core.treeHash(frontier)).toString('hex'));
  }
  assert.deepEqual(found, hashes); assert.equal(maximum, 16); assert.equal(historyReads, Math.ceil(frontier / 16));
  const empty = await scanTnkSignedPage(msb, { from: frontier, frontier, signal: AbortSignal.timeout(1000), addressPrefix });
  assert.equal(empty.transfers.length, 0); assert.equal(historyReads, Math.ceil(frontier / 16));
});

test('discovery does not advance over corrupt signed payloads or wrong transfer hashes', async t => {
  const f = await fixture(t);
  for (const bytes of [Buffer.from([255]), encoded(other), encoded(hash, 0n), encoded(hash, 1n, 'invalid-address'), Buffer.alloc(16385)]) {
    await f.view.put(hash, bytes); const entry = await f.view.get(hash), frontier = f.view.core.signedLength;
    const msb = { state: { base: { view: f.view }, getSignedLength: () => frontier } };
    await assert.rejects(scanTnkSignedPage(msb, { from: entry.seq, frontier, signal: AbortSignal.timeout(1000), addressPrefix }));
  }
});

test('discovery aborts a stalled Merkle read before the history stream opens', async () => {
  const controller = new AbortController(); let started, closed = 0, opens = 0;
  const ready = new Promise(resolve => { started = resolve; });
  const core = { key: Buffer.alloc(32, 1), fork: 0, treeHash: () => { started(); return new Promise(() => {}); } };
  const base = { core, checkout: () => ({ createHistoryStream() { opens++; throw Error('must not open'); }, async close() { closed++; } }) };
  const msb = { state: { base: { view: base }, getSignedLength: () => 20 } };
  const pending = scanTnkSignedPage(msb, { from: 1, frontier: 20, signal: controller.signal, addressPrefix });
  await ready; controller.abort(); await assert.rejects(pending); assert.equal(opens, 0); assert.ok(closed > 0);
});

test('discovery fences fork/key changes and cancellation, always closing the snapshot', async () => {
  for (const fault of ['fork', 'key', 'abort', 'order']) {
    const controller = new AbortController(); let closed = 0;
    const core = { key: Buffer.alloc(32, 1), fork: 0, treeHash: async () => Buffer.alloc(32, 2) };
    const base = { core, checkout: () => ({ createHistoryStream() {
      const stream = (async function* () {
        if (fault === 'fork') core.fork++;
        if (fault === 'key') core.key = Buffer.alloc(32, 3);
        if (fault === 'abort') controller.abort();
        yield { type: 'put', seq: fault === 'order' ? 100 : 1, key: hash, value: encoded() };
      })(); stream.destroy = () => {}; return stream;
    }, async close() { closed++; } }) };
    const msb = { state: { base: { view: base }, getSignedLength: () => 20 } };
    await assert.rejects(scanTnkSignedPage(msb, { from: 1, frontier: 20, signal: controller.signal, addressPrefix }));
    assert.ok(closed > 0);
  }
});

test('an old queued TNK payment survives backlog/reopen with one exact signed read and unchanged observed amount', async t => {
  const f = await fixture(t); await f.view.put(hash, encoded());
  const old = await f.view.get(hash);
  const batch = f.view.batch();
  for (let i = 0; i < 180; i++) await batch.put(`unrelated/${i}`, Buffer.from('not a payment'));
  await batch.flush(); await batch.close();
  await f.reopen();
  const signed = f.view.core.signedLength;
  assert.ok(signed - old.seq > 50, 'transfer is outside the former configured lookback');
  let reads = 0, closes = 0;
  const msb = { state: { getSignedLength: () => signed, base: { view: { checkout(length) {
    assert.equal(length, signed); const snapshot = f.view.checkout(length);
    return { async get(key) { reads++; assert.equal(key, hash); return snapshot.get(key); },
      async close() { closes++; await snapshot.close(); } };
  } } } }, getExtendedTxDetails() { throw Error('must not scan history'); }, getTxHashes() { throw Error('must not scan'); } };
  const work = { invoice: { rail: 'tnk', collection: { network: 'testnet1', destination }, amount_base_units: '10' },
    payment_reference: { network: 'testnet1', transaction_hash: hash } };
  const verify = tnkVerifier({ network: 'testnet1', verifyTransfer: (input, signal) => verifyTnkObservedTransfer(msb, input, { ...options(signed), signal }) });
  const receipt = await verify(work, options(signed).signal);
  assert.deepEqual(receipt, { rail: 'tnk', physical_key: `tnk/testnet1/${hash}`, amount_base_units: '9', finalized: true,
    network: 'testnet1', transaction_hash: hash, from_address: sender, to_address: destination, confirmed_signed_length: old.seq });
  assert.equal(reads, 1); assert.equal(closes, 1);
  // An ACK loss/retry rereads the same signed evidence; it does not reprice or
  // infer payment from an old cached success.
  assert.deepEqual(await verify(work, options(signed).signal), receipt);
  assert.equal(reads, 2); assert.equal(closes, 2);
});

test('the fixed signed snapshot excludes newer writes; exact finality boundary stays pending', async t => {
  const f = await fixture(t); await f.view.put(other, encoded(other));
  const before = f.view.core.signedLength;
  await f.view.put(hash, encoded());
  const entry = await f.view.get(hash);
  const msb = signed => ({ state: { getSignedLength: () => signed, base: { view: f.view } } });
  await assert.rejects(verifyTnkObservedTransfer(msb(before), intent, options(before)), /transfer_pending/);
  await f.view.put('later/1', Buffer.from('1')); await f.view.put('later/2', Buffer.from('2'));
  const boundary = entry.seq + 3;
  await assert.rejects(verifyTnkObservedTransfer(msb(boundary), intent, options(boundary)), /awaiting_finality/);
  await f.view.put('later/3', Buffer.from('3'));
  assert.equal((await verifyTnkObservedTransfer(msb(boundary + 1), intent, options(boundary + 1))).blockNumber, BigInt(entry.seq));
});

test('wrong signed key, sequence, payload hash, destination, amount and malformed bytes never confirm', async () => {
  for (const entry of [
    { key: other, seq: 1, value: encoded() }, { key: hash, seq: 100, value: encoded() },
    { key: hash, seq: -1, value: encoded() }, { key: hash, seq: 1, value: encoded(other) },
    { key: hash, seq: 1, value: encoded(hash, 9n, sender) }, { key: hash, seq: 1, value: encoded(hash, 0n) },
    { key: hash, seq: 1, value: Buffer.from([255]) },
  ]) {
    let closed = false;
    const msb = { state: { getSignedLength: () => 100, base: { view: { checkout: () => ({ get: async () => entry, close: async () => { closed = true; } }) } } } };
    await assert.rejects(verifyTnkObservedTransfer(msb, intent, options(100)));
    assert.equal(closed, true);
  }
});

test('abort/deadline close the exact-key read and a reader behind the canonical frontier cannot verify', async () => {
  for (const timed of [false, true]) {
    let rejectRead, closed = 0;
    const controller = new AbortController();
    const msb = { state: { getSignedLength: () => 100, base: { view: { checkout: () => ({
      get: async () => new Promise((resolve, reject) => { rejectRead = reject; }),
      close: async () => { closed++; rejectRead?.(Error('snapshot closed')); },
    }) } } } };
    const pending = verifyTnkObservedTransfer(msb, intent, { ...options(100), signal: controller.signal });
    // Keep the process alive for the unref'ed AbortSignal timeout.
    const timer = setTimeout(() => { if (!timed) controller.abort(); }, timed ? 1500 : 10);
    try { await assert.rejects(pending); assert.ok(closed >= 1); } finally { clearTimeout(timer); }
  }
  const controller = new AbortController();
  const pending = verifyTnkObservedTransfer({ state: { getSignedLength: () => 1 } }, intent, { ...options(100), signal: controller.signal });
  controller.abort(); await assert.rejects(pending);
});
