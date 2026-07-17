import fs from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';

const ROOT_KINDS = ['dep', 'use', 'earn', 'fee', 'price'];
const LEDGER_RAILS = new Set(['fiat', 'tap', 'tnk']);
const LEDGER_RAIL_ORDER = ['fiat', 'tap', 'tnk'];
const MAX_OPERATOR_FEE_BPS = 1_500;
const TAP_BURN_BPS = 1_000;
const SESSION_RECEIPT_SCHEMA_VERSION = 9;

export const stableValue = (value) => {
  if (Array.isArray(value)) return value.map((item) => stableValue(item));
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stableValue(value[key])])
    );
  }
  return value;
};

export const stableJson = (value) => JSON.stringify(stableValue(value));

export async function opaqueHash(domain, value) {
  const digest = await blake3(b4a.from(stableJson({ domain, value })));
  return b4a.toString(digest, 'hex');
}

async function merkleRoot(kind, leaves) {
  if (leaves.length === 0) return await opaqueHash(`mayhem-${kind}-empty-root-v1`, {});
  let level = leaves.slice().sort();
  while (level.length > 1) {
    const next = [];
    for (let idx = 0; idx < level.length; idx += 2) {
      const left = level[idx];
      const right = idx + 1 < level.length ? level[idx + 1] : left;
      next.push(await opaqueHash(`mayhem-${kind}-node-v1`, { left, right }));
    }
    level = next;
  }
  return level[0];
}

function safeCount(value, label, { allowZero = false } = {}) {
  if (!Number.isSafeInteger(value) || value < 0 || (!allowZero && value === 0)) {
    throw new Error(`${label} must be a ${allowZero ? 'non-negative' : 'positive'} safe integer`);
  }
  return value;
}

function safeAu(value, label, { allowZero = false } = {}) {
  let parsed;
  if (typeof value === 'bigint') {
    parsed = value;
  } else if (typeof value === 'string' && /^(0|[1-9][0-9]*)$/.test(value)) {
    parsed = BigInt(value);
  } else if (Number.isSafeInteger(value) && value >= 0) {
    parsed = BigInt(value);
  } else {
    throw new Error(`${label} must be a canonical decimal au string`);
  }
  if (parsed < 0n || (!allowZero && parsed === 0n)) {
    throw new Error(`${label} must be ${allowZero ? 'non-negative' : 'positive'}`);
  }
  return parsed;
}

function canonicalAu(value) {
  if (typeof value !== 'bigint' || value < 0n) throw new Error('invalid au value');
  return value.toString();
}

function normalizeLedgerRail(value) {
  if (typeof value !== 'string') throw new Error('receipt rail is required');
  const rail = value.toLowerCase();
  if (!LEDGER_RAILS.has(rail)) throw new Error('receipt rail is unsupported');
  return rail;
}

function addRailAmount(map, rail, id, amount, label) {
  if (typeof id !== 'string' || id.length === 0) throw new Error(`${label} id is required`);
  const key = JSON.stringify([rail, id]);
  const current = map.get(key) ?? { rail, id, au: 0n };
  const next = current.au + amount;
  safeAu(next, label, { allowZero: true });
  map.set(key, { ...current, au: next });
}

function marketUsageKey(enclaveId, ctxBracket) {
  return JSON.stringify([enclaveId, ctxBracket ?? null]);
}

