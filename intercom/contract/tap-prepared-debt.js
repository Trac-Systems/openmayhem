import b4a from 'b4a';
import { sha256 } from 'ethereum-cryptography/sha256';
import { verifySkippedTapEpoch, verifyWinningTapEpoch } from './tap-epoch-proof.js';

const hash = x => typeof x === 'string' && /^[0-9a-f]{64}$/.test(x);
const positive = x => Number.isSafeInteger(x) && x > 0;
const same = (a, b) => a === b || (a && b && typeof a === 'object' && typeof b === 'object' &&
  Array.isArray(a) === Array.isArray(b) && Object.keys(a).length === Object.keys(b).length &&
  Object.keys(a).every(k => Object.hasOwn(b, k) && same(a[k], b[k])));
function need(ok, message) { if (!ok) throw new Error('TAP prepared debt: ' + message); }
function checked(value) { if (value instanceof Error) throw value; return value; }

// Include existing historical-parameter and holdback lookups in the budget.
// All new recovery operations use deterministic point reads.
class TapReadLimit extends Error {}
class TapReadFailure extends Error {}

function boundedReads(contract) {
  let remaining = 4096;
  return Object.assign(Object.create(contract), { get: async key => {
    if (remaining-- <= 0) throw new TapReadLimit('TAP recovery point-read bound exceeded.');
    try { return await contract.get(key); }
    catch (error) {
      if (error instanceof TapReadLimit || error instanceof TapReadFailure) throw error;
      throw new TapReadFailure('TAP recovery storage read failed', { cause: error });
    }
  } });
}

export const isTapRateKey = key => typeof key === 'string' && /^rate\/tap\/(0|[1-9][0-9]{0,15})\/[0-9a-f]{64}$/.test(key);
export async function verifyPreparedTapPrice(contract, lock, at, key, intent, openingRefunds) {
  // Always authenticate the original lock. A renewed observation must explain
  // exactly the same conversion, including any AU-denominated buyer refunds.
  if (key === undefined) return contract.guardianRequireHistoricalTapRateLock(lock, at);
  try {
    return await renewedTapPrice(boundedReads(contract), lock, at, key, intent, openingRefunds);
  } catch (error) {
    if (error instanceof TapReadLimit) return error;
    throw error;
  }
}

async function renewedTapPrice(contract, lock, at, key, intent, openingRefunds) {
  const original = await contract.guardianRequireHistoricalTapRateLock(lock, at);
  if (original?.message !== 'Guardian TAP settlement rate invariant failed: oracle is stale.' || !isTapRateKey(key)) {
    return new Error('TAP quote renewal requires a stale authenticated original and an exact rate key.');
  }
  const fresh = await contract.get(key);
  if (!fresh || fresh.source !== lock.source || fresh.ts <= lock.rate_ts || fresh.updated_at !== key) {
    return new Error('TAP renewal lacks a later observation from the original source.');
  }
  const observation = await contract.guardianRequireHistoricalTapRateLock({ ...lock, tap_usd_au: fresh.tap_usd_au,
    rate_ts: fresh.ts, rate_record_key: key }, at);
  if (observation instanceof Error || fresh.tap_usd_au === lock.tap_usd_au) return observation;
  // A different numerical price can still produce identical integer wei on
  // every signed output. A refund increment lacks its source denomination in
  // an old preparation, so only an identical rate can renew such a plan.
  if (!intent || !Array.isArray(intent.outputs) || !intent.outputs.length || intent.outputs.length > 32 ||
      intent.outputs.some(o => contract.normalizeTargetedTapSettlementOutput(o, fresh.tap_usd_au) instanceof Error)) {
    return new Error('TAP renewal changes the prepared price outcome.');
  }
  if (openingRefunds === undefined) {
    const state = await contract.get('settle/targeted/tap/state/' + lock.chain_id + '/' + lock.pool_address);
    if (state && (!scopeMatches(state, lock) || state.last_epoch >= intent.epoch)) return new Error('Conflicting TAP renewal basis.');
    const prior = state?.last_epoch ? await contract.get('settle/targeted/tap/' + state.last_epoch) : null;
    if (state?.last_epoch && (!prior || prior.root !== state.last_root || !scopeMatches(prior, lock))) {
      return new Error('Missing reconciled TAP renewal basis.');
    }
    openingRefunds = prior?.refunds ?? [];
  }
  return same(intent.refunds, openingRefunds) ? observation : new Error('TAP renewal cannot revalue an original refund.');
}

const scopeFields = ['chain_id', 'token_address', 'pool_address', 'payment_config_ver'];
const scopeMatches = (a, b) => scopeFields.every(k => a[k] === b[k]);
export const tapDebtKey = async (c, v) =>
  'payout/recovery-submit/tap/' + v.epoch + '/' + await c.opaqueHash('mayhem-tap-prepared-debt-v1', v);
// Keys in stable JSON order, matching settlementBundleSha256 in the roller.
export function tapDebtBundle(v, at) {
  return { at, epoch: v.epoch, epoch_apply_hash: v.epoch_apply_hash, rail: 'tap', receipts: [],
    tap_prepared_debt: { basis_epoch: v.basis_epoch, opening_epoch: v.opening_epoch,
      root_preparation_id: v.source.root_preparation_id } };
}
export const tapDebtResolutionKey = id => 'payout/recovery/tap/' + id + '/resolution';
const evidenceKey = (id, epoch) => 'payout/recovery/tap/' + id + '/basis/' + epoch;

