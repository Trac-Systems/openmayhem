// Called under the same exclusive spool lock as preparation and broadcast.
// Only the payout worker can publish a successor, under its payout lock too.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Contract, FetchRequest, JsonRpcProvider, ZeroHash } from 'ethers';
import { confirmedTapContract, MAINNET_POOL as POOL, MAINNET_TOKEN as TOKEN, tapContractAt, tapDraftWatermarks, verifyPriorTapClaims } from './tap-liability-sweep.mjs';
import { canonicalTapPaymentScope, POOL_SETTLEMENT_ABI, resolveTargetedTapPayoutsFromLedger, tapRateRecordToLock, transferableTapRefunds,
  validateTapRateLock } from './tap-settlement-roller.mjs';
import { loadCursor, sweepPointerForIntent, validateIntent } from './prepare-tap-sweep-bundle.mjs';
import { digest, digestFile, durableWrite, handoffFile, holdTapLock, nextRefunds, readDraftFile, readDraftHeader, readHandoff, refundQueue, refundSourceFile, reservedHandoffTicket, syncDirectory } from './tap-draft-files.mjs';

const hash = s => typeof s === 'string' && /^[0-9a-f]{64}$/.test(s);
const evmHash = s => typeof s === 'string' && /^0x[0-9a-f]{64}$/.test(s);
const uint = x => Number.isSafeInteger(x) && x >= 0;
const json = file => JSON.parse(readDraftFile(file));
const exists = file => fs.lstatSync(file, { throwIfNoEntry: false }) !== undefined;

async function idlePool(pool, epoch, tag = 'latest') {
  const provider = pool.runner.provider;
  assert.equal(await provider.send('eth_chainId', []), '0x1', 'Wrong TAP recovery network');
  assert.equal((await pool.getAddress()).toLowerCase(), POOL);
  const head = await provider.getBlock(tag);
  assert(uint(head?.number) && evmHash(head.hash) && uint(head.timestamp), 'Invalid TAP recovery block');
  const block = { blockTag: head.number };
  const [token, previousEpoch, spent, root, pending, posted, pendingCap] = await Promise.all([
    pool.token(block), pool.epoch(block), pool.cumulativeSpent(block), pool.merkleRoot(block),
    pool.pendingRoot(block), pool.merkleRootAtEpoch(epoch, block), pool.pendingMaxEpochDelta(block),
  ]);
  assert(token.toLowerCase() === TOKEN && typeof previousEpoch === 'bigint' && uint(Number(previousEpoch)) &&
    previousEpoch < BigInt(epoch) && typeof spent === 'bigint' && spent >= 0n && evmHash(root) &&
    posted === ZeroHash && pending.length === 7 && pending[0] === ZeroHash &&
    Array.from(pending).slice(1).every(n => n === 0n), 'TAP pool has a pending, executed or ambiguous root');
  assert(pendingCap.length === 3 && Array.from(pendingCap).every(n => n === 0n), 'TAP pool has a pending governance cap change');
  const retained = await provider.getBlock(head.number);
  assert(retained?.hash === head.hash && retained.number === head.number && retained.timestamp === head.timestamp,
    'TAP recovery block changed');
  return { epoch: Number(previousEpoch), spent: String(spent), root, block: head.number,
    block_hash: head.hash, timestamp: head.timestamp };
}

