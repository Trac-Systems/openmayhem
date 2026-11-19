// One unattended tick. Both spool locks are held by the daemon for this action.
// Only a confirmed checkpoint closes a debt. Every POST can lose its response.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ZeroHash } from 'ethers';
import { safeErrorMessage } from './safe-output.mjs';
import { digestFile, durableWrite, holdTapLock, indexedDebt, readDraftFile, registerFencedDebt, syncDirectory, tapPlanSource, verifyTapSourceSeal } from './tap-draft-files.mjs';
import { confirmedTapContract, MAINNET_POOL, MAINNET_TOKEN, tapContractAt, verifyPriorTapClaims } from './tap-liability-sweep.mjs';
import { connectTapSettlementPool, cumulativeTapRefunds, lookupProposedRoot, projectTapRefunds, rollPreparedTapSettlement, rollTapAuAlternative, settlementBundleSha256,
  submitCanonicalTapPreparationPlan, tapPreparationAdminSignerFromEnv } from './tap-settlement-roller.mjs';
import { fencedSourceEvidence } from './tap-draft-recovery.mjs';
import { tapRootEventReader } from './tap-root-events.mjs';
import { loadCursor } from './prepare-tap-sweep-bundle.mjs';
import { verifyFinalizedTapProposal } from './tap-finalized-proposal.mjs';
import { buildCanonicalTapSubmission, buildTapAuWinnerSubmission } from './verify-tap-sweep-report.mjs';

const jobName = /^epoch-([1-9][0-9]*)-([0-9a-f]{64})$/;
const json = file => JSON.parse(readDraftFile(file));
const exists = file => fs.lstatSync(file, { throwIfNoEntry: false }) !== undefined;
const recordKey = epoch => 'settle/targeted/tap/' + epoch;
const resolutionKey = source => 'payout/recovery/tap/' + source.root_preparation_id + '/resolution';
const checked = result => { if (result instanceof Error) throw result; return result; };
const nameFor = (epoch, anchor) => 'epoch-' + epoch + '-' + anchor;

function immutable(file, value) {
  if (exists(file)) assert.deepEqual(json(file), value, 'Conflicting TAP debt evidence');
  else durableWrite(file, value, false);
}

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  syncDirectory(path.dirname(dir));
}

function importEarlierFences(spool, payout) {
  const dir = path.join(spool, 'deferred');
  if (!exists(dir)) return;
  const root = path.join(spool, 'debt');
  const done = path.join(root, 'indexed-import.json');
  if (exists(done)) return;
  const stream = fs.opendirSync(dir);
  try {
    let entry;
    while ((entry = stream.readSync()) !== null) {
      const file = entry.name;
      const name = file.replace(/\.draft\.json$/, '');
      if (file === name || !jobName.test(name)) continue;
      const marker = path.join(payout, name, 'tap.complete');
      if (!exists(marker)) continue; // its resumable publisher registers it before completion
      let receipt;
      try { receipt = json(marker); } catch { /* retain a retryable entry for this source alone */ }
      if (receipt?.status === 'superseded') continue;
      const [, epoch, anchor] = jobName.exec(name);
      registerFencedDebt(spool, name, { epoch: Number(epoch), anchor,
        proof_sha256: /^[0-9a-f]{64}$/.test(receipt?.evidence_sha256 ?? '') ? receipt.evidence_sha256 : digestFile(path.join(dir, file)), reserved: true });
    }
  } finally {
    stream.closeSync();
  }
  ensureDir(root);
  durableWrite(done, true, false);
}

