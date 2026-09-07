import fs from 'fs';
import path from 'path';
import b4a from 'b4a';
import nativeFs from 'fs-native-extensions';

export const CHECKPOINT_SECONDS = 30;
const clone = (value) => JSON.parse(JSON.stringify(value));
const integer = (value) => Number.isSafeInteger(value) && value >= 0;

export function durableJson(file, value) {
  const directory = path.dirname(file);
  const temporary = `${file}.tmp`;
  const fd = fs.openSync(temporary, 'w', 0o600);
  try {
    // bare-fs writeFileSync accepts paths, not Node's file-descriptor overload.
    // Use the shared writeSync buffer API and handle short writes explicitly.
    const bytes = b4a.from(`${JSON.stringify(value)}\n`);
    let written = 0;
    while (written < bytes.length) {
      const count = fs.writeSync(fd, bytes, written, bytes.length - written, written);
      if (!Number.isSafeInteger(count) || count < 1) throw new Error('Checkpoint journal write made no progress.');
      written += count;
    }
    fs.fsyncSync(fd);
  } finally { fs.closeSync(fd); }
  fs.renameSync(temporary, file);
  const parent = fs.openSync(directory, 'r');
  try { fs.fsyncSync(parent); } finally { fs.closeSync(parent); }
}

export class CheckpointJournal {
  constructor(directory) {
    this.directory = directory;
    fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
    fs.mkdirSync(path.join(directory, 'completed'), { recursive: true, mode: 0o700 });
    this.lock = fs.openSync(path.join(directory, 'writer.lock'), 'a', 0o600);
    if (!nativeFs.tryLock(this.lock)) {
      fs.closeSync(this.lock);
      this.lock = null;
      throw new Error('Another checkpoint writer holds the spending lock.');
    }
    this.file = path.join(directory, 'state.json');
    try {
      this.state = fs.existsSync(this.file) ? JSON.parse(fs.readFileSync(this.file, 'utf8')) : null;
      if (this.state && (this.state.schema_version !== 1 ||
          !integer(this.state.start_slot) || this.state.start_slot < 1 ||
          !integer(this.state.next_slot) || this.state.next_slot < this.state.start_slot ||
          !integer(this.state.due_through) || this.state.due_through < this.state.next_slot - 1 ||
          (this.state.active && (this.state.active.slot !== this.state.next_slot ||
            !['DUE', 'PREPARED', 'BROADCAST_OR_UNCERTAIN', 'MSB_CONFIRMED'].includes(this.state.active.phase) ||
            (this.state.active.phase !== 'DUE' && !this.state.active.prepared))))) {
        throw new Error('Invalid checkpoint journal; refusing to reset payment history.');
      }
    } catch (error) { this.close(); throw error; }
  }

  save(state) {
    durableJson(this.file, state);
    this.state = clone(state);
  }

  complete(record) {
    durableJson(path.join(this.directory, 'completed', `${record.slot}.json`), record);
  }

  close() {
    if (this.lock !== null) { fs.closeSync(this.lock); this.lock = null; }
  }
}

// The scheduler records a compact contiguous range of due slot identities even
// while the independent submit/reconcile step is awaiting the network.
export class WriterCheckpointWorker {
  constructor({ journal, transport, now = () => Date.now(), reserveAu = '5000000000000000000',
    monotonicNow = () => globalThis.performance.now(), catchupSpacingMs = 5000, log = console.log }) {
    this.journal = journal;
    this.transport = transport;
    this.now = now;
    this.monotonicNow = monotonicNow;
    this.reserveAu = BigInt(reserveAu);
    if (this.reserveAu < 0n) throw new Error('Checkpoint reserve must be non-negative.');
    if (!integer(catchupSpacingMs) || catchupSpacingMs < 1000) throw new Error('Invalid catch-up spacing.');
    this.catchupSpacingMs = catchupSpacingMs;
    this.log = log;
    this.running = null;
    this.stopped = false;
    this.nextBroadcastAt = 0;
    this.timer = null;
  }

  async initialize() {
    const identity = await this.transport.identity();
    if (this.journal.state) {
      if (JSON.stringify(this.journal.state.identity) !== JSON.stringify(identity)) {
        throw new Error('Checkpoint journal belongs to a different wallet or network.');
      }
      return;
    }
    const { current, preparing } = await this.transport.history();
    if (preparing && preparing.slot > (current?.slot ?? 0)) {
      throw new Error('An unfinished canonical checkpoint has no local payment journal; restore its journal before spending.');
    }
    const next = current ? current.slot + 1 : Math.floor(this.now() / 1000 / CHECKPOINT_SECONDS);
    this.journal.save({ schema_version: 1, identity, start_slot: next, next_slot: next,
      due_through: next - 1, active: null, last_completed: current ?? null,
      status: 'ready', error: null });
  }

  schedule() {
    if (this.stopped || !this.journal.state) return;
    const due = Math.floor(this.now() / 1000 / CHECKPOINT_SECONDS);
    if (due > this.journal.state.due_through) {
      this.journal.save({ ...this.journal.state, due_through: due });
    }
  }

  update(fields) { this.journal.save({ ...this.journal.state, ...fields }); }

  async step() {
    if (this.stopped || this.running) return;
    this.running = this.processSlot().catch((error) => {
      this.update({ status: 'degraded', error: String(error?.message ?? error), observed_at: this.now() });
      this.log('Writer checkpoint delayed:', error?.message ?? error);
    });
    try { await this.running; } finally { this.running = null; }
  }

