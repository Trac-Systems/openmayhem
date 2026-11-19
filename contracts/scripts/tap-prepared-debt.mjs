#!/usr/bin/env node
// Offline planning consumes explicit public evidence. Signatures are supplied by
// the operator's existing signer. This module neither signs Ethereum nor sends.
import assert from 'node:assert/strict';
import { createPublicKey, verify } from 'node:crypto';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { encodeRlp, Interface, toBeHex, toQuantity } from 'ethers';
import MayhemContract, { CONTRACT_VERSION, payoutPreparationMessage, targetedPayoutControlMessage } from '../../intercom/contract/contract.js';
import { projectTapAuAlternative, projectTapDebt, tapAlternativeKey, tapDebtBundle, tapDebtEnvelope, tapDebtKey, tapDebtPlan, tapDebtResolutionKey, tapOpeningClaimCeilings, verifyPreparedTapPrice, verifyTapDebtBasis } from '../../intercom/contract/tap-prepared-debt.js';
import { packTapProof, tapEpochSlot, tapClaimedSlot, tapPoolHeadSlots, tapPendingSlots, verifyPostedTapEpoch, verifySkippedTapEpoch, verifyWinningTapEpoch } from '../../intercom/contract/tap-epoch-proof.js';
import { distribution } from './merkle.mjs';
import { cumulativeTapRefunds, guardianPreSignReport, projectTapRefunds, settlementBundleSha256, transferableTapRefunds, verifyTapFinancialPolicy } from './tap-settlement-roller.mjs';
import { confirmedTapContract, resolveTapLiabilitySweep, validTapSweepPointer } from './tap-liability-sweep.mjs';

function checked(x) { if (x instanceof Error) throw x; return x; }
const checkedId = x => typeof x === 'string' && /^[0-9a-f]{64}$/.test(x);

function validSignature(admin, msg, sig) {
  assert(checkedId(admin) && typeof sig === 'string' && /^[0-9a-f]{128}$/.test(sig), 'Invalid TAP recovery signature');
  const key = createPublicKey({ type: 'spki', format: 'der',
    key: Buffer.from('302a300506032b6570032100' + admin, 'hex') });
  assert(verify(null, Buffer.from(msg), key, Buffer.from(sig, 'hex')), 'Invalid TAP recovery signature');
}

// Fetch only the exact header and the pool's epoch and cumulative claim paths. The later checkpoint comes
// from confirmed Core. The proof itself is checked against its execution hash.
function encodeHeader(block) {
  const fields = ['parentHash','sha3Uncles','miner','stateRoot','transactionsRoot','receiptsRoot','logsBloom',
    'difficulty','number','gasLimit','gasUsed','timestamp','extraData','mixHash','nonce'];
  const quantities = new Set(['difficulty','number','gasLimit','gasUsed','timestamp','baseFeePerGas','blobGasUsed','excessBlobGas']);
  const suffix = ['baseFeePerGas','withdrawalsRoot','blobGasUsed','excessBlobGas','parentBeaconBlockRoot','requestsHash'];
  for (const field of suffix) if (block[field] != null) fields.push(field);
  return encodeRlp(fields.map(k => quantities.has(k) ? (BigInt(block[k]) === 0n ? '0x' : toBeHex(block[k])) : block[k]));
}
const poolEvents = new Interface([
  'event RootProposed(uint256 indexed epoch,bytes32 merkleRoot,uint256 cumulativeSpent,uint256 indexed nonce,uint64 executeAfter)',
  'event RootPosted(uint256 indexed epoch,bytes32 merkleRoot,uint256 cumulativeSpent,uint256 indexed nonce)',
]);
async function verifyCandidateEvents(cp, block, read) {
  const finalized = await read('eth_getBlockByNumber', ['finalized', false]);
  assert(BigInt(await read('eth_chainId', [])) === 1n && BigInt(finalized.number) >= BigInt(cp.finalized_block_number) &&
    BigInt(finalized.number) - BigInt(block.number) >= 12n, 'Candidate has insufficient mainnet finality');
  let nonce, proposal;
  for (const [event, kind] of [['RootProposed','proposal'],['RootPosted','execution']]) {
    const height = cp[kind + '_block_number'], hash = cp[kind + '_block_hash'], tx = cp[kind + '_tx'];
    const canonical = await read('eth_getBlockByNumber', [toQuantity(height), false]);
    const receipt = await read('eth_getTransactionReceipt', [tx]);
    assert(canonical.hash === hash && BigInt(canonical.number) === BigInt(height) && receipt?.transactionHash === tx &&
      receipt.blockHash === hash && BigInt(receipt.blockNumber) === BigInt(height) && BigInt(receipt.status) === 1n,
      'Candidate receipt differs from canonical finalized chain');
    const topic = poolEvents.getEvent(event).topicHash;
    const logs = receipt.logs.filter(log => !log.removed && log.address.toLowerCase() === cp.pool_address &&
      log.topics[0] === topic).map(log => poolEvents.parseLog(log).args)
      .filter(a => a.epoch === BigInt(cp.epoch) && a.merkleRoot === cp.root && a.cumulativeSpent === BigInt(cp.cumulative_spent_wei));
    assert(logs.length === 1, 'Missing or ambiguous candidate event');
    if (kind === 'proposal') {
      nonce = logs[0].nonce;
      proposal = canonical;
      assert(BigInt(canonical.timestamp) === BigInt(cp.proposal_block_timestamp) &&
        logs[0].executeAfter <= BigInt(block.timestamp), 'Candidate proposal clock or delay mismatch');
    } else assert.equal(logs[0].nonce, nonce, 'Candidate governance nonce mismatch');
  }
  return proposal;
}
export async function collectTapEpochProof({ checkpoint: cp, source, openingCheckpoint, previousCheckpoint, read }) {
  const oldEpoch = source.intent.epoch;
  const opening = openingCheckpoint ?? { provider_entries: [], refunds: [], cumulative_spent_wei: '0' };
  const ceilings = tapOpeningClaimCeilings(source.intent, opening);
  const block = await read('eth_getBlockByHash', [cp.execution_block_hash, false]);
  assert(block?.hash === cp.execution_block_hash, 'Missing reconciled Ethereum block');
  const parent = await read('eth_getBlockByHash', [block.parentHash, false]);
  assert(parent?.hash === block.parentHash, 'Missing execution parent');
  const sameEpoch = oldEpoch === cp.epoch;
  const claimSlots = Array.from(ceilings.keys(), tapClaimedSlot);
  const priorSlots = sameEpoch ? [...tapPoolHeadSlots, ...claimSlots, ...tapPendingSlots] : tapPoolHeadSlots;
  const prior = await read('eth_getProof', [cp.pool_address, priorSlots, { blockHash: parent.hash, requireCanonical: true }]);
  const slots = [tapEpochSlot(cp.epoch), ...(sameEpoch ? [] : [tapEpochSlot(oldEpoch)]), ...tapPoolHeadSlots, ...claimSlots];
  const proof = await read('eth_getProof', [cp.pool_address, slots, { blockHash: cp.execution_block_hash, requireCanonical: true }]);
  const path = (slot, evidence = proof) => evidence.storageProof.find(row => BigInt(row.key) === BigInt(slot))?.proof;
  const claimed = Object.fromEntries(Array.from(ceilings.keys(), payee => [payee, path(tapClaimedSlot(payee))]));
  const witness = { header: encodeHeader(block), account: proof.accountProof, posted: path(slots[0]), skipped: sameEpoch ? null : path(slots[1]), claimed,
    head: tapPoolHeadSlots.map(slot => path(slot)), parent_header: encodeHeader(parent),
    parent_account: prior.accountProof, parent_head: tapPoolHeadSlots.map(slot => path(slot, prior)) };
  if (sameEpoch) {
    const proposal = await verifyCandidateEvents(cp, block, read);
    const before = await read('eth_getBlockByHash', [proposal.parentHash, false]);
    const proofAt = (hash, slots) => read('eth_getProof', [cp.pool_address, slots, { blockHash: hash, requireCanonical: true }]);
    const p = await proofAt(proposal.hash, tapPendingSlots), b = await proofAt(before.hash, [tapPendingSlots[0]]);
    Object.assign(witness, { proposal_header: encodeHeader(proposal), proposal_account: p.accountProof,
      proposal_pending: tapPendingSlots.map(slot => path(slot, p)), before_proposal_header: encodeHeader(before),
      before_proposal_account: b.accountProof, before_proposal_pending: path(tapPendingSlots[0], b),
      parent_pending: tapPendingSlots.map(slot => path(slot, prior)) });
    witness.parent_claimed = Object.fromEntries(Array.from(ceilings.keys(), a => [a, path(tapClaimedSlot(a), prior)]));
  }
  const packed = packTapProof(witness);
  if (sameEpoch) {
    const verify = cp.root === source.intent.root ? verifyPostedTapEpoch : verifyWinningTapEpoch;
    verify(cp, source.intent, packed, ceilings, opening, new Map(cp.entries.map(e => [e.account, BigInt(e.cumulative_wei)])));
  }
  else verifySkippedTapEpoch(cp, oldEpoch, packed, ceilings, previousCheckpoint ?? opening);
  return packed;
}

