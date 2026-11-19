import assert from 'node:assert/strict';
import { ZeroHash } from 'ethers';
import { digest as sha, durableWrite, nextRefunds, readHandoff, refundQueue, retainRefundRemainder } from './tap-draft-files.mjs';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { cumulativeTapRefunds, hasTapRefunds, projectTapRefunds } from './tap-settlement-roller.mjs';
import { tapContractAt } from './tap-liability-sweep.mjs';

const hash32 = /^[0-9a-f]{64}$/;
const cursorKey = /^payout\/liability\/tap\/[0-9a-f]{64}\/[0-9a-f]{64}$/;
const positive = n => Number.isSafeInteger(n) && n > 0;
const validCursor = key => key === null || (typeof key === 'string' && cursorKey.test(key));
const load = file => JSON.parse(fs.readFileSync(file, 'utf8'));
const cursorFile = spool => path.join(spool, 'processed/liability-sweep-cursor.json');
const intentFile = dir => path.join(dir, 'tap-sweep-intent.json');

function refundBasis(spool) {
  const file = path.join(spool, 'processed/latest.json');
  if (!fs.existsSync(file)) return null;
  const prior = load(file);
  assert(prior.root_confirmed === true && positive(prior.epoch) && hash32.test(prior.epoch_apply_hash) &&
    /^0x[0-9a-f]{64}$/.test(prior.root), 'Invalid finalized TAP refund basis');
  return { epoch: prior.epoch, anchor: prior.epoch_apply_hash, root: prior.root };
}

export function loadCursor(spool) {
  if (!fs.existsSync(cursorFile(spool))) return null;
  const value = load(cursorFile(spool));
  assert([1, 2].includes(value.schema_version) && positive(value.epoch) && hash32.test(value.epoch_apply_hash) &&
    positive(value.signed_length) && validCursor(value.after) && validCursor(value.next_after) &&
    hash32.test(value.report_sha256), 'Invalid saved TAP sweep cursor');
  return value;
}

export function validateIntent(intent, epoch, anchor) {
  assert([1, 2].includes(intent.schema_version) && intent.epoch === epoch && intent.epoch_apply_hash === anchor &&
    positive(intent.signed_length) && positive(intent.at) && validCursor(intent.after) &&
    Number.isSafeInteger(intent.params?.fee_bps) && intent.params.fee_bps >= 0 &&
    /^(0|[1-9][0-9]*)$/.test(intent.params?.payout_min_au) && typeof intent.params.payout_min_au === 'string',
  'TAP sweep intent does not match the requested epoch');
}

export function sweepPointerForIntent(intent) {
  const pointer = { schema_version: intent.schema_version, signed_length: intent.signed_length, after: intent.after };
  // A simulation failure is isolated to one row. A reserved provider already
  // has a Core tail that the v2 selector excludes on a normal-width page.
  if (intent.handoff && !intent.handoff.reserved) pointer.limit = 1;
  return pointer;
}