export async function proveUnpreparedTapDraft({ bundle, peerRpcUrl, fetchImpl = fetch, pool, prior, reports }) {
  assert(bundle.rail === 'tap' && uint(bundle.epoch) && bundle.epoch > 0 && hash(bundle.epoch_apply_hash) &&
    Array.isArray(bundle.receipts), 'Invalid TAP draft identity');
  const page = await tapDraftWatermarks({ bundle, peerRpcUrl, fetchImpl });
  const contract = await confirmedTapContract(peerRpcUrl, fetchImpl);
  assert(contract.signedLength >= bundle.tap_liability_sweep.signed_length, 'TAP recovery Core view regressed');
  const admin = await contract.get('admin');
  const scope = canonicalTapPaymentScope(await contract.get('payments/current'), admin);
  assert(scope.chain_id === 1 && scope.pool_address === POOL && scope.token_address === TOKEN);
  const epochAnchor = await contract.get(contract.epochApplyAnchorKey(bundle.epoch));
  assert(epochAnchor?.type === 'epoch_apply_anchor' && epochAnchor.epoch === bundle.epoch &&
    epochAnchor.apply_hash === bundle.epoch_apply_hash && epochAnchor.settlement_unix === bundle.at,
  'Current Core view conflicts with TAP draft anchor');
  assert.equal(await contract.get('settle/targeted/tap/' + bundle.epoch), null, 'TAP checkpoint already exists');

  const preparationBeforeDraft = async id => {
    assert(hash(id), 'Invalid TAP reservation ID');
    const prep = await contract.get('payout/preparation/tap/' + id);
    assert(prep?.type === 'targeted_payout_preparation' && prep.rail === 'tap' && prep.kind === 'liability' &&
      prep.economic_op_id === id && uint(prep.epoch) && prep.epoch > 0 && prep.epoch < bundle.epoch &&
      hash(prep.epoch_apply_hash) && typeof prep.consumed === 'boolean',
    'TAP draft has a prior preparation or ambiguous reservation');
    const source = await contract.requireEpochApplyAnchor(prep.epoch, prep.epoch_apply_hash, 'Earlier TAP reservation');
    if (source instanceof Error) throw source;
    assert(uint(source.settlement_unix) && source.settlement_unix <= bundle.at, 'Invalid earlier TAP reservation epoch');
    return prep;
  };
  for (const { provider, revision, before, target, chainId } of page.rows) {
    const current = await contract.get(contract.providerPayoutLiabilityKey(provider, 'tap', revision));
    const invalid = contract.guardianValidatePayoutLiabilityRecord(current, provider, 'tap', revision);
    if (invalid instanceof Error) throw invalid;
    assert(current.target === target && current.chain_id === chainId && BigInt(current.paid_cum_au) >= BigInt(before),
      'TAP revision identity or watermark changed inconsistently');
    const lock = await contract.get('payout/preparation-lock/tap/' + provider + '/' + revision + '/' + before);
    if (lock === null) {
      assert.equal(current.paid_cum_au, before, 'TAP watermark advanced without its preparation');
      continue;
    }
    assert(lock.rail === 'tap' && lock.provider === provider && lock.payout_revision === revision &&
      lock.paid_cum_au_before === before, 'Ambiguous TAP liability lock');
    const prep = await preparationBeforeDraft(lock.economic_op_id);
    assert(prep.liability?.provider === provider && prep.liability.payout_revision === revision &&
      prep.liability.paid_cum_au_before === before && typeof prep.liability.paid_au === 'string' &&
      /^[1-9][0-9]*$/.test(prep.liability.paid_au) && (prep.consumed ?
        BigInt(current.paid_cum_au) >= BigInt(before) + BigInt(prep.liability.paid_au) : current.paid_cum_au === before),
    'Inconsistent TAP liability lock');
  }
  for (const provider of new Set(page.rows.map(row => row.provider))) {
    const earning = await contract.get(contract.earningKey(provider, 'tap'));
    const invalid = contract.guardianValidateEarningRecord(earning, provider, 'tap');
    if (invalid instanceof Error) throw invalid;
    assert(earning.chain_id === 1 && earning.pool_address === POOL, 'TAP aggregate scope changed');
    const tail = await contract.get(contract.payoutPreparationAggregateTailKey('tap', provider));
    if (tail === null) continue;
    const prep = await preparationBeforeDraft(tail.economic_op_id);
    assert(prep.liability?.provider === provider && prep.consumed === tail.consumed &&
      prep.liability.aggregate_paid_cum_au_before === tail.paid_cum_au_before &&
      typeof tail.paid_cum_au_before === 'string' && /^(0|[1-9][0-9]*)$/.test(tail.paid_cum_au_before) &&
      typeof prep.liability.paid_au === 'string' && /^[1-9][0-9]*$/.test(prep.liability.paid_au) &&
      String(BigInt(tail.paid_cum_au_before) + BigInt(prep.liability.paid_au)) === tail.paid_cum_au_after &&
      (tail.consumed ? earning.paid_cum_au === tail.paid_cum_au_after :
        BigInt(earning.paid_cum_au) <= BigInt(tail.paid_cum_au_before)), 'Inconsistent TAP aggregate reservation');
  }
  // Dry reports can retain IDs for an independently submitted root preparation.
  // Any record at an exact ID binds this intent, including a consumed record.
  for (const dry of reports) {
    if (dry.root_preparation_id == null && [dry.preparation_ids, dry.external_effect_ids].every(ids =>
      ids == null || (Array.isArray(ids) && ids.length === 0))) continue;
    assert(hash(dry.root_preparation_id) && Array.isArray(dry.preparation_ids) &&
      dry.preparation_ids.length <= page.rows.length && Array.isArray(dry.external_effect_ids) &&
      dry.external_effect_ids.length === 2, 'Invalid TAP draft preparation IDs');
    for (const id of [dry.root_preparation_id, ...dry.preparation_ids]) {
      assert(hash(id));
      assert.equal(await contract.get('payout/preparation/tap/' + id), null, 'TAP preparation exists');
    }
    for (const id of dry.external_effect_ids) {
      assert(hash(id));
      assert.equal(await contract.get('payout/preparation-effect/tap/' + id), null, 'TAP effect exists');
    }
  }
  const basis = await idlePool(pool, bundle.epoch);
  await verifyPoolBasis(contract, scope, basis, prior, bundle.at);
  return { contract, scope, basis, page };
}