// A historical out-of-policy RootPosted still permanently created the pool
// entitlement. Return only proof arguments. Never estimate current unpaid
// amounts from the execution-block read or authorize a second transfer.
export async function postedTapDebtClaims({ contract, source, checkpoint, openingEpoch, executionProof }) {
  const old = source.intent, plan = await tapDebtPlan(contract, old);
  const opening = openingEpoch === 0 ? { provider_entries: [], refunds: [], cumulative_spent_wei: '0' } :
    await contract.get('settle/targeted/tap/' + openingEpoch);
  checked(contract.normalizeTargetedTapSettlementValue(checkpoint));
  for (const [k, val] of Object.entries({ ...old, ...sealedPlan(plan) })) {
    if (k !== 'intent') assert.deepEqual(checkpoint[k], val, 'Posted root differs from the immutable preparation');
  }
  const root = await contract.get(contract.payoutPreparationRecordKey('tap', plan.root_preparation_id));
  assert(root?.kind === 'tap_root' && root.economic_op_id === source.root_preparation_id &&
    root.admin === await contract.get('admin'), 'Missing original signed root');
  validSignature(root.admin, payoutPreparationMessage(root), root.admin_sig);
  assert.deepEqual(root.payload, plan.preparations.at(-1).payload);
  const ceilings = tapOpeningClaimCeilings(old, opening), entries = new Map(old.entries.map(e => [e.account, BigInt(e.cumulative_wei)]));
  verifyPostedTapEpoch(checkpoint, old, executionProof, ceilings, opening, entries);
  const rate = await contract.guardianRequireHistoricalTapRateLock(old.tap_rate_lock, checkpoint.proposal_block_timestamp);
  if (rate instanceof Error && !rate.message.endsWith('oracle is stale.')) throw rate;
  const tree = distribution([...entries].map(([account,amount]) => ({ account, amount })));
  assert.equal(tree.root.toLowerCase(), old.root);
  const claims = [...ceilings.keys()].map(account => ({ pool: old.pool_address, epoch: old.epoch, account,
    cumulative_wei: String(entries.get(account)), proof: tree.proofFor(account) }));
  return { status: 'posted_original', original_quote: rate instanceof Error ? 'expired' : 'admissible', claims };
}

