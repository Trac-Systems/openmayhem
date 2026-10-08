import fs from 'fs';
import path from 'path';
import b4a from 'b4a';
import nativeFs from 'fs-native-extensions';
import { PROXY_MAX_RECORD_BYTES, isProxyPublication } from '../../contract/proxy-protocol.js';

const clone = value => JSON.parse(JSON.stringify(value));
const stable = value => JSON.stringify(value, (_, item) => item && typeof item === 'object' && !Array.isArray(item)
  ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
const fail = message => { throw new Error(`Proxy publication journal: ${message}.`); };
const hex = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const integer = value => Number.isSafeInteger(value) && value >= 0;

// Publication-only bounds, not catalog limits. No completed history is retained:
// the canonical registry/fr records provide durable completion deduplication.
export const PROXY_PENDING_DEFAULT_MAX = 256;
const MAX_ENTRY_BYTES = PROXY_MAX_RECORD_BYTES + 72_000;

export function validateProxyPublicationFences(fences) {
  if (!fences || Object.keys(fences).sort().join('|') !== 'reads|writes') fail('invalid publication fences');
  for (const keys of [fences.reads, fences.writes]) {
    if (!Array.isArray(keys) || keys.length > 128 || keys.some((key, index) =>
      typeof key !== 'string' || !key.length || key.length > 256 || (index && keys[index - 1] >= key))) {
      fail('invalid publication dependency keys');
    }
  }
}

export function proxyPublicationsConflict(a, b) {
  if (a.scope === b.scope) return true;
  // Old local journal entries had only a provider fence. Recover these first.
  if (!a.fences || !b.fences) return Boolean(a.fences || b.fences);
  const aw = new Set(a.fences.writes), bw = new Set(b.fences.writes);
  return b.fences.reads.some(key => aw.has(key)) || a.fences.reads.some(key => bw.has(key))
    || b.fences.writes.some(key => aw.has(key));
}

export function validateProxyPendingEntry(entry) {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry) ||
      JSON.stringify(Object.keys(entry).sort()) !== JSON.stringify([
        'created_at', 'envelope', ...(Object.hasOwn(entry, 'fences') ? ['fences'] : []), 'hash', 'key', 'nonce', 'result_key', 'scope', 'source',
      ]) || typeof entry.key !== 'string' || entry.key.length > 256 || !entry.key.startsWith('proxy/') ||
      typeof entry.scope !== 'string' || entry.scope.length > 96 ||
      !hex(entry.nonce) || !/^[0-9a-f]{128}$/.test(entry.hash) ||
      entry.result_key !== `fr/${entry.hash}` || !integer(entry.created_at) ||
      !entry.source || !hex(entry.source.writer_key) || !integer(entry.source.fork) ||
      !integer(entry.source.length) ||
      !integer(entry.source.checked_length) || entry.source.checked_length < entry.source.length ||
      !(entry.source.found_index === null || (integer(entry.source.found_index) && entry.source.found_index >= entry.source.length &&
        entry.source.found_index < entry.source.checked_length)) ||
      JSON.stringify(Object.keys(entry.source).sort()) !== JSON.stringify(['checked_length', 'fork', 'found_index', 'length', 'writer_key']) ||
      !isProxyPublication(entry.envelope) ||
      b4a.byteLength(JSON.stringify(entry)) > MAX_ENTRY_BYTES) fail('invalid pending record');
  if (entry.fences) validateProxyPublicationFences(entry.fences);
  else if (['proxy_spend_reserve', 'proxy_record_usage'].includes(entry.envelope.op)) fail('financial publication fences are required');
}

// One small bounded file, exclusive OS lock, and asynchronous durable replacement.
// Failure after rename is deliberately ambiguous: poison this handle until reopen,
// preserving whatever reached disk instead of continuing from stale memory.
export class ProxyPublicationJournal {
  static async open({ directory, identity, maxEntries = PROXY_PENDING_DEFAULT_MAX }) {
    if (!path.isAbsolute(directory) || !integer(maxEntries) || maxEntries < 1 || maxEntries > 4096 ||
        !identity || b4a.byteLength(stable(identity)) > 4096) fail('invalid configuration');
    const journal = new ProxyPublicationJournal(directory, identity, maxEntries);
    try { await journal._open(); return journal; }
    catch (error) { await journal.close(); throw error; }
  }

  constructor(directory, identity, maxEntries) {
    this.directory = directory;
    this.file = path.join(directory, 'pending.json');
    this.identity = clone(identity);
    this.maxEntries = maxEntries;
    this.maxBytes = MAX_ENTRY_BYTES * maxEntries + 8192;
    this.entries = new Map();
    this.tail = Promise.resolve();
    this.queued = 0;
    this.lock = null;
    this.closed = false;
    this.poisoned = false;
  }