async function verifyPoolBasis(contract, scope, basis, prior, at) {
  assert(basis.timestamp >= at, 'Ethereum head predates TAP snapshot');
  const state = await contract.get('settle/targeted/tap/state/1/' + POOL);
  if (basis.epoch === 0) {
    assert(state === null && prior === null && basis.root === ZeroHash && basis.spent === '0',
      'TAP genesis basis conflicts with Core or its archive');
  } else {
    const cp = prior?.tap_settlement_checkpoint;
    assert(state?.type === 'targeted_tap_settlement_state' && state.last_epoch === basis.epoch &&
      state.last_root === basis.root && state.cumulative_spent_wei === basis.spent &&
      state.cumulative_provider_claimed_wei === String(BigInt(basis.spent) * 7500n / 10000n) &&
      state.cumulative_buyer_refund_wei === cp?.buyer_refund_wei &&
      Object.entries(scope).every(([k,v]) => state[k] === v && cp?.[k] === v) &&
      state.last_execution_tx === cp.execution_tx && cp.root_confirmed === true && cp.epoch === basis.epoch &&
      cp.root === basis.root && cp.cumulative_spent_wei === basis.spent && prior.root_confirmed === true &&
      prior.epoch === cp.epoch && prior.root === cp.root && prior.cumulative_spent_wei === basis.spent &&
      prior.operator_fee?.completed === true && prior.burn?.completed === true, 'Conflicting TAP cumulative predecessor');
    await verifyPriorTapClaims(contract, prior);
  }
}