async function originalRefundProjection(contract, originalBundle, old, opening) {
  const rawRefunds = [...transferableTapRefunds(originalBundle),
    ...transferableTapRefunds({ refunds: originalBundle.tap_carried_refunds ?? [] })];
  const buyers = {};
  for (const { buyer } of rawRefunds) {
    if (!buyer || buyers[buyer]) continue;
    assert(checkedId(buyer), 'Unresolved original refund identity');
    const binding = await contract.get(contract.tapAccountBindingKey(buyer, old.chain_id, old.pool_address));
    if (!binding) continue;
    assert(binding.type === 'tap_account_binding' && binding.user === buyer && binding.chain_id === old.chain_id &&
      binding.pool_address === old.pool_address, 'Conflicting original refund identity');
    buyers[buyer] = binding.ethereum_address;
  }
  const projected = projectTapRefunds(originalBundle, buyers, old.tap_rate_lock.tap_usd_au, true);
  assert(!projected.unpaid.length || originalBundle.tap_refund_queue_version === 1, 'Unresolved original refund');
  return { ...projected, cumulative: cumulativeTapRefunds(projected.credits, { refunds: opening.refunds }) };
}
async function originalRefunds(contract, originalBundle, old, opening) {
  return (await originalRefundProjection(contract, originalBundle, old, opening)).cumulative;
}
async function verifyPartialTerms(contract, originalBundle, old, opening, needsLiabilities, frozenFetch) {
  assert.deepEqual(await originalRefunds(contract, originalBundle, old, opening), old.refunds,
    'Original refunds differ from the rate-locked bundle');
  const claims = new Map(opening.provider_entries.map(o => [o.account, BigInt(o.cumulative_wei)]));
  for (const o of old.outputs) {
    const before = claims.get(o.to) ?? 0n;
    assert.equal(o.prior_cumulative_claim_wei, String(before), 'Original claim opening changed');
    claims.set(o.to, before + BigInt(o.tap_wei));
  }
  assert.deepEqual([...claims].sort(([a],[b]) => a.localeCompare(b)).map(([account,n]) => ({ account, cumulative_wei: String(n) })),
    old.provider_entries, 'Original provider distribution differs from signed terms');
  const total = BigInt(old.provider_cumulative_claimed_wei);
  assert.equal(old.cumulative_spent_wei, String((total * 4n + 2n) / 3n), 'Noncanonical original gross spend');
  if (!needsLiabilities) return;
  const expected = (await frozenRows(originalBundle, old, frozenFetch)).map(({ aggregate_before, ...row }) => row);
  assert.deepEqual(old.outputs.map(o => ({ provider: o.provider, payout_revision: o.payout_revision, to: o.to,
    paid_cum_au_before: o.paid_cum_au_before, paid_au: o.paid_au })), expected, 'Source differs from the frozen liability page');
}

async function frozenRows(originalBundle, old, frozenFetch) {
  const p = originalBundle.tap_liability_sweep;
  assert(validTapSweepPointer(p) && typeof frozenFetch === 'function',
    'Missing liability preparations require the original signed sweep page');
  // The bundle hash in the confirmed prefix authenticates the selector. Never
  // backfill an unsigned row from today's balance or an operator-supplied sum.
  const page = await resolveTapLiabilitySweep({ epoch: old.epoch, epochApplyHash: old.epoch_apply_hash, at: old.at,
    signedLength: p.signed_length, after: p.after, schemaVersion: p.schema_version, limit: p.limit,
    peerRpcUrl: 'http://frozen.invalid', fetchImpl: frozenFetch });
  return page.liabilities.map(row => ({ provider: row.provider, payout_revision: row.payout_revision, to: row.target,
    aggregate_before: row.aggregate_paid_cum_au, paid_cum_au_before: row.paid_cum_au, paid_au: String(BigInt(row.total_au) - BigInt(row.held_au) - BigInt(row.paid_cum_au)) }))
    .filter(o => BigInt(o.paid_au) * 10n**18n / BigInt(old.tap_rate_lock.tap_usd_au) > 0n);
}

// Old .prepare-intent.json files contain identities only. Reconstruct the full
// sealed plan from the confirmed prefix and its pinned inputs, then verify
// every hash including the full distribution commitment before returning it.
export async function rebuildPreparedTapSource({ contract, sealed, originalBundle, openingEpoch, frozenFetch }) {
  assert(sealed && checkedId(sealed.root_preparation_id) && Array.isArray(sealed.preparation_ids) &&
    sealed.preparation_ids.length > 0 && sealed.preparation_ids.length <= 32 && sealed.preparation_ids.every(checkedId),
    'Invalid sealed preparation identifiers');
  const { epoch, epoch_apply_hash } = sealed, anchor = checked(await contract.requireEpochApplyAnchor(epoch, epoch_apply_hash, 'TAP source'));
  const stored = [];
  for (const id of sealed.preparation_ids) stored.push(await contract.get(contract.payoutPreparationRecordKey('tap', id)));
  assert(stored[0]?.kind === 'liability' && stored[0].economic_op_id === sealed.preparation_ids[0], 'Missing confirmed prefix');
  const lock = checked(contract.normalizeTargetedTapRateLock(stored[0].payload.tap_rate_lock));
  assert.equal(lock.bundle_sha256, settlementBundleSha256(originalBundle), 'Original bundle digest changed');
  assert(lock.epoch === epoch && originalBundle.epoch === epoch && originalBundle.epoch_apply_hash === epoch_apply_hash &&
    originalBundle.at === anchor.settlement_unix && stored[0].prepared_at === anchor.settlement_unix, 'Source anchor changed');
  const old = { op: 'settle_targeted_tap', rail: 'tap', epoch, epoch_apply_hash, at: anchor.settlement_unix,
    chain_id: lock.chain_id, token_address: lock.token_address, pool_address: lock.pool_address,
    payment_config_ver: lock.payment_config_ver, tap_rate_lock: lock, fee_bps: 1500, tap_burn_bps: 1000, provider_share_bps: 7500 };
  const opening = openingEpoch === 0 ? { provider_entries: [], refunds: [] } : await contract.get('settle/targeted/tap/' + openingEpoch);
  assert(Number.isSafeInteger(openingEpoch) && openingEpoch >= 0 && openingEpoch < epoch && opening &&
    (openingEpoch === 0 || (opening.epoch === openingEpoch && opening.chain_id === lock.chain_id &&
      opening.pool_address === lock.pool_address && opening.token_address === lock.token_address &&
      opening.payment_config_ver === lock.payment_config_ver && opening.root_confirmed === true)), 'Unknown source opening');
  const admin = await contract.get('admin'), missing = stored.findIndex(r => r === null);
  for (const [i,r] of stored.entries()) if (r !== null) {
    assert((missing < 0 || i < missing) && r.epoch === epoch && r.epoch_apply_hash === epoch_apply_hash &&
      r.admin === admin && r.prepared_at === old.at && r.economic_op_id === sealed.preparation_ids[i] &&
      r.output_index === i && !r.superseded_by && !r.consumed, 'Conflicting original prefix');
    validSignature(admin, payoutPreparationMessage(r), r.admin_sig);
  }
  const sourceRows = missing < 0 ? stored.map(r => r.payload.output) : await frozenRows(originalBundle, old, frozenFetch);
  const claims = new Map(opening.provider_entries.map(r => [r.account, BigInt(r.cumulative_wei)])), cursors = new Map();
  const outputs = [];
  for (const row of sourceRows) {
    const { provider, payout_revision, to, paid_au, paid_cum_au_before } = row;
    const prior = claims.get(to) ?? 0n, wei = BigInt(paid_au) * 10n**18n / BigInt(lock.tap_usd_au);
    const aggregate = cursors.get(provider) ?? BigInt(row.aggregate_before ?? row.aggregate_paid_cum_au_before);
    outputs.push({ provider, payout_revision, to, paid_au, paid_cum_au_before, aggregate_paid_cum_au_before: String(aggregate),
      tap_wei: String(wei), prior_cumulative_claim_wei: String(prior), cumulative_claim_wei: String(prior + wei) });
    claims.set(to, prior + wei);
    cursors.set(provider, aggregate + BigInt(paid_au));
  }
  const sum = (data, field) => data.reduce((n,r) => n + BigInt(r[field]), 0n);
  const rows = data => [...data].sort(([a],[b]) => a.localeCompare(b)).map(([account,n]) => ({ account, cumulative_wei: String(n) }));
  const provider_entries = rows(claims), total = sum(provider_entries, 'cumulative_wei');
  const refunds = await originalRefunds(contract, originalBundle, old, opening);
  for (const r of refunds) claims.set(r.account, (claims.get(r.account) ?? 0n) + BigInt(r.cumulative_wei));
  const entries = rows(claims);
  const intent = { ...old, entries, refunds, provider_entries, outputs, root: contract.targetedTapSettlementRoot(entries),
    provider_count: outputs.length, provider_paid_au: String(sum(outputs,'paid_au')), provider_tap_wei: String(sum(outputs,'tap_wei')),
    provider_cumulative_claimed_wei: String(total), buyer_refund_wei: String(sum(refunds,'cumulative_wei')),
    cumulative_spent_wei: String((total * 4n + 2n) / 3n) };
  checked(contract.normalizeTargetedTapDistribution(intent, lock));
  const plan = await tapDebtPlan(contract, intent);
  for (const key of ['root_preparation_id','preparation_ids','external_effect_ids']) assert.deepEqual(plan[key], sealed[key], 'Rebuilt plan differs from sealed identifiers');
  return plan;
}