async function sourceEvidence({ spool, name, ticket, cert, bundle, contract, pool, peerRpcUrl, fetchImpl }) {
  const archive = path.join(spool, 'deferred', name);
  for (const [ext, digest] of Object.entries(cert.companion_sha256)) {
    assert(/^\.[a-z.-]+$/.test(ext) && digestFile(archive + ext) === digest, 'Fenced TAP companion changed');
  }
  const companions = Object.fromEntries(Object.keys(cert.companion_sha256).map(ext => [ext, archive + ext]));
  const basis = cert.pool_basis;
  const oldContract = tapContractAt(peerRpcUrl, cert.signed_length, fetchImpl);
  oldContract.signedLength = cert.signed_length;
  let prior = null, opening = null;
  if (basis.epoch) {
    const cp = await oldContract.get(recordKey(basis.epoch));
    assert(cp?.root === basis.root && cp.cumulative_spent_wei === basis.spent, 'Conflicting original opening checkpoint');
    const { type, settled_by_role, settled_by, ...value } = cp;
    verifyCheckpoint(cp, value, await oldContract.get('admin'));
    opening = cp;
    prior = { ...value, providers: cp.provider_entries, tap_settlement_checkpoint: value };
  }
  const evidence = cert.debt_evidence ?? await fencedSourceEvidence({ bundle, prior, companions, peerRpcUrl, fetchImpl,
    proof: { contract: oldContract, basis, at: cert.at, lock: json(archive + '.tap-rate.json'), reservations: cert.observed_liability_preparations } });
  assert.deepEqual(evidence.opening_checkpoint, opening);
  const source = evidence.source, intent = source.intent;
  assert(intent.epoch === ticket.epoch && intent.epoch_apply_hash === ticket.anchor && intent.at === cert.at &&
    intent.chain_id === 1 && intent.pool_address === MAINNET_POOL && intent.token_address === MAINNET_TOKEN &&
    intent.tap_rate_lock.bundle_sha256 === settlementBundleSha256(bundle), 'Archived TAP source identity changed');
  assert.deepEqual(intent.tap_rate_lock, json(archive + '.tap-rate.json'));
  const { tapDebtPlan } = await import('../../intercom/contract/tap-prepared-debt.js');
  const plan = await tapDebtPlan(contract, intent);
  assert.deepEqual(tapPlanSource(plan), source);
  for (const id of cert.observed_liability_preparations) {
    assert(source.preparation_ids.includes(id) && evidence.records.some(r => r.economic_op_id === id));
  }
  if (companions['.sealed-plan.json']) {
    const seal = verifyTapSourceSeal(json(companions['.sealed-plan.json']), await contract.get('admin'));
    assert.deepEqual(seal.source, source);
    assert.deepEqual(seal.opening_checkpoint, prior?.tap_settlement_checkpoint ?? null);
    assert.deepEqual(seal.selection_block, evidence.selection_block);
    const selected = await pool.runner.provider.getBlock(seal.selection_block.number);
    assert(selected?.hash === seal.selection_block.hash && selected.timestamp === seal.selection_block.timestamp &&
      selected.timestamp >= source.intent.at, 'Unconfirmed TAP financial selection block');
    checked(await oldContract.guardianRequireHistoricalTapRateLock(intent.tap_rate_lock, selected.timestamp));
  }
  return { source, evidence, plan, archive };
}

// The rate accepted for an already pending or mined effect must precede its
// broadcast. A subsequent rate with an earlier oracle timestamp cannot renew it.
async function proposalRate({ contract, base, pool, originalId, at, proposal }) {
  if (!proposal) return (await contract.get('tap/rate/latest'))?.updated_at;
  const file = base + '.' + originalId + '.' + proposal.nonce + '.quotes.json';
  if (!exists(file)) return undefined; // only the original lock may authorize that mined time
  for (const choice of json(file).toReversed()) {
    if (choice.block.number >= proposal.block_number || choice.block.timestamp > at) continue;
    const selected = await pool.runner.provider.getBlock(choice.block.number);
    assert(selected?.hash === choice.block.hash && selected.timestamp === choice.block.timestamp,
      'Conflicting pre-broadcast TAP quote block');
    return choice.rate_key ?? undefined;
  }
  return undefined;
}

// Clients which implement EIP-1186 but only accept a block number can
// supply the same witness. Both header reads and the trie root bind its block.
async function epochProofRead(cp, read, method, params) {
  if (method !== 'eth_getProof') return read(method, params);
  try { return await read(method, params); } catch (error) {
    const hash = params[2]?.blockHash;
    if (params[2]?.requireCanonical !== true || !/^0x[0-9a-f]{64}$/.test(hash)) throw error;
    const header = await read('eth_getBlockByHash', [hash, false]);
    assert(header?.hash === hash, 'Unexpected TAP proof block hash');
    const height = Number(BigInt(header.number));
    assert(Number.isSafeInteger(height) && height >= 0 && height <= cp.execution_block_number);
    const tag = '0x' + height.toString(16);
    const canonical = async () => assert.equal((await read('eth_getBlockByNumber', [tag, false]))?.hash,
      hash, 'TAP proof block is not canonical');
    await canonical();
    const proof = await read(method, [params[0], params[1], tag]);
    await canonical();
    return proof;
  }
}

async function postFeature(peerRpcUrl, fetchImpl, feature) {
  const url = new URL('contract/feature', peerRpcUrl.replace(/\/+$/, '') + '/');
  const res = await fetchImpl(url, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(30_000),
    headers: { 'content-type': 'application/json' }, body: JSON.stringify({ feature: 'mayhem', ...feature }) });
  assert(res.ok && (await res.json()).ok === true, 'TAP debt Core submission was not acknowledged');
}

function verifyCheckpoint(record, cp, admin) {
  assert.deepEqual(record, { ...cp, type: 'targeted_tap_settlement', settled_by: admin, settled_by_role: 'admin' },
    'TAP debt checkpoint lacks an exact confirmed Core readback');
}