async function captureIntent({ epoch, anchor, dir, spool, peer, empty, bundle, fetchImpl }) {
  const state = async (key, signedLength) => {
    const url = new URL('state', peer.replace(/\/+$/, '') + '/');
    url.search = new URLSearchParams({ key, confirmed: 'true', ...(signedLength && { signed_length: signedLength }) });
    const response = await fetchImpl(url, { signal: AbortSignal.timeout(10_000), redirect: 'error' });
    assert(response.ok, 'TAP sweep state read failed');
    const row = await response.json();
    assert(row.key === key && row.confirmed === true && !Object.hasOwn(row, 'error') && positive(row.signed_length) &&
      (!signedLength || row.signed_length === signedLength) && Object.hasOwn(row, 'value'), 'Invalid TAP sweep snapshot');
    return row;
  };
  let head = await state('epoch/apply/state');
  if (head.value?.updated_epoch > epoch) {
    // The producer may have registered this epoch while the preceding pool
    // root was pending. The exact retained checkout still authenticates it.
    const savedFile = path.join(dir, 'apply-state.json');
    const saved = fs.existsSync(savedFile) ? load(savedFile) : null;
    if (saved && Object.hasOwn(saved, 'signed_length')) {
      assert(saved.key === 'epoch/apply/state' && saved.confirmed === true && positive(saved.signed_length) &&
        saved.signed_length <= head.signed_length && !Object.hasOwn(saved, 'error'), 'Invalid retained TAP checkout');
      head = await state('epoch/apply/state', saved.signed_length);
      assert.deepEqual(head.value, saved.value, 'Retained TAP epoch checkout changed');
    } else {
      // Older schedules may lack a retained length. Find the last signed view
      // before the next application started, then verify its exact anchor below.
      let low = 1, high = head.signed_length;
      let candidate;
      while (low <= high) {
        const mid = low + Math.floor((high - low) / 2);
        const row = await state('epoch/apply/state', mid);
        const value = row.value;
        assert(value === null || (Number.isSafeInteger(value.updated_epoch) && value.updated_epoch >= 0),
          'Ambiguous historical TAP apply state');
        if (value === null || value.updated_epoch < epoch ||
            (value.updated_epoch === epoch && value.pending_epoch == null)) {
          candidate = row;
          low = mid + 1;
        } else high = mid - 1;
      }
      assert(candidate?.value?.updated_epoch === epoch, 'No fully applied signed checkout for TAP epoch');
      head = candidate;
    }
  }
  assert(head.value?.updated_epoch === epoch && head.value.last_apply_hash === anchor &&
    head.value.pending_epoch == null && positive(head.value.last_settlement_unix),
  'TAP sweep requires a fully applied current epoch');
  const at = head.value.last_settlement_unix;
  const contract = tapContractAt(peer, head.signed_length, fetchImpl);
  const applied = await contract.get(contract.epochApplyAnchorKey(epoch));
  assert(applied?.type === 'epoch_apply_anchor' && applied.epoch === epoch && applied.apply_hash === anchor &&
    applied.settlement_unix === at, 'Invalid confirmed TAP sweep anchor');
  if (empty) {
    const seal = await contract.get('epoch/seal/' + epoch);
    assert(seal?.type === 'epoch_empty_seal' && seal.epoch === epoch && seal.seal_hash === anchor,
      'Missing authenticated empty TAP epoch seal');
    assert.deepEqual(seal.totals, { debited_au:'0', earned_au:'0', fee_au:'0', burn_au:'0' });
  }
  const params = await contract.targetedTapParamsAt(at, ['fee_bps', 'payout_min_au']);
  if (params instanceof Error) throw params;
  const cursor = loadCursor(spool);
  assert(!cursor || cursor.epoch < epoch, 'TAP sweep epoch already used or out of order');
  const handoff = readHandoff(spool, cursor);
  assert(!handoff || handoff.ticket.epoch < epoch, 'TAP successor must advance the draft epoch');
  const queue = refundQueue(cursor, handoff);
  const batch = nextRefunds(spool, queue);
  const trackRefunds = Boolean(batch) || hasTapRefunds(bundle);
  return {
    schema_version: 2, epoch, epoch_apply_hash: anchor, signed_length: head.signed_length,
    at, after: handoff ? handoff.proof.retry_after : (cursor?.next_after ?? null), params, empty,
    ...(handoff && { handoff: handoff.ticket }),
    ...(queue.tail > 0 && { refund_queue: queue }),
    ...(batch && { refund_source: batch.source }),
    ...(trackRefunds && { refund_queue_version: 1, refund_basis: refundBasis(spool) }),
  };
}