function amounts(rows) {
  need(Array.isArray(rows) && rows.length <= 5000, 'distribution exceeds the recovery bound');
  const map = new Map();
  for (const { account, cumulative_wei: wei } of rows) {
    need(/^0x[0-9a-f]{40}$/.test(account) && /^[1-9][0-9]{0,77}$/.test(wei) && !map.has(account), 'invalid distribution');
    map.set(account, BigInt(wei));
  }
  return map;
}
const rows = map => [...map].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
  .map(([account, wei]) => ({ account, cumulative_wei: String(wei) }));
const sum = map => [...map.values()].reduce((a, b) => a + b, 0n);
const add = (map, k, n) => map.set(k, (map.get(k) ?? 0n) + n);
const fees = spent => spent * 1500n / 10000n;
const cap = spent => spent * 7500n / 10000n;
const normalized = intent => ({ ...intent, rate_lock: intent.tap_rate_lock });

async function checkpoint(c, epoch, old) {
  if (epoch === 0) return { provider_entries: [], refunds: [], cumulative_spent_wei: '0' };
  const cp = await c.get('settle/targeted/tap/' + epoch);
  need(cp?.type === 'targeted_tap_settlement' && cp.epoch === epoch && cp.rail === 'tap' &&
    cp.settled_by_role === 'admin' && cp.root_confirmed === true && scopeMatches(old, cp), 'missing reconciled checkpoint');
  return cp;
}

// claimed[] combines provider and buyer entitlements at an Ethereum address.
export function tapOpeningClaimCeilings(old, opening) {
  const previousProviders = amounts(opening.provider_entries), previousRefunds = amounts(opening.refunds);
  const affected = new Set(old.outputs.map(o => o.to));
  for (const [account, n] of amounts(old.refunds)) {
    if (n > (previousRefunds.get(account) ?? 0n)) affected.add(account);
  }
  need(affected.size <= 64, 'too many affected payees for bounded recovery');
  return new Map([...affected].map(account => [account,
    (previousProviders.get(account) ?? 0n) + (previousRefunds.get(account) ?? 0n)]));
}

// Derive the only dispatchable successor. There is no caller-selected amount.
// The old root commits the complete lists, including the opening paid prefix.
export function projectTapDebt(c, v) { return project(boundedReads(c), v); }

function validateSource(c, s) {
  const old = s?.intent;
  checked(c.validateExactObjectKeys(s, ['intent', 'root_preparation_id', 'preparation_ids', 'external_effect_ids'], 'sealed TAP plan'));
  checked(c.validateExactObjectKeys(old, ['op', 'rail', 'epoch', 'epoch_apply_hash', 'at', ...scopeFields, 'tap_rate_lock',
    'root', 'cumulative_spent_wei', 'provider_cumulative_claimed_wei', 'buyer_refund_wei', 'fee_bps', 'tap_burn_bps',
    'provider_share_bps', 'provider_count', 'provider_paid_au', 'provider_tap_wei', 'provider_entries', 'refunds',
    'entries', 'outputs'], 'original TAP intent'));
  need(old?.rail === 'tap' && old.op === 'settle_targeted_tap' && hash(s.root_preparation_id) &&
    Array.isArray(old.outputs) && old.outputs.length > 0 && old.outputs.length <= 32 &&
    Array.isArray(s.preparation_ids) && s.preparation_ids.length === old.outputs.length &&
    s.preparation_ids.every(hash) && new Set(s.preparation_ids).size === old.outputs.length &&
    Array.isArray(s.external_effect_ids) && s.external_effect_ids.length === 2 && s.external_effect_ids.every(hash) &&
    positive(old.epoch), 'invalid source identities');
  need([old.provider_entries, old.refunds, old.entries].every(a => Array.isArray(a) && a.length <= 5000), 'oversized source distribution');
  need(old.chain_id === 1 && old.pool_address === '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28' &&
    old.token_address === '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454', 'unsupported pool');
  return old;
}