// Two kinds of refund work can coexist. Only the original, as-yet-uncarried
// inputs which this signed plan actually credits may leave their queue.
function refundDisposition({ cert, source, bundle, archive, prior, plan, spool, evidence }) {
  if (!cert.refunds.length && !cert.reserved_refund_credits) return null;
  const buyers = exists(archive + '.buyers.json') ? json(archive + '.buyers.json') : {};
  const projection = projectTapRefunds(bundle, buyers, source.intent.tap_rate_lock.tap_usd_au, true);
  const originalOpening = cert.pool_basis.epoch ? evidence.opening_checkpoint : null;
  const credited = cumulativeTapRefunds(projection.credits, originalOpening);
  assert.deepEqual(credited, source.intent.refunds, 'Original TAP refunds do not match fenced inputs');
  if (cert.reserved_refund_credits) {
    assert(cert.companion_sha256['.sealed-plan.json'], 'Refund assignment lacks an authenticated whole plan');
    assert.deepEqual(cert.reserved_refund_credits, projection.credits);
    assert.deepEqual(cert.refunds, projection.unpaid);
    assert.deepEqual(plan.intent.refunds, cumulativeTapRefunds(projection.credits, prior));
    return null; // the ordinary queue already owns exactly the unsigned remainder
  }
  if (!projection.credits.length && plan.intent.epoch !== source.intent.epoch) return null;
  if (plan.intent.epoch !== source.intent.epoch &&
      JSON.stringify(plan.intent.refunds) === JSON.stringify(prior?.refunds ?? [])) return null;
  assert.deepEqual(plan.intent.refunds, cumulativeTapRefunds(projection.credits, prior));
  const cursor = loadCursor(spool)?.refund_queue;
  const index = cert.refund_queue.tail;
  assert(!cursor || cursor.head < index - 1 || (cursor.head === index - 1 && cursor.offset === 0),
    'A disjoint sweep already consumed original refund inputs');
  return { index, unpaid: projection.unpaid };
}

const changesAmount = error => ['TAP renewal changes the prepared price outcome.',
  'TAP renewal cannot revalue an original refund.'].includes(error.message);

function offerFile(base, id) {
  assert(/^[0-9a-f]{64}$/.test(id), 'Invalid TAP candidate identifier');
  return base + '.offer-' + id + '.json';
}

function attemptFile(base, root) {
  assert(/^0x[0-9a-f]{64}$/.test(root));
  return base + '.' + root + '.attempts.json';
}

function eventsFor(base, pool, intent) {
  return tapRootEventReader({ pool, epoch: intent.epoch, rateTimestamp: intent.tap_rate_lock.rate_ts,
    file: base + '.' + intent.root + '.events.json' });
}

// A root and nonce bind the on-chain outcome. If two signed offers round to
// that outcome, use the last admitted selection preceding the actual mined block.
async function minedOffer({ base, pool, root, epoch, spent, nonce, head, source }) {
  const proposal = await lookupProposedRoot({ pool, epoch, root, cumulativeSpentWei: spent, nonce,
    tapRateLock: source.intent.tap_rate_lock, throughBlock: head.number,
    rootEventReader: await eventsFor(base, pool, { ...source.intent, root }) });
  const selection = json(attemptFile(base, root)).findLast(a =>
    a.nonce === proposal.nonce && a.block.number < proposal.block_number && a.block.timestamp <= proposal.block_timestamp);
  assert(selection, 'No admitted TAP alternative preceded the mined proposal');
  const selected = await pool.runner.provider.getBlock(selection.block.number);
  assert(selected?.hash === selection.block.hash && selected.timestamp === selection.block.timestamp, 'Candidate selection block changed');
  const candidate = json(offerFile(base, selection.candidate_id));
  assert.deepEqual(candidate.feature.value.source, source);
  assert(candidate.plan.intent.root === root && candidate.plan.intent.epoch === epoch &&
    candidate.plan.intent.cumulative_spent_wei === String(spent) &&
    candidate.plan.root_preparation_id === selection.candidate_id, 'Conflicting candidate payment evidence');
  return candidate;
}

async function restoreOriginalPrefix({ ctx, contract, source, cert, archive, bundle, basisEpoch, previousEpoch, proof }) {
  const { peerRpcUrl, fetchImpl, adminSigner } = ctx;
  const journal = archive + '.prepare-intent.json';
  const ids = { epoch: source.intent.epoch, epoch_apply_hash: source.intent.epoch_apply_hash,
    preparation_ids: source.preparation_ids, root_preparation_id: source.root_preparation_id,
    external_effect_ids: source.external_effect_ids };
  if (exists(journal)) assert.deepEqual(json(journal), ids, 'Original TAP journal conflicts with its reservation');
  // The first signed preparation commits the bundle hash. A signed sweep
  // selector in that bundle lets the canonical restorer independently derive
  // every missing row even if the old writer died before creating its journal.
  assert(exists(journal) || cert.companion_sha256['.sealed-plan.json'] || bundle.tap_liability_sweep,
    'Partial historical TAP reservation lacks a signed full plan or a committed frozen liability page');
  const { restoreTapDebtPrefix } = await import('./tap-prepared-debt.mjs');
  const { features } = await restoreTapDebtPrefix({ contract, source, sealed: ids, originalBundle: bundle,
    openingEpoch: cert.pool_basis.epoch, basisEpoch, previousEpoch, skippedEpochProof: proof,
    sign: message => adminSigner.sign(message), frozenFetch: (url, init) => {
      const target = new URL('state', peerRpcUrl.replace(/\/+$/, '') + '/');
      target.search = url.search;
      return fetchImpl(target, init);
    } });
  assert(features.length, 'No missing original preparation at the pinned checkout');
  await postFeature(peerRpcUrl, fetchImpl, features[0]);
  return 'original preparation submitted';
}

