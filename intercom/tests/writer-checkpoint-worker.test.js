import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { CheckpointJournal, WriterCheckpointWorker } from '../src/writer-checkpoint-worker.js';

async function harness(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-paid-checkpoints-'));
  let clock = 3000000;
  let monotonic = 0;
  const paid = new Set();
  const applied = new Map();
  const snapshots = new Map();
  let preparations = 0;
  let broadcasts = 0;
  const transport = {
    enoughFunds: true, loseAck: false, contextChanged: false, delayApply: false,
    identity: async () => ({ public_key: 'admin', network_id: 918, subnet_bootstrap: 'subnet' }),
    history: async () => ({ current: [...applied.values()].at(-1) ?? null,
      preparing: [...snapshots.values()].at(-1) ?? null }),
    funding: async () => ({ balance_au: transport.enoughFunds ? '100000' : '0', fee_au: '3' }),
    prepareSnapshot: async (slot, observed_at) => {
      if (!snapshots.has(slot)) snapshots.set(slot, { slot, observed_at, snapshot_hash: `snapshot-${slot}` });
      return snapshots.get(slot);
    },
    preparePayment: async (snapshot) => {
      preparations++;
      return { surrogate: { tx: `tx-${preparations}` }, dispatch: { value: { slot: snapshot.slot } } };
    },
    inspect: async (prepared) => paid.has(prepared.surrogate.tx)
      ? { confirmed: true, proof: { tx: prepared.surrogate.tx, signed_length: 42 } }
      : { confirmed: false, context_changed: transport.contextChanged },
    broadcast: async (prepared) => {
      broadcasts++;
      paid.add(prepared.surrogate.tx);
      if (transport.loseAck) throw new Error('ACK lost after MSB deducted fee');
    },
    reconcileSubnet: async (prepared) => {
      if (transport.delayApply) return null;
      const checkpoint = { slot: prepared.dispatch.value.slot, tx: prepared.surrogate.tx };
      applied.set(checkpoint.slot, checkpoint);
      return checkpoint;
    },
  };
  let worker;
  const start = async () => {
    worker = new WriterCheckpointWorker({ journal: new CheckpointJournal(directory), transport,
      now: () => clock, monotonicNow: () => monotonic, reserveAu: '0', log: () => {} });
    await worker.initialize();
    worker.schedule();
    return worker;
  };
  t.after(async () => { await worker?.stop(); fs.rmSync(directory, { recursive: true, force: true }); });
  await start();
  return { directory, transport, paid, applied, snapshots, start,
    get worker() { return worker; }, get preparations() { return preparations; },
    get broadcasts() { return broadcasts; },
    advance(ms) { clock += ms; monotonic += Math.abs(ms); worker.schedule(); },
  };
}

test('zero traffic produces a distinct paid checkpoint in every thirty-second slot', async (t) => {
  const ctx = await harness(t);
  for (let i = 0; i < 60; i++) {
    if (i) ctx.advance(30000);
    await ctx.worker.step();
    await ctx.worker.step();
  }
  assert.equal(ctx.paid.size, 60);
  assert.equal(ctx.applied.size, 60);
  assert.equal(ctx.worker.status().backlog_slots, 0);
});

test('lost ACK after fee deduction and process restart reconcile the original paid hash', async (t) => {
  const ctx = await harness(t);
  ctx.transport.loseAck = true;
  await ctx.worker.step();
  assert.equal(ctx.paid.size, 1);
  const persisted = JSON.parse(fs.readFileSync(path.join(ctx.directory, 'state.json'), 'utf8'));
  assert.equal(persisted.active.phase, 'BROADCAST_OR_UNCERTAIN');
  await ctx.worker.stop();
  await ctx.start();
  await ctx.worker.step();
  assert.equal(ctx.preparations, 1);
  assert.equal(ctx.broadcasts, 1);
  assert.equal(ctx.paid.size, 1);
  assert.equal(ctx.applied.size, 1);
});

test('MSB-paid checkpoint missing from subnet retries only proof application', async (t) => {
  const ctx = await harness(t);
  ctx.transport.delayApply = true;
  await ctx.worker.step();
  for (let i = 0; i < 5; i++) { ctx.advance(30000); await ctx.worker.step(); }
  assert.equal(ctx.worker.status().backlog_slots, 6);
  assert.equal(ctx.worker.status().active_phase, 'MSB_CONFIRMED');
  assert.equal(ctx.broadcasts, 1);
  ctx.transport.delayApply = false;
  await ctx.worker.step();
  assert.equal(ctx.applied.size, 1);
  assert.equal(ctx.preparations, 1);
});

test('scheduler persists new due slots while an earlier broadcast is pending', async (t) => {
  const ctx = await harness(t);
  let finish;
  let entered;
  const broadcastEntered = new Promise((resolve) => { entered = resolve; });
  ctx.transport.broadcast = async () => { entered(); await new Promise((resolve) => { finish = resolve; }); };
  const ongoing = ctx.worker.step();
  await broadcastEntered;
  ctx.advance(120000);
  await ctx.worker.step(); // No parallel spending worker.
  assert.equal(ctx.worker.status().backlog_slots, 5);
  assert.equal(ctx.preparations, 1);
  assert.equal(JSON.parse(fs.readFileSync(path.join(ctx.directory, 'state.json'))).due_through, 104);
  finish();
  await ongoing;
});