function addMarketUsage(map, body, sessionId, amount) {
  const enclaveId = body.enclave_id;
  if (typeof enclaveId !== 'string' || enclaveId.length === 0) {
    throw new Error('receipt enclave_id is required');
  }
  const ctxBracket = body.ctx_bracket ?? null;
  if (ctxBracket !== null && (typeof ctxBracket !== 'string' || ctxBracket.length === 0)) {
    throw new Error('receipt ctx_bracket is invalid');
  }
  const ctxBracketTableVer = body.ctx_bracket_table_ver ?? null;
  if (
    ctxBracketTableVer !== null &&
    (!Number.isSafeInteger(ctxBracketTableVer) || ctxBracketTableVer < 1)
  ) {
    throw new Error('receipt ctx_bracket_table_ver is invalid');
  }
  if (typeof sessionId !== 'string' || sessionId.length === 0) {
    throw new Error('receipt session_id is required');
  }
  const key = marketUsageKey(enclaveId, ctxBracket);
  const current = map.get(key) ?? {
    enclave_id: enclaveId,
    ...(ctxBracket ? { ctx_bracket: ctxBracket } : {}),
    ...(ctxBracketTableVer ? { ctx_bracket_table_ver: ctxBracketTableVer } : {}),
    demand_au: 0n,
    sessions: new Set(),
    providers: new Set(),
  };
  if ((current.ctx_bracket_table_ver ?? null) !== (ctxBracketTableVer ?? current.ctx_bracket_table_ver ?? null)) {
    throw new Error('receipt ctx_bracket_table_ver mismatch within market usage bucket');
  }
  const next = current.demand_au + amount;
  safeAu(next, 'market demand_au', { allowZero: true });
  current.demand_au = next;
  current.sessions.add(sessionId);
  current.providers.add(body.provider);
  map.set(key, current);
}

function sortedRailEntries(map) {
  return Array.from(map.values()).sort((a, b) => {
    const railOrder = LEDGER_RAIL_ORDER.indexOf(a.rail) - LEDGER_RAIL_ORDER.indexOf(b.rail);
    if (railOrder !== 0) return railOrder;
    return a.id.localeCompare(b.id);
  });
}

function sortedMarketUsageEntries(map) {
  return Array.from(map.values())
    .sort((a, b) => (
      a.enclave_id.localeCompare(b.enclave_id) ||
      String(a.ctx_bracket ?? '').localeCompare(String(b.ctx_bracket ?? ''))
    ))
    .map((entry) => ({
      enclave_id: entry.enclave_id,
      ...(entry.ctx_bracket ? { ctx_bracket: entry.ctx_bracket } : {}),
      ...(entry.ctx_bracket_table_ver ? { ctx_bracket_table_ver: entry.ctx_bracket_table_ver } : {}),
      demand_au: canonicalAu(entry.demand_au),
      session_count: entry.sessions.size,
      provider_count: entry.providers.size,
    }));
}

function canonicalUsageUnit(unit) {
  switch (unit) {
    case 'in':
    case 'in_tokens':
    case 'input':
    case 'input_tokens':
    case 'prompt_tokens':
    case 'input_token':
      return 'input_token';
    case 'cached_input':
    case 'cached_inputs':
    case 'cached_input_tokens':
    case 'cached_prompt_tokens':
    case 'cached_tokens':
    case 'cached_input_token':
      return 'cached_input_token';
    case 'out':
    case 'out_tokens':
    case 'output':
    case 'output_tokens':
    case 'completion_tokens':
    case 'output_token':
      return 'output_token';
    case 'images':
    case 'image':
      return 'image';
    case 'steps':
    case 'step':
      return 'step';
    default:
      return unit;
  }
}

function normalizeReceiptUsage(usageSource) {
  if (!usageSource || typeof usageSource !== 'object' || Array.isArray(usageSource)) {
    throw new Error('receipt usage must be an object');
  }
  const usage = {};
  for (const [rawUnit, count] of Object.entries(usageSource)) {
    if (typeof rawUnit !== 'string' || rawUnit.length === 0 || rawUnit.length > 64) {
      throw new Error('receipt usage unit is invalid');
    }
    const unit = canonicalUsageUnit(rawUnit);
    if (!/^[a-zA-Z0-9._:-]{1,128}$/.test(unit)) {
      throw new Error('receipt usage unit is invalid');
    }
    if (!Number.isSafeInteger(count) || count < 0) {
      throw new Error('receipt usage count must be a non-negative safe integer');
    }
    if (count === 0) continue;
    const next = (usage[unit] ?? 0) + count;
    safeCount(next, 'receipt usage count', { allowZero: true });
    usage[unit] = next;
  }
  return Object.fromEntries(Object.entries(usage).sort(([left], [right]) => left.localeCompare(right)));
}

