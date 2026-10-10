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
