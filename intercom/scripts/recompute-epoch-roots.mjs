import fs from 'node:fs/promises';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';

const ROOT_KINDS = ['dep', 'use', 'earn', 'fee', 'price'];
const LEDGER_RAILS = new Set(['fiat', 'tap', 'tnk']);
const LEDGER_RAIL_ORDER = ['fiat', 'tap', 'tnk'];
const MAX_OPERATOR_FEE_BPS = 1_500;
const TAP_BURN_BPS = 1_000;
const SESSION_RECEIPT_SCHEMA_VERSION = 11;
const SETTLEMENT_RECEIPT_SCHEMA_VERSIONS = new Set([10, SESSION_RECEIPT_SCHEMA_VERSION]);
const CANONICAL_RECEIPT_SNAPSHOT_SCHEMA_VERSION = 1;
// trac-peer's canonical feature ceiling is 64,000 bytes. Keep enough room for
// its compact-encoding envelope instead of discovering an oversized page only
// after the immutable epoch commit has been submitted.
const TARGETED_EPOCH_FEATURE_JSON_MAX_BYTES = 60_000;

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

function canonicalHex(value, bytes, label) {
  if (typeof value !== 'string' || !new RegExp(`^[0-9a-f]{${bytes * 2}}$`).test(value)) {
    throw new Error(`${label} must be ${bytes} bytes of lowercase hex`);
  }
  return value;
}

function canonicalSnapshotHashValue(snapshot) {
  return {
    schema_version: snapshot.schema_version,
    type: snapshot.type,
    settlement_epoch: snapshot.settlement_epoch,
    metadata: snapshot.metadata,
    identities: snapshot.identities,
    heads: snapshot.heads,
  };
}