function normalizeCurrentReceiptBody(body) {
  if (body.schema_version !== SESSION_RECEIPT_SCHEMA_VERSION) {
    throw new Error(`receipt schema_version must be ${SESSION_RECEIPT_SCHEMA_VERSION}`);
  }
  if (typeof body.billing_id !== 'string' || !/^[0-9a-f]{64}$/.test(body.billing_id)) {
    throw new Error('receipt billing_id must be 32 bytes of lowercase hex');
  }
  safeCount(body.billing_attempt, 'receipt billing_attempt', { allowZero: true });
  const billingPriorUsage = normalizeReceiptUsage(body.billing_prior_usage);
  if (stableJson(billingPriorUsage) !== stableJson(body.billing_prior_usage)) {
    throw new Error('receipt billing_prior_usage must be canonical');
  }
  const current = {
    ...body,
    billing_prior_usage: billingPriorUsage,
    usage: normalizeReceiptUsage(body.usage),
  };
  if (stableJson(current.usage) !== stableJson(body.usage)) {
    throw new Error('receipt usage must be canonical');
  }
  for (const field of [
    'billing_prior_au_owed_cum',
    'locked_per_req_au',
    'locked_min_session_au',
    'au_owed_cum',
  ]) {
    current[field] = canonicalAu(safeAu(current[field], `receipt ${field}`, { allowZero: true }));
  }
  if (
    current.billing_attempt === 0 &&
    (Object.keys(current.billing_prior_usage).length > 0 || current.billing_prior_au_owed_cum !== '0')
  ) {
    throw new Error('initial receipt billing attempt must have an empty baseline');
  }
  if (
    current.billing_prior_au_owed_cum === '0' &&
    Object.keys(current.billing_prior_usage).length > 0
  ) {
    throw new Error('receipt billing prior usage requires a prior cumulative amount');
  }
  assertUsageMonotonic(current.billing_prior_usage, current.usage);
  if (safeAu(current.au_owed_cum, 'receipt au_owed_cum') <
      safeAu(current.billing_prior_au_owed_cum, 'receipt billing_prior_au_owed_cum', { allowZero: true })) {
    throw new Error('receipt cumulative au regressed below its signed billing baseline');
  }
  return current;
}

function assertUsageMonotonic(previous, current) {
  for (const [unit, previousCount] of Object.entries(previous)) {
    if ((current[unit] ?? 0) < previousCount) {
      throw new Error(`receipt cumulative usage regressed for ${unit}`);
    }
  }
}

function receiptEnvelope(entry) {
  const receipt = entry.receipt ?? entry;
  const bodySource = receipt.body ?? receipt;
  const {
    enclave_sig: _enclaveSig,
    enclave_pubkey: _enclavePubkey,
    user_sig: _userSig,
    receipt_ack: _receiptAck,
    voucher: _voucher,
    ...body
  } = bodySource;
  const currentBody = normalizeCurrentReceiptBody(body);
  const envelope = {
    body: currentBody,
    enclave_sig: receipt.enclave_sig ?? entry.enclave_sig ?? null,
    user_sig: receipt.user_sig ?? entry.user_sig ?? null,
  };
  return envelope;
}

function receiptLeafEnvelope(envelope) {
  return {
    body: stableValue(envelope.body),
    enclave_sig: envelope.enclave_sig,
    user_sig: envelope.user_sig,
  };
}

