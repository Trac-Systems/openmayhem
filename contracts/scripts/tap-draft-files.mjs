import assert from 'node:assert/strict';
import { createHash, createPublicKey, randomUUID, verify } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const digest = bytes => createHash('sha256').update(bytes).digest('hex');

// A direct CLI replay takes the same lock as the service. Reuse the shell's
// open file description when supplied, so its child cannot deadlock its parent.
export function holdTapLock(file, inherited) {
  const fd = fs.openSync(file, 'a', 0o600);
  const target = fs.fstatSync(fd);
  assert(target.isFile(), 'Invalid TAP attempt lock');
  let fromShell;
  try { fromShell = fs.fstatSync(inherited); } catch { /* direct CLI */ }
  const shared = fd !== inherited && fromShell?.dev === target.dev && fromShell.ino === target.ino;
  const lock = shared ? inherited : fd;
  const result = spawnSync('python3', ['-c', 'import fcntl\nfcntl.flock(3, fcntl.LOCK_EX | fcntl.LOCK_NB)'],
    { stdio: ['ignore', 'ignore', 'ignore', lock], timeout: 10_000 });
  if (shared || result.status !== 0) fs.closeSync(fd);
  assert(result.status === 0, 'TAP attempt lock is busy or unavailable');
  // Keep the acquired descriptor open until this one-shot process exits.
}

export function durableWrite(file, obj, replace) {
  const temporary = file + '.' + randomUUID() + '.tmp';
  const fd = fs.openSync(temporary, 'wx', 0o600);
  try {
    try {
      fs.writeFileSync(fd, JSON.stringify(obj, null, 2) + '\n');
      fs.fsyncSync(fd);
    } finally { fs.closeSync(fd); }
    if (replace) fs.renameSync(temporary, file);
    else fs.linkSync(temporary, file);
    syncDirectory(path.dirname(file));
  } finally { fs.rmSync(temporary, { force: true }); }
}