// Only restore the identical Core submissions. At the original opening this
// reserves the same obligations even if an old transaction is in flight.
// After a later opening, require positive historical exclusion. Neither path
// releases locks or authorizes an EVM send on a missing transaction read.
export async function restoreTapDebtPrefix({ contract, source, sealed, originalBundle, openingEpoch, basisEpoch, previousEpoch,
  skippedEpochProof, frozenFetch, sign, preparationSignatures }) {
  const old = source.intent;
  checked(contract.normalizeTargetedTapDistribution(old, checked(contract.normalizeTargetedTapRateLock(old.tap_rate_lock))));
  const plan = await tapDebtPlan(contract, old);
  const { root_preparation_id, preparation_ids, external_effect_ids } = plan;
  assert.deepEqual({ epoch: old.epoch, epoch_apply_hash: old.epoch_apply_hash,
    root_preparation_id, preparation_ids, external_effect_ids }, sealed, 'Source differs from sealed journal');
  for (const k of ['root_preparation_id','preparation_ids','external_effect_ids']) assert.deepEqual(plan[k], source[k]);
  assert.equal(settlementBundleSha256(originalBundle), old.tap_rate_lock.bundle_sha256, 'Original bundle digest changed');
  assert(originalBundle.epoch === old.epoch && originalBundle.epoch_apply_hash === old.epoch_apply_hash &&
    originalBundle.at === old.at && old.outputs.length > 0 && old.outputs.length <= 32, 'Original bundle anchor changed');
  const opening = openingEpoch === 0 ? { provider_entries: [], refunds: [], cumulative_spent_wei: '0' } : await contract.get('settle/targeted/tap/' + openingEpoch);
  assert(opening && (openingEpoch === 0 || (opening.epoch === openingEpoch && opening.type === 'targeted_tap_settlement' &&
      opening.root_confirmed === true && opening.pool_address === old.pool_address && opening.token_address === old.token_address &&
      opening.chain_id === old.chain_id && opening.payment_config_ver === old.payment_config_ver)) && openingEpoch < old.epoch,
    'Missing same-pool reconciled opening');
  if (basisEpoch === undefined && skippedEpochProof === undefined) {
    const head = await contract.get('settle/targeted/tap/state/' + old.chain_id + '/' + old.pool_address);
    assert((head?.last_epoch ?? 0) === openingEpoch && (head?.last_root ?? null) === (opening.root ?? null) &&
      (head?.cumulative_spent_wei ?? '0') === opening.cumulative_spent_wei &&
      await contract.get('settle/targeted/tap/' + old.epoch) === null, 'Source no longer at its confirmed opening');
  } else {
    const cp = await contract.get('settle/targeted/tap/' + basisEpoch);
    assert(cp?.type === 'targeted_tap_settlement' && cp.epoch === basisEpoch && cp.settled_by_role === 'admin' &&
      cp.pool_address === old.pool_address && cp.token_address === old.token_address && cp.chain_id === old.chain_id,
      'Missing same-pool reconciled basis');
    await verifyTapDebtBasis(contract, source, opening, cp, skippedEpochProof, previousEpoch);
  }
  const admin = await contract.get('admin');
  const missing = [], staged = new Map();
  const view = Object.assign(Object.create(contract), { get: k => staged.has(k) ? staged.get(k) : contract.get(k) });
  checked(await view.targetedPayoutSettlementUpdates(old.outputs.map(o => ({ ...o, role: 'provider', au: o.paid_au })),
    'tap', old.epoch, old.at, preparation_ids));
  for (const [i, p] of plan.preparations.entries()) {
    const existing = await view.get(view.payoutPreparationRecordKey('tap', p.economic_op_id));
    if (existing) {
      assert(!missing.length && existing.prepared_at === old.at && !existing.superseded_by, 'Original preparation is not a prefix');
      checked(await view.validatePayoutPreparationRecord({ rail: 'tap', epoch: old.epoch, epochApplyHash: old.epoch_apply_hash,
        economicOpId: p.economic_op_id, kind: p.kind, outputIndex: p.output_index,
        payload: p.payload, liability: p.liability, externalEffectIds: p.external_effect_ids }));
      continue;
    }
    assert(i > 0, 'No confirmed source preparation binds the original bundle');
    if (p.liability) {
      const lock = view.payoutPreparationLiabilityLockKey('tap', p.liability);
      assert.equal(await view.get(lock), null, 'Conflicting revision preparation');
      const result = checked(await view.validatePayoutPreparationLiability({
        rail: 'tap', epoch: old.epoch, prepared_at: old.at, liability: p.liability }));
      staged.set(lock, { economic_op_id: p.economic_op_id });
      staged.set(result.tail_key, { economic_op_id: p.economic_op_id, paid_cum_au_before: p.liability.aggregate_paid_cum_au_before,
        paid_cum_au_after: result.paid_cum_au_after, consumed: false });
    }
    for (const id of p.external_effect_ids) assert.equal(await view.get(view.payoutPreparationEffectLockKey('tap', id)), null,
      'Conflicting source effect reservation');
    const value = await tapDebtEnvelope(contract, old, p, admin, CONTRACT_VERSION);
    const message = payoutPreparationMessage(value);
    missing.push({ value, message });
  }
  if (missing.length) await verifyPartialTerms(contract, originalBundle, old, opening,
    missing.some(({ value }) => value.kind === 'liability'), frozenFetch);
  checked(await view.validatePayoutPreparationConsumption(plan.preparations.map(p => ({ ...p, rail: 'tap' }))));
  if (!sign && !preparationSignatures) return { preparation_messages: missing.map(({ message }) => message) };
  assert(sign || preparationSignatures.length === missing.length, 'Incomplete prefix signatures');
  const features = [];
  for (const [i, { value, message }] of missing.entries()) {
    value.admin_sig = sign ? await sign(message) : preparationSignatures[i];
    validSignature(admin, message, value.admin_sig);
    features.push({ key: checked(await contract.targetedPayoutPreparationFeatureKey(value)), value });
  }
  return { features };
}