function receiptAmount(entry, body, billingStates) {
  const explicit = entry.settle_au ?? entry.au_delta;
  if (explicit !== undefined) {
    throw new Error('receipt settlement amount must derive from the signed logical billing chain');
  }

  const current = safeAu(body.au_owed_cum, 'receipt au_owed_cum');
  const signedPrior = safeAu(
    body.billing_prior_au_owed_cum,
    'receipt billing_prior_au_owed_cum',
    { allowZero: true }
  );
  const state = billingStates.get(body.billing_id);
  let previous = signedPrior;
  let attemptPriorUsage = body.billing_prior_usage;
  let attemptPriorAu = signedPrior;

  if (!state && (signedPrior !== 0n || Object.keys(body.billing_prior_usage).length > 0)) {
    throw new Error('receipt logical billing baseline has no prior signed receipt in the batch');
  }
  if (state) {
    if (body.billing_attempt < state.attempt) {
      throw new Error('receipt billing attempt regressed');
    }
    if (body.billing_attempt === state.attempt) {
      if (body.session_id !== state.sessionId) {
        throw new Error('receipt transport session changed within one billing attempt');
      }
      if (
        signedPrior !== state.attemptPriorAu ||
        stableJson(body.billing_prior_usage) !== stableJson(state.attemptPriorUsage)
      ) {
        throw new Error('receipt billing attempt baseline changed');
      }
      if (body.seq <= state.seq) throw new Error('receipt sequence did not advance');
    } else if (
      signedPrior !== state.currentAu ||
      stableJson(body.billing_prior_usage) !== stableJson(state.currentUsage)
    ) {
      throw new Error('receipt redispatch baseline does not match prior logical settlement');
    }
    assertUsageMonotonic(state.currentUsage, body.usage);
    previous = state.currentAu;
    if (body.billing_attempt === state.attempt) {
      attemptPriorUsage = state.attemptPriorUsage;
      attemptPriorAu = state.attemptPriorAu;
    }
  }

  if (entry.previous_au_owed_cum !== undefined) {
    const declaredPrevious = safeAu(
      entry.previous_au_owed_cum,
      'receipt previous_au_owed_cum',
      { allowZero: true }
    );
    if (declaredPrevious !== previous) {
      throw new Error('receipt previous_au_owed_cum contradicts the signed logical billing chain');
    }
  }
  if (current < previous) throw new Error('receipt cumulative au regressed');
  billingStates.set(body.billing_id, {
    attempt: body.billing_attempt,
    attemptPriorUsage,
    attemptPriorAu,
    sessionId: body.session_id,
    seq: body.seq,
    currentUsage: body.usage,
    currentAu: current,
  });
  return current - previous;
}

async function depositRoot(deposits) {
  let root = null;
  let count = 0;
  let auTotal = 0n;
  for (const deposit of deposits) {
    const au = safeAu(deposit.au, 'deposit au');
    count += 1;
    auTotal += au;
    safeAu(auTotal, 'deposit au_total', { allowZero: true });
    const leaf = deposit.leaf ?? await opaqueHash('mayhem-deposit-leaf-v1', stableValue(deposit));
    root = root
      ? await opaqueHash('mayhem-deposit-root-v1', { previous_root: root, leaf, count })
      : leaf;
  }
  return {
    root: root ?? await opaqueHash('mayhem-dep-empty-root-v1', {}),
    count,
    auTotal: canonicalAu(auTotal),
  };
}

function depositRootFromEvidence(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return null;
  }
  const root = value.root ?? value.merkle_root;
  if (typeof root !== 'string' || !/^[0-9a-f]{64}$/.test(root)) {
    throw new Error('deposit_root.merkle_root must be 32 bytes of hex');
  }
  const count = safeCount(value.count, 'deposit_root count', { allowZero: true });
  const auTotal = canonicalAu(safeAu(value.au_total, 'deposit_root au_total', { allowZero: true }));
  return { root, count, auTotal };
}