// Only the scheduler moves. The old root, its signed preparations, all
// watermarks and even ambiguous submission evidence remain bound. A governed
// proposal signs the predecessor epoch, gross spend and pool nonce. Competing
// roots on that basis cannot both execute, including after a lost response.
async function proveReservedTapDraft({ bundle, peerRpcUrl, fetchImpl, pool, prior, companions }) {
  const contract = await confirmedTapContract(peerRpcUrl, fetchImpl);
  const admin = await contract.get('admin');
  const scope = canonicalTapPaymentScope(await contract.get('payments/current'), admin);
  assert(scope.chain_id === 1 && scope.pool_address === POOL && scope.token_address === TOKEN);
  assert.equal(bundle.rail, 'tap');
  const anchor = await contract.requireEpochApplyAnchor(bundle.epoch, bundle.epoch_apply_hash, 'TAP reservation fence');
  if (anchor instanceof Error) throw anchor;
  const at = anchor.settlement_unix;
  if (bundle.at !== undefined) assert.equal(bundle.at, at);
  assert.equal(await contract.get('settle/targeted/tap/' + bundle.epoch), null, 'TAP checkpoint already exists');
  const lock = validateTapRateLock(json(companions['.tap-rate.json']), bundle, scope);
  const ids = new Set();
  const page = bundle.tap_liability_sweep ? await tapDraftWatermarks({ bundle, peerRpcUrl, fetchImpl }) : null;
  if (page) {
    assert(contract.signedLength >= bundle.tap_liability_sweep.signed_length);
    for (const { provider, revision, before } of page.rows) {
      const reservation = await contract.get('payout/preparation-lock/tap/' + provider + '/' + revision + '/' + before);
      if (reservation !== null) ids.add(reservation.economic_op_id);
    }
  }
  // Managed receipt-only jobs also journal their ordered plan. Submission
  // confirms each liability in order, before the root or any Ethereum send.
  for (const suffix of ['.prepare-intent.json', '.draft-plan.json']) {
    if (!companions[suffix]) continue;
    const plan = json(companions[suffix]);
    assert(plan.epoch === bundle.epoch && plan.epoch_apply_hash === bundle.epoch_apply_hash &&
      Array.isArray(plan.preparation_ids) && hash(plan.preparation_ids[0]), 'Invalid TAP preparation journal');
    ids.add(plan.preparation_ids[0]);
  }
  const reservations = [];
  const verifyReservation = async (view, id) => {
    assert(hash(id), 'Invalid TAP preparation identity');
    const prep = await view.get(view.payoutPreparationRecordKey('tap', id));
    if (prep === null || prep.epoch !== bundle.epoch) return;
    assert(prep.epoch_apply_hash === bundle.epoch_apply_hash && prep.admin === admin && prep.kind === 'liability' &&
      hash(prep.liability?.provider) && prep.payload?.tap_rate_lock, 'Conflicting TAP reservation');
    assert.deepEqual(prep.payload.tap_rate_lock, lock);
    const checked = await view.validatePayoutPreparationRecord({ economicOpId: id, rail: 'tap',
      epoch: bundle.epoch, epochApplyHash: bundle.epoch_apply_hash, kind: 'liability', outputIndex: prep.output_index,
      payload: prep.payload, liability: prep.liability, externalEffectIds: [] });
    if (checked instanceof Error) throw checked;
    const tail = await view.get(view.payoutPreparationAggregateTailKey('tap', prep.liability.provider));
    assert(tail?.consumed === false, 'TAP provider has no outstanding aggregate reservation');
    reservations.push(id);
  };
  for (const id of ids) await verifyReservation(contract, id);
  if (!page && !reservations.length) {
    // Older receipt jobs may lack a submission journal. A later dry-run can
    // also discover different amounts. The signed native/proxy allocations
    // identify candidate watermarks; only a confirmed original record counts.
    const { liabilities } = await resolveTargetedTapPayoutsFromLedger({ bundle, peerRpcUrl, fetchImpl });
    let view;
    for (const [index, row] of liabilities.entries()) {
      if (index % 32 === 0) view = tapContractAt(peerRpcUrl, contract.signedLength, fetchImpl);
      const key = view.payoutPreparationLiabilityLockKey('tap', { ...row, paid_cum_au_before: row.paid_cum_au });
      const reservation = await view.get(key);
      if (reservation !== null) await verifyReservation(view, reservation.economic_op_id);
      if (reservations.length) break;
    }
  }
  assert(reservations.length, 'No confirmed TAP liability reservation for this bundle');
  const finalized = await idlePool(pool, bundle.epoch, 'finalized');
  const basis = await idlePool(pool, bundle.epoch);
  assert(finalized.block <= basis.block && at <= finalized.timestamp && finalized.timestamp <= basis.timestamp &&
    finalized.epoch === basis.epoch && finalized.root === basis.root && finalized.spent === basis.spent,
  'TAP pool basis has not finalized');
  await verifyPoolBasis(contract, scope, basis, prior, at);
  for (const block of [finalized, basis]) {
    assert((await contract.guardianRequireHistoricalTapRateLock(lock, block.timestamp))?.message ===
      'Guardian TAP settlement rate invariant failed: oracle is stale.', 'TAP reservation rate must be authentic and expired');
  }
  return { contract, basis, at, reservations, finalized };
}