function sha256Stable(value) {
  return crypto.createHash('sha256').update(stableJson(value)).digest('hex');
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

function normalizeSettlementReceiptBody(body) {
  if (!SETTLEMENT_RECEIPT_SCHEMA_VERSIONS.has(body.schema_version)) {
    throw new Error(
      `receipt schema_version must be one of ${Array.from(SETTLEMENT_RECEIPT_SCHEMA_VERSIONS).join(', ')}`
    );
  }
  if (typeof body.billing_id !== 'string' || !/^[0-9a-f]{64}$/.test(body.billing_id)) {
    throw new Error('receipt billing_id must be 32 bytes of lowercase hex');
  }
  safeCount(body.billing_attempt, 'receipt billing_attempt', { allowZero: true });
  safeCount(body.billing_epoch, 'receipt billing_epoch');
  canonicalHex(body.reservation_id, 32, 'receipt reservation_id');
  canonicalHex(body.payout_revision, 32, 'receipt payout_revision');
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

function canonicalReceiptHead(entry, envelope, expectedSettlementEpoch) {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
    throw new Error('canonical receipt head must be an object');
  }
  const { body } = envelope;
  canonicalHex(envelope.enclave_sig, 64, 'canonical receipt enclave_sig');
  canonicalHex(envelope.enclave_pubkey, 32, 'canonical receipt enclave_pubkey');
  canonicalHex(envelope.user_sig, 64, 'canonical receipt user_sig');
  const settlementEpoch = safeCount(
    entry.epoch,
    'canonical receipt head settlement epoch'
  );
  const billingEpoch = safeCount(
    entry.billing_epoch,
    'canonical receipt head billing_epoch'
  );
  const billingAttempt = safeCount(
    entry.billing_attempt,
    'canonical receipt head billing_attempt',
    { allowZero: true }
  );
  const billingId = canonicalHex(entry.billing_id, 32, 'canonical receipt head billing_id');
  const reservationId = canonicalHex(
    entry.reservation_id,
    32,
    'canonical receipt head reservation_id'
  );
  const payoutRevision = canonicalHex(
    entry.payout_revision,
    32,
    'canonical receipt head payout_revision'
  );
  const receiptHash = canonicalHex(entry.receipt_hash, 32, 'canonical receipt head receipt_hash');
  const incrementalAu = canonicalAu(
    safeAu(entry.incremental_au, 'canonical receipt head incremental_au')
  );
  if (settlementEpoch !== expectedSettlementEpoch) {
    throw new Error('canonical receipt settlement epoch does not match requested epoch');
  }
  if (billingEpoch !== body.billing_epoch) {
    throw new Error('canonical receipt head billing_epoch does not match signed receipt body');
  }
  if (billingEpoch > settlementEpoch) {
    throw new Error('canonical receipt billing_epoch cannot be after its settlement epoch');
  }
  if (billingId !== body.billing_id || billingAttempt !== body.billing_attempt) {
    throw new Error('canonical receipt head identity does not match signed receipt body');
  }
  if (reservationId !== body.reservation_id) {
    throw new Error('canonical receipt head reservation_id does not match signed receipt body');
  }
  if (payoutRevision !== body.payout_revision) {
    throw new Error('canonical receipt head payout_revision does not match signed receipt body');
  }
  return {
    epoch: settlementEpoch,
    billing_epoch: billingEpoch,
    billing_id: billingId,
    billing_attempt: billingAttempt,
    reservation_id: reservationId,
    payout_revision: payoutRevision,
    receipt_hash: receiptHash,
    incremental_au: incrementalAu,
  };
}

function validateCanonicalReceiptSnapshot(bundle, receipts, settlementEpoch) {
  const snapshot = bundle.receipt_snapshot;
  if (!snapshot || typeof snapshot !== 'object' || Array.isArray(snapshot)) {
    throw new Error('canonical receipt_snapshot is required');
  }
  if (
    snapshot.schema_version !== CANONICAL_RECEIPT_SNAPSHOT_SCHEMA_VERSION ||
    snapshot.type !== 'canonical_epoch_receipt_snapshot' ||
    snapshot.settlement_epoch !== settlementEpoch ||
    Object.keys(snapshot).sort().join(',') !==
      'heads,identities,metadata,schema_version,settlement_epoch,snapshot_sha256,type'
  ) {
    throw new Error('canonical receipt_snapshot identity is invalid');
  }
  if (!snapshot.metadata || typeof snapshot.metadata !== 'object' || Array.isArray(snapshot.metadata)) {
    throw new Error('canonical receipt_snapshot metadata is invalid');
  }
  if (
    Object.keys(snapshot.metadata).sort().join(',') !==
    'count,epoch,page_count,page_size,revision,type,updated_at'
  ) {
    throw new Error('canonical receipt_snapshot metadata shape is invalid');
  }
  const count = safeCount(snapshot.metadata.count, 'canonical receipt metadata count', {
    allowZero: true,
  });
  const pageCount = safeCount(
    snapshot.metadata.page_count,
    'canonical receipt metadata page_count',
    { allowZero: true }
  );
  const pageSize = safeCount(
    snapshot.metadata.page_size,
    'canonical receipt metadata page_size'
  );
  const revision = safeCount(
    snapshot.metadata.revision,
    'canonical receipt metadata revision',
    { allowZero: true }
  );
  if (
    snapshot.metadata.type !== 'canonical_receipt_epoch_index' ||
    snapshot.metadata.epoch !== settlementEpoch
  ) {
    throw new Error('canonical receipt metadata epoch does not match requested epoch');
  }
  if (typeof snapshot.metadata.updated_at !== 'string' || snapshot.metadata.updated_at.length === 0) {
    throw new Error('canonical receipt metadata updated_at is invalid');
  }
  if ((count === 0) !== (pageCount === 0)) {
    throw new Error('canonical receipt metadata count/page_count mismatch');
  }
  if (pageCount > count || Math.ceil(count / pageSize) !== pageCount) {
    throw new Error('canonical receipt metadata page bounds are inconsistent');
  }
  if (pageSize > 1_000 || revision < count) {
    throw new Error('canonical receipt metadata counters are inconsistent');
  }
  if (!Array.isArray(snapshot.identities) || !Array.isArray(snapshot.heads)) {
    throw new Error('canonical receipt_snapshot identities and heads must be arrays');
  }
  if (
    snapshot.identities.length !== count ||
    snapshot.heads.length !== count ||
    receipts.length !== count ||
    stableJson(snapshot.heads) !== stableJson(receipts)
  ) {
    throw new Error('canonical receipt_snapshot count or frozen heads mismatch');
  }
  canonicalHex(snapshot.snapshot_sha256, 32, 'canonical receipt snapshot_sha256');
  const expectedHash = sha256Stable(canonicalSnapshotHashValue(snapshot));
  if (snapshot.snapshot_sha256 !== expectedHash) {
    throw new Error('canonical receipt snapshot hash mismatch');
  }
  const seenIdentities = new Set();
  for (let index = 0; index < snapshot.identities.length; index += 1) {
    const identity = snapshot.identities[index];
    if (
      !identity ||
      typeof identity !== 'object' ||
      Array.isArray(identity) ||
      Object.keys(identity).sort().join(',') !== 'billing_attempt,billing_id'
    ) {
      throw new Error('canonical receipt identity shape is invalid');
    }
    const normalized = {
      billing_id: canonicalHex(identity.billing_id, 32, 'canonical receipt identity billing_id'),
      billing_attempt: safeCount(
        identity.billing_attempt,
        'canonical receipt identity billing_attempt',
        { allowZero: true }
      ),
    };
    const identityKey = `${normalized.billing_id}/${normalized.billing_attempt}`;
    if (seenIdentities.has(identityKey)) {
      throw new Error('canonical receipt identities contain a replay');
    }
    seenIdentities.add(identityKey);
    const head = snapshot.heads[index];
    if (
      head?.billing_id !== normalized.billing_id ||
      head?.billing_attempt !== normalized.billing_attempt
    ) {
      throw new Error('canonical receipt identity does not match its head');
    }
  }
  return {
    count,
    pageCount,
    pageSize,
    revision,
    metadata: stableValue(snapshot.metadata),
  };
}

function addTargetedEarning(map, rail, provider, payoutRevision, amount) {
  const key = JSON.stringify([rail, provider, payoutRevision]);
  const current = map.get(key) ?? {
    rail,
    provider,
    payout_revision: payoutRevision,
    au: 0n,
  };
  current.au += amount;
  safeAu(current.au, 'targeted earning', { allowZero: true });
  map.set(key, current);
}

function sortedTargetedEarnings(map) {
  return Array.from(map.values()).sort((left, right) => (
    LEDGER_RAIL_ORDER.indexOf(left.rail) - LEDGER_RAIL_ORDER.indexOf(right.rail) ||
    left.provider.localeCompare(right.provider) ||
    left.payout_revision.localeCompare(right.payout_revision)
  ));
}

function applyPageLimits(bundle, receiptPageSize) {
  const maxApplyBatch = safeCount(bundle.params?.max_apply_batch, 'params.max_apply_batch');
  const maxMarketUsageEntries = safeCount(
    bundle.params?.max_market_usage_entries,
    'params.max_market_usage_entries',
    { allowZero: true }
  );
  if (maxApplyBatch < 2) {
    throw new Error('params.max_apply_batch cannot fit one debit and one earning');
  }
  return {
    maxApplyBatch,
    maxMarketUsageEntries,
    maxAllocations: Math.min(receiptPageSize, Math.floor(maxApplyBatch / 2)),
    maxFeatureJsonBytes: TARGETED_EPOCH_FEATURE_JSON_MAX_BYTES,
  };
}

function emptyApplyPageState() {
  return {
    allocations: [],
    debits: new Map(),
    earnings: new Map(),
    marketUsage: new Map(),
  };
}

function targetedEpochFeatureOperationJsonBytes(
  state,
  page,
  receiptIndex,
  { earningFinals = [], marketUsage = [] } = {}
) {
  const materialized = {
    ...materializeApplyPage(state, page, receiptIndex),
    ...(earningFinals.length > 0 ? { earning_finals: earningFinals } : {}),
    ...(marketUsage.length > 0 ? { market_usage: marketUsage } : {}),
  };
  const key = `epoch/targeted/${Number.MAX_SAFE_INTEGER}/${'0'.repeat(64)}`;
  const common = {
    epoch: Number.MAX_SAFE_INTEGER,
    at: Number.MAX_SAFE_INTEGER,
    epoch_commit_hash: '0'.repeat(64),
    receipt_index: materialized.receipt_index,
    debits: materialized.debits,
    earnings: materialized.earnings,
    allocations: materialized.allocations,
    ...(materialized.earning_finals ? { earning_finals: materialized.earning_finals } : {}),
    ...(materialized.market_usage ? { market_usage: materialized.market_usage } : {}),
    // false is one byte longer than true, so it covers both page positions.
    last_page: false,
  };
  const value = page === 0
    ? {
        op: 'commit_apply_targeted_epoch_page0',
        ...common,
        roots: Object.fromEntries(ROOT_KINDS.map((kind) => [kind, '0'.repeat(64)])),
        totals: {
          dep_count: Number.MAX_SAFE_INTEGER,
          dep_au: '9'.repeat(78),
          use_count: Number.MAX_SAFE_INTEGER,
          use_au: '9'.repeat(78),
          provider_count: Number.MAX_SAFE_INTEGER,
          earn_au: '9'.repeat(78),
          fee_au: '9'.repeat(78),
          fee_cum_au: '9'.repeat(78),
          burn_au: '9'.repeat(78),
          burn_cum_au: '9'.repeat(78),
          price_count: Number.MAX_SAFE_INTEGER,
        },
        supersedes_commit_hash: '0'.repeat(64),
      }
    : {
        op: 'apply_targeted_epoch',
        ...common,
        page: Number.MAX_SAFE_INTEGER,
      };
  const operation = {
    type: 'feature',
    key: `mayhem_${key}`,
    value: {
      dispatch: {
        type: 'mayhem_feature',
        contract_version: Number.MAX_SAFE_INTEGER,
        key,
        hash: '0'.repeat(128),
        value,
        nonce: '0'.repeat(64),
        address: '0'.repeat(64),
      },
    },
  };
  return Buffer.byteLength(JSON.stringify(operation));
}

function applyPageCanFit(state, row, limits, page, receiptIndex, finalMetadata) {
  if (state.allocations.length + 1 > limits.maxAllocations) return false;
  const debitKey = JSON.stringify([row.allocation.rail, row.allocation.user]);
  const earningKey = JSON.stringify([
    row.allocation.rail,
    row.allocation.provider,
    row.allocation.payout_revision,
  ]);
  const usageKey = marketUsageKey(row.body.enclave_id, row.body.ctx_bracket ?? null);
  const debitCount = state.debits.size + (state.debits.has(debitKey) ? 0 : 1);
  const earningCount = state.earnings.size + (state.earnings.has(earningKey) ? 0 : 1);
  const marketUsageCount = state.marketUsage.size + (state.marketUsage.has(usageKey) ? 0 : 1);
  if (!(
    debitCount + earningCount <= limits.maxApplyBatch &&
    marketUsageCount <= limits.maxMarketUsageEntries
  )) return false;

  const candidate = {
    allocations: state.allocations.slice(),
    debits: new Map(state.debits),
    earnings: new Map(Array.from(state.earnings, ([key, value]) => [key, { ...value }])),
    marketUsage: new Map(Array.from(state.marketUsage, ([key, value]) => [
      key,
      {
        ...value,
        sessions: new Set(value.sessions),
        providers: new Set(value.providers),
      },
    ])),
  };
  addApplyPageRow(candidate, row);
  return targetedEpochFeatureOperationJsonBytes(
    candidate,
    page,
    receiptIndex,
    finalMetadata,
  ) <= limits.maxFeatureJsonBytes;
}

function addApplyPageRow(state, row) {
  const amount = safeAu(row.allocation.au, 'apply page allocation au');
  state.allocations.push(row.allocation);
  addRailAmount(
    state.debits,
    row.allocation.rail,
    row.allocation.user,
    amount,
    'apply page debit'
  );
  addTargetedEarning(
    state.earnings,
    row.allocation.rail,
    row.allocation.provider,
    row.allocation.payout_revision,
    amount
  );
  addMarketUsage(state.marketUsage, row.body, row.allocation.session_id, amount);
}

function materializeApplyPage(state, page, receiptIndex) {
  const allocations = state.allocations.slice();
  const debits = sortedRailEntries(state.debits)
    .map(({ rail, id: user, au }) => ({ rail, user, au: canonicalAu(au) }));
  const earnings = sortedTargetedEarnings(state.earnings)
    .map(({ rail, provider, payout_revision, au }) => ({
      rail,
      provider,
      payout_revision,
      gross_au: canonicalAu(au),
    }));
  const pageValue = {
    page,
    receipt_index: receiptIndex.metadata,
    allocations,
    debits,
    earnings,
  };
  return {
    ...pageValue,
    page_sha256: sha256Stable(pageValue),
  };
}

function buildApplyPages(rows, receiptIndex, limits, { earningFinals, marketUsage }) {
  const pages = [];
  let state = emptyApplyPageState();
  for (const [rowIndex, row] of rows.entries()) {
    const finalMetadata = rowIndex === rows.length - 1
      ? { earningFinals, marketUsage }
      : undefined;
    if (!applyPageCanFit(state, row, limits, pages.length, receiptIndex, finalMetadata)) {
      if (state.allocations.length === 0) {
        throw new Error('one canonical receipt allocation exceeds the active apply-page limits');
      }
      pages.push(materializeApplyPage(state, pages.length, receiptIndex));
      state = emptyApplyPageState();
    }
    if (!applyPageCanFit(state, row, limits, pages.length, receiptIndex, finalMetadata)) {
      throw new Error('one canonical receipt allocation exceeds the active apply-page limits');
    }
    addApplyPageRow(state, row);
  }
  if (state.allocations.length > 0) {
    pages.push(materializeApplyPage(state, pages.length, receiptIndex));
  }
  let cumulativeAllocations = 0;
  for (const page of pages) {
    cumulativeAllocations += page.allocations.length;
    page.last_page = cumulativeAllocations === receiptIndex.count;
    if (page.last_page) {
      page.earning_finals = earningFinals;
      page.market_usage = marketUsage;
    }
    const sizingState = {
      allocations: [],
      debits: new Map(),
      earnings: new Map(),
      marketUsage: new Map(),
    };
    for (const row of rows.slice(cumulativeAllocations - page.allocations.length, cumulativeAllocations)) {
      addApplyPageRow(sizingState, row);
    }
    page.max_feature_operation_json_bytes = targetedEpochFeatureOperationJsonBytes(
      sizingState,
      page.page,
      receiptIndex,
      page.last_page ? { earningFinals, marketUsage } : undefined
    );
    if (page.max_feature_operation_json_bytes > limits.maxFeatureJsonBytes) {
      throw new Error('final canonical receipt page exceeds the active feature byte limit');
    }
    page.page_sha256 = sha256Stable({
      page: page.page,
      receipt_index: page.receipt_index,
      allocations: page.allocations,
      debits: page.debits,
      earnings: page.earnings,
      ...(page.earning_finals ? { earning_finals: page.earning_finals } : {}),
      ...(page.market_usage ? { market_usage: page.market_usage } : {}),
      last_page: page.last_page,
      max_feature_operation_json_bytes: page.max_feature_operation_json_bytes,
    });
  }
  if (
    cumulativeAllocations !== receiptIndex.count ||
    pages.length === 0 ||
    pages.slice(0, -1).some((page) => page.last_page) ||
    pages.at(-1)?.last_page !== true
  ) {
    throw new Error('bounded apply pages do not cover the frozen receipt index exactly');
  }
  return pages;
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
  const currentBody = normalizeSettlementReceiptBody(body);
  const envelope = {
    body: currentBody,
    enclave_sig: receipt.enclave_sig ?? entry.enclave_sig ?? null,
    enclave_pubkey: receipt.enclave_pubkey ?? entry.enclave_pubkey ?? null,
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

function canonicalReceiptEnvelope(envelope) {
  return {
    body: stableValue(envelope.body),
    enclave_sig: envelope.enclave_sig,
    enclave_pubkey: envelope.enclave_pubkey,
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
  const epochSeconds = safeCount(bundle.params?.epoch_seconds ?? 3_600, 'params.epoch_seconds');
  if (feeBps > MAX_OPERATOR_FEE_BPS) throw new Error(`fee_bps must be <= ${MAX_OPERATOR_FEE_BPS}`);

  const deposits = Array.isArray(bundle.deposits) ? bundle.deposits : [];
  const receipts = Array.isArray(bundle.receipts) ? bundle.receipts : [];
  const receiptIndex = validateCanonicalReceiptSnapshot(bundle, receipts, epoch);
  const pageLimits = applyPageLimits(bundle, receiptIndex.pageSize);
  const priceDerivations = Array.isArray(bundle.price_derivations) ? bundle.price_derivations : [];
  if (priceDerivations.length > 0) {
    throw new Error(
      'bounded receipt settlement does not accept external price derivations; price updates require a dedicated canonical operation'
    );
  }
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
  const selectedPayoutRevisions = new Map();
  const marketUsageMap = new Map();
  const billingStates = new Map();
  const allocations = [];
  const allocationRows = [];
  const seenBillingAttempts = new Map();

  const normalizedReceipts = receipts
    .map((entry) => {
      const envelope = receiptEnvelope(entry);
      const head = canonicalReceiptHead(entry, envelope, epoch);
      return { entry, envelope, head };
    })
    .sort((a, b) => (
      String(a.envelope.body.billing_id).localeCompare(String(b.envelope.body.billing_id)) ||
      Number(a.envelope.body.billing_attempt ?? 0) - Number(b.envelope.body.billing_attempt ?? 0) ||
      Number(a.envelope.body.seq ?? 0) - Number(b.envelope.body.seq ?? 0)
    ));

  for (const { entry, envelope, head } of normalizedReceipts) {
    const { body } = envelope;
    for (const field of ['session_id', 'user', 'provider']) {
      if (typeof body[field] !== 'string' || body[field].length === 0) {
        throw new Error(`receipt ${field} is required`);
      }
    }
    const identity = `${body.billing_id}/${body.billing_attempt}`;
    const fingerprint = stableJson({
      head,
      receipt: receiptLeafEnvelope(envelope),
    });
    const priorFingerprint = seenBillingAttempts.get(identity);
    if (priorFingerprint !== undefined) {
      if (priorFingerprint !== fingerprint) {
        throw new Error('conflicting canonical receipt for billing attempt');
      }
      continue;
    }
    seenBillingAttempts.set(identity, fingerprint);
    const computedReceiptHash = await opaqueHash(
      'mayhem-canonical-receipt-v1',
      canonicalReceiptEnvelope(envelope)
    );
    if (computedReceiptHash !== head.receipt_hash) {
      throw new Error('canonical receipt head hash does not match signed receipt envelope');
    }
    const settleAu = receiptAmount(entry, body, billingStates);
    if (canonicalAu(settleAu) !== head.incremental_au) {
      throw new Error('canonical receipt incremental_au does not match signed billing high-water');
    }
    if (settleAu === 0n) continue;
    const rail = normalizeLedgerRail(entry.rail ?? body.rail);
    addRailAmount(debitMap, rail, body.user, settleAu, 'debit');
    const providerRail = JSON.stringify([rail, body.provider]);
    const selectedRevision = selectedPayoutRevisions.get(providerRail);
    if (selectedRevision && selectedRevision !== body.payout_revision) {
      throw new Error('provider rail selected conflicting payout revisions within one epoch');
    }
    selectedPayoutRevisions.set(providerRail, body.payout_revision);
    addTargetedEarning(
      grossEarningMap,
      rail,
      body.provider,
      body.payout_revision,
      settleAu
    );
    addMarketUsage(marketUsageMap, body, body.session_id, settleAu);
    usageLeaves.push(await opaqueHash('mayhem-usage-leaf-v1', receiptLeafEnvelope(envelope)));
    const allocation = {
      session_id: body.session_id,
      user: body.user,
      rail,
      provider: body.provider,
      payout_revision: body.payout_revision,
      billing_epoch: body.billing_epoch,
      billing_id: body.billing_id,
      billing_attempt: body.billing_attempt,
      receipt_seq: body.seq,
      receipt_hash: computedReceiptHash,
      au: canonicalAu(settleAu),
    };
    allocations.push(allocation);
    allocationRows.push({ allocation, body });
  }

  const earningEntries = [];
  let feeAu = 0n;
  let burnAu = 0n;
  let earnCumAu = 0n;
  for (const entry of sortedTargetedEarnings(grossEarningMap)) {
    const { rail, provider, au: grossAu } = entry;
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
  const earnings = sortedTargetedEarnings(grossEarningMap)
    .map(({ rail, provider, payout_revision, au }) => ({
      rail,
      provider,
      payout_revision,
      gross_au: canonicalAu(au),
    }));
  allocations.sort((left, right) => (
    LEDGER_RAIL_ORDER.indexOf(left.rail) - LEDGER_RAIL_ORDER.indexOf(right.rail) ||
    left.user.localeCompare(right.user) ||
    left.billing_epoch - right.billing_epoch ||
    left.billing_id.localeCompare(right.billing_id) ||
    left.billing_attempt - right.billing_attempt ||
    left.session_id.localeCompare(right.session_id)
  ));
  allocationRows.sort((left, right) => (
    LEDGER_RAIL_ORDER.indexOf(left.allocation.rail) -
      LEDGER_RAIL_ORDER.indexOf(right.allocation.rail) ||
    left.allocation.user.localeCompare(right.allocation.user) ||
    left.allocation.billing_epoch - right.allocation.billing_epoch ||
    left.allocation.billing_id.localeCompare(right.allocation.billing_id) ||
    left.allocation.billing_attempt - right.allocation.billing_attempt ||
    left.allocation.session_id.localeCompare(right.allocation.session_id)
  ));
  if (stableJson(allocations) !== stableJson(allocationRows.map((row) => row.allocation))) {
    throw new Error('canonical allocation/page ordering diverged');
  }
  const market_usage = sortedMarketUsageEntries(marketUsageMap);
  const apply_pages = buildApplyPages(allocationRows, receiptIndex, pageLimits, {
    earningFinals: earningEntries,
    marketUsage: market_usage,
  });
  const useAu = sortedRailEntries(debitMap).reduce((sum, entry) => sum + entry.au, 0n);
  const totals = {
    dep_count: dep.count,
    dep_au: dep.auTotal,
    use_count: receiptIndex.count,
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
    params: {
      epoch_seconds: epochSeconds,
      fee_bps: feeBps,
      tap_burn_bps: TAP_BURN_BPS,
    },
    roots,
    totals,
    debits,
    earnings,
    allocations,
    market_usage,
    receipt_index: receiptIndex.metadata,
    apply_page_limits: {
      max_allocations: pageLimits.maxAllocations,
      max_apply_batch: pageLimits.maxApplyBatch,
      max_market_usage_entries: pageLimits.maxMarketUsageEntries,
      max_feature_json_bytes: pageLimits.maxFeatureJsonBytes,
    },
    apply_pages,
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