export function syncDirectory(directory) {
  const fd = fs.openSync(directory, 'r');
  try { fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
}

export function readDraftFile(file) {
  const stat = fs.lstatSync(file);
  assert(stat.isFile() && stat.size <= 16 * 1024 * 1024, 'Invalid or oversized TAP draft file');
  return fs.readFileSync(file);
}

// Reports may contain proofs for every historical claimant and bundles may
// contain many receipts. Recovery needs only their financial header fields.
export function readDraftHeader(file, keys, receipts = false) {
  const stat = fs.lstatSync(file);
  assert(stat.isFile(), 'Invalid TAP evidence file');
  if (stat.size <= 16 * 1024 * 1024) {
    const raw = fs.readFileSync(file, 'utf8').trim();
    return raw ? JSON.parse(raw) : null;
  }
  const filter = `reduce inputs as $event ({value:{}, bytes:0, done:false};
    ($event[0]) as $p | if .done then error("extra document")
    elif ($event|length) == 1 then .done = (($p|length) == 1)
    elif ($p|length) == 0 then if $event[1] == {} then .done = true else error("invalid document") end
    elif ($p[0]|type) != "string" then error("invalid document")
    elif $receipts and $p[0] == "receipts" then
      if ($p[1]|type) == "number" or (($p|length) == 1 and $event[1] == [])
      then .value.receipts = [] else error("invalid receipt list") end
    elif ($keys | index($p[0])) != null then
      .bytes += ($event | tojson | utf8bytelength) |
      if .bytes > 16777216 then error("oversized financial header") else .value |= setpath($p; $event[1]) end
    else . end) | if .done then .value else error("incomplete document") end`;
  const projected = spawnSync('jq', ['-cn', '--stream', '--argjson', 'keys', JSON.stringify(keys),
    '--argjson', 'receipts', String(receipts), filter, file], { encoding: 'utf8', timeout: 60_000, maxBuffer: 16 * 1024 * 1024 });
  assert(projected.status === 0, 'Invalid or oversized TAP financial header');
  return JSON.parse(projected.stdout);
}

export function digestFile(file) {
  assert(fs.lstatSync(file).isFile(), 'Invalid TAP evidence file');
  const fd = fs.openSync(file, 'r');
  try {
    const hasher = createHash('sha256');
    const block = Buffer.allocUnsafe(64 * 1024);
    const size = fs.fstatSync(fd).size;
    for (let offset = 0; offset < size;) {
      const n = fs.readSync(fd, block, 0, Math.min(block.length, size - offset), offset);
      assert(n > 0, 'TAP evidence was truncated');
      hasher.update(block.subarray(0, n));
      offset += n;
    }
    assert.equal(fs.fstatSync(fd).size, size, 'TAP evidence changed size');
    return hasher.digest('hex');
  } finally { fs.closeSync(fd); }
}

export function handoffFile(spool) { return path.join(spool, 'deferred/next-sweep.json'); }

// The fence ticket is committed before its working file disappears. After a
// crash only the publisher may finish that exact move, even for a legacy job.
export function reservedHandoffTicket(spool, epoch, anchor) {
  const file = handoffFile(spool);
  if (!fs.existsSync(file)) return null;
  const ticket = JSON.parse(readDraftFile(file));
  return ticket.reserved === true && ticket.epoch === epoch && ticket.anchor === anchor ? ticket : null;
}

export function readHandoff(spool, cursor, firstArchive) {
  if (!fs.lstatSync(handoffFile(spool), { throwIfNoEntry: false })) {
    const folder = path.dirname(handoffFile(spool));
    if (!fs.existsSync(folder)) return null;
    // Only the initial archive can legitimately exist without a ticket. Allow
    // its publisher to resume a crash between the two durable writes.
    const allowed = new Set(firstArchive ? [firstArchive + '.draft.json', firstArchive + '.receipts.json'] : []);
    const directory = fs.opendirSync(folder);
    try {
      for (let entry, count = 0; (entry = directory.readSync()); ) {
        assert(++count <= 32, 'Ambiguous initial TAP handoff');
        const scratch = entry.name.replace(/\.[\da-f-]{36}\.tmp$/, '');
        if (firstArchive && scratch !== entry.name && entry.isFile() &&
            (scratch === firstArchive + '.draft.json' || scratch === 'next-sweep.json')) continue;
        assert(allowed.delete(entry.name) && entry.isFile(), 'Missing TAP draft handoff');
      }
    } finally { directory.closeSync(); }
    return null;
  }
  const ticket = JSON.parse(readDraftFile(handoffFile(spool)));
  assert(Number.isSafeInteger(ticket.epoch) && ticket.epoch > 0 &&
    /^[0-9a-f]{64}$/.test(ticket.anchor) && /^[0-9a-f]{64}$/.test(ticket.proof_sha256), 'Invalid TAP draft handoff');
  if (cursor && cursor.epoch > ticket.epoch) return null;
  const prefix = path.join(spool, 'deferred', 'epoch-' + ticket.epoch + '-' + ticket.anchor);
  const raw = readDraftFile(prefix + '.draft.json');
  const proof = JSON.parse(raw);
  assert(digest(raw) === ticket.proof_sha256 && proof.epoch === ticket.epoch &&
    proof.epoch_apply_hash === ticket.anchor && ['unprepared_tap_sweep', 'reserved_tap_sweep'].includes(proof.type) &&
    ticket.reserved === (proof.type === 'reserved_tap_sweep' ? true : undefined) &&
    proof.bundle_sha256 === digestFile(prefix + '.receipts.json') &&
    Array.isArray(proof.refunds), 'TAP handoff differs from archived draft');
  assert.deepEqual(proof.completed_cursor, cursor, 'TAP handoff has a different completed predecessor');
  return { ticket, proof };
}

// The link and directory fsync precede the first possible request. An
// interrupted POST remains bound even while Core still reports a null key.
export function journalTapPreparation(file, plan, draft = false) {
  const intent = { epoch: plan.intent.epoch, epoch_apply_hash: plan.intent.epoch_apply_hash,
    root_preparation_id: plan.root_preparation_id, preparation_ids: plan.preparation_ids,
    external_effect_ids: plan.external_effect_ids };
  if (!fs.existsSync(file)) durableWrite(file, intent, false);
  else {
    const previous = JSON.parse(readDraftFile(file));
    if (draft && previous.root_preparation_id !== intent.root_preparation_id) durableWrite(file, intent, true);
    else assert.deepEqual(previous, intent, 'TAP submission journal differs from preparation');
  }
}

export function tapPlanSource({ intent, preparation_ids, root_preparation_id, external_effect_ids }) {
  return { intent, preparation_ids, root_preparation_id, external_effect_ids };
}

const sourceMessage = value => 'mayhem-tap-worker-source-v1/' + digest(JSON.stringify(value));

export function verifyTapSourceSeal(seal, admin) {
  const { evidence, signature } = seal;
  assert(/^[0-9a-f]{64}$/.test(admin) && /^[0-9a-f]{128}$/.test(signature), 'Invalid TAP source signer');
  const publicKey = createPublicKey({ key: Buffer.from('302a300506032b6570032100' + admin, 'hex'), type: 'spki', format: 'der' });
  assert(verify(null, Buffer.from(sourceMessage(evidence)), publicKey, Buffer.from(signature, 'hex')),
    'Invalid TAP worker source signature');
  return evidence;
}

export async function sealTapSource(prefix, plan, openingCheckpoint, selectionBlock, signer) {
  const file = prefix + '.sealed-plan.json';
  const source = tapPlanSource(plan);
  if (fs.existsSync(file)) {
    const prior = verifyTapSourceSeal(JSON.parse(readDraftFile(file)), signer.publicKey);
    assert.deepEqual(prior.source, source, 'Conflicting TAP worker source');
    assert.deepEqual(prior.opening_checkpoint, openingCheckpoint);
    return;
  }
  assert(Number.isSafeInteger(selectionBlock?.number) && selectionBlock.number >= 0 &&
    /^0x[0-9a-f]{64}$/.test(selectionBlock.hash) && Number.isSafeInteger(selectionBlock.timestamp),
    'Missing TAP financial selection block');
  const evidence = { source, opening_checkpoint: openingCheckpoint, selection_block: selectionBlock };
  const seal = { evidence, signature: await signer.sign(sourceMessage(evidence)) };
  verifyTapSourceSeal(seal, signer.publicKey);
  durableWrite(file, seal, false);
}

// Each source is bound to a failed draft or a reconciled page's unpaid inputs.
// A successor takes a bounded slice from the oldest source. Repeated failures
// carry the position and never concatenate the history of refund inputs.
export function refundQueue(cursor, handoff) {
  const q = handoff?.proof.refund_queue ?? cursor?.refund_queue ?? { head: 0, offset: 0, tail: 0 };
  assert([q.head, q.offset, q.tail].every(n => Number.isSafeInteger(n) && n >= 0) &&
    q.head <= q.tail && (q.head < q.tail || q.offset === 0) &&
    (q.reconciled_epoch === undefined || (Number.isSafeInteger(q.reconciled_epoch) && q.reconciled_epoch >= 0)), 'Invalid TAP refund queue');
  return q;
}

export function refundSourceFile(spool, index) {
  return path.join(spool, 'processed', 'refund-source-' + index + '.json');
}

function immutableJson(file, value) {
  if (fs.lstatSync(file, { throwIfNoEntry: false })) assert.deepEqual(JSON.parse(readDraftFile(file)), value);
  else durableWrite(file, value, false);
}

export function retainRefundRemainder(spool, prefix, record, refunds, queue) {
  if (!refunds.length) return queue;
  const tail = queue.tail + 1;
  assert(Number.isSafeInteger(tail));
  const proof = { type: 'unpaid_tap_refunds', epoch: record.epoch, epoch_apply_hash: record.epoch_apply_hash,
    bundle_sha256: digestFile(prefix + '.receipts.json'), report_sha256: record.report_sha256,
    refund_index: tail, refunds };
  const file = prefix + '.unpaid-refunds.json';
  immutableJson(file, proof);
  immutableJson(refundSourceFile(spool, tail), { epoch: record.epoch, anchor: record.epoch_apply_hash,
    remainder: true, proof_sha256: digestFile(file) });
  return { ...queue, tail };
}

const debtIndex = fileURLToPath(new URL('./tap-debt-index.py', import.meta.url));

export function indexedDebt(root, action, name, ticket) {
  const result = spawnSync('python3', [debtIndex, action, path.join(root, 'pending.sqlite3'), name], {
    input: ticket === undefined ? '' : JSON.stringify(ticket), encoding: 'utf8', timeout: 10_000, maxBuffer: 4096,
  });
  assert(result.status === 0, 'TAP debt index operation failed: ' + action);
  return JSON.parse(result.stdout);
}

export function registerFencedDebt(spool, name, ticket) {
  assert(ticket.reserved && /^[0-9a-f]{64}$/.test(ticket.anchor) && Number.isSafeInteger(ticket.epoch) &&
    name === 'epoch-' + ticket.epoch + '-' + ticket.anchor);
  const root = path.join(spool, 'debt');
  fs.mkdirSync(root, { recursive: true, mode: 0o700 });
  syncDirectory(spool);
  syncDirectory(root);
  if (!fs.existsSync(path.join(root, name + '.settled.json'))) indexedDebt(root, 'put', name, ticket);
}

export function nextRefunds(spool, q) {
  if (q.head === q.tail) return null;
  const index = q.head + 1;
  const ticket = JSON.parse(readDraftFile(refundSourceFile(spool, index)));
  assert(Number.isSafeInteger(ticket.epoch) && ticket.epoch > 0 && /^[0-9a-f]{64}$/.test(ticket.anchor));
  assert(ticket.remainder === undefined || ticket.remainder === true);
  const prefix = path.join(spool, ticket.remainder ? 'processed' : 'deferred', 'epoch-' + ticket.epoch + '-' + ticket.anchor);
  const raw = readDraftFile(prefix + (ticket.remainder ? '.unpaid-refunds.json' : '.draft.json'));
  const source = JSON.parse(raw);
  assert(digest(raw) === ticket.proof_sha256 &&
    (ticket.remainder ? source.type === 'unpaid_tap_refunds' && source.refund_index === index &&
      source.report_sha256 === digestFile(prefix + '.settlement.json') :
      ['unprepared_tap_sweep', 'reserved_tap_sweep'].includes(source.type) &&
      ticket.reserved === (source.type === 'reserved_tap_sweep' ? true : undefined) && source.refund_queue.tail === index) &&
    source.epoch === ticket.epoch && source.epoch_apply_hash === ticket.anchor &&
    source.bundle_sha256 === digestFile(prefix + '.receipts.json') &&
    Array.isArray(source.refunds) && q.offset < source.refunds.length,
  'Invalid TAP refund source');
  const reserved = source.type === 'reserved_tap_sweep';
  let rows = source.refunds, recovered;
  let reservedRemainder;
  if (reserved && Array.isArray(source.reserved_refund_credits)) {
    assert(source.companion_sha256?.['.sealed-plan.json'] === digestFile(prefix + '.sealed-plan.json') &&
      /^[0-9a-f]{64}$/.test(source.debt_evidence?.source.root_preparation_id), 'Missing signed refund assignment');
    reservedRemainder = source.debt_evidence.source.root_preparation_id;
  }
  const name = 'epoch-' + ticket.epoch + '-' + ticket.anchor;
  const closed = path.join(spool, 'debt', name + '.settled.json');
  if (reserved && fs.existsSync(closed)) {
    const marker = JSON.parse(readDraftFile(closed));
    if (marker.refund) {
      assert(marker.refund.index === index && Array.isArray(marker.refund.unpaid) &&
        marker.fence_sha256 === ticket.proof_sha256, 'Conflicting TAP refund recovery');
      if (marker.checkpoint_sha256) {
        const file = path.join(spool, 'debt', name + '.confirmed-checkpoint.json');
        assert(digestFile(file) === marker.checkpoint_sha256 && JSON.parse(readDraftFile(file)).root_preparation_id === marker.checkpoint,
          'Conflicting reconciled historical refund evidence');
      } else {
        const paid = path.join(spool, 'processed', 'epoch-' + marker.epoch + '-' + marker.anchor + '.debt-settlement.json');
        assert(digestFile(paid) === marker.report_sha256, 'Recovered refund checkpoint changed');
      }
      recovered = marker.checkpoint;
      rows = marker.refund.unpaid;
      assert(q.offset === 0 || q.offset < rows.length);
    }
  }
  if (reserved && !reservedRemainder && !recovered && !(q.reconciled_epoch > source.epoch)) return null;
  const entries = rows.slice(q.offset, q.offset + 32);
  const end = entries.length + q.offset;
  return { entries, source: { index, offset: q.offset, count: entries.length, ...ticket, ...(reservedRemainder && { reserved_remainder: reservedRemainder }), ...(recovered && { recovered }) },
    next: end === rows.length ? { ...q, head: index, offset: 0 } : { ...q, offset: end } };
}