// The outer payout worker holds payout.lock across intent creation, publication and completion.
export async function prepareSweep({ epoch, anchor, dir, spool, peer, file, empty, fetchImpl = fetch }) {
  assert(positive(epoch) && hash32.test(anchor), 'Invalid TAP sweep epoch or hash');
  const bundle = empty ? { epoch, rail: 'tap', epoch_apply_hash: anchor, receipts: [] } : load(file);
  const stored = intentFile(dir);
  const intent = fs.existsSync(stored) ? load(stored) : await captureIntent({ epoch, anchor, dir, spool, peer, empty, bundle, fetchImpl });
  validateIntent(intent, epoch, anchor);
  assert.equal(intent.empty, empty, 'TAP epoch source changed after preparation');
  assert(bundle.epoch === epoch && bundle.epoch_apply_hash === anchor && bundle.rail === 'tap' &&
    Array.isArray(bundle.receipts) && !Object.hasOwn(bundle, 'tap_liability_sweep'), 'Invalid derived TAP bundle');
  for (const params of [bundle.params, bundle.audited_epoch?.params, bundle.recomputed?.params]) {
    for (const [name,value] of Object.entries(intent.params)) {
      if (params && Object.hasOwn(params,name)) assert.equal(params[name],value, 'TAP bundle policy mismatch: ' + name);
    }
  }
  assert(!['tap_carried_refunds', 'tap_refund_queue_version', 'tap_refund_basis'].some(k => Object.hasOwn(bundle, k)),
    'Unexpected refund queue fields in epoch source');
  const cursor = loadCursor(spool);
  const handoff = readHandoff(spool, cursor);
  assert.deepEqual(handoff?.ticket, intent.handoff, 'Invalid TAP successor handoff');
  if (handoff) assert.equal(intent.after, handoff.proof.retry_after);
  const queue = refundQueue(cursor, handoff);
  assert.deepEqual(intent.refund_queue, queue.tail > 0 ? queue : undefined);
  const batch = nextRefunds(spool, queue);
  assert.deepEqual(batch?.source, intent.refund_source, 'TAP refund source changed');
  if (batch) bundle.tap_carried_refunds = batch.entries;
  if (intent.refund_queue_version !== undefined) {
    assert(intent.refund_queue_version === 1 && (batch || hasTapRefunds(bundle)), 'Invalid TAP refund intent');
    bundle.tap_refund_queue_version = 1;
    bundle.tap_refund_basis = intent.refund_basis;
  }
  if (bundle.at !== undefined) assert.equal(bundle.at, intent.at, 'TAP bundle preparation timestamp changed');
  bundle.at = intent.at;
  bundle.params = { ...bundle.params, ...intent.params };
  bundle.tap_liability_sweep = sweepPointerForIntent(intent);
  if (!fs.existsSync(stored)) durableWrite(stored, intent, false);
  durableWrite(file, bundle, true);
}

// The roller holds roller.lock. Recheck the scheduling basis immediately
// before using a queued draft, including one published before an upgrade.
export function validateQueuedSweep(spool, bundle) {
  const cursor = loadCursor(spool);
  const handoff = readHandoff(spool, cursor);
  const batch = nextRefunds(spool, refundQueue(cursor, handoff));
  assert(!cursor || cursor.epoch < bundle.epoch, 'TAP queued sweep is older than the completed page');
  assert(!handoff || handoff.ticket.epoch < bundle.epoch, 'TAP queued sweep does not follow its draft handoff');
  assert.equal(bundle.tap_liability_sweep.after, handoff ? handoff.proof.retry_after : cursor?.next_after ?? null,
    'TAP queued sweep has a different opening cursor');
  if (handoff?.ticket.reserved) assert.equal(bundle.tap_liability_sweep.schema_version, 2, 'TAP fence requires reservation-aware selection');
  assert.equal(bundle.tap_liability_sweep.limit, handoff && !handoff.ticket.reserved ? 1 : undefined,
    'TAP queued sweep has a different page size');
  assert.deepEqual(bundle.tap_carried_refunds, batch?.entries, 'TAP queued sweep differs from its unpaid refunds');
  if (batch || Object.hasOwn(bundle, 'tap_refund_queue_version')) {
    assert(bundle.tap_refund_queue_version === 1 && (batch || hasTapRefunds(bundle)), 'Invalid TAP refund queue version');
    assert.deepEqual(bundle.tap_refund_basis, refundBasis(spool), 'TAP queued refund predecessor changed');
  }
  return batch?.source;
}