export async function recomputeEpoch(bundle) {
  if (!bundle || typeof bundle !== 'object' || Array.isArray(bundle)) {
    throw new Error('epoch bundle must be an object');
  }
  const epoch = safeCount(bundle.epoch, 'epoch');
  const feeBps = adminFeeBps(bundle);
  if (feeBps > MAX_OPERATOR_FEE_BPS) throw new Error(`fee_bps must be <= ${MAX_OPERATOR_FEE_BPS}`);

  const deposits = Array.isArray(bundle.deposits) ? bundle.deposits : [];
  const receipts = Array.isArray(bundle.receipts) ? bundle.receipts : [];
  const priceDerivations = Array.isArray(bundle.price_derivations) ? bundle.price_derivations : [];
  const payouts = Array.isArray(bundle.payouts) ? bundle.payouts : [];
  if (payouts.length > 0) {
    throw new Error('payouts are non-custodial TAP claims; do not include payout entries in epoch bundles');
  }
  const priorEarnings = bundle.prior_earnings && typeof bundle.prior_earnings === 'object'
    ? bundle.prior_earnings
    : {};
  const priorFeeCumAu = safeAu(bundle.prior_fee_cum_au ?? '0', 'prior_fee_cum_au', { allowZero: true });
  if (!Object.prototype.hasOwnProperty.call(bundle, 'prior_burn_cum_au')) {
    throw new Error('prior_burn_cum_au is required by the current epoch bundle');
  }
  const priorBurnCumAu = safeAu(bundle.prior_burn_cum_au, 'prior_burn_cum_au', { allowZero: true });

  const dep = depositRootFromEvidence(bundle.deposit_root) ?? await depositRoot(deposits);
  const usageLeaves = [];
  const debitMap = new Map();
  const grossEarningMap = new Map();
  const marketUsageMap = new Map();
  const billingStates = new Map();
  const sessions = new Set();

  const normalizedReceipts = receipts
    .map((entry) => ({ entry, envelope: receiptEnvelope(entry) }))
    .sort((a, b) => (
      String(a.envelope.body.billing_id).localeCompare(String(b.envelope.body.billing_id)) ||
      Number(a.envelope.body.billing_attempt ?? 0) - Number(b.envelope.body.billing_attempt ?? 0) ||
      Number(a.envelope.body.seq ?? 0) - Number(b.envelope.body.seq ?? 0)
    ));

  for (const { entry, envelope } of normalizedReceipts) {
    const { body } = envelope;
    for (const field of ['session_id', 'user', 'provider']) {
      if (typeof body[field] !== 'string' || body[field].length === 0) {
        throw new Error(`receipt ${field} is required`);
      }
    }
    const settleAu = receiptAmount(entry, body, billingStates);
    if (settleAu === 0n) continue;
    const rail = normalizeLedgerRail(entry.rail ?? body.rail);
    sessions.add(body.billing_id);
    addRailAmount(debitMap, rail, body.user, settleAu, 'debit');
    addRailAmount(grossEarningMap, rail, body.provider, settleAu, 'earning');
    addMarketUsage(marketUsageMap, body, body.billing_id, settleAu);
    usageLeaves.push(await opaqueHash('mayhem-usage-leaf-v1', receiptLeafEnvelope(envelope)));
  }

  const earningEntries = [];
  let feeAu = 0n;
  let burnAu = 0n;
  let earnCumAu = 0n;
  for (const entry of sortedRailEntries(grossEarningMap)) {
    const { rail, id: provider, au: grossAu } = entry;
    const providerFeeAu = (grossAu * BigInt(feeBps)) / 10_000n;
    const providerBurnAu = rail === 'tap'
      ? (grossAu * BigInt(TAP_BURN_BPS)) / 10_000n
      : 0n;
    const netAu = grossAu - providerFeeAu - providerBurnAu;
    feeAu += providerFeeAu;
    safeAu(feeAu, 'fee_au', { allowZero: true });
    burnAu += providerBurnAu;
    safeAu(burnAu, 'burn_au', { allowZero: true });
    const priorAu = safeAu(priorEarnings[`${rail}/${provider}`] ?? '0', 'prior provider earning', { allowZero: true });
    const cumulativeAu = priorAu + netAu;
    safeAu(cumulativeAu, 'provider cumulative earning', { allowZero: true });
    earnCumAu += cumulativeAu;
    safeAu(earnCumAu, 'earn_au', { allowZero: true });
    earningEntries.push({
      rail,
      provider,
      gross_au: canonicalAu(grossAu),
      net_au: canonicalAu(netAu),
      cumulative_au: canonicalAu(cumulativeAu),
    });
  }
  const feeCumAu = priorFeeCumAu + feeAu;
  safeAu(feeCumAu, 'fee_cum_au', { allowZero: true });
  const burnCumAu = priorBurnCumAu + burnAu;
  safeAu(burnCumAu, 'burn_cum_au', { allowZero: true });

  const roots = {
    dep: dep.root,
    use: await merkleRoot('use', usageLeaves),
    earn: await merkleRoot(
      'earn',
      await Promise.all(earningEntries.map((entry) => opaqueHash('mayhem-earn-leaf-v1', entry)))
    ),
    fee: await opaqueHash('mayhem-fee-root-v1', {
      epoch,
      fee_au: canonicalAu(feeAu),
      fee_cum_au: canonicalAu(feeCumAu),
      burn_au: canonicalAu(burnAu),
      burn_cum_au: canonicalAu(burnCumAu),
      tap_burn_bps: TAP_BURN_BPS,
    }),
    price: await merkleRoot(
      'price',
      await Promise.all(priceDerivations.map((entry) => opaqueHash(
        'mayhem-price-derivation-leaf-v1',
        stableValue(priceDerivationLeafValue(entry))
      )))
    ),
  };
  for (const key of ROOT_KINDS) {
    if (!/^[0-9a-f]{64}$/.test(roots[key])) throw new Error(`invalid ${key} root`);
  }

  const debits = sortedRailEntries(debitMap)
    .map(({ rail, id: user, au }) => ({ rail, user, au: canonicalAu(au) }));
  const earnings = sortedRailEntries(grossEarningMap)
    .map(({ rail, id: provider, au }) => ({ rail, provider, gross_au: canonicalAu(au) }));
  const market_usage = sortedMarketUsageEntries(marketUsageMap);
  const useAu = sortedRailEntries(debitMap).reduce((sum, entry) => sum + entry.au, 0n);
  const totals = {
    dep_count: dep.count,
    dep_au: dep.auTotal,
    use_count: sessions.size,
    use_au: canonicalAu(useAu),
    provider_count: grossEarningMap.size,
    earn_au: canonicalAu(earnCumAu),
    fee_au: canonicalAu(feeAu),
    fee_cum_au: canonicalAu(feeCumAu),
    burn_au: canonicalAu(burnAu),
    burn_cum_au: canonicalAu(burnCumAu),
    price_count: priceDerivations.length,
  };

  return {
    epoch,
    params: { fee_bps: feeBps, tap_burn_bps: TAP_BURN_BPS },
    roots,
    totals,
    debits,
    earnings,
    market_usage,
  };
}