async function project(c, v) {
  const s = v.source, old = validateSource(c, s);
  need(positive(v.basis_epoch) && Number.isSafeInteger(v.opening_epoch) && v.opening_epoch >= 0 &&
    v.opening_epoch < old.epoch && old.epoch < v.basis_epoch && positive(v.epoch) && v.basis_epoch < v.epoch &&
    hash(v.epoch_apply_hash), 'invalid source or epoch ordering');
  const lock = checked(c.normalizeTargetedTapRateLock(v.tap_rate_lock));
  need((await c.get('tap/rate/latest'))?.updated_at === lock.rate_record_key, 'successor must use the current canonical observation');
  const oldLock = checked(c.normalizeTargetedTapRateLock(old.tap_rate_lock));
  need(same(lock, v.tap_rate_lock) && scopeMatches(lock, old) && scopeMatches(oldLock, old) &&
    lock.epoch === v.epoch && oldLock.epoch === old.epoch,
    'scope or sealed source bundle changed');
  const anchor = checked(await c.requireEpochApplyAnchor(v.epoch, v.epoch_apply_hash, 'TAP debt successor'));
  need(lock.bundle_sha256 === b4a.toString(sha256(b4a.from(JSON.stringify(tapDebtBundle(v, anchor.settlement_unix)))), 'hex'),
    'successor bundle commitment changed');
  const oldAnchor = checked(await c.requireEpochApplyAnchor(old.epoch, old.epoch_apply_hash, 'TAP debt source'));
  need(old.at === oldAnchor.settlement_unix, 'source snapshot changed');
  const payments = checked(await c.canonicalTapPaymentConfig());
  const config = await c.get('payments/current');
  need(payments.chain_id === old.chain_id && payments.pool_address === old.pool_address &&
    config.tap.token_address?.toLowerCase() === old.token_address && config.ver === old.payment_config_ver &&
    config.set_by === await c.get('admin'), 'payment configuration changed');
  const scope = old.chain_id + '/' + old.pool_address;
  const basis = await checkpoint(c, v.basis_epoch, old);
  const state = await c.get('settle/targeted/tap/state/' + scope);
  need(state?.type === 'targeted_tap_settlement_state' && scopeMatches(state, old) &&
    state.last_epoch === basis.epoch && state.last_root === basis.root &&
    state.last_execution_tx === basis.execution_tx && state.cumulative_spent_wei === basis.cumulative_spent_wei &&
    state.cumulative_provider_claimed_wei === basis.provider_cumulative_claimed_wei &&
    state.cumulative_buyer_refund_wei === basis.buyer_refund_wei, 'basis changed');
  need(await c.get('settle/targeted/tap/' + old.epoch) === null &&
    await c.get('rail/seen/tap-settlement-root/' + scope + '/' + old.root) === null, 'conflicting historical effect');
  const opening = await checkpoint(c, v.opening_epoch, old);
  checked(c.normalizeTargetedTapDistribution(old, oldLock));
  checked(await c.guardianRequireHistoricalTapRateLock(oldLock, Math.max(old.at, oldLock.rate_ts)));
  let sourceIntent = old;
  if (v.stage === 'recover_excluded_au') {
    const now = Math.max(anchor.settlement_unix, lock.rate_ts);
    checked(await c.guardianRequireHistoricalTapRateLock(lock, now));
    need(lock.source === oldLock.source && lock.rate_ts > oldLock.rate_ts && config.denom === 'au_usd' &&
      (await c.guardianRequireHistoricalTapRateLock(oldLock, now))?.message ===
      'Guardian TAP settlement rate invariant failed: oracle is stale.', 'original AU quote is not eligible for renewal');
    checkAuBundle(v.original_bundle, old, BigInt(lock.tap_usd_au));
    sourceIntent = await convertOriginalAu(c, old, opening, { ...lock, epoch: old.epoch });
  } else {
    const observation = checked(await verifyPreparedTapPrice(c, oldLock, Math.max(anchor.settlement_unix, lock.rate_ts),
      lock.rate_record_key, old, opening.refunds));
    need(lock.tap_usd_au === observation.tap_usd_au && lock.source === observation.source &&
      lock.posted_by === observation.posted_by && lock.rate_ts === observation.ts, 'successor differs from renewed observation');
  }
  await verifyTapDebtBasis(c, s, opening, basis, v.skipped_epoch_proof, v.previous_epoch);
  const cumulative = await rebaseTapDistribution(c, sourceIntent, opening, basis, scope);
  need(await c.get('settle/targeted/tap/' + v.epoch) === null &&
    await c.get('rail/seen/tap-settlement-root/' + scope + '/' + cumulative.root) === null,
    'successor epoch or root already reconciled');
  const { records, tails } = await sourceReservations(c, s);
  if (v.stage === 'recover_excluded_au') need(records.every(r => r.contract_version === 31), 'requires original V31 AU reservations');
  const params = checked(await c.targetedTapParamsAt(anchor.settlement_unix, ['max_tap_settlement_outputs', 'fee_bps', 'payout_min_au']));
  checked(c.targetedTapPayoutSplit(params.fee_bps));
  need(old.outputs.length <= params.max_tap_settlement_outputs &&
    old.outputs.every(o => BigInt(o.paid_au) >= BigInt(params.payout_min_au)), 'successor violates payout policy');
  const intent = { ...sourceIntent, epoch: v.epoch, epoch_apply_hash: v.epoch_apply_hash, at: anchor.settlement_unix,
    tap_rate_lock: lock, ...cumulative };
  need(old.fee_bps === 1500 && old.provider_share_bps === 7500 && old.tap_burn_bps === 1000, 'source split changed');
  checked(c.normalizeTargetedTapDistribution(intent, lock));
  return { intent, records, tails };
}

async function sourceReservations(c, s) {
  const old = s.intent;
  const records = checked(await c.payoutPreparationsForSettlement({ ...old, ...ids(s) }, normalized(old), 'tap'));
  need(records.every(r => [30, 31].includes(r.contract_version) && r.prepared_at === old.at && !r.superseded_by &&
    r.consumed_by === null), 'source is partial, consumed or already replaced');
  const tails = checked(await c.validatePayoutPreparationConsumption(records));
  for (const tail of tails) {
    const last = records.find(r => r.economic_op_id === tail.value.economic_op_id).liability;
    need(tail.value.paid_cum_au_before === last.aggregate_paid_cum_au_before &&
      tail.value.paid_cum_au_after === c.safeAddAu(last.aggregate_paid_cum_au_before, last.paid_au), 'source tail mismatch');
  }
  return { records, tails };
}