  async processSlot() {
    const state = this.journal.state;
    if (!state || state.next_slot > state.due_through) return;
    let active = state.active ?? { slot: state.next_slot, phase: 'DUE', prepared: null, proof: null };
    if (!state.active) this.update({ active, status: 'working', error: null });
    if (!active.prepared) {
      const funding = await this.transport.funding();
      if (BigInt(funding.balance_au) < BigInt(funding.fee_au) + this.reserveAu) {
        this.update({ status: 'awaiting_funding', funding, error: null });
        return;
      }
      const snapshot = await this.transport.prepareSnapshot(active.slot, Math.floor(this.now() / 1000));
      const prepared = await this.transport.preparePayment(snapshot);
      active = { ...active, phase: 'PREPARED', snapshot_hash: snapshot.snapshot_hash,
        prepared, expected_fee_au: funding.fee_au, prepared_at: this.now() };
      // The exact signed MSB transaction is durable BEFORE the first broadcast.
      this.update({ active, funding, status: 'working', error: null });
    }
    const inspection = await this.transport.inspect(active.prepared, active.expected_fee_au);
    if (inspection.confirmed) {
      active = { ...active, phase: 'MSB_CONFIRMED', proof: inspection.proof };
      this.update({ active, status: 'reconciling', error: null });
      const checkpoint = await this.transport.reconcileSubnet(active.prepared, inspection.proof);
      if (!checkpoint) return;
      const completed = { ...active, phase: 'COMPLETE', checkpoint, completed_at: this.now() };
      this.journal.complete(completed);
      this.update({ active: null, next_slot: active.slot + 1, last_completed: checkpoint,
        status: 'ready', error: null, last_completed_at: completed.completed_at });
      this.log('Writer checkpoint confirmed:', JSON.stringify({ slot: active.slot,
        tx: checkpoint.tx, fee_au: active.expected_fee_au, msb_signed_length: inspection.proof.signed_length }));
      return;
    }
    if (inspection.context_changed) {
      const evidence = inspection.replacement_evidence;
      if (evidence?.type === 'unexecuted_at_signed_context_change' &&
          evidence.tx === active.prepared.surrogate.tx &&
          evidence.previous_txv === active.prepared.surrogate.txv) {
        // Preserve every retired attempt and its authoritative non-execution
        // evidence. The new preparation still belongs to this same logical slot.
        const snapshot = await this.transport.prepareSnapshot(active.slot, Math.floor(this.now() / 1000));
        const replacement = await this.transport.preparePayment(snapshot);
        active = { ...active, phase: 'PREPARED', prepared: replacement, proof: null,
          broadcast_attempts: 0, last_broadcast_at: null, prepared_at: this.now(),
          retired_attempts: [...(active.retired_attempts ?? []), {
            prepared: active.prepared, broadcast_attempts: active.broadcast_attempts ?? 0,
            nonexecution_evidence: evidence,
          }] };
        this.update({ active, status: 'working', error: null });
        return;
      }
      // An unsigned/live context mismatch remains an uncertain outcome.
      this.update({ status: 'awaiting_context_reconciliation', error: null });
      return;
    }
    const funding = await this.transport.funding();
    if (BigInt(funding.fee_au) !== BigInt(active.expected_fee_au)) {
      throw new Error('MSB fee changed after payment preparation.');
    }
    if (BigInt(funding.balance_au) < BigInt(active.expected_fee_au) + this.reserveAu) {
      this.update({ status: 'awaiting_funding', funding, error: null });
      return;
    }
    if (this.stopped || this.monotonicNow() < this.nextBroadcastAt) return;
    this.nextBroadcastAt = this.monotonicNow() + this.catchupSpacingMs;
    active = { ...active, phase: 'BROADCAST_OR_UNCERTAIN', last_broadcast_at: this.now(),
      broadcast_attempts: (active.broadcast_attempts ?? 0) + 1 };
    this.update({ active, funding, status: 'awaiting_confirmation', error: null });
    await this.transport.broadcast(active.prepared);
  }

  async start() {
    await this.initialize();
    this.schedule();
    this.timer = setInterval(() => {
      try { this.schedule(); } catch (error) {
        // Losing durability must stop spending, including after a disk fills.
        this.stopped = true;
        this.log('Writer checkpoint scheduler stopped:', error.message);
        return;
      }
      this.step().catch((error) => { this.stopped = true; this.log('Writer checkpoint stopped:', error.message); });
    }, 1000);
  }

  async stop() {
    this.stopped = true;
    clearInterval(this.timer);
    try { if (this.running) await this.running; }
    finally { this.journal.close(); }
  }

  status() {
    const state = this.journal.state;
    return state ? { enabled: true, status: this.stopped ? 'stopped' : state.status,
      scheduled_through: state.due_through, next_slot: state.next_slot,
      backlog_slots: Math.max(0, state.due_through - state.next_slot + 1),
      active_phase: state.active?.phase ?? null, active_tx: state.active?.prepared?.surrogate?.tx ?? null,
      last_completed: state.last_completed, last_completed_at: state.last_completed_at ?? null,
      funding: state.funding ?? null, error: state.error } : { enabled: true, status: 'initializing' };
  }
}
