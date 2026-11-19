// Called under the same exclusive spool lock as preparation and broadcast.
// Only the payout worker can publish a successor, under its payout lock too.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Contract, FetchRequest, JsonRpcProvider, ZeroHash } from 'ethers';
import { confirmedTapContract, MAINNET_POOL as POOL, MAINNET_TOKEN as TOKEN, tapDraftWatermarks } from './tap-liability-sweep.mjs';
import { canonicalTapPaymentScope, POOL_SETTLEMENT_ABI, tapRateRecordToLock, transferableTapRefunds,
  validateTapRateLock } from './tap-settlement-roller.mjs';
import { loadCursor, validateIntent } from './prepare-tap-sweep-bundle.mjs';
import { digest, durableWrite, handoffFile, holdTapLock, readDraftFile, readHandoff, refundQueue, refundSourceFile, syncDirectory } from './tap-draft-files.mjs';

const hash = s => typeof s === 'string' && /^[0-9a-f]{64}$/.test(s);
const evmHash = s => typeof s === 'string' && /^0x[0-9a-f]{64}$/.test(s);
const uint = x => Number.isSafeInteger(x) && x >= 0;
const json = file => JSON.parse(readDraftFile(file));
const exists = file => fs.lstatSync(file, { throwIfNoEntry: false }) !== undefined;

async function idlePool(pool, epoch) {
  const provider = pool.runner.provider;
  assert.equal(await provider.send('eth_chainId', []), '0x1', 'Wrong TAP recovery network');
  assert.equal((await pool.getAddress()).toLowerCase(), POOL);
  const head = await provider.getBlock('latest');
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
      prep.liability.paid_cum_au_before === before, 'Inconsistent TAP liability lock');
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
  assert(basis.timestamp >= bundle.at, 'Ethereum head predates TAP snapshot');
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
    const retained = await contract.get('settle/targeted/tap/' + basis.epoch);
    assert(retained?.type === 'targeted_tap_settlement' && retained.settled_by === admin &&
      retained.settled_by_role === 'admin' && retained.root === cp.root, 'Missing TAP predecessor reconciliation');
    for (const [key,value] of Object.entries(cp)) assert.deepEqual(retained[key], value, 'Conflicting TAP predecessor ' + key);
    assert.deepEqual(prior.providers, cp.provider_entries, 'Conflicting TAP predecessor provider claims');
    assert.deepEqual(prior.refunds, cp.refunds, 'Conflicting TAP predecessor refund claims');
    assert.deepEqual(prior.entries, cp.entries, 'Conflicting TAP predecessor distribution');
  }
  return { contract, scope, basis, page };
}