export async function preparedTapReplay({ contract, source, financialAt, rateRecordKey }) {
  const { intent, preparation_ids, root_preparation_id, external_effect_ids } = source;
  checked(contract.normalizeTargetedTapDistribution(intent, checked(contract.normalizeTargetedTapRateLock(intent.tap_rate_lock))));
  const valid = await tapDebtPlan(contract, intent);
  for (const key of ['preparation_ids','root_preparation_id','external_effect_ids']) assert.deepEqual(source[key], valid[key]);
  const receiptKey = 'settle/targeted/tap/' + intent.epoch;
  const existing = await contract.get(receiptKey);
  if (existing !== null) {
    assert(existing.type === 'targeted_tap_settlement' && existing.settled_by_role === 'admin' &&
      existing.root_confirmed === true && existing.proposal_block_timestamp === financialAt, 'Conflicting reconciled proposal');
    for (const [key, field] of Object.entries({ ...intent, preparation_ids, root_preparation_id, external_effect_ids })) {
      assert.deepEqual(existing[key], field, 'Conflicting reconciled TAP terms');
    }
    const root = await contract.get(contract.payoutPreparationRecordKey('tap', root_preparation_id));
    assert(root?.consumed === true && root.consumed_by === receiptKey, 'Missing consumed TAP root');
    const { type, settled_by, settled_by_role, ...checkpoint } = existing;
    checked(contract.normalizeTargetedTapSettlementValue(checkpoint));
    const fields = checkpoint.proposal_rate_record_key ? { proposal_rate_record_key: checkpoint.proposal_rate_record_key } : {};
    return { plan: valid, checkpoint_fields: fields, checkpoint };
  }
  const records = checked(await contract.payoutPreparationsForSettlement(
    { ...intent, preparation_ids, root_preparation_id, external_effect_ids }, { ...intent, rate_lock: intent.tap_rate_lock }, 'tap'));
  assert(records.every(r => !r.superseded_by && r.prepared_at === intent.at), 'Superseded or incomplete source');
  checked(await contract.validatePayoutPreparationConsumption(records));
  const head = await contract.get('settle/targeted/tap/state/' + intent.chain_id + '/' + intent.pool_address);
  assert((head?.last_epoch ?? 0) < intent.epoch, 'Source epoch cannot advance the reconciled pool');
  assert(Number.isSafeInteger(financialAt) && financialAt >= intent.at, 'Invalid proposal clock');
  const result = await contract.guardianRequireHistoricalTapRateLock(intent.tap_rate_lock, financialAt);
  if (!(result instanceof Error)) return { plan: valid, checkpoint_fields: {} };
  checked(await verifyPreparedTapPrice(contract, intent.tap_rate_lock, financialAt, rateRecordKey, intent));
  return { plan: valid, checkpoint_fields: { proposal_rate_record_key: rateRecordKey } };
}

// Substitute this call only for verifyTapFinancialPolicy on sealed prepared
// replay. The normal pool screen, owner/governance signatures, mined-event
// check and checkpoint/finality checks still run in the roller.
export async function verifyPreparedTapFinancialPolicy({ source, financialAt, rateRecordKey, peerRpcUrl, fetchImpl, poolBasis }) {
  const contract = await confirmedTapContract(peerRpcUrl, fetchImpl);
  const result = await preparedTapReplay({ contract, source, financialAt, rateRecordKey });
  if (result.checkpoint) {
    assert(poolBasis === undefined, 'Already reconciled TAP epoch cannot be proposed');
    return result;
  }
  let policyIntent = source.intent;
  const key = result.checkpoint_fields.proposal_rate_record_key;
  if (key) {
    const r = await contract.get(key);
    // Only the policy check uses the new observation. The signed plan and
    // checkpoint keep the old lock, with the exact observation key as witness.
    policyIntent = { ...source.intent, tap_rate_lock: { ...source.intent.tap_rate_lock,
      tap_usd_au: r.tap_usd_au, rate_ts: r.ts, rate_record_key: key } };
  }
  await verifyTapFinancialPolicy({ intent: policyIntent, financialAt, peerRpcUrl, fetchImpl, poolBasis });
  return result;
}