function assertLocalDraft(prefix, processedPrefix) {
  // The old wrapper created .settlement.raw.json BEFORE entering --confirm.
  // Its presence, even empty, can mean a submitted request lost its reply.
  for (const ext of ['.prepare-intent.json', '.settlement.raw.json', '.settlement.bound.json', '.settlement.json']) {
    assert(!exists(prefix + ext), 'TAP confirmation previously started');
  }
  assert(!exists(processedPrefix + '.settlement.json'), 'TAP settlement publication already started');
  const reports = [];
  const phase = ['posted', 'root_confirmed', 'root_already_posted', 'root_pending', 'root_proposed',
    'awaiting_finality', 'awaiting_governance_delay', 'canonical_preparation_confirmed'];
  const effects = ['proposal_tx', 'execution_tx', 'pending_root', 'root_confirmation', 'tap_settlement_checkpoint'];
  for (const ext of ['.dry-run.json', '.confirmation.json', '.draft-plan.json']) {
    if (!exists(prefix + ext)) continue;
    const report = readDraftHeader(prefix + ext, [...phase, ...effects, 'operator_fee', 'burn',
      'root_preparation_id', 'preparation_ids', 'external_effect_ids']);
    if (report === null) {
      assert(fs.statSync(prefix + ext).size === 0, 'Ambiguous TAP attempt report');
      continue;
    }
    reports.push(report);
    assert(report && typeof report === 'object' && !Array.isArray(report) &&
      phase.every(k => report[k] == null || report[k] === false) && effects.every(k => report[k] == null) &&
      [report.operator_fee, report.burn].every(value => value == null || (typeof value === 'object' &&
        !Array.isArray(value) && value.tx == null && (value.auto_sent == null || value.auto_sent === false))),
    'TAP report records a preparation or external effect');
  }
  const cursorFile = prefix + '.tap-rate.json.events.json';
  if (exists(cursorFile)) {
    const c = json(cursorFile);
    assert(c.version === 1 && c.pool === POOL && c.chain_id === '1' && uint(c.start_block) &&
      c.epoch === Number(path.basename(prefix).split('-')[1]) && c.intent == null &&
      c.events && !Array.isArray(c.events) && Object.keys(c.events).length === 0,
    'TAP event search already observed an effect or is ambiguous');
  }
  return reports;
}

const draftCompanions = ['.buyers.json', '.tap-rate.json', '.tap-rate.json.events.json',
  '.dry-run.json', '.draft-plan.json', '.draft-retry', '.confirmation.json'];
const reservedCompanions = [...draftCompanions, '.prepare-intent.json', '.settlement.raw.json',
  '.settlement.bound.json', '.settlement.json', '.proposal-block.json'];

function fenceCompanions(prefix, item) {
  const files = {};
  for (const ext of reservedCompanions) {
    const matches = [...new Set([item + ext, prefix + ext])].filter(exists);
    assert(matches.length <= 1, 'Duplicate TAP reservation evidence ' + ext);
    if (matches.length) files[ext] = matches[0];
  }
  return files;
}

function archiveBundle(file, archive) {
  fs.renameSync(file, archive + '.receipts.json');
  syncDirectory(path.dirname(archive));
  syncDirectory(path.dirname(file));
}