  async _open() {
    await fs.promises.mkdir(this.directory, { recursive: true, mode: 0o700 });
    this.lock = await fs.promises.open(path.join(this.directory, 'publication.lock'), 'a', 0o600);
    if (!nativeFs.tryLock(this.lock.fd)) fail('another process holds the writer lock');
    let file;
    try { file = await fs.promises.open(this.file, 'r'); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (file) {
      try {
        const info = await file.stat();
        if (!info.isFile() || info.size > this.maxBytes) fail('pending file exceeds its bound');
        // Read from this same descriptor so replacement cannot bypass the size check.
        const bytes = b4a.alloc(info.size);
        let offset = 0;
        while (offset < bytes.length) {
          const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset);
          if (!bytesRead) fail('pending file was truncated');
          offset += bytesRead;
        }
        const state = JSON.parse(b4a.toString(bytes));
        if (state.schema_version !== 1 || stable(state.identity) !== stable(this.identity) ||
            !Array.isArray(state.entries) || state.entries.length > this.maxEntries) fail('identity/schema mismatch');
        const scopes = new Set();
        for (const entry of state.entries) {
          validateProxyPendingEntry(entry);
          if (this.entries.has(entry.key) || scopes.has(entry.scope) ||
              [...this.entries.values()].some(other => proxyPublicationsConflict(entry, other))) fail('conflicting pending identity/dependencies');
          this.entries.set(entry.key, clone(entry)); scopes.add(entry.scope);
        }
      } finally { await file.close(); }
    } else {
      await this._save(this.entries);
    }
    // A crash before rename cannot have dispatched the not-yet-durable intent.
    // The old pending.json remains authoritative; do not replay a temporary file.
    await fs.promises.unlink(`${this.file}.tmp`).catch(error => { if (error.code !== 'ENOENT') throw error; });
  }

  get(key) { return this.entries.has(key) ? clone(this.entries.get(key)) : null; }
  list() { return [...this.entries.values()].map(clone); }

  async _save(entries) {
    const bytes = b4a.from(JSON.stringify({ schema_version: 1, identity: this.identity, entries: [...entries.values()] }));
    if (entries.size > this.maxEntries || bytes.length > this.maxBytes) fail('pending capacity reached');
    const temporary = `${this.file}.tmp`;
    const file = await fs.promises.open(temporary, 'w', 0o600);
    try {
      let offset = 0;
      while (offset < bytes.length) {
        const { bytesWritten } = await file.write(bytes, offset, bytes.length - offset, offset);
        if (!integer(bytesWritten) || !bytesWritten) fail('write made no progress');
        offset += bytesWritten;
      }
      await file.sync();
    } finally { await file.close(); }
    await fs.promises.rename(temporary, this.file);
    const directory = await fs.promises.open(this.directory, 'r');
    try { await directory.sync(); } finally { await directory.close(); }
  }

  async _mutate(change) {
    if (this.closed || this.poisoned) fail('closed or storage failure requires recovery');
    if (this.queued >= this.maxEntries) fail('write queue is full');
    this.queued++;
    const operation = this.tail.then(async () => {
      if (this.poisoned) fail('storage failure requires recovery');
      const next = new Map(this.entries);
      if (!change(next)) return;
      try { await this._save(next); }
      catch (error) { this.poisoned = true; throw error; }
      this.entries = next;
    });
    this.tail = operation.catch(() => {});
    try { await operation; } finally { this.queued--; }
  }

  async put(entry) {
    validateProxyPendingEntry(entry); entry = clone(entry);
    return await this._mutate(entries => {
      const previous = entries.get(entry.key);
      if (previous) {
        if (stable(previous) !== stable(entry)) fail('cannot replace a pending operation');
        return false;
      }
      if (entries.size >= this.maxEntries) fail('pending capacity reached');
      if ([...entries.values()].some(value => proxyPublicationsConflict(entry, value))) {
        fail('another operation in this provider/policy scope is pending or an accounting dependency is reserved; recover it before retrying');
      }
      entries.set(entry.key, entry); return true;
    });
  }

  async remove(key) { return await this._mutate(entries => entries.delete(key)); }

  async advance(key, source) {
    return await this._mutate(entries => {
      const previous = entries.get(key);
      if (!previous) fail('cannot advance an unknown operation');
      const next = { ...previous, source: clone(source) };
      validateProxyPendingEntry(next);
      if (source.writer_key !== previous.source.writer_key || source.fork !== previous.source.fork ||
          source.length !== previous.source.length || source.checked_length < previous.source.checked_length ||
          (previous.source.found_index !== null && source.found_index !== previous.source.found_index)) fail('source recovery regressed');
      if (stable(previous) === stable(next)) return false;
      entries.set(key, next); return true;
    });
  }

  async close() {
    this.closed = true;
    await this.tail;
    if (this.lock) { const lock = this.lock; this.lock = null; await lock.close(); }
  }
}