function sealedPlan(source) {
  return { intent: source.intent, preparation_ids: source.preparation_ids, root_preparation_id: source.root_preparation_id,
    external_effect_ids: source.external_effect_ids };
}
async function signedPlan(contract, bundle, plan, terms, sign, preparationSignatures, adminSignature) {
  const preparation_messages = [];
  for (const p of plan.preparations) {
    const prep = await tapDebtEnvelope(contract, plan.intent, p, terms.admin, CONTRACT_VERSION);
    preparation_messages.push(payoutPreparationMessage(prep));
  }
  const signatures = sign ? await Promise.all(preparation_messages.map(m => sign(m))) : preparationSignatures;
  if (!signatures) return { bundle, plan, preparation_messages };
  assert(signatures.length === plan.preparations.length, 'Invalid signature set');
  preparation_messages.forEach((m, i) => validSignature(terms.admin, m, signatures[i]));
  const value = { ...terms, preparation_signatures: signatures };
  return { bundle, plan, ...await signedControl(contract, value, sign, adminSignature) };
}
async function signedControl(contract, value, sign, adminSignature) {
  const control_message = targetedPayoutControlMessage(value);
  if (!sign && !adminSignature) return { control_message };
  value.admin_sig = sign ? await sign(control_message) : adminSignature;
  validSignature(value.admin, control_message, value.admin_sig);
  return { feature: { key: await tapDebtKey(contract, value), value } };
}

export async function prepareTapAuAlternative({ contract, source, originalBundle, openingEpoch, rateRecordKey, financialAt,
  sign, preparationSignatures, adminSignature }) {
  const terms = { op: 'recover_prepared_tap', stage: 'authorize_au_alternative', contract_version: CONTRACT_VERSION, rail: 'tap',
    epoch: source.intent.epoch, opening_epoch: openingEpoch, original_bundle: originalBundle, source: sealedPlan(source),
    rate_record_key: rateRecordKey, admin: await contract.get('admin') };
  assert.equal((await contract.get('tap/rate/latest'))?.updated_at, rateRecordKey, 'Candidate needs current canonical observation');
  const { intent } = await projectTapAuAlternative(contract, terms);
  assert(Number.isSafeInteger(financialAt) && financialAt >= intent.at, 'Invalid proposal clock');
  checked(await contract.guardianRequireHistoricalTapRateLock(intent.tap_rate_lock, financialAt));
  return signedPlan(contract, originalBundle, await tapDebtPlan(contract, intent), terms, sign, preparationSignatures, adminSignature);
}

// A positive conditional reservation authorizes only this particular root.
// The ordinary owner/governance pool screens still bind the current EVM basis.
export async function confirmTapAuAlternative({ contract, candidate }) {
  const { plan, feature } = candidate, offer = feature.value;
  assert(offer.stage === 'authorize_au_alternative' && feature.key === await tapDebtKey(contract, offer), 'Candidate identity mismatch');
  const record = await contract.get(tapAlternativeKey(plan.root_preparation_id));
  assert(record?.submission === feature.key && record.candidate_id === plan.root_preparation_id, 'No confirmed AU alternative');
  assert.deepEqual(record.value, offer, 'Conflicting authorized candidate');
  validSignature(await contract.get('admin'), targetedPayoutControlMessage(offer), offer.admin_sig);
  const cp = await contract.get('settle/targeted/tap/' + offer.epoch);
  if (cp !== null) {
    let source = plan, status = 'settled';
    if (cp.root_preparation_id === offer.source.root_preparation_id) {
      source = offer.source;
      status = 'original_settled';
    } else if (cp.root_preparation_id !== plan.root_preparation_id) {
      const winner = await contract.get(tapAlternativeKey(cp.root_preparation_id));
      const resolution = await contract.get(tapDebtResolutionKey(offer.source.root_preparation_id));
      assert(winner?.value?.source?.root_preparation_id === offer.source.root_preparation_id &&
        resolution?.root_preparation_id === cp.root_preparation_id && resolution.candidate_submission === winner.submission,
        'Unidentified winner at the original epoch');
      source = await tapDebtPlan(contract, Object.fromEntries(Object.keys(offer.source.intent).map(k => [k, cp[k]])));
      status = 'other_candidate_settled';
    }
    const replay = await preparedTapReplay({ contract, source, financialAt: cp.proposal_block_timestamp });
    return { status, ...replay };
  }
  const resolution = await contract.get(tapDebtResolutionKey(offer.source.root_preparation_id));
  if (resolution?.basis_epoch > offer.epoch && resolution.epoch > resolution.basis_epoch) {
    return { status: 'superseded', resolution };
  }
  const derived = await projectTapAuAlternative(contract, offer);
  const canonical = await tapDebtPlan(contract, derived.intent);
  assert.deepEqual(plan, canonical);
  for (const [i,p] of canonical.preparations.entries()) {
    const envelope = await tapDebtEnvelope(contract, canonical.intent, p, offer.admin, CONTRACT_VERSION);
    validSignature(offer.admin, payoutPreparationMessage(envelope), offer.preparation_signatures[i]);
  }
  return { status: 'authorized', plan: canonical, opening: derived.opening };
}
export async function verifyTapAuAlternativeFinancialPolicy({ candidate, financialAt, peerRpcUrl, fetchImpl, poolBasis }) {
  const contract = await confirmedTapContract(peerRpcUrl, fetchImpl);
  const result = await confirmTapAuAlternative({ contract, candidate });
  if (result.status === 'authorized') {
    await verifyTapFinancialPolicy({ intent: result.plan.intent, financialAt, peerRpcUrl, fetchImpl, poolBasis });
  } else assert(poolBasis === undefined, 'Already executed TAP epoch cannot be proposed');
  return result;
}