// Pool epochs increase strictly. Find the very next execution by block height,
// then authenticate it through the canonical proof collector. Each tick reads
// O(log(blocks)) scalar heads and stores at most one certificate.
async function nextBasis({ ctx, contract, base, cert, source, opening, latest, read }) {
  const { pool, adminSigner, peerRpcUrl, fetchImpl } = ctx;
  const { collectTapEpochProof, certifyTapDebtBasis } = await import('./tap-prepared-debt.mjs');
  const progress = base + '.basis.json';
  const openingEpoch = cert.pool_basis.epoch;
  const previousEpoch = exists(progress) ? json(progress).epoch : openingEpoch;
  const certKey = epoch => 'payout/recovery/tap/' + source.root_preparation_id + '/basis/' + epoch;
  const previous = previousEpoch === openingEpoch ? opening : await contract.get(recordKey(previousEpoch));
  if (previousEpoch !== openingEpoch) {
    const saved = json(progress);
    const accepted = await contract.get(certKey(previousEpoch));
    assert(previous?.execution_block_hash === accepted?.execution_block_hash && accepted.submission === saved.submission &&
      accepted.root === previous.root && accepted.opening_epoch === openingEpoch &&
      accepted.basis_epoch === previousEpoch, 'Missing confirmed historical TAP evidence');
  }
  assert(previousEpoch < latest.epoch);
  let cp = latest;
  const epochAt = async blockTag => Number(await pool.epoch({ blockTag }));
  let low = previousEpoch === openingEpoch ? cert.pool_basis.block : previous.execution_block_number;
  let high = latest.execution_block_number;
  assert(Number.isSafeInteger(low) && low >= 0 && low < high && Number.isSafeInteger(high));
  if (await epochAt(high - 1) !== previousEpoch) {
    while (low + 1 < high) {
      const mid = low + Math.floor((high - low) / 2);
      if (await epochAt(mid) <= previousEpoch) low = mid;
      else high = mid;
    }
    const epoch = await epochAt(high);
    cp = await contract.get(recordKey(epoch));
    assert(cp?.epoch === epoch && cp.execution_block_number === high && previousEpoch < epoch && epoch < latest.epoch,
      'Ethereum execution lacks its corresponding confirmed Core checkpoint');
  }
  Object.assign(ctx.facts, { history_previous_epoch: previousEpoch, history_next_epoch: cp.epoch,
    history_execution_block: cp.execution_block_hash });
  const proofFor = () => collectTapEpochProof({ checkpoint: cp, source, openingCheckpoint: opening, previousCheckpoint: previous,
    read: (method, params) => epochProofRead(cp, read, method, params) });
  if (cp.epoch === latest.epoch) return { proof: await proofFor(), previousEpoch };
  const accepted = await contract.get(certKey(cp.epoch));
  if (accepted) {
    assert(accepted.opening_epoch === openingEpoch && accepted.basis_epoch === cp.epoch &&
      accepted.root === cp.root && accepted.execution_block_hash === cp.execution_block_hash);
    durableWrite(progress, { epoch: cp.epoch, submission: accepted.submission }, true);
    return { status: 'historical checkpoint confirmed' };
  }
  const file = base + '.basis-' + cp.epoch + '.json';
  const saved = exists(file) ? json(file) : null;
  const { feature } = await certifyTapDebtBasis({ contract, source, openingEpoch, basisEpoch: cp.epoch, previousEpoch,
    skippedEpochProof: saved?.value.skipped_epoch_proof ?? await proofFor(),
    ...(saved ? { adminSignature: saved.value.admin_sig } : { sign: message => adminSigner.sign(message) }) });
  immutable(file, feature);
  await postFeature(peerRpcUrl, fetchImpl, feature);
  return { status: 'historical checkpoint submitted' };
}