function assertLocalDraft(prefix, processedPrefix) {
  // The old wrapper created .settlement.raw.json BEFORE entering --confirm.
  // Its presence, even empty, can mean a submitted request lost its reply.
  for (const ext of ['.prepare-intent.json', '.settlement.raw.json', '.settlement.bound.json', '.settlement.json']) {
    assert(!exists(prefix + ext), 'TAP confirmation previously started');
  }
  assert(!exists(processedPrefix + '.settlement.json'), 'TAP settlement publication already started');
  const reports = [];
  for (const ext of ['.dry-run.json', '.confirmation.json', '.draft-plan.json']) {
    if (!exists(prefix + ext)) continue;
    const bytes = readDraftFile(prefix + ext).toString().trim();
    if (!bytes) continue;
    const report = JSON.parse(bytes);
    reports.push(report);
    assert(report && typeof report === 'object' && !Array.isArray(report) &&
      ['posted', 'root_confirmed', 'root_already_posted', 'root_pending', 'root_proposed',
        'awaiting_finality', 'awaiting_governance_delay', 'canonical_preparation_confirmed'].every(k =>
        report[k] == null || report[k] === false) &&
      ['proposal_tx', 'execution_tx', 'pending_root', 'root_confirmation', 'tap_settlement_checkpoint'].every(k => report[k] == null) &&
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

export async function recoverDraft({ action, spool, name, dir, successor, peerRpcUrl, pool, openPool, fetchImpl }) {
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
    assert.equal(action, 'retire');
    return publishHandoff({ spool, name, dir, cursor });
  }
  const state = states[0];
  assert(state === 'working' || (action === 'retire' && state === 'failed'), 'TAP draft is not available for recovery');
  const item = path.join(spool, state, name);
  const raw = readDraftFile(item + '.receipts.json');
  const bundle = JSON.parse(raw);
  assert(bundle.epoch === epoch && bundle.epoch_apply_hash === anchor);
  const reports = assertLocalDraft(prefix, path.join(spool, 'processed', name));
  if (state === 'failed') reports.push(...assertLocalDraft(item, path.join(spool, 'processed', name)));
  const latest = path.join(spool, 'processed/latest.json');
  const prior = exists(latest) ? json(latest) : null;
  const proof = await proveUnpreparedTapDraft({ bundle, pool: pool ?? openPool(), prior, peerRpcUrl, fetchImpl, reports });
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

  assert.equal(action, 'retire');
  assert(state === 'failed' || exists(prefix + '.draft-retry'), 'TAP draft has not failed simulation');
  const head = await proof.contract.get('epoch/apply/state');
  assert(head?.updated_epoch === successor.epoch && head.last_apply_hash === successor.anchor &&
    uint(successor.epoch) && successor.epoch > epoch && hash(successor.anchor) && head.pending_epoch == null,
  'TAP successor is not the confirmed applied epoch');
  const intent = json(path.join(dir, 'tap-sweep-intent.json'));
  validateIntent(intent, epoch, anchor);
  const active = readHandoff(spool, cursor, name);
  assert((!cursor || cursor.epoch < epoch) && (!active || active.ticket.epoch < epoch),
    'TAP recovery would regress the scheduler');
  const openingCursor = active ? active.proof.retry_after : cursor?.next_after ?? null;
  assert.deepEqual(bundle.tap_liability_sweep, { schema_version: intent.schema_version,
    signed_length: intent.signed_length, after: intent.after, ...(intent.handoff && { limit: 1 }) });

  assert.equal(bundle.at, intent.at);
  const produced = json(path.join(dir, 'tap.produced'));
  assert(produced.schema_version === 1 && produced.rail === 'tap' && produced.epoch === epoch && produced.epoch_apply_hash === anchor &&
    produced.status === 'produced' && produced.evidence_sha256 === digest(raw), 'TAP produced draft changed');
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
  const nextQueue = { ...q, tail: q.tail + (refunds.length ? 1 : 0) };
  assert(Number.isSafeInteger(nextQueue.tail));
  const cert = { at: intent.at, source_state: state, buyers_sha256: buyers ? digest(readDraftFile(buyers)) : null,
    refund_queue: nextQueue, type: 'unprepared_tap_sweep', epoch, epoch_apply_hash: anchor, bundle_sha256: digest(raw),
    signed_length: proof.contract.signedLength, pool_basis: proof.basis, completed_cursor: cursor,
    predecessor: active?.ticket ?? null, successor, refunds,
    retry_after: bundle.tap_liability_sweep.limit === 1 && intent.after === openingCursor ? proof.page.next : openingCursor };
  fs.mkdirSync(path.dirname(archive), { recursive: true, mode: 0o700 });
  syncDirectory(spool);
  durableWrite(archive + '.draft.json', cert, true);
  fs.renameSync(item + '.receipts.json', archive + '.receipts.json');
  syncDirectory(path.dirname(archive));
  syncDirectory(path.dirname(item));
  return publishHandoff({ spool, name, dir, cursor });
}

function publishHandoff({ spool, name, dir, cursor }) {
  const archive = path.join(spool, 'deferred', name);
  const raw = readDraftFile(archive + '.draft.json');
  const cert = JSON.parse(raw);
  const ticket = { epoch: cert.epoch, anchor: cert.epoch_apply_hash, proof_sha256: digest(raw) };
  assert(name === 'epoch-' + ticket.epoch + '-' + ticket.anchor && cert.type === 'unprepared_tap_sweep' &&
    cert.bundle_sha256 === digest(readDraftFile(archive + '.receipts.json')) &&
    cert.bundle_sha256 === json(path.join(dir, 'tap.produced')).evidence_sha256, 'Conflicting archived TAP draft');
  assert.deepEqual(cert.completed_cursor, cursor);
  const active = readHandoff(spool, cursor, name);
  if (active?.ticket.epoch === ticket.epoch) assert.deepEqual(active.ticket, ticket);
  else {
    assert.deepEqual(cert.predecessor, active?.ticket ?? null);
    if (cert.refunds.length) {
      const file = refundSourceFile(spool, cert.refund_queue.tail);
      if (exists(file)) assert.deepEqual(json(file), ticket, 'Conflicting TAP refund source');
      else durableWrite(file, ticket, false);
    }
    durableWrite(handoffFile(spool), ticket, true);
  }
  const working = path.join(spool, 'working', name);
  assert(['working', 'failed'].includes(cert.source_state));
  for (const origin of new Set([working, path.join(spool, cert.source_state, name)])) {
    for (const ext of ['.buyers.json', '.tap-rate.json', '.tap-rate.json.events.json', '.dry-run.json',
      '.draft-plan.json', '.draft-retry', '.confirmation.json']) {
      if (!exists(origin + ext)) continue;
      assert(!exists(archive + ext), 'Duplicate archived TAP draft companion');
      fs.renameSync(origin + ext, archive + ext);
    }
    syncDirectory(path.dirname(origin));
  }
  assert.equal(exists(archive + '.buyers.json') ? digest(readDraftFile(archive + '.buyers.json')) : null,
    cert.buyers_sha256, 'Archived TAP refund companion changed');
  syncDirectory(path.dirname(archive));
  // A later cursor can advance as soon as this marker is visible. Make it
  // durable before releasing either lock, including on crash recovery.
  const marker = { schema_version: 1, rail: 'tap', status: 'superseded', epoch: cert.epoch,
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
    assert(['reprice', 'retire'].includes(action));
    if (action === 'retire') {
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