// Read-only entry point for the existing automated owner and governance
// signers. Only 'ready' returns arguments to signRootProposal. Bind the
// signature to the observed epoch, gross spend and nonce even across retries.
export async function screenTapAuProposal({ candidate, pool, peerRpcUrl, fetchImpl }) {
  const contract = await confirmedTapContract(peerRpcUrl, fetchImpl);
  const result = await confirmTapAuAlternative({ contract, candidate });
  if (result.status !== 'authorized') return result;
  const { plan, opening } = result, intent = plan.intent, offer = candidate.feature.value;
  const provider = pool.runner.provider, block = await provider.getBlock('latest');
  assert(block?.hash && Number.isSafeInteger(block.timestamp) && Number.isSafeInteger(block.number), 'Missing proposal clock');
  const tag = { blockTag: block.number };
  assert((await provider.getNetwork()).chainId === BigInt(intent.chain_id) && (await pool.getAddress()).toLowerCase() === intent.pool_address &&
    (await pool.token(tag)).toLowerCase() === intent.token_address, 'Wrong recovery pool');
  const [epoch, root, spent, nonce, pending, capChange, deposited, cap] = await Promise.all([
    pool.epoch(tag), pool.merkleRoot(tag), pool.cumulativeSpent(tag), pool.governanceNonce(tag), pool.pendingRoot(tag),
    pool.pendingMaxEpochDelta(tag), pool.totalDeposited(tag), pool.maxEpochDelta(tag),
  ]);
  if (Number(epoch) >= intent.epoch) return { status: 'requires_finalized_evidence', epoch: Number(epoch) };
  if (pending.executeAfter > 0n) return { status: 'pending', root: pending.merkleRoot, epoch: Number(pending.newEpoch) };
  assert.equal(capChange.executeAfter, 0n, 'Pool cap change is pending');
  await verifyTapFinancialPolicy({ intent, financialAt: block.timestamp, peerRpcUrl, fetchImpl, poolBasis: { root, spent, epoch: Number(epoch) } });
  const refunds = await originalRefundProjection(contract, offer.original_bundle, intent, opening);
  assert.deepEqual(refunds.cumulative, intent.refunds, 'Frozen buyer refund disposition changed');
  const screen = await guardianPreSignReport({ settlement: { ...intent, providers: intent.provider_entries, pending_refund_wei: String(refunds.reservedWei) },
    pool, previous: opening, epoch: intent.epoch, currentEpoch: Number(epoch), prevSpentWei: spent, totalDepositedWei: deposited, maxEpochDeltaWei: cap });
  assert(screen.ok, screen.reasons.join(', '));
  assert.equal((await provider.getBlock(block.number)).hash, block.hash, 'Reorganized pool basis');
  return { status: 'ready', plan, screen, proposal: { merkleRoot: intent.root, newEpoch: intent.epoch,
    newCumulativeSpent: intent.cumulative_spent_wei, previousEpoch: Number(epoch), previousCumulativeSpent: String(spent), nonce: String(nonce) },
    pending_refund_wei: String(refunds.reservedWei), original_unpaid_refunds: refunds.unpaid };
}
export async function settleTapAuAlternative({ contract, candidate, checkpoint, executionProof, sign, adminSignature }) {
  const result = await confirmTapAuAlternative({ contract, candidate });
  if (result.status === 'settled') {
    assert.deepEqual(checkpoint, result.checkpoint, 'Conflicting final checkpoint');
    return result;
  }
  assert.equal(result.status, 'authorized', 'Another root won this epoch');
  const { plan, opening } = result, offer = candidate.feature.value;
  checked(contract.normalizeTargetedTapSettlementValue(checkpoint));
  for (const [k,val] of Object.entries({ ...plan.intent, ...sealedPlan(plan) })) {
    if (k !== 'intent') assert.deepEqual(checkpoint[k], val, 'Checkpoint differs from the authorized AU outcome');
  }
  checked(await contract.guardianRequireHistoricalTapRateLock(plan.intent.tap_rate_lock, checkpoint.proposal_block_timestamp));
  verifyWinningTapEpoch(checkpoint, offer.source.intent, executionProof, tapOpeningClaimCeilings(offer.source.intent, opening),
    opening, new Map(plan.intent.entries.map(e => [e.account, BigInt(e.cumulative_wei)])));
  const value = { op: 'recover_prepared_tap', stage: 'settle_au_alternative', contract_version: CONTRACT_VERSION, rail: 'tap',
    epoch: checkpoint.epoch, candidate_id: plan.root_preparation_id, checkpoint, execution_proof: executionProof,
    admin: await contract.get('admin') };
  return signedControl(contract, value, sign, adminSignature);
}

// The returned messages can be sent to the normal Ed25519 signer. Once each
// preparation is signed, sign the control message over those exact signatures.
export async function prepareTapDebtRecovery({ contract, source, openingEpoch, basisEpoch, epoch, epochApplyHash,
  rateRecordKey, skippedEpochProof, financialAt, sign, preparationSignatures, adminSignature, amountBasis = 'fixed_wei', originalBundle, previousEpoch }) {
  assert(checkedId(source?.root_preparation_id), 'Missing source root preparation');
  const admin = await contract.get('admin');
  const anchor = checked(await contract.requireEpochApplyAnchor(epoch, epochApplyHash, 'TAP debt successor'));
  const terms = { op: 'recover_prepared_tap', contract_version: CONTRACT_VERSION, rail: 'tap', epoch,
    epoch_apply_hash: epochApplyHash, opening_epoch: openingEpoch, basis_epoch: basisEpoch,
    source: sealedPlan(source), skipped_epoch_proof: skippedEpochProof, admin };
  assert(['fixed_wei','authorized_au'].includes(amountBasis), 'Unknown debt denomination');
  if (amountBasis === 'authorized_au') Object.assign(terms, { stage: 'recover_excluded_au', original_bundle: originalBundle });
  if (previousEpoch !== undefined) terms.previous_epoch = previousEpoch;
  const bundle = tapDebtBundle(terms, anchor.settlement_unix);
  const rate = await contract.get(rateRecordKey);
  assert(rate && rate.updated_at === rateRecordKey, 'Missing renewed rate record');
  terms.tap_rate_lock = { ...source.intent.tap_rate_lock, epoch, bundle_sha256: settlementBundleSha256(bundle),
    rate_record_key: rateRecordKey, rate_ts: rate.ts, tap_usd_au: rate.tap_usd_au, source: rate.source };
  assert(Number.isSafeInteger(financialAt) && financialAt >= bundle.at, 'Invalid proposal clock');
  checked(await contract.guardianRequireHistoricalTapRateLock(terms.tap_rate_lock, financialAt));
  const { intent } = await projectTapDebt(contract, terms);
  const plan = await tapDebtPlan(contract, intent);
  return signedPlan(contract, bundle, plan, terms, sign, preparationSignatures, adminSignature);
}