// A reconciled newer root permits revisiting a fenced refund. The immutable
// per-epoch mapping must independently prove the older root never executed.
// Check the same finalized block before authorizing a new financial plan.
export async function verifyQueuedTapSweep(spool, bundle, pool) {
  const source = validateQueuedSweep(spool, bundle);
  if (!source?.reserved) return;
  const provider = pool.runner.provider;
  const finalized = await provider.getBlock('finalized');
  assert(positive(finalized?.number) && /^0x[0-9a-f]{64}$/.test(finalized.hash), 'Missing finalized TAP refund basis');
  const block = { blockTag: finalized.number };
  const [epoch, root] = await Promise.all([pool.epoch(block), pool.merkleRootAtEpoch(source.epoch, block)]);
  assert(typeof epoch === 'bigint' && epoch > BigInt(source.epoch) && root === ZeroHash &&
    (await provider.getBlock(finalized.number))?.hash === finalized.hash,
  'Fenced TAP refund epoch has not been irrevocably skipped');
}

function verifyRefundDistribution({ spool, prefix, bundle, report, intent, paid }) {
  const basis = intent.refund_basis;
  let prior = null;
  if (basis !== null) {
    assert(positive(basis.epoch) && basis.epoch < intent.epoch && hash32.test(basis.anchor));
    prior = load(path.join(spool, 'processed', 'epoch-' + basis.epoch + '-' + basis.anchor + '.settlement.json'));
    assert(prior.root_confirmed === true && prior.epoch === basis.epoch && prior.epoch_apply_hash === basis.anchor &&
      prior.tap_settlement_checkpoint?.root === basis.root && prior.root === basis.root);
    assert.deepEqual(prior.refunds, prior.tap_settlement_checkpoint.refunds);
  }
  const buyers = prefix + '.buyers.json';
  const accounts = fs.existsSync(buyers) ? load(buyers) : {};
  // A carry keeps its inherited position. Only its own epoch's new inputs
  // need a source. A settlement moves the batch and requeues every unpaid row.
  const projected = projectTapRefunds(paid ? bundle : { ...bundle, tap_carried_refunds: [] },
    accounts, report.tap_rate_lock.tap_usd_au, paid);
  const expected = cumulativeTapRefunds(projected.credits, prior);
  assert.deepEqual(report.refunds, expected, 'TAP report did not pay its refund inputs exactly once');
  if (paid) {
    assert(report.checkpoint_outputs?.length > 0, 'TAP refund batch lacks a provider payment');
    assert.deepEqual(report.tap_settlement_checkpoint.refunds, expected, 'TAP root differs from its refund inputs');
  }
  return projected.unpaid;
}