// Advance over one real pool execution at a time. A confirmed predecessor
// certificate bounds replay cost independently of the number of later roots.
export async function verifyTapDebtBasis(c, source, opening, basis, proof, previousEpoch = opening.epoch ?? 0) {
  const old = source.intent, ceilings = tapOpeningClaimCeilings(old, opening), start = opening.epoch ?? 0;
  let parent = opening;
  if (previousEpoch !== start) {
    need(positive(previousEpoch) && old.epoch < previousEpoch && previousEpoch < basis.epoch, 'invalid certified predecessor epoch');
    parent = await checkpoint(c, previousEpoch, old);
    const path = await c.get(evidenceKey(source.root_preparation_id, previousEpoch));
    need(path?.opening_epoch === start && path.basis_epoch === previousEpoch && path.root === parent.root &&
      path.execution_block_hash === parent.execution_block_hash, 'missing certified predecessor');
  }
  need((parent.epoch ?? 0) < basis.epoch && (!parent.execution_block_number ||
    parent.execution_block_number < basis.execution_block_number), 'nonmonotonic pool history');
  verifySkippedTapEpoch(basis, old.epoch, proof, ceilings, parent);
  for (const field of ['provider_entries', 'refunds']) {
    const before = amounts(opening[field]), after = amounts(basis[field]);
    for (const account of ceilings.keys()) need((before.get(account) ?? 0n) === (after.get(account) ?? 0n),
      'intervening entitlement for affected payee');
  }
}

async function rebaseTapDistribution(c, old, opening, basis, scope) {
  const openingClaims = amounts(opening.provider_entries), openingRefunds = amounts(opening.refunds);
  const oldClaims = amounts(old.provider_entries), oldRefunds = amounts(old.refunds);
  const claims = amounts(basis.provider_entries), refunds = amounts(basis.refunds);
  const ceilings = tapOpeningClaimCeilings(old, opening);
  for (const account of ceilings.keys()) {
    for (const [kind, field, ledger] of [['claim', 'cumulative_claim_wei', claims], ['refund', 'cumulative_refund_wei', refunds]]) {
      const point = await c.get('settle/targeted/tap/' + kind + '/' + scope + '/' + account);
      need(point === null ? !ledger.has(account) : point.type === 'targeted_tap_cumulative_' + kind &&
        point.chain_id === old.chain_id && point.pool_address === old.pool_address && point.token_address === old.token_address &&
        point.account === account && point[field] === String(ledger.get(account) ?? 0n), 'ambiguous historical payee watermark');
    }
  }
  const changes = new Map();
  for (const output of old.outputs) {
    const before = (openingClaims.get(output.to) ?? 0n) + (changes.get(output.to) ?? 0n);
    need(String(before) === output.prior_cumulative_claim_wei, 'source claim watermark changed');
    add(changes, output.to, BigInt(output.tap_wei));
  }
  const originalClaims = new Map(openingClaims);
  for (const [account, n] of changes) add(originalClaims, account, n);
  need(same(rows(originalClaims), old.provider_entries), 'source provider distribution conflicts with its opening');
  for (const [account, n] of openingRefunds) need(oldRefunds.get(account) >= n, 'source removed a refund');
  for (const [account, n] of oldRefunds) {
    const delta = n - (openingRefunds.get(account) ?? 0n);
    if (delta > 0n) add(refunds, account, delta);
  }
  for (const [account, n] of changes) add(claims, account, n);
  const oldSpend = BigInt(old.cumulative_spent_wei), start = BigInt(opening.cumulative_spent_wei);
  const base = BigInt(basis.cumulative_spent_wei), spent = base + oldSpend - start;
  need(oldSpend > start && cap(start) === sum(openingClaims) && cap(oldSpend) === sum(oldClaims) &&
    cap(base) === BigInt(basis.provider_cumulative_claimed_wei) && cap(spent) === sum(claims) &&
    fees(spent) - fees(base) === fees(oldSpend) - fees(start), 'cumulative fee, provider or burn rounding changed');
  const entries = new Map(claims);
  for (const [account, n] of refunds) add(entries, account, n);
  const combined = rows(entries);
  need(combined.length <= 5000, 'successor distribution exceeds bound');
  return { root: c.targetedTapSettlementRoot(combined), cumulative_spent_wei: String(spent),
    provider_cumulative_claimed_wei: String(sum(claims)), buyer_refund_wei: String(sum(refunds)),
    provider_entries: rows(claims), refunds: rows(refunds), entries: combined };
}

function ids(s) {
  return { preparation_ids: s.preparation_ids, external_effect_ids: s.external_effect_ids, root_preparation_id: s.root_preparation_id };
}