export async function recoverDraft({ action, spool, name, dir, successor, peerRpcUrl, pool, openPool, fetchImpl, loadedBundle }) {
  const m = /^epoch-([1-9][0-9]*)-([0-9a-f]{64})$/.exec(name);
  assert(m && uint(Number(m[1])));
  const epoch = Number(m[1]), anchor = m[2];
  const prefix = path.join(spool, 'working', name);
  const archive = path.join(spool, 'deferred', name);
  const filename = name + '.receipts.json';
  const states = ['ready', 'working', 'deferred', 'processed', 'failed'].filter(state =>
    exists(path.join(spool, state, filename)));
  assert(states.length === 1, 'Ambiguous TAP draft lifecycle');
  const cursor = loadCursor(spool);
  if (states[0] === 'deferred') {
    assert(['retire', 'fence'].includes(action));
    return publishHandoff({ spool, name, dir, cursor });
  }
  const state = states[0];
  const fence = action === 'fence';
  assert(state === 'working' || (action !== 'reprice' && state === 'failed'), 'TAP draft is not available for recovery');
  const item = path.join(spool, state, name);
  const file = item + '.receipts.json';
  const committed = reservedHandoffTicket(spool, epoch, anchor);
  if (committed) {
    assert(action !== 'reprice', 'TAP reservation already fenced');
    const raw = readDraftFile(archive + '.draft.json');
    const cert = JSON.parse(raw);
    assert(digest(raw) === committed.proof_sha256 && cert.type === 'reserved_tap_sweep' &&
      cert.epoch === epoch && cert.epoch_apply_hash === anchor && cert.source_state === state &&
      cert.bundle_sha256 === digestFile(file) && cert.bundle_sha256 === json(path.join(dir, 'tap.produced')).evidence_sha256,
    'TAP fence differs from its committed source');
    assert.deepEqual(cert.completed_cursor, cursor);
    archiveBundle(file, archive);
    return publishHandoff({ spool, name, dir, cursor });
  }
  let bundle;
  if (fence) {
    // The confirmed reservation binds the hash of the complete canonical
    // bundle, including signed receipts. A projected header cannot match it.
    assert(fs.lstatSync(file).isFile(), 'Invalid TAP reservation bundle');
    bundle = JSON.parse(fs.readFileSync(file, 'utf8'));
  } else if (action === 'reprice') {
    bundle = loadedBundle ?? json(file);
  } else {
    bundle = readDraftHeader(file,
      ['rail', 'epoch', 'epoch_apply_hash', 'at', 'tap_liability_sweep', 'refunds', 'buyer_refunds', 'buyerRefunds'], true);
  }
  assert(bundle.epoch === epoch && bundle.epoch_apply_hash === anchor);
  const processed = path.join(spool, 'processed', name);
  assert(!exists(processed + '.settlement.json'), 'TAP settlement publication already started');
  const latest = path.join(spool, 'processed/latest.json');
  const prior = exists(latest) ? readDraftHeader(latest, ['epoch', 'epoch_apply_hash', 'root', 'root_confirmed', 'root_preparation_id', 'cumulative_spent_wei',
    'operator_fee', 'burn', 'providers', 'refunds', 'entries', 'tap_settlement_checkpoint']) : null;
  const inputs = { bundle, prior, peerRpcUrl, fetchImpl };
  const companions = fence ? fenceCompanions(prefix, item) : null;
  let proof;
  if (fence) {
    proof = await proveReservedTapDraft({ ...inputs, pool: pool ?? openPool(), companions });
  } else {
    const reports = assertLocalDraft(prefix, processed);
    if (state === 'failed') reports.push(...assertLocalDraft(item, processed));
    proof = await proveUnpreparedTapDraft({ ...inputs, pool: pool ?? openPool(), reports });
  }
  if (action === 'reprice') {
    const lockFile = prefix + '.tap-rate.json';
    const old = validateTapRateLock(json(lockFile), bundle, proof.scope);
    const { contract, basis } = proof;
    const verdict = await contract.guardianRequireHistoricalTapRateLock(old, basis.timestamp);
    if (!(verdict instanceof Error)) return false;
    if (verdict.message !== 'Guardian TAP settlement rate invariant failed: oracle is stale.') throw verdict;
    const next = tapRateRecordToLock(await contract.get('tap/rate/latest'), bundle, await contract.get('admin'), proof.scope);
    assert(next.rate_ts > old.rate_ts && next.rate_ts <= basis.timestamp, 'No newer usable TAP quote');
    const valid = await contract.guardianRequireHistoricalTapRateLock(next, basis.timestamp);
    if (valid instanceof Error) throw valid;
    durableWrite(lockFile, next, true);
    return true;
  }

  assert(fence || action === 'retire');
  assert(state === 'failed' || exists(prefix + '.draft-retry'), 'TAP draft has not failed simulation');
  const head = await proof.contract.get('epoch/apply/state');
  assert(head?.updated_epoch === successor.epoch && head.last_apply_hash === successor.anchor &&
    uint(successor.epoch) && successor.epoch > epoch && hash(successor.anchor) && head.pending_epoch == null,
  'TAP successor is not the confirmed applied epoch');
  const intentFile = path.join(dir, 'tap-sweep-intent.json');
  const intent = exists(intentFile) ? json(intentFile) : null;
  if (intent) validateIntent(intent, epoch, anchor);
  else assert(fence && bundle.tap_liability_sweep === undefined, 'Missing TAP sweep intent');
  const active = readHandoff(spool, cursor, name);
  assert((!cursor || cursor.epoch < epoch) && (!active || active.ticket.epoch < epoch),
    'TAP recovery would regress the scheduler');
  const openingCursor = active ? active.proof.retry_after : cursor?.next_after ?? null;
  if (intent) {
    assert.deepEqual(bundle.tap_liability_sweep, sweepPointerForIntent(intent));
    assert.equal(bundle.at, intent.at);
  }
  const produced = json(path.join(dir, 'tap.produced'));
  const bundleDigest = digestFile(file);
  assert(produced.schema_version === 1 && produced.rail === 'tap' && produced.epoch === epoch && produced.epoch_apply_hash === anchor &&
    produced.status === 'produced' && produced.evidence_sha256 === bundleDigest, 'TAP produced draft changed');
  // Never generate a competing transition after local checkpoint recovery started.
  for (const suffix of ['tap-contract-checkpoint.json', 'tap-contract-submission.json', 'tap.complete']) {
    assert(!exists(path.join(dir, suffix)), 'TAP checkpoint recovery already started');
  }
  const buyerFiles = [...new Set([prefix, item])].map(file => file + '.buyers.json').filter(exists);
  assert(buyerFiles.length <= 1, 'Ambiguous TAP buyer companion');
  const buyers = buyerFiles[0];
  assert(!exists(path.join(spool, 'ready', name + '.buyers.json')), 'TAP buyer companion is in transit');
  const refunds = transferableTapRefunds(bundle, buyers ? json(buyers) : {});
  const q = refundQueue(cursor, active);
  // The original root may have included this inherited slice as well as
  // its own refund inputs. Move both to the same fenced source exactly once.
  let position = q;
  if (fence && Object.hasOwn(bundle, 'tap_carried_refunds')) {
    const batch = nextRefunds(spool, q);
    assert(batch && Array.isArray(bundle.tap_carried_refunds), 'Missing inherited TAP refund slice');
    assert.deepEqual(bundle.tap_carried_refunds, batch.entries, 'TAP reservation refund source changed');
    position = batch.next;
    refunds.push(...batch.entries);
  }
  const nextQueue = { ...position, tail: position.tail + (refunds.length ? 1 : 0) };
  if (fence && refunds.length) nextQueue.reconciled_epoch ??= 0;
  assert(Number.isSafeInteger(nextQueue.tail));
  const cert = { at: intent?.at ?? proof.at, source_state: state, buyers_sha256: buyers ? digestFile(buyers) : null,
    refund_queue: nextQueue, type: fence ? 'reserved_tap_sweep' : 'unprepared_tap_sweep',
    ...(fence && { observed_liability_preparations: proof.reservations, finalized_pool_basis: proof.finalized,
      companion_sha256: Object.fromEntries(Object.entries(companions).map(([ext, file]) => [ext, digestFile(file)])) }),
    epoch, epoch_apply_hash: anchor, bundle_sha256: bundleDigest,
    signed_length: proof.contract.signedLength, pool_basis: proof.basis, completed_cursor: cursor,
    predecessor: active?.ticket ?? null, successor, refunds,
    retry_after: !fence && bundle.tap_liability_sweep.limit === 1 && intent.after === openingCursor ? proof.page.next : openingCursor };
  fs.mkdirSync(path.dirname(archive), { recursive: true, mode: 0o700 });
  syncDirectory(spool);
  durableWrite(archive + '.draft.json', cert, true);
  if (fence) {
    // A queued successor must see this exclusion before it can see an empty
    // working slot, even when an older handoff ticket already exists.
    const ticket = { epoch, anchor, proof_sha256: digest(readDraftFile(archive + '.draft.json')), reserved: true };
    durableWrite(handoffFile(spool), ticket, true);
  }
  archiveBundle(file, archive);
  return publishHandoff({ spool, name, dir, cursor });
}