export async function certifyTapDebtBasis({ contract, source, openingEpoch, basisEpoch, previousEpoch = openingEpoch,
  skippedEpochProof, sign, adminSignature }) {
  const opening = openingEpoch === 0 ? { provider_entries: [], refunds: [], cumulative_spent_wei: '0' } :
    await contract.get('settle/targeted/tap/' + openingEpoch);
  const basis = await contract.get('settle/targeted/tap/' + basisEpoch);
  assert(basis?.epoch === basisEpoch && basis.root_confirmed && basis.settled_by_role === 'admin' &&
    basis.pool_address === source.intent.pool_address && basis.token_address === source.intent.token_address &&
    basis.chain_id === source.intent.chain_id && opening, 'Missing reconciled evidence endpoint');
  await verifyTapDebtBasis(contract, source, opening, basis, skippedEpochProof, previousEpoch);
  const value = { op: 'recover_prepared_tap', stage: 'certify_basis', contract_version: CONTRACT_VERSION, rail: 'tap',
    epoch: basisEpoch, opening_epoch: openingEpoch, previous_epoch: previousEpoch, source: sealedPlan(source),
    skipped_epoch_proof: skippedEpochProof, admin: await contract.get('admin') };
  return signedControl(contract, value, sign, adminSignature);
}

// Recover a lost submission response from the canonical result and the exact
// persisted feature. An unknown result is never permission for an EVM send.
export async function confirmTapDebtRecovery({ contract, feature, plan }) {
  const v = feature.value, key = await tapDebtKey(contract, v);
  assert.equal(feature.key, key, 'Recovery feature digest changed');
  validSignature(await contract.get('admin'), targetedPayoutControlMessage(v), v.admin_sig);
  const record = await contract.get(tapDebtResolutionKey(v.source.root_preparation_id));
  assert(record?.submission === key && record.epoch === v.epoch && record.basis_epoch === v.basis_epoch &&
    record.root_preparation_id === plan.root_preparation_id, 'No matching confirmed recovery');
  const expected = await tapDebtPlan(contract, plan.intent);
  for (const k of ['preparation_ids','root_preparation_id','external_effect_ids']) assert.deepEqual(expected[k], plan[k]);
  const cp = await contract.get('settle/targeted/tap/' + v.epoch);
  if (cp) return { status: 'settled', ...(await preparedTapReplay({ contract, source: plan, financialAt: cp.proposal_block_timestamp })) };
  const admin = v.admin;
  for (const [i, p] of expected.preparations.entries()) {
    const envelope = await tapDebtEnvelope(contract, plan.intent, p, admin, CONTRACT_VERSION);
    const stored = checked(await contract.validatePayoutPreparationRecord({ rail: 'tap', epoch: v.epoch, epochApplyHash: v.epoch_apply_hash,
      economicOpId: p.economic_op_id, kind: p.kind, outputIndex: p.output_index,
      payload: p.payload, liability: p.liability, externalEffectIds: p.external_effect_ids }));
    validSignature(admin, payoutPreparationMessage(envelope), v.preparation_signatures[i]);
    assert.equal(stored.admin_sig, v.preparation_signatures[i]);
    assert.equal(stored.prepared_at_tx, key);
  }
  checked(await contract.validatePayoutPreparationConsumption(expected.preparations.map(p => ({ ...p, rail: 'tap' }))));
  return { status: 'prepared', plan: expected };
}

if (process.argv[1] && fs.existsSync(process.argv[1]) && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [mode, inputs, records] = process.argv.slice(2);
    assert(['posted-claims','certify','rebuild','replay','restore-prefix','successor','confirm','offer-au','confirm-au','settle-au'].includes(mode) && inputs && records,
      'Usage: tap-prepared-debt.mjs posted-claims|certify|rebuild|replay|restore-prefix|successor|confirm|offer-au|confirm-au|settle-au inputs.json confirmed-point-reads.json');
    const json = file => {
      assert(fs.statSync(file).size <= 16 * 1024 * 1024, 'Oversized evidence file');
      return JSON.parse(fs.readFileSync(file, 'utf8'));
    };
    const state = json(records);
    const contract = Object.assign(Object.create(MayhemContract.prototype), { get: async key => {
      assert(Object.hasOwn(state, key), 'Missing explicit confirmed point read: ' + key);
      return state[key];
    } });
    const args = { ...json(inputs), contract };
    if (args.frozenResponses) {
      args.frozenFetch = async url => {
        const q = new URL(url).searchParams, response = args.frozenResponses[q.get('key') ?? 'page'];
        assert(response, 'Missing original pinned Core response');
        return Response.json(response);
      };
    }
    const action = { 'posted-claims': postedTapDebtClaims, certify: certifyTapDebtBasis, rebuild: rebuildPreparedTapSource, replay: preparedTapReplay, 'restore-prefix': restoreTapDebtPrefix, 'offer-au': prepareTapAuAlternative, 'confirm-au': confirmTapAuAlternative, 'settle-au': settleTapAuAlternative, successor: prepareTapDebtRecovery, confirm: confirmTapDebtRecovery }[mode];
    const result = await action(args);
    process.stdout.write(JSON.stringify(result) + '\n');
  } catch (e) { console.error(e.message); process.exitCode = 1; }
}