function priceDerivationLeafValue(derivation) {
  if (!derivation || typeof derivation !== 'object' || Array.isArray(derivation)) {
    throw new Error('price_derivations entries must be objects');
  }
  const {
    derivation_hash: _derivationHash,
    price_root: _priceRoot,
    updated_at: _updatedAt,
    ...leaf
  } = derivation;
  return leaf;
}

function adminFeeBps(bundle) {
  if (Object.prototype.hasOwnProperty.call(bundle, 'fee_bps')) {
    throw new Error('top-level fee_bps is not accepted; use params.fee_bps from admin contract params');
  }
  if (!bundle.params || typeof bundle.params !== 'object' || Array.isArray(bundle.params)) {
    throw new Error('epoch bundle params.fee_bps is required from admin contract params');
  }
  const feeBps = bundle.params.fee_bps;
  return safeCount(feeBps, 'params.fee_bps', { allowZero: true });
}

async function main() {
  const inputPath = process.argv[2];
  if (!inputPath) throw new Error('usage: node scripts/recompute-epoch-roots.mjs <bundle.json>');
  const bundle = JSON.parse(await fs.readFile(inputPath, 'utf8'));
  const result = await recomputeEpoch(bundle);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((err) => {
    console.error(err.message);
    process.exitCode = 1;
  });
}