// Same domains as buildCanonicalTapPreparationPlan. Keep standard signed
// successor records so ordinary receipt, readback and consumption paths apply.
export async function tapDebtPlan(c, intent) {
  const preparations = [];
  for (const [i, o] of intent.outputs.entries()) {
    const payload = c.payoutPreparationOutputPayload(intent, o, i, 'tap');
    const liability = c.payoutPreparationLiabilityForOutput(intent, o, 'tap');
    const id = await c.opaqueHash('mayhem-targeted-tap-liability-preparation-id-v1', {
      epoch: intent.epoch, epoch_apply_hash: intent.epoch_apply_hash, output_index: i, payload, liability });
    preparations.push({ economic_op_id: id, kind: 'liability', output_index: i, payload, liability, external_effect_ids: [] });
  }
  const external_effect_ids = [];
  for (const action of ['propose_root', 'execute_root']) {
    external_effect_ids.push(await c.opaqueHash('mayhem-targeted-tap-' + action + '-effect-id-v1', {
      chain_id: intent.chain_id, pool_address: intent.pool_address, epoch: intent.epoch, root: intent.root,
      cumulative_spent_wei: intent.cumulative_spent_wei }));
  }
  const preparation_ids = preparations.map(p => p.economic_op_id);
  const payload = await c.payoutPreparationTapRootPayload(intent, normalized(intent));
  const root_preparation_id = await c.opaqueHash('mayhem-targeted-tap-root-preparation-id-v1', payload);
  preparations.push({ economic_op_id: root_preparation_id, kind: 'tap_root', output_index: 0, payload,
    liability: null, external_effect_ids });
  return { intent, preparation_ids, external_effect_ids, root_preparation_id, preparations };
}

async function stageTapReplacement(c, key, intent, records, tails, plan, admin, signatures, version) {
  const writes = new Map();
  const view = Object.assign(Object.create(c), {
    get: k => writes.has(k) ? writes.get(k) : c.get(k),
    put: (k, value) => writes.set(k, value),
  });
  // Hide only locks proven to belong to this complete old plan, privately
  // while validating the standard signed successor. No unlock is published.
  for (const r of records) if (r.liability) writes.set(c.payoutPreparationLiabilityLockKey('tap', r.liability), null);
  for (const tail of tails) writes.set(tail.key, null);
  checked(await view.targetedPayoutSettlementUpdates(intent.outputs.map(o => ({ ...o, role: 'provider', au: o.paid_au })),
    'tap', intent.epoch, intent.at, plan.preparation_ids));
  for (const [i, p] of plan.preparations.entries()) {
    need(await c.get(c.payoutPreparationRecordKey('tap', p.economic_op_id)) === null, 'successor already prepared');
    const prep = await tapDebtEnvelope(c, intent, p, admin, version);
    prep.admin_sig = signatures[i];
    const prepKey = checked(await c.targetedPayoutPreparationFeatureKey(prep));
    view.tx = key;
    const result = checked(await view.applyTargetedPayoutPreparationFeature(prepKey, prep));
    need(result?.ok && !result.idempotent, 'successor preparation was not accepted');
    writes.set(c.payoutPreparationRecordKey('tap', records[i].economic_op_id),
      { ...records[i], superseded_by: p.economic_op_id, superseded_at: key });
  }
  need([...writes.values()].every(x => x !== null), 'incomplete successor lock transfer');
  return { view, writes };
}

// A conditional offer shares the original pool epoch and all its AU locks.
// No ordinary preparation or lock is replaced until its root has won on chain.
export const tapAlternativeKey = id => 'payout/recovery/tap/au-candidate/' + id;

