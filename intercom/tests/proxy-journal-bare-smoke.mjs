// Run with `bare intercom/tests/proxy-journal-bare-smoke.mjs` to exercise the
// actual production filesystem/lock bindings, independently of Node's fs shim.
import fs from 'fs';
import path from 'path';
import os from 'bare-os';
import { ProxyPublicationJournal } from '../features/mayhem/proxy-publication-journal.js';

const check = (condition, message) => { if (!condition) throw new Error(message); };
const directory = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'mayhem-proxy-journal-bare-'));
const identity = { admin: 'ab'.repeat(32), subnet: 'ac'.repeat(32) };
let journal;
try {
  journal = await ProxyPublicationJournal.open({ directory, identity });
  const entry = {
    key: 'proxy/test/durable', envelope: { op: 'proxy_registry' },
    nonce: 'ad'.repeat(32), hash: 'ae'.repeat(64), result_key: `fr/${'ae'.repeat(64)}`,
    scope: `provider:${'af'.repeat(32)}`, created_at: 1,
    source: { writer_key: 'bc'.repeat(32), fork: 0, length: 3, checked_length: 3, found_index: null },
  };
  await journal.put(entry);
  let refused = false;
  try { await ProxyPublicationJournal.open({ directory, identity }); } catch { refused = true; }
  check(refused, 'duplicate writer lock was accepted');
  await journal.close();
  journal = await ProxyPublicationJournal.open({ directory, identity });
  check(journal.get(entry.key)?.hash === entry.hash, 'durable intent was not recovered');
  await journal.remove(entry.key);
  await journal.close();
  journal = await ProxyPublicationJournal.open({ directory, identity });
  check(journal.list().length === 0, 'completed entry reappeared');
  console.log('Bare publication journal: write, fsync, rename, lock, reopen and deletion passed.');
} finally {
  await journal?.close();
  await fs.promises.rm(directory, { recursive: true, force: true });
}