function publishHandoff({ spool, name, dir, cursor }) {
  const archive = path.join(spool, 'deferred', name);
  const raw = readDraftFile(archive + '.draft.json');
  const cert = JSON.parse(raw);
  const ticket = { epoch: cert.epoch, anchor: cert.epoch_apply_hash, proof_sha256: digest(raw),
    ...(cert.type === 'reserved_tap_sweep' && { reserved: true }) };
  assert(name === 'epoch-' + ticket.epoch + '-' + ticket.anchor && ['unprepared_tap_sweep', 'reserved_tap_sweep'].includes(cert.type) &&
    cert.bundle_sha256 === digestFile(archive + '.receipts.json') &&
    cert.bundle_sha256 === json(path.join(dir, 'tap.produced')).evidence_sha256, 'Conflicting archived TAP draft');
  assert.deepEqual(cert.completed_cursor, cursor);
  const active = readHandoff(spool, cursor, name);
  const committed = active?.ticket.epoch === ticket.epoch;
  if (committed) assert.deepEqual(active.ticket, ticket);
  else assert.deepEqual(cert.predecessor, active?.ticket ?? null);
  if (cert.refunds.length) {
    const file = refundSourceFile(spool, cert.refund_queue.tail);
    if (exists(file)) assert.deepEqual(json(file), ticket, 'Conflicting TAP refund source');
    else durableWrite(file, ticket, false);
  }
  if (!committed) durableWrite(handoffFile(spool), ticket, true);
  const working = path.join(spool, 'working', name);
  assert(['working', 'failed'].includes(cert.source_state));
  for (const origin of new Set([working, path.join(spool, cert.source_state, name)])) {
    for (const ext of cert.type === 'reserved_tap_sweep' ? reservedCompanions : draftCompanions) {
      if (!exists(origin + ext)) continue;
      assert(!exists(archive + ext), 'Duplicate archived TAP draft companion');
      fs.renameSync(origin + ext, archive + ext);
    }
    syncDirectory(path.dirname(origin));
  }
  if (cert.type === 'reserved_tap_sweep') {
    for (const [ext, expected] of Object.entries(cert.companion_sha256)) {
      assert(reservedCompanions.includes(ext) && digestFile(archive + ext) === expected, 'Archived TAP reservation evidence changed');
    }
  }
  assert.equal(exists(archive + '.buyers.json') ? digestFile(archive + '.buyers.json') : null,
    cert.buyers_sha256, 'Archived TAP refund companion changed');
  syncDirectory(path.dirname(archive));
  // A later cursor can advance as soon as this marker is visible. Make it
  // durable before releasing either lock, including on crash recovery.
  const marker = { schema_version: 1, rail: 'tap', status: cert.type === 'reserved_tap_sweep' ? 'fenced' : 'superseded', epoch: cert.epoch,
    epoch_apply_hash: cert.epoch_apply_hash, canonical_settlement_unix: cert.at,
    evidence: archive + '.draft.json', evidence_sha256: ticket.proof_sha256 };
  const file = path.join(dir, 'tap.complete');
  if (exists(file)) assert.deepEqual(json(file), marker);
  else durableWrite(file, marker, false);
  return true;
}