function checkAuBundle(bundle, old, price) {
  let nodes = 0;
  function canonical(value, depth) {
    need(++nodes <= 16384 && depth <= 20, 'oversized source bundle');
    if (Array.isArray(value)) return '[' + value.map(x => canonical(x, depth + 1)).join(',') + ']';
    if (value && typeof value === 'object') return '{' + Object.keys(value).sort()
      .map(k => JSON.stringify(k) + ':' + canonical(value[k], depth + 1)).join(',') + '}';
    return JSON.stringify(value);
  }
  const encoded = canonical(bundle, 0);
  need(encoded.length <= 262144 && b4a.toString(sha256(b4a.from(encoded)), 'hex') === old.tap_rate_lock.bundle_sha256 &&
    bundle.at === old.at && bundle.epoch === old.epoch && bundle.epoch_apply_hash === old.epoch_apply_hash &&
    bundle.rail === 'tap' && Array.isArray(bundle.receipts) &&
    [1,2].includes(bundle.tap_liability_sweep?.schema_version) && positive(bundle.tap_liability_sweep.signed_length) &&
    !['liabilities','canonicalLiabilities','checkpoint_outputs'].some(k => Object.hasOwn(bundle, k)), 'unbound AU liability sweep');
  // Old buyer entitlements and each queued conversion boundary must survive.
  // A materially repriced AU refund is a separate customer obligation.
  const raw = bundle.buyer_refunds ?? bundle.buyerRefunds ?? bundle.refunds ?? [];
  const carried = bundle.tap_carried_refunds ?? [];
  for (const group of [raw, carried]) {
    need(group && typeof group === 'object' && Object.keys(group).length <= 256, 'unbounded buyer refunds');
    for (const entry of Object.values(group)) {
      const wei = typeof entry === 'object' && entry !== null ?
        entry.cumulative_wei ?? entry.amount_wei ?? entry.refund_wei : entry;
      if (typeof wei === 'string' && /^(0|[1-9][0-9]{0,77})$/.test(wei)) continue;
      const au = entry?.cumulative_au ?? entry?.amount_au ?? entry?.refund_au;
      need(wei === undefined && typeof au === 'string' && /^(0|[1-9][0-9]{0,77})$/.test(au) &&
        BigInt(au) * 10n**18n / price === BigInt(au) * 10n**18n / BigInt(old.tap_rate_lock.tap_usd_au),
        'repricing would change a buyer refund or queue item');
    }
  }
}
export function projectTapAuAlternative(c, v) { return projectAu(boundedReads(c), v); }
async function projectAu(c, v) {
  const s = v.source, old = validateSource(c, s);
  need(old.epoch === v.epoch && Number.isSafeInteger(v.opening_epoch) && v.opening_epoch >= 0 &&
    v.opening_epoch < old.epoch && isTapRateKey(v.rate_record_key), 'invalid AU alternative scope');
  const anchor = checked(await c.requireEpochApplyAnchor(old.epoch, old.epoch_apply_hash, 'TAP AU alternative'));
  need(old.at === anchor.settlement_unix, 'original liability clock changed');
  const oldLock = checked(c.normalizeTargetedTapRateLock(old.tap_rate_lock));
  checked(c.normalizeTargetedTapDistribution(old, oldLock));
  checked(await c.guardianRequireHistoricalTapRateLock(oldLock, Math.max(old.at, oldLock.rate_ts)));
  const r = await c.get(v.rate_record_key);
  need(r && r.source === oldLock.source && r.ts > oldLock.rate_ts && r.ts >= old.at, 'missing renewed source price');
  const lock = { ...oldLock, tap_usd_au: r.tap_usd_au, rate_ts: r.ts, rate_record_key: v.rate_record_key };
  checked(await c.guardianRequireHistoricalTapRateLock(lock, r.ts));
  need((await c.guardianRequireHistoricalTapRateLock(oldLock, r.ts))?.message ===
    'Guardian TAP settlement rate invariant failed: oracle is stale.', 'original quote still admissible or invalid');
  const price = BigInt(lock.tap_usd_au);
  checkAuBundle(v.original_bundle, old, price);
  const config = await c.get('payments/current');
  need(config?.denom === 'au_usd' && config.set_by_role === 'admin' && config.set_by === await c.get('admin') &&
    config.ver === old.payment_config_ver && config.rails?.includes('tap') && config.tap?.chain_id === 1 &&
    config.tap.pool_address?.toLowerCase() === old.pool_address && config.tap.token_address?.toLowerCase() === old.token_address,
    'AU payout configuration changed');
  const opening = await checkpoint(c, v.opening_epoch, old);
  const scope = '1/' + old.pool_address, head = await c.get('settle/targeted/tap/state/' + scope);
  need((head === null && v.opening_epoch === 0 || head?.type === 'targeted_tap_settlement_state' &&
    scopeMatches(head, old) && head.last_epoch === v.opening_epoch && head.last_root === (opening.root ?? null) &&
    head.cumulative_spent_wei === opening.cumulative_spent_wei &&
    head.cumulative_provider_claimed_wei === (opening.provider_cumulative_claimed_wei ?? '0') &&
    head.cumulative_buyer_refund_wei === (opening.buyer_refund_wei ?? '0')) &&
    await c.get('settle/targeted/tap/' + old.epoch) === null &&
    await c.get('rail/seen/tap-settlement-root/' + scope + '/' + old.root) === null, 'original epoch or opening has changed');
  const { records, tails } = await sourceReservations(c, s);
  need(records.every(r => r.contract_version === 31), 'requires an original V31 reservation');
  const oldPlan = await tapDebtPlan(c, old);
  need(same(ids(oldPlan), ids(s)), 'noncanonical original reservation identifiers');
  const intent = await convertOriginalAu(c, old, opening, lock);
  need(intent.root !== old.root && await c.get('rail/seen/tap-settlement-root/' + scope + '/' + intent.root) === null,
    'alternative must be a distinct unposted root');
  return { intent, records, tails, opening };
}

async function convertOriginalAu(c, old, opening, lock) {
  const price = BigInt(lock.tap_usd_au);
  const params = checked(await c.targetedTapParamsAt(old.at, ['max_tap_settlement_outputs','fee_bps','payout_min_au']));
  checked(c.targetedTapPayoutSplit(params.fee_bps));
  need(old.fee_bps === 1500 && old.tap_burn_bps === 1000 && old.provider_share_bps === 7500 &&
    old.outputs.length <= params.max_tap_settlement_outputs, 'original payout policy mismatch');
  const original = amounts(opening.provider_entries), claims = new Map(original);
  const refunds = amounts(old.refunds);
  for (const [account, n] of amounts(opening.refunds)) need(refunds.get(account) >= n, 'original refund watermark regressed');
  const outputs = [];
  for (const o of old.outputs) {
    const row = await c.get(c.providerPayoutLiabilityKey(o.provider, 'tap', o.payout_revision));
    need(row?.denom === 'au_usd' && BigInt(o.paid_au) >= BigInt(params.payout_min_au) &&
      String(original.get(o.to) ?? 0n) === o.prior_cumulative_claim_wei, 'source is not the original payable AU liability');
    add(original, o.to, BigInt(o.tap_wei));
    const wei = BigInt(o.paid_au) * 10n**18n / price, before = claims.get(o.to) ?? 0n;
    need(wei > 0n, 'AU liability rounds to zero TAP');
    add(claims, o.to, wei);
    outputs.push({ ...o, tap_wei: String(wei), prior_cumulative_claim_wei: String(before), cumulative_claim_wei: String(before + wei) });
  }
  need(same(rows(original), old.provider_entries) && BigInt(old.cumulative_spent_wei) === (sum(original) * 4n + 2n) / 3n, 'source claim opening changed');
  const total = sum(claims), gross = (total * 4n + 2n) / 3n, start = BigInt(opening.cumulative_spent_wei);
  need(cap(start) === sum(amounts(opening.provider_entries)) && gross > start, 'invalid original pool basis');
  const merged = new Map(claims);
  for (const [account, n] of refunds) add(merged, account, n);
  const entries = rows(merged);
  const intent = { ...old, root: c.targetedTapSettlementRoot(entries), tap_rate_lock: lock, outputs, entries,
    provider_entries: rows(claims), provider_cumulative_claimed_wei: String(total), cumulative_spent_wei: String(gross),
    provider_tap_wei: String(total - cap(start)) };
  checked(c.normalizeTargetedTapDistribution(intent, lock));
  return intent;
}

