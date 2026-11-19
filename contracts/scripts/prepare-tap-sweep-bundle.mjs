import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import MayhemContract from '../../intercom/contract/contract.js';

const hash32 = /^[0-9a-f]{64}$/;
const cursorKey = /^payout\/liability\/tap\/[0-9a-f]{64}\/[0-9a-f]{64}$/;
const positive = n => Number.isSafeInteger(n) && n > 0;
const validCursor = key => key === null || (typeof key === 'string' && cursorKey.test(key));
const sha = raw => createHash('sha256').update(raw).digest('hex');
const load = file => JSON.parse(fs.readFileSync(file, 'utf8'));
const cursorFile = spool => path.join(spool, 'processed/liability-sweep-cursor.json');
const intentFile = dir => path.join(dir, 'tap-sweep-intent.json');

function durableWrite(file, obj, replace) {
  const temporary = file + '.' + randomUUID() + '.tmp';
  const fd = fs.openSync(temporary, 'wx', 0o600);
  try {
    try {
      fs.writeFileSync(fd, JSON.stringify(obj, null, 2) + '\n');
      fs.fsyncSync(fd);
    } finally { fs.closeSync(fd); }
    if (replace) fs.renameSync(temporary, file);
    else fs.linkSync(temporary, file);
    const dir = fs.openSync(path.dirname(file), 'r');
    try { fs.fsyncSync(dir); } finally { fs.closeSync(dir); }
  } finally { fs.rmSync(temporary, { force: true }); }
}

function loadCursor(spool) {
  if (!fs.existsSync(cursorFile(spool))) return null;
  const value = load(cursorFile(spool));
  assert([1, 2].includes(value.schema_version) && positive(value.epoch) && hash32.test(value.epoch_apply_hash) &&
    positive(value.signed_length) && validCursor(value.after) && validCursor(value.next_after) &&
    hash32.test(value.report_sha256), 'Invalid saved TAP sweep cursor');
  return value;
}

function validateIntent(intent, epoch, anchor) {
  assert([1, 2].includes(intent.schema_version) && intent.epoch === epoch && intent.epoch_apply_hash === anchor &&
    positive(intent.signed_length) && positive(intent.at) && validCursor(intent.after) &&
    Number.isSafeInteger(intent.params?.fee_bps) && intent.params.fee_bps >= 0 &&
    /^(0|[1-9][0-9]*)$/.test(intent.params?.payout_min_au) && typeof intent.params.payout_min_au === 'string',
  'TAP sweep intent does not match the requested epoch');
}

async function captureIntent({ epoch, anchor, spool, peer, empty, fetchImpl }) {
  const state = async (key, signedLength) => {
    const url = new URL('state', peer.replace(/\/+$/, '') + '/');
    url.search = new URLSearchParams({ key, confirmed: 'true', ...(signedLength && { signed_length: signedLength }) });
    const response = await fetchImpl(url, { signal: AbortSignal.timeout(10_000), redirect: 'error' });
    assert(response.ok, 'TAP sweep state read failed');
    const row = await response.json();
    assert(row.key === key && row.confirmed === true && positive(row.signed_length) &&
      (!signedLength || row.signed_length === signedLength) && Object.hasOwn(row, 'value'), 'Invalid TAP sweep snapshot');
    return row;
  };
  const head = await state('epoch/apply/state');
  assert(head.value?.updated_epoch === epoch && head.value.last_apply_hash === anchor &&
    head.value.pending_epoch == null && positive(head.value.last_settlement_unix),
  'TAP sweep requires a fully applied current epoch');
  const at = head.value.last_settlement_unix;
  const contract = Object.create(MayhemContract.prototype);
  contract.get = async key => (await state(key, head.signed_length)).value;
  const applied = await contract.get(contract.epochApplyAnchorKey(epoch));
  assert(applied?.type === 'epoch_apply_anchor' && applied.epoch === epoch && applied.apply_hash === anchor &&
    applied.settlement_unix === at, 'Invalid confirmed TAP sweep anchor');
  if (empty) {
    const seal = await contract.get('epoch/seal/' + epoch);
    assert(seal?.type === 'epoch_empty_seal' && seal.epoch === epoch && seal.seal_hash === anchor,
      'Missing authenticated empty TAP epoch seal');
    assert.deepEqual(seal.totals, { debited_au:'0', earned_au:'0', fee_au:'0', burn_au:'0' });
  }
  const params = await contract.activeParamsAt(at, ['fee_bps', 'payout_min_au']);
  const validation = contract.validateParamValues(params);
  if (validation instanceof Error) throw validation;
  const cursor = loadCursor(spool);
  assert(!cursor || cursor.epoch < epoch, 'TAP sweep epoch already used or out of order');
  return { schema_version: 2, epoch, epoch_apply_hash: anchor, signed_length: head.signed_length,
    at, after: cursor?.next_after ?? null, params, empty };
}

// The outer payout worker holds payout.lock across intent creation, publication and completion.
export async function prepareSweep({ epoch, anchor, dir, spool, peer, file, empty, fetchImpl = fetch }) {
  assert(positive(epoch) && hash32.test(anchor), 'Invalid TAP sweep epoch or hash');
  const stored = intentFile(dir);
  const intent = fs.existsSync(stored) ? load(stored) : await captureIntent({ epoch, anchor, spool, peer, empty, fetchImpl });
  validateIntent(intent, epoch, anchor);
  assert.equal(intent.empty, empty, 'TAP epoch source changed after preparation');
  const bundle = empty ? { epoch, rail: 'tap', epoch_apply_hash: anchor, receipts: [] } : load(file);
  assert(bundle.epoch === epoch && bundle.epoch_apply_hash === anchor && bundle.rail === 'tap' &&
    Array.isArray(bundle.receipts) && !Object.hasOwn(bundle, 'tap_liability_sweep'), 'Invalid derived TAP bundle');
  for (const params of [bundle.params, bundle.audited_epoch?.params, bundle.recomputed?.params]) {
    for (const [name,value] of Object.entries(intent.params)) {
      if (params && Object.hasOwn(params,name)) assert.equal(params[name],value, 'TAP bundle policy mismatch: ' + name);
    }
  }
  if (bundle.at !== undefined) assert.equal(bundle.at, intent.at, 'TAP bundle preparation timestamp changed');
  bundle.at = intent.at;
  bundle.params = { ...bundle.params, ...intent.params };
  bundle.tap_liability_sweep = { schema_version:intent.schema_version, signed_length:intent.signed_length, after:intent.after };
  if (!fs.existsSync(stored)) durableWrite(stored, intent, false);
  durableWrite(file, bundle, true);
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
  const start = { schema_version:intent.schema_version, signed_length:intent.signed_length, after:intent.after };
  assert.deepEqual(bundle.tap_liability_sweep, start);
  const page = report.tap_liability_sweep;
  assert(page?.schema_version === intent.schema_version && page.signed_length === intent.signed_length && page.after === intent.after &&
    validCursor(page.next_after) && (!page.after || !page.next_after || page.next_after > page.after) &&
    page.at === intent.at && bundle.at === intent.at, 'TAP page did not complete its frozen cursor');
  const record = { ...start, epoch, epoch_apply_hash:anchor, next_after:page.next_after, report_sha256:sha(reportRaw) };
  const previous = loadCursor(spool);
  if (previous?.epoch === epoch) {
    assert.deepEqual(previous, record, 'Conflicting TAP page already completed');
    return;
  }
  assert((!previous || previous.epoch < epoch) && (previous?.next_after ?? null) === intent.after,
    'TAP sweep cursor changed while its page was in progress');
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
