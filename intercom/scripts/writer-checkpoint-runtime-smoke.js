// Filesystem, locking and scheduler acceptance under the actual Pear/Bare runtime.
// This smoke uses an in-memory transport and submits no network transactions.
import fs from 'fs';
import path from 'path';
import assert from 'assert';
import hrtime from 'bare-hrtime';
import { CheckpointJournal, WriterCheckpointWorker } from '../src/writer-checkpoint-worker.js';
import { WriterCheckpointTransport } from '../src/writer-checkpoint-transport.js';

const directory = globalThis.Bare?.argv?.at(-1);
assert(typeof directory === 'string' && path.isAbsolute(directory));
assert(path.basename(directory).startsWith('writer-checkpoint-smoke-'));
assert(fs.readdirSync(directory).length === 0, 'smoke directory must be newly created and empty');
assert(typeof WriterCheckpointTransport === 'function');
let now = 3000000;
const paid = new Set();
const applied = new Map();
const journal = new CheckpointJournal(directory);
const transport = {
  identity: async () => ({ public_key: 'runtime-smoke', network_id: 0 }),
  history: async () => ({ current: null, preparing: null }),
  funding: async () => ({ balance_au: '1000', fee_au: '3' }),
  prepareSnapshot: async (slot) => ({ slot, snapshot_hash: `snapshot-${slot}` }),
  preparePayment: async ({ slot }) => ({ surrogate: { tx: `tx-${slot}` }, dispatch: { value: { slot } } }),
  inspect: async (prepared) => ({ confirmed: paid.has(prepared.surrogate.tx),
    proof: { tx: prepared.surrogate.tx, signed_length: 1 } }),
  broadcast: async (prepared) => { paid.add(prepared.surrogate.tx); },
  reconcileSubnet: async (prepared) => {
    const checkpoint = { slot: prepared.dispatch.value.slot, tx: prepared.surrogate.tx };
    applied.set(checkpoint.slot, checkpoint);
    return checkpoint;
  },
};
const worker = new WriterCheckpointWorker({ journal, transport, now: () => now,
  monotonicNow: () => Number(hrtime.bigint() / 1000000n) + now, reserveAu: '0', log: () => {} });
try {
  await worker.initialize();
  let lockRejected = false;
  try { new CheckpointJournal(directory); } catch (error) { lockRejected = /spending lock/.test(error.message); }
  assert(lockRejected);
  for (let slot = 0; slot < 2; slot++) {
    if (slot) now += 30000;
    worker.schedule();
    await worker.step();
    await worker.step();
  }
  assert(paid.size === 2);
  assert(applied.size === 2);
  assert(worker.status().backlog_slots === 0);
  assert(fs.readdirSync(path.join(directory, 'completed')).length === 2);
  await worker.stop();
  const reopened = new CheckpointJournal(directory);
  assert(reopened.state.next_slot === 102);
  reopened.close();
  console.log(JSON.stringify({ ok: true, runtime: 'pear/bare', paid_transport: 'in-memory', slots: 2,
    durable_restart: true, exclusive_lock: true }));
} finally {
  await worker.stop();
}
Bare.exit(0);