async function dispatchDebt(ctx, name, ticket) {
  const { spool, payout, peerRpcUrl, fetchImpl, pool, ownerSigner, governanceSigner, operatorAddress, adminSigner } = ctx;
  const base = path.join(spool, 'debt', name);
  const archive = path.join(spool, 'deferred', name);
  const marker = json(path.join(payout, name, 'tap.complete'));
  assert(marker.status === 'fenced' && marker.evidence_sha256 === ticket.proof_sha256 &&
    digestFile(archive + '.draft.json') === ticket.proof_sha256);
  const cert = json(archive + '.draft.json');
  assert(ticket.reserved && cert.type === 'reserved_tap_sweep' && name === nameFor(cert.epoch, cert.epoch_apply_hash) &&
    digestFile(archive + '.receipts.json') === cert.bundle_sha256);
  const bundle = JSON.parse(fs.readFileSync(archive + '.receipts.json'));
  const contract = await confirmedTapContract(peerRpcUrl, fetchImpl);
  const { source, evidence, plan: original } = await sourceEvidence({ ...ctx, name, ticket, cert, bundle, contract });
  const admin = await contract.get('admin');
  const intent = source.intent;
  const read = (method, params) => pool.runner.provider.send(method, params);
  const latestFile = path.join(spool, 'processed/latest.json');
  const prior = exists(latestFile) ? json(latestFile) : null;
  const state = await contract.get('settle/targeted/tap/state/1/' + MAINNET_POOL);
  const basisEpoch = state?.last_epoch ?? 0;
  Object.assign(ctx.facts, { signed_length: contract.signedLength, original_epoch: intent.epoch,
    original_root: intent.root, original_preparation: source.root_preparation_id,
    observed_reservations: evidence.records.map(r => r.economic_op_id), opening_epoch: cert.pool_basis.epoch,
    basis_epoch: basisEpoch, basis_root: state?.last_root ?? ZeroHash });
  const slotFile = path.join(spool, 'debt/pool-slot.json');
  const resultFile = base + '.result.json';
  const transferFile = base + '.transfer.json';
  const finished = base + '.settled.json';
  if (exists(finished)) return 'settled';
  if (exists(resultFile)) {
    const { report, refund, root_preparation_id, candidate_id } = json(resultFile);
    const cp = report.tap_settlement_checkpoint;
    assert.equal(root_preparation_id, source.root_preparation_id);
    const block = await verifyFinalizedTapProposal({ report, chainId: 1, pool: MAINNET_POOL, read });
    const seen = await contract.get(recordKey(cp.epoch));
    let value;
    if (candidate_id) {
      const { collectTapEpochProof, confirmTapAuAlternative, settleTapAuAlternative } = await import('./tap-prepared-debt.mjs');
      const candidate = json(offerFile(base, candidate_id));
      assert.deepEqual(candidate.feature.value.source, source);
      assert.equal(candidate.plan.root_preparation_id, candidate_id);
      const admitted = await confirmTapAuAlternative({ contract, candidate });
      value = buildTapAuWinnerSubmission({ report, proposalBlock: block }, candidate);
      const winnerFile = base + '.winner.json';
      if (seen === null) {
        assert.equal(admitted.status, 'authorized');
        const saved = exists(winnerFile) ? json(winnerFile) : null;
        const executionProof = saved?.value.execution_proof ?? await collectTapEpochProof({ checkpoint: value, source,
          openingCheckpoint: evidence.opening_checkpoint, read: (method, params) => epochProofRead(cp, read, method, params) });
        const { feature } = await settleTapAuAlternative({ contract, candidate, checkpoint: value, executionProof,
          ...(saved ? { adminSignature: saved.value.admin_sig } : { sign: m => adminSigner.sign(m) }) });
        immutable(winnerFile, feature);
        await postFeature(peerRpcUrl, fetchImpl, feature);
        return 'winner submitted';
      }
      const resolution = await contract.get(resolutionKey(source));
      assert(admitted.status === 'settled' && resolution?.candidate_submission === candidate.feature.key &&
        resolution.root_preparation_id === candidate_id && resolution.submission === json(winnerFile).key,
        'Missing Core resolution for the finalized AU winner');
    } else {
      assert(cp.root_preparation_id === root_preparation_id ||
        (await contract.get(resolutionKey(source)))?.root_preparation_id === cp.root_preparation_id);
      value = buildCanonicalTapSubmission({ report, proposalBlock: block });
      if (seen === null) {
        assert(basisEpoch < cp.epoch, 'TAP recovery is behind the canonical settlement basis');
        await postFeature(peerRpcUrl, fetchImpl, { key: checked(await contract.targetedTapSettlementFeatureKey(value)), value });
        return 'checkpoint submitted';
      }
    }
    verifyCheckpoint(seen, value, admin);
    const history = path.join(spool, 'processed', nameFor(cp.epoch, cp.epoch_apply_hash) + '.debt-settlement.json');
    immutable(history, report);
    if (!prior || prior.epoch < cp.epoch) durableWrite(latestFile, report, true);
    else if (prior.epoch === cp.epoch) assert.deepEqual(prior, report);
    immutable(finished, { source: source.root_preparation_id, epoch: cp.epoch, root: cp.root,
      checkpoint: cp.root_preparation_id, anchor: cp.epoch_apply_hash, fence_sha256: ticket.proof_sha256, report_sha256: digestFile(history), refund });
    return 'settled';
  }
  const slot = exists(slotFile) ? json(slotFile) : null;
  const queued = ['working', 'ready'].some(dir => fs.readdirSync(path.join(spool, dir)).some(file => file.endsWith('.receipts.json')));
  const submitted = exists(transferFile) ? json(transferFile) : null;
  const candidate = submitted?.plan?.intent ?? intent;
  const pending = await pool.pendingRoot();
  const posted = await pool.merkleRootAtEpoch(candidate.epoch);
  Object.assign(ctx.facts, { candidate_epoch: candidate.epoch, candidate_root: candidate.root, historical_root: posted,
    pending_epoch: String(pending[1]), pending_root: pending[0] });
  const head = await pool.runner.provider.getBlock('latest');
  assert(Number.isSafeInteger(head?.timestamp));
  const effect = posted !== ZeroHash ? posted : pending[1] === BigInt(candidate.epoch) ? pending[0] : ZeroHash;
  let offer;
  if (!submitted && effect !== ZeroHash && effect !== intent.root && exists(attemptFile(base, effect))) {
    const mined = posted === effect;
    offer = await minedOffer({ base, pool, root: effect, epoch: intent.epoch, source, head,
      spent: mined ? await pool.cumulativeSpent() : pending[2], nonce: mined ? undefined : pending[5] });
  }
  const onChain = Boolean(offer) || (pending[0] === candidate.root && pending[1] === BigInt(candidate.epoch)) || posted === candidate.root;
  if (queued && !onChain) return 'ordinary work';
  if (slot && slot.name !== name) return 'another governed root';
  const claimSlot = () => immutable(slotFile, { name, epoch: plan.intent.epoch, root: plan.intent.root });
  const { prepareTapDebtRecovery, confirmTapDebtRecovery } = await import('./tap-prepared-debt.mjs');
  const proposalRateAt = (at, proposal) => proposalRate({ contract, base, pool,
    originalId: plan.root_preparation_id, at, proposal });
  const rateRecordKey = (await contract.get('tap/rate/latest'))?.updated_at;
  Object.assign(ctx.facts, { head: head.hash, financial_at: head.timestamp, observed_rate_key: rateRecordKey ?? null });
  let plan = original, paymentBundle = bundle;
  if (basisEpoch > intent.epoch || submitted) {
    let transfer = submitted;
    if (!transfer) {
      const target = loadCursor(spool);
      ctx.facts.completed_page = target?.epoch ?? null;
      // The ordinary producer already concluded this epoch, including all native
      // and proxy receipts. Only a completed no-payment page offers a free slot.
      assert(target?.epoch > basisEpoch, 'Awaiting an applied empty TAP settlement epoch');
      const prefix = path.join(spool, 'processed', nameFor(target.epoch, target.epoch_apply_hash));
      const carry = json(prefix + '.settlement.json');
      assert(target.report_sha256 === digestFile(prefix + '.settlement.json') && carry.posted === false &&
        carry.spent_au === '0' && !carry.tap_settlement_checkpoint && carry.checkpoint_outputs?.length === 0,
        'TAP recovery target is not a reconciled empty page');
      const cp = await contract.get(recordKey(basisEpoch));
      assert(cp?.root === state.last_root && cp.root === prior?.root && prior.epoch === basisEpoch,
        'Missing reconciled TAP recovery basis');
      const step = await nextBasis({ ctx, contract, base, cert, source, opening: evidence.opening_checkpoint, latest: cp, read });
      if (step.status) return step.status;
      const { proof, previousEpoch } = step;
      const missing = await Promise.all(original.preparations.map(p => contract.get(contract.payoutPreparationRecordKey('tap', p.economic_op_id))));
      if (missing.includes(null)) {
        return restoreOriginalPrefix({ ctx, contract, source, cert, archive, bundle, basisEpoch, previousEpoch, proof });
      }
      const terms = { contract, source, openingEpoch: cert.pool_basis.epoch, basisEpoch, previousEpoch,
        epoch: target.epoch, epochApplyHash: target.epoch_apply_hash, rateRecordKey, skippedEpochProof: proof,
        financialAt: head.timestamp, sign: message => adminSigner.sign(message) };
      try { transfer = await prepareTapDebtRecovery(terms); } catch (error) {
        if (!changesAmount(error)) throw error;
        transfer = await prepareTapDebtRecovery({ ...terms, amountBasis: 'authorized_au', originalBundle: bundle });
      }
      refundDisposition({ cert, source, bundle, archive, prior, plan: transfer.plan, spool, evidence });
      immutable(transferFile, transfer); // bind all signatures before posting, even if the response is lost
    }
    const resolved = await contract.get(resolutionKey(source));
    if (resolved === null) {
      if (submitted) {
        // Readback may lag an accepted request. Revalidate the persisted
        // control and its Ed25519 signatures before submitting those bytes again.
        const v = transfer.feature.value;
        const verified = await prepareTapDebtRecovery({ contract, source, openingEpoch: cert.pool_basis.epoch, basisEpoch,
          epoch: v.epoch, epochApplyHash: v.epoch_apply_hash, rateRecordKey: v.tap_rate_lock.rate_record_key,
          skippedEpochProof: v.skipped_epoch_proof, previousEpoch: v.previous_epoch, financialAt: head.timestamp,
          preparationSignatures: v.preparation_signatures, adminSignature: v.admin_sig,
          ...(v.stage === 'recover_excluded_au' && { amountBasis: 'authorized_au', originalBundle: bundle }) });
        assert.deepEqual(transfer, verified, 'Conflicting pending TAP recovery submission');
      }
      await postFeature(peerRpcUrl, fetchImpl, transfer.feature);
      return 'successor submitted';
    }
    const confirmed = await confirmTapDebtRecovery({ contract, feature: transfer.feature, plan: transfer.plan });
    plan = confirmed.plan;
    paymentBundle = transfer.bundle;
  } else {
    assert(basisEpoch === cert.pool_basis.epoch, 'Original TAP predecessor changed');
  }
  if (prior) await verifyPriorTapClaims(contract, prior);
  const have = await Promise.all(plan.preparations.map(p => contract.get(contract.payoutPreparationRecordKey('tap', p.economic_op_id))));
  if (have.some(row => row === null)) {
    assert(plan === original && have.some(row => row?.consumed === false), 'Missing original TAP reservations');
    return restoreOriginalPrefix({ ctx, contract, source, cert, archive, bundle });
  }
  if (plan === original && !onChain && !submitted) {
    assert(effect === ZeroHash && pending[6] === 0n && Number(await pool.epoch()) === cert.pool_basis.epoch,
      'Another Ethereum outcome occupies the original opening');
    const { preparedTapReplay, prepareTapAuAlternative, confirmTapAuAlternative } = await import('./tap-prepared-debt.mjs');
    try { await preparedTapReplay({ contract, source, financialAt: head.timestamp, rateRecordKey }); } catch (error) {
      if (!changesAmount(error)) throw error;
      const terms = { contract, source, originalBundle: bundle, openingEpoch: cert.pool_basis.epoch,
        rateRecordKey, financialAt: head.timestamp };
      // The contents, identifiers and both kinds of signature come from the
      // canonical AU projection. Store it before even the first Core POST.
      const draft = await prepareTapAuAlternative(terms);
      const file = offerFile(base, draft.plan.root_preparation_id);
      const saved = exists(file) ? json(file) : null;
      offer = await prepareTapAuAlternative({ ...terms, ...(saved ? {
        preparationSignatures: saved.feature.value.preparation_signatures, adminSignature: saved.feature.value.admin_sig,
      } : { sign: m => adminSigner.sign(m) }) });
      immutable(file, offer);
      const { tapAlternativeKey } = await import('../../intercom/contract/tap-prepared-debt.js');
      if (await contract.get(tapAlternativeKey(offer.plan.root_preparation_id)) === null) {
        await postFeature(peerRpcUrl, fetchImpl, offer.feature);
        return 'alternative submitted';
      }
      assert.equal((await confirmTapAuAlternative({ contract, candidate: offer })).status, 'authorized');
    }
  }
  if (offer) { plan = offer.plan; paymentBundle = offer.bundle; }
  const refund = refundDisposition({ cert, source, bundle, archive, prior, plan, spool, evidence });
  const projection = projectTapRefunds(bundle, exists(archive + '.buyers.json') ? json(archive + '.buyers.json') : {},
    plan.intent.tap_rate_lock.tap_usd_au, true);
  const roll = offer ? rollTapAuAlternative : rollPreparedTapSettlement;
  const report = await roll({ bundle: paymentBundle, rootEventReader: await eventsFor(base, pool, plan.intent), prior, peerRpcUrl, fetchImpl,
    pool, ownerSigner, governanceSigner, operatorAddress, targetedSessionBindings: {},
    tapRateLock: plan.intent.tap_rate_lock, tapUsdAu: plan.intent.tap_rate_lock.tap_usd_au,
    ledgerFeeBps: plan.intent.fee_bps, payoutMinAu: paymentBundle.params?.payout_min_au ?? '0',
    epochApplyHash: plan.intent.epoch_apply_hash, proposalRateAt, pendingRefundWei: projection.reservedWei,
    beforeProposal: choice => {
      if (offer) {
        const file = attemptFile(base, plan.intent.root);
        const previous = exists(file) ? json(file) : [];
        const entry = { ...choice, candidate_id: plan.root_preparation_id };
        if (previous.at(-1)?.candidate_id !== entry.candidate_id || previous.at(-1)?.nonce !== entry.nonce) {
          durableWrite(file, [...previous, entry], true);
        }
      }
      const file = base + '.' + plan.root_preparation_id + '.' + choice.nonce + '.quotes.json';
      const prior = exists(file) ? json(file) : [];
      if (prior.at(-1)?.rate_key !== choice.rate_key) durableWrite(file, [...prior, choice], true);
      claimSlot();
    },
    beforeEthereumEffect: onChain ? claimSlot : undefined,
    canonicalPreparationSubmitter: request => submitCanonicalTapPreparationPlan({ ...request, peerRpcUrl, fetchImpl, adminSigner }),
  }, offer ?? plan);
  if (report.awaiting_governance_delay) return 'governance delay';
  if (report.awaiting_finality) return 'finality';
  assert(report.tap_settlement_checkpoint && !report.blocked, report.reasons?.join(', ') ?? 'Incomplete TAP recovery');
  const block = await verifyFinalizedTapProposal({ report, chainId: 1, pool: MAINNET_POOL, read });
  if (offer) buildTapAuWinnerSubmission({ report, proposalBlock: block }, offer);
  else buildCanonicalTapSubmission({ report, proposalBlock: block });
  immutable(resultFile, { report, refund, root_preparation_id: source.root_preparation_id,
    ...(offer && { candidate_id: plan.root_preparation_id }) });
  return 'finalized';
}