export function advanceSweep({ epoch, anchor, dir, spool, marker }) {
  const intent = load(intentFile(dir));
  validateIntent(intent, epoch, anchor);
  const prefix = path.join(spool, 'processed', 'epoch-' + epoch + '-' + anchor);
  const bundleRaw = fs.readFileSync(prefix + '.receipts.json');
  const reportRaw = fs.readFileSync(prefix + '.settlement.json');
  const bundle = JSON.parse(bundleRaw);
  const report = JSON.parse(reportRaw);
  const produced = load(path.join(dir, 'tap.produced'));
  const proof = load(marker);
  assert(produced.epoch === epoch && produced.epoch_apply_hash === anchor && produced.evidence_sha256 === sha(bundleRaw) &&
    proof.epoch === epoch && proof.epoch_apply_hash === anchor && proof.rail === 'tap', 'TAP sweep evidence mismatch');
  const evidence = fs.readFileSync(proof.evidence);
  assert.equal(sha(evidence), proof.evidence_sha256, 'TAP completion evidence changed');
  if (proof.status === 'carry') assert.equal(sha(reportRaw), proof.evidence_sha256, 'TAP carry report changed');
  else {
    assert.equal(proof.status, 'settled');
    const reconciliation = JSON.parse(evidence);
    assert(reconciliation.type === 'canonical_tap_settlement_reconciliation' &&
      reconciliation.processed_report_sha256 === sha(reportRaw), 'TAP payment lacks an exact reconciled report');
  }
  const start = sweepPointerForIntent(intent);
  assert.deepEqual(bundle.tap_liability_sweep, start);
  const page = report.tap_liability_sweep;
  assert(page?.limit === start.limit && page?.schema_version === intent.schema_version && page.signed_length === intent.signed_length && page.after === intent.after &&
    validCursor(page.next_after) && (!page.after || !page.next_after || page.next_after > page.after) &&
    page.at === intent.at && bundle.at === intent.at, 'TAP page did not complete its frozen cursor');
  const record = { ...start, epoch, epoch_apply_hash:anchor, next_after:page.next_after, report_sha256:sha(reportRaw) };
  const queue = refundQueue(intent, null);
  const batch = nextRefunds(spool, queue);
  assert.deepEqual(batch?.source, intent.refund_source);
  assert.deepEqual(bundle.tap_carried_refunds, batch?.entries, 'Inherited TAP refunds changed');
  assert.equal(bundle.tap_refund_queue_version, intent.refund_queue_version);
  assert.deepEqual(bundle.tap_refund_basis, intent.refund_basis);
  const paid = proof.status === 'settled';
  let unpaid = [];
  if (intent.refund_queue_version === 1) {
    unpaid = verifyRefundDistribution({ spool, prefix, bundle, report, intent, paid });
  } else if (hasTapRefunds(bundle)) {
    // A historical verified carry spent nothing. A historical positive root
    // can also contain AU inputs which individually converted to zero wei.
    const file = prefix + '.buyers.json';
    const rate = report.tap_rate_lock.tap_usd_au;
    const projection = projectTapRefunds(bundle, fs.existsSync(file) ? load(file) : {}, rate, paid);
    if (paid) assert(projection.reservedWei === 0n && projection.unpaid.every(row => row.account && row.amount_au),
      'Ambiguous refund destination in historical TAP payment');
    else assert(report.spent_au === '0' && report.checkpoint_outputs?.length === 0 &&
      report.tap_settlement_checkpoint == null, 'Historical TAP carry includes a payment');
    unpaid = projection.unpaid;
  }
  const previous = loadCursor(spool);
  if (previous?.epoch !== epoch) {
    const handoff = readHandoff(spool, previous);
    assert.deepEqual(handoff?.ticket, intent.handoff, 'TAP completed page differs from its draft handoff');
    assert((!previous || previous.epoch < epoch) &&
      (handoff ? handoff.proof.retry_after : (previous?.next_after ?? null)) === intent.after,
      'TAP sweep cursor changed while its page was in progress');
    assert.deepEqual(queue, refundQueue(previous, handoff), 'TAP refund cursor changed during preparation');
  }
  const next = retainRefundRemainder(spool, prefix, record, unpaid, batch && paid ? batch.next : queue);
  if (paid && next.reconciled_epoch !== undefined) next.reconciled_epoch = epoch;
  if (next.tail > 0) record.refund_queue = next;
  if (previous?.epoch === epoch) {
    assert.deepEqual(previous, record, 'Conflicting TAP page already completed');
    return;
  }
  durableWrite(cursorFile(spool), record, true);
}

if (import.meta.main ?? (process.argv[1] && fs.existsSync(process.argv[1]) && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url))) {
  try {
    const [command, n, anchor, dir, spool, arg, file, mode] = process.argv.slice(2);
    const epoch = Number(n);
    if (command === 'prepare') await prepareSweep({ epoch, anchor, dir, spool, peer:arg, file, empty:mode === 'empty' });
    else if (command === 'advance') advanceSweep({ epoch, anchor, dir, spool, marker:arg });
    else throw new Error('Unknown TAP sweep operation');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