// Journal and adapter are trusted local process wiring, never request fields.
export class ProxyPublicationController {
  constructor({ journal, prepare, inspect, admit, append, result, maxInFlight = 16 }) {
    this.journal = journal;
    this.prepare = prepare; this.inspect = inspect; this.admit = admit;
    this.append = append; this.result = result;
    if (!integer(maxInFlight) || maxInFlight < 1 || maxInFlight > journal.maxEntries) fail('invalid in-flight bound');
    this.maxInFlight = maxInFlight;
    this.inFlight = new Map();
    this.closed = false;
    this.cursor = 0;
    this.stepping = false;
  }

  pending(entry, reason = 'awaiting_canonical_result') {
    return { ok: false, accepted: true, status: 'pending', feature: 'mayhem', key: entry.key,
      hash: entry.hash, result_key: entry.result_key, recovery_reason: reason,
      message: 'Proxy publication is retained for canonical recovery; retry this same operation to recover its outcome.' };
  }

  async submit(key, envelope) {
    if (this.closed) fail('controller is stopping');
    envelope = clone(envelope);
    const previous = this.inFlight.get(key);
    if (previous) {
      if (stable(previous.envelope) !== stable(envelope)) fail('pending operation envelope differs');
      return await previous.promise;
    }
    if (this.inFlight.size >= this.maxInFlight) fail('active publication capacity reached; retry the same operation');
    // Schedule after inserting the entry, before any asynchronous work can yield.
    const promise = Promise.resolve().then(() => this._submit(key, envelope));
    this.inFlight.set(key, { envelope, promise });
    try { return await promise; } finally { this.inFlight.delete(key); }
  }

  async _submit(key, envelope) {
    let entry = this.journal.get(key);
    if (entry) {
      if (stable(entry.envelope) !== stable(envelope)) fail('pending operation envelope differs');
      const evidence = await this.inspect(entry);
      if (evidence.source) { await this.journal.advance(key, evidence.source); entry = this.journal.get(key); }
      if (evidence.state === 'confirmed') {
        await this.journal.remove(key);
        return this.result(entry, evidence.result);
      }
      if (evidence.state !== 'absent') return this.pending(entry, evidence.reason);
    }
    let dispatchStarted = false;
    try {
      const response = await this.admit(key, envelope, async ({ fences } = {}) => {
        if (this.closed) fail('controller is stopping');
        if (!entry) {
          entry = await this.prepare(key, envelope, { fences });
          await this.journal.put(entry);
        }
        // Durable I/O may yield to a revocation or another registry operation.
        // Revalidate after persistence, immediately before dispatch.
        return await this.admit(key, entry.envelope, async ({ fences: refreshed } = {}) => {
          if (entry.fences && stable(refreshed) !== stable(entry.fences)) fail('admission dependencies changed; retry the same operation');
          if (this.closed) fail('controller is stopping');
          dispatchStarted = true;
          // Exactly the saved nonce and envelope; a retry never invents another hash.
          const response = await this.append(entry);
          const evidence = await this.inspect(entry);
          if (evidence.source) { await this.journal.advance(key, evidence.source); entry = this.journal.get(key); }
          if (evidence.state === 'confirmed') {
            await this.journal.remove(key);
            return this.result(entry, evidence.result);
          }
          if (evidence.state === 'absent' && response?.accepted === false && response?.status === 'rejected') {
            await this.journal.remove(key);
            return response;
          }
          return this.pending(entry, evidence.reason);
        });
      });
      if (!dispatchStarted && entry) await this.journal.remove(key); // Canonically applied duplicate.
      return response;
    } catch (error) {
      if (!dispatchStarted) {
        // A recovered operation proved absent before this admission was rejected.
        // Do not strand its provider behind an expired/revoked never-sent intent.
        if (entry && this.journal.get(key)) await this.journal.remove(key);
        throw error;
      }
      // Append/ACK/storage failure does not prove non-execution. Preserve intent.
      return this.pending(entry, this.journal.poisoned ? 'journal_storage_failure' : 'dispatch_outcome_unknown');
    }
  }

  async step(limit = Math.min(8, this.maxInFlight)) {
    if (this.closed || this.stepping || !integer(limit) || limit < 1 || limit > this.maxInFlight) return;
    this.stepping = true;
    const started = [];
    try {
      const entries = this.journal.list();
      if (!entries.length) return;
      for (let n = 0; n < Math.min(limit, entries.length); n++) {
        const entry = entries[(this.cursor + n) % entries.length];
        if (this.inFlight.has(entry.key)) continue;
        started.push(this.submit(entry.key, entry.envelope));
      }
      this.cursor = (this.cursor + Math.min(limit, entries.length)) % entries.length;
    } finally { this.stepping = false; }
    // A stalled append retains its in-flight fence but cannot prevent subsequent
    // timer rounds from inspecting other entries. submit enforces the shared bound.
    return await Promise.allSettled(started);
  }

  async close() {
    this.stop();
    await Promise.allSettled([...this.inFlight.values()].map(value => value.promise));
    await this.journal.close();
  }

  stop() { this.closed = true; clearInterval(this.timer); this.timer = null; }
}