if (process.argv[1] && fs.existsSync(process.argv[1]) && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let provider;
  try {
    const [action, spool, name, peerRpcUrl, dir, epoch, anchor] = process.argv.slice(2);
    assert(['reprice', 'retire', 'fence'].includes(action));
    if (action !== 'reprice') {
      assert(path.basename(dir) === name && path.basename(path.dirname(dir)) === 'payout');
      holdTapLock(path.resolve(dir, '../../payout.lock'), 9);
    }
    holdTapLock(path.join(spool, 'roller.lock'), 8);
    const openPool = () => {
      const eth = process.env.MAYHEM_TAP_ETH_RPC;
      assert(eth, 'Missing TAP recovery RPC');
      const rpc = new FetchRequest(eth);
      rpc.timeout = 10_000;
      provider = new JsonRpcProvider(rpc, 1, { staticNetwork: true, cacheTimeout: -1 });
      return new Contract(POOL, POOL_SETTLEMENT_ABI, provider);
    };
    await recoverDraft({ action, spool, name, peerRpcUrl, dir, successor: { epoch: Number(epoch), anchor }, openPool });
  } catch {
    console.error('TAP draft recovery has insufficient evidence to change this attempt');
    process.exitCode = 1;
  } finally { provider?.destroy(); }
}