// Round-robin over outstanding debts. A conflicting/partial source must not
// hold the ordinary spool or starve the next independent reservation.
export async function dispatchFencedTapTick({ spool, payout, peerRpcUrl, fetchImpl = fetch, connect, ...runtime }) {
  importEarlierFences(spool, payout);
  const root = path.join(spool, 'debt');
  if (!exists(path.join(root, 'pending.sqlite3'))) return 'idle';
  const cursor = path.join(root, 'last.json');
  const last = exists(cursor) ? json(cursor) : '';
  const slot = path.join(root, 'pool-slot.json');
  const selected = exists(slot) ? indexedDebt(root, 'pinned', json(slot).name) : indexedDebt(root, 'after', last);
  if (!selected) return 'idle';
  const { name, ticket } = selected;
  assert(jobName.test(name));
  durableWrite(cursor, name, true);
  const ctx = connect ? await connect() : runtime;
  const facts = { fence_sha256: ticket.proof_sha256 };
  const statusFile = path.join(root, name + '.status.json');
  function record(status, reason) {
    const value = { ...facts, status, ...(reason && { reason }) };
    if (!exists(statusFile) || JSON.stringify(json(statusFile)) !== JSON.stringify(value)) durableWrite(statusFile, value, true);
  }
  try {
    const result = await dispatchDebt({ spool, payout, peerRpcUrl, fetchImpl, ...ctx, facts }, name, ticket);
    record(result);
    if (result === 'settled') {
      if (exists(slot) && json(slot).name === name) { fs.unlinkSync(slot); syncDirectory(root); }
      indexedDebt(root, 'delete', name);
    }
    return result;
  } catch (error) {
    record('pending', safeErrorMessage(error, 400));
    // An unanswered send remains journalled. If another job could legally
    // compete for the pool slot, release only its scheduler turn. Nonce and
    // predecessor binding still choose one winner on Ethereum.
    if (exists(slot)) {
      const selected = json(slot);
      const [p, rootAtEpoch] = await Promise.all([ctx.pool.pendingRoot(), ctx.pool.merkleRootAtEpoch(selected.epoch)]);
      if (selected.name === name && rootAtEpoch !== selected.root &&
          !(p[0] === selected.root && p[1] === BigInt(selected.epoch))) {
        fs.unlinkSync(slot);
        syncDirectory(root);
      }
    }
    throw error;
  } finally { await ctx.close?.(); }
}

if (process.argv[1] && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [spool, payoutState, peerRpcUrl] = process.argv.slice(2);
    holdTapLock(path.join(spool, 'roller.lock'), 8);
    holdTapLock(path.join(payoutState, 'payout.lock'), 9);
    const outcome = await dispatchFencedTapTick({ spool, payout: path.join(payoutState, 'payout'), peerRpcUrl, connect: async () => {
      const adminSigner = await tapPreparationAdminSignerFromEnv();
      const { pool, provider, signer, governance } = await connectTapSettlementPool({ ethRpc: process.env.MAYHEM_TAP_ETH_RPC,
        poolAddress: MAINNET_POOL, tapRateLock: { chain_id: 1, token_address: MAINNET_TOKEN } });
      return { pool, ownerSigner: signer.wallet, governanceSigner: governance.wallet, adminSigner,
        operatorAddress: process.env.MAYHEM_TAP_OPERATOR_ADDRESS, close: () => provider.destroy() };
    } });
    if (outcome !== 'idle') console.log(outcome);
  } catch (error) {
    console.error(safeErrorMessage(error));
    process.exitCode = 1;
  }
}