async function applyAu(c, key, v, version) {
  const offer = v.stage === 'authorize_au_alternative';
  const common = ['op','stage','contract_version','rail','epoch','admin','admin_sig'];
  checked(c.validateExactObjectKeys(v, common.concat(offer ? ['source','opening_epoch','rate_record_key','original_bundle',
    'preparation_signatures'] : ['candidate_id','checkpoint','execution_proof']), 'TAP AU recovery'));
  need((offer || v.stage === 'settle_au_alternative') && v.op === 'recover_prepared_tap' && v.rail === 'tap' &&
    v.contract_version === version && positive(v.epoch) && hash(v.admin) && c.isHexBytes(v.admin_sig, 64) &&
    key === await tapDebtKey(c, v), 'invalid AU recovery envelope');
  checked(await c.requireTargetedPayoutAdminSignature(v, 'TAP AU recovery'));
  let candidate = v, record;
  if (!offer) {
    need(hash(v.candidate_id), 'missing candidate identity');
    record = await c.get(tapAlternativeKey(v.candidate_id));
    need(record?.value.epoch === v.epoch && record.value.admin === v.admin && record.candidate_id === v.candidate_id, 'missing authorized candidate');
    candidate = record.value;
    const resolved = await c.get(tapDebtResolutionKey(candidate.source.root_preparation_id));
    if (resolved !== null) {
      need(resolved.submission === key && resolved.root_preparation_id === v.candidate_id, 'different winner already reconciled');
      return { result: { ok: true, idempotent: true, root_preparation_id: v.candidate_id } };
    }
  } else {
    need(Array.isArray(v.preparation_signatures) && v.preparation_signatures.length <= 33 &&
      v.preparation_signatures.every(s => c.isHexBytes(s, 64)), 'invalid candidate signatures');
    // Replay stays idempotent even if the other root has since won.
    const seen = await c.get('payout/recovery/accepted/' + key);
    if (seen?.candidate_id && seen.submission === key) return { result: { ok: true, idempotent: true, root_preparation_id: seen.candidate_id } };
  }
  if (offer) need((await c.get('tap/rate/latest'))?.updated_at === v.rate_record_key, 'candidate must use the current canonical observation');
  const { intent, records, tails, opening } = await projectAu(c, candidate);
  const plan = await tapDebtPlan(c, intent), id = plan.root_preparation_id;
  need(candidate.preparation_signatures.length === plan.preparations.length, 'incomplete candidate signatures');
  const { view, writes } = await stageTapReplacement(c, offer ? key : record.submission, intent, records, tails, plan,
    v.admin, candidate.preparation_signatures, version);
  if (offer) {
    need(await c.get(tapAlternativeKey(id)) === null, 'candidate already authorized under another envelope');
    // Keep only the signed conditional authorization. Every financial lock
    // stays with the original until its same-epoch competitor actually wins.
    return { writes: new Map([[tapAlternativeKey(id), { submission: key, candidate_id: id, value: v }],
      ['payout/recovery/accepted/' + key, { submission: key, candidate_id: id }]]), id };
  }
  need(id === v.candidate_id && v.checkpoint?.root_preparation_id === id &&
    same(ids(v.checkpoint), ids(plan)) && Object.keys(intent).every(k => same(v.checkpoint[k], intent[k])), 'winner differs from authorized candidate');
  checked(c.normalizeTargetedTapSettlementValue(v.checkpoint));
  verifyWinningTapEpoch(v.checkpoint, candidate.source.intent, v.execution_proof,
    tapOpeningClaimCeilings(candidate.source.intent, opening), opening, amounts(intent.entries));
  view.tx = key;
  // The ordinary current V31 checkpoint enforces the actual mined proposal
  // clock, both cumulative caps and all liability, payee and replay keys.
  checked(await view.targetedTapSettlement(v.checkpoint));
  writes.set(tapDebtResolutionKey(candidate.source.root_preparation_id), { submission: key,
    candidate_submission: record.submission, epoch: v.epoch, root_preparation_id: id });
  return { writes, id };
}