test('funding exhaustion retains missed slots and recovers with honest late observations', async (t) => {
  const ctx = await harness(t);
  ctx.transport.enoughFunds = false;
  await ctx.worker.step();
  ctx.advance(60000);
  await ctx.worker.step();
  assert.equal(ctx.preparations, 0);
  assert.equal(ctx.worker.status().backlog_slots, 3);
  ctx.transport.enoughFunds = true;
  await ctx.worker.step();
  assert.equal(ctx.snapshots.get(100).observed_at, 3060);
  await ctx.worker.step();
  assert.equal(ctx.paid.size, 1);
});

test('changed validator context never causes a blind second fee-paying preparation', async (t) => {
  const ctx = await harness(t);
  ctx.transport.contextChanged = true;
  await ctx.worker.step();
  for (let i = 0; i < 5; i++) { ctx.advance(30000); await ctx.worker.step(); }
  assert.equal(ctx.preparations, 1);
  assert.equal(ctx.broadcasts, 0);
  assert.equal(ctx.worker.status().status, 'awaiting_context_reconciliation');
  ctx.transport.contextChanged = false;
  await ctx.worker.step();
  await ctx.worker.step();
  assert.equal(ctx.paid.size, 1);
  assert.equal(ctx.applied.size, 1);
});

test('journal refuses a second spending lock and malformed state without deleting history', async (t) => {
  const ctx = await harness(t);
  assert.throws(() => new CheckpointJournal(ctx.directory), /spending lock/);
  await ctx.worker.stop();
  fs.writeFileSync(path.join(ctx.directory, 'state.json'), '{broken');
  assert.throws(() => new CheckpointJournal(ctx.directory));
  assert.equal(fs.readFileSync(path.join(ctx.directory, 'state.json'), 'utf8'), '{broken');
});

test('confirmed non-execution permits a durable context replacement under the same slot', async (t) => {
  const ctx = await harness(t);
  const inspect = ctx.transport.inspect;
  let retired = false;
  ctx.transport.inspect = async (prepared) => {
    if (!retired) {
      retired = true;
      return { confirmed: false, context_changed: true, replacement_evidence: {
        type: 'unexecuted_at_signed_context_change', tx: prepared.surrogate.tx,
        previous_txv: prepared.surrogate.txv,
      } };
    }
    return inspect(prepared);
  };
  await ctx.worker.step();
  assert.equal(ctx.preparations, 2);
  assert.equal(ctx.broadcasts, 0);
  assert.equal(ctx.worker.journal.state.active.retired_attempts.length, 1);
  assert.equal(ctx.worker.journal.state.active.slot, 100);
  await ctx.worker.stop();
  await ctx.start();
  await ctx.worker.step();
  await ctx.worker.step();
  assert.equal(ctx.preparations, 2);
  assert.equal(ctx.paid.size, 1);
  assert.equal(ctx.applied.size, 1);
});

test('crash after preparation recovers exact signed bytes and does not prepare a fresh payment', async (t) => {
  const ctx = await harness(t);
  const inspect = ctx.transport.inspect;
  ctx.transport.inspect = async () => { throw new Error('process interrupted before broadcast'); };
  await ctx.worker.step();
  const prepared = ctx.worker.journal.state.active.prepared;
  assert.equal(ctx.broadcasts, 0);
  await ctx.worker.stop();
  ctx.transport.inspect = inspect;
  await ctx.start();
  await ctx.worker.step();
  assert.deepEqual(ctx.worker.journal.state.active.prepared, prepared);
  await ctx.worker.step();
  assert.equal(ctx.preparations, 1);
  assert.equal(ctx.paid.size, 1);
});

test('wall-clock rollback never reschedules completed slots', async (t) => {
  const ctx = await harness(t);
  await ctx.worker.step();
  await ctx.worker.step();
  ctx.advance(-60000);
  await ctx.worker.step();
  assert.equal(ctx.paid.size, 1);
  ctx.advance(90000);
  await ctx.worker.step();
  await ctx.worker.step();
  assert.equal(ctx.applied.size, 2);
  assert.deepEqual([...ctx.applied.keys()], [100, 101]);
});

test('journal failure before broadcast stops spending and retains the prepared attempt for restart', async (t) => {
  const ctx = await harness(t);
  const save = ctx.worker.journal.save.bind(ctx.worker.journal);
  let diskFull = false;
  ctx.worker.journal.save = (state) => {
    if (state.active?.phase === 'BROADCAST_OR_UNCERTAIN') diskFull = true;
    if (diskFull) throw new Error('ENOSPC: journal disk full');
    save(state);
  };
  await assert.rejects(ctx.worker.step(), /ENOSPC/);
  assert.equal(ctx.broadcasts, 0);
  const prepared = ctx.worker.journal.state.active.prepared;
  assert.equal(ctx.worker.journal.state.active.phase, 'PREPARED');
  await ctx.worker.stop();
  await ctx.start();
  await ctx.worker.step();
  assert.deepEqual(ctx.worker.journal.state.active.prepared, prepared);
  await ctx.worker.step();
  assert.equal(ctx.preparations, 1);
  assert.equal(ctx.paid.size, 1);
});