async function applyExcluded(c, key, v, version) {
  checked(c.validateExactObjectKeys(v, ['op', 'contract_version', 'rail', 'epoch', 'epoch_apply_hash', 'opening_epoch',
    'basis_epoch', 'source', 'tap_rate_lock', 'skipped_epoch_proof', 'preparation_signatures', 'admin', 'admin_sig',
    ...(v.stage === 'recover_excluded_au' ? ['stage','original_bundle'] : []),
    ...(Object.hasOwn(v, 'previous_epoch') ? ['previous_epoch'] : [])],
  'TAP prepared debt recovery'));
  need(v.contract_version === version && v.rail === 'tap' && v.op === 'recover_prepared_tap' &&
    positive(v.epoch) && hash(v.source?.root_preparation_id) && hash(v.admin) &&
    c.isHexBytes(v.admin_sig, 64) && Array.isArray(v.preparation_signatures) &&
    v.preparation_signatures.length <= 33 && v.preparation_signatures.every(s => c.isHexBytes(s, 64)), 'invalid envelope');
  need(key === await tapDebtKey(c, v), 'submission key mismatch');
  checked(await c.requireTargetedPayoutAdminSignature(v, 'TAP prepared debt recovery'));
  const resolvedKey = tapDebtResolutionKey(v.source.root_preparation_id);
  const existing = await c.get(resolvedKey);
  if (existing !== null) {
    need(existing.submission === key, 'source already assigned to another recovery');
    return { result: { ok: true, idempotent: true, root_preparation_id: existing.root_preparation_id } };
  }
  const { intent, records, tails } = await project(c, v);
  const plan = await tapDebtPlan(c, intent);
  need(plan.preparations.length === v.preparation_signatures.length, 'incomplete successor signatures');
  const { writes } = await stageTapReplacement(c, key, intent, records, tails, plan, v.admin, v.preparation_signatures, version);
  writes.set(resolvedKey, { submission: key, basis_epoch: v.basis_epoch, epoch: v.epoch,
    root_preparation_id: plan.root_preparation_id });
  return { writes, id: plan.root_preparation_id };
}

// Certificates establish history only. They never replace a liability lock,
// mark an AU paid, create an executable root, or claim an external effect.
async function certifyBasis(c, key, v, version) {
  checked(c.validateExactObjectKeys(v, ['op','stage','contract_version','rail','epoch','source','opening_epoch','previous_epoch',
    'skipped_epoch_proof','admin','admin_sig'], 'TAP prepared debt evidence'));
  const s = v.source, old = validateSource(c, s);
  need(v.contract_version === version && v.op === 'recover_prepared_tap' && v.rail === 'tap' &&
    Number.isSafeInteger(v.opening_epoch) && v.opening_epoch >= 0 && v.opening_epoch < old.epoch &&
    old.epoch < v.epoch && hash(v.admin) && c.isHexBytes(v.admin_sig, 64) && key === await tapDebtKey(c, v), 'invalid evidence envelope');
  checked(await c.requireTargetedPayoutAdminSignature(v, 'TAP exclusion evidence'));
  const opening = await checkpoint(c, v.opening_epoch, old), basis = await checkpoint(c, v.epoch, old);
  const id = s.root_preparation_id;
  const previous = await c.get(evidenceKey(id, v.epoch));
  if (previous !== null) {
    need(previous.submission === key, 'different exclusion certificate exists');
    return { result: { ok: true, idempotent: true, root_preparation_id: id } };
  }
  checked(c.normalizeTargetedTapDistribution(old, checked(c.normalizeTargetedTapRateLock(old.tap_rate_lock))));
  need(same(ids(await tapDebtPlan(c, old)), ids(s)), 'uncertified source identities');
  // A confirmed prefix binds the bundle. A later transfer independently
  // requires the complete canonical source and all of its live locks.
  const first = await c.get(c.payoutPreparationRecordKey('tap', s.preparation_ids[0]));
  need(first?.payload?.tap_rate_lock?.bundle_sha256 === old.tap_rate_lock.bundle_sha256 &&
    first.epoch === old.epoch && first.consumed === false && !first.superseded_by, 'missing source prefix');
  await verifyTapDebtBasis(c, s, opening, basis, v.skipped_epoch_proof, v.previous_epoch);
  return { id, writes: new Map([[evidenceKey(id, v.epoch), { submission: key, opening_epoch: v.opening_epoch,
    basis_epoch: v.epoch, root: basis.root, execution_block_hash: basis.execution_block_hash }]]) };
}

export async function applyTapDebt(c, key, v, version) {
  let validated;
  c = boundedReads(c);
  try {
    const handler = v.stage === 'certify_basis' ? certifyBasis :
      v.stage === undefined || v.stage === 'recover_excluded_au' ? applyExcluded : applyAu;
    validated = await handler(c, key, v, version);
    if (validated.result) return validated.result;
  } catch (error) {
    if (error instanceof TapReadFailure) throw error;
    return error;
  }
  // Propagate storage write failures to the consensus batch owner.
  for (const [k, value] of validated.writes) await c.put(k, value);
  return { ok: true, idempotent: false, root_preparation_id: validated.id };
}

export async function tapDebtEnvelope(c, intent, p, admin, version) {
  const payload_hash = await c.opaqueHash('mayhem-targeted-payout-preparation-payload-v1', {
    economic_op_id: p.economic_op_id, rail: 'tap', epoch: intent.epoch, epoch_apply_hash: intent.epoch_apply_hash,
    kind: p.kind, output_index: p.output_index, payload: p.payload });
  return { op: 'prepare_targeted_payout', contract_version: version, rail: 'tap', epoch: intent.epoch,
    epoch_apply_hash: intent.epoch_apply_hash, prepared_at: intent.at, ...p, payload_hash, admin };
}
