import fs from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';

const ROOT_KINDS = ['dep', 'use', 'earn', 'fee', 'price'];
const LEDGER_RAILS = new Set(['fiat', 'tap', 'tnk']);
const LEDGER_RAIL_ORDER = ['fiat', 'tap', 'tnk'];
const MAX_OPERATOR_FEE_BPS = 5_000;
const SESSION_RECEIPT_SCHEMA_VERSION = 6;
const CTX_BRACKET_TABLE_VERSION = 1;

function ctxBracketForTokens(tokens) {
  if (!Number.isSafeInteger(tokens) || tokens < 0) {
    throw new Error('receipt served_ctx is invalid');
  }
  if (tokens <= 8_192) return 'le8k';
  if (tokens <= 32_768) return 'le32k';
  if (tokens <= 131_072) return 'le128k';
  if (tokens <= 262_144) return 'le256k';
  return 'gt256k';
}

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

function safeAmount(value, label, { allowZero = false } = {}) {
  if (!Number.isSafeInteger(value) || value < 0 || (!allowZero && value === 0)) {
    throw new Error(`${label} must be a ${allowZero ? 'non-negative' : 'positive'} safe integer`);
  }
  return value;
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
  const current = map.get(key) ?? { rail, id, mu: 0 };
  const next = current.mu + amount;
  safeAmount(next, label, { allowZero: true });
  map.set(key, { ...current, mu: next });
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
    demand_mu: 0,
    sessions: new Set(),
  };
  if ((current.ctx_bracket_table_ver ?? null) !== (ctxBracketTableVer ?? current.ctx_bracket_table_ver ?? null)) {
    throw new Error('receipt ctx_bracket_table_ver mismatch within market usage bucket');
  }
  const next = current.demand_mu + amount;
  safeAmount(next, 'market demand_mu', { allowZero: true });
  current.demand_mu = next;
  current.sessions.add(sessionId);
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
      demand_mu: entry.demand_mu,
      session_count: entry.sessions.size,
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
    safeAmount(next, 'receipt usage count', { allowZero: true });
    usage[unit] = next;
  }
  return Object.fromEntries(Object.entries(usage).sort(([left], [right]) => left.localeCompare(right)));
}

function migrateReceiptBody(body, targetSchemaVersion = SESSION_RECEIPT_SCHEMA_VERSION) {
  if (!Number.isSafeInteger(body.schema_version) || body.schema_version < 1) {
    throw new Error('receipt schema_version is unsupported');
  }
  if (body.schema_version > targetSchemaVersion) {
    throw new Error(`unsupported receipt schema migration ${body.schema_version} -> ${targetSchemaVersion}`);
  }
  const migrated = {
    ...body,
    usage: normalizeReceiptUsage(body.usage),
  };
  while (migrated.schema_version < targetSchemaVersion) {
    if (migrated.schema_version === 1) {
      migrated.schema_version = 2;
    } else if (migrated.schema_version === 2) {
      migrated.schema_version = 3;
    } else if (migrated.schema_version === 3) {
      migrated.locked_per_req_mu ??= 0;
      migrated.locked_min_session_mu ??= 0;
      migrated.schema_version = 4;
    } else if (migrated.schema_version === 4) {
      migrated.served_ctx ??= 0;
      migrated.schema_version = 5;
    } else if (migrated.schema_version === 5) {
      migrated.ctx_bracket ??= ctxBracketForTokens(migrated.served_ctx ?? 0);
      migrated.ctx_bracket_table_ver ??= CTX_BRACKET_TABLE_VERSION;
      migrated.schema_version = 6;
    } else {
      throw new Error(`unsupported receipt schema migration ${migrated.schema_version} -> ${targetSchemaVersion}`);
    }
  }
  return migrated;
}

function receiptEnvelope(entry) {
  const receipt = entry.receipt ?? entry;
  const bodySource = receipt.body ?? receipt;
  const {
    enclave_sig: _enclaveSig,
    user_sig: _userSig,
    receipt_ack: _receiptAck,
    voucher: _voucher,
    ...body
  } = bodySource;
  const migratedBody = migrateReceiptBody(body);
  const envelope = {
    body: migratedBody,
    enclave_sig: receipt.enclave_sig ?? entry.enclave_sig ?? null,
    user_sig: receipt.user_sig ?? entry.user_sig ?? null,
  };
  if (stableJson(body) !== stableJson(migratedBody)) envelope.signed_body = stableValue(body);
  return envelope;
}

function receiptLeafEnvelope(envelope) {
  return {
    body: stableValue(envelope.body),
    enclave_sig: envelope.enclave_sig,
    user_sig: envelope.user_sig,
  };
}

function receiptAmount(entry, body, previousBySession) {
  const explicit = entry.settle_mu ?? entry.mu_delta;
  if (explicit !== undefined) return safeAmount(explicit, 'receipt settle_mu');

  const current = safeAmount(body.mu_owed_cum, 'receipt mu_owed_cum');
  const previous = entry.previous_mu_owed_cum ?? previousBySession.get(body.session_id) ?? 0;
  safeAmount(previous, 'receipt previous_mu_owed_cum', { allowZero: true });
  if (current < previous) throw new Error('receipt cumulative mu regressed');
  return current - previous;
}

async function depositRoot(deposits) {
  let root = null;
  let count = 0;
  let muTotal = 0;
  for (const deposit of deposits) {
    const mu = safeAmount(deposit.mu, 'deposit mu');
    count += 1;
    muTotal += mu;
    safeAmount(muTotal, 'deposit mu_total', { allowZero: true });
    const leaf = deposit.leaf ?? await opaqueHash('mayhem-deposit-leaf-v1', stableValue(deposit));
    root = root
      ? await opaqueHash('mayhem-deposit-root-v1', { previous_root: root, leaf, count })
      : leaf;
  }
  return {
    root: root ?? await opaqueHash('mayhem-dep-empty-root-v1', {}),
    count,
    muTotal,
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
  const count = safeAmount(value.count, 'deposit_root count', { allowZero: true });
  const muTotal = safeAmount(value.mu_total ?? value.muTotal, 'deposit_root mu_total', { allowZero: true });
  return { root, count, muTotal };
}

export async function recomputeEpoch(bundle) {
  if (!bundle || typeof bundle !== 'object' || Array.isArray(bundle)) {
    throw new Error('epoch bundle must be an object');
  }
  const epoch = safeAmount(bundle.epoch, 'epoch');
  const feeBps = adminFeeBps(bundle);
  if (feeBps > MAX_OPERATOR_FEE_BPS) throw new Error('fee_bps must be <= 5000');

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
  const priorFeeCumMu = bundle.prior_fee_cum_mu ?? 0;
  safeAmount(priorFeeCumMu, 'prior_fee_cum_mu', { allowZero: true });

  const dep = depositRootFromEvidence(bundle.deposit_root) ?? await depositRoot(deposits);
  const usageLeaves = [];
  const debitMap = new Map();
  const grossEarningMap = new Map();
  const marketUsageMap = new Map();
  const previousBySession = new Map();
  const sessions = new Set();

  const normalizedReceipts = receipts
    .map((entry) => ({ entry, envelope: receiptEnvelope(entry) }))
    .sort((a, b) => (
      String(a.envelope.body.session_id).localeCompare(String(b.envelope.body.session_id)) ||
      Number(a.envelope.body.seq ?? 0) - Number(b.envelope.body.seq ?? 0)
    ));

  for (const { entry, envelope } of normalizedReceipts) {
    const { body } = envelope;
    for (const field of ['session_id', 'user', 'provider']) {
      if (typeof body[field] !== 'string' || body[field].length === 0) {
        throw new Error(`receipt ${field} is required`);
      }
    }
    const settleMu = receiptAmount(entry, body, previousBySession);
    previousBySession.set(body.session_id, body.mu_owed_cum ?? settleMu);
    if (settleMu === 0) continue;
    const rail = normalizeLedgerRail(entry.rail ?? body.rail);
    sessions.add(body.session_id);
    addRailAmount(debitMap, rail, body.user, settleMu, 'debit');
    addRailAmount(grossEarningMap, rail, body.provider, settleMu, 'earning');
    addMarketUsage(marketUsageMap, body, body.session_id, settleMu);
    usageLeaves.push(await opaqueHash('mayhem-usage-leaf-v1', receiptLeafEnvelope(envelope)));
  }

  const earningEntries = [];
  let feeMu = 0;
  let earnCumMu = 0;
  for (const entry of sortedRailEntries(grossEarningMap)) {
    const { rail, id: provider, mu: grossMu } = entry;
    const providerFeeMu = Math.floor((grossMu * feeBps) / 10_000);
    const netMu = grossMu - providerFeeMu;
    feeMu += providerFeeMu;
    safeAmount(feeMu, 'fee_mu', { allowZero: true });
    const priorMu = priorEarnings[`${rail}/${provider}`] ?? 0;
    safeAmount(priorMu, 'prior provider earning', { allowZero: true });
    const cumulative_mu = priorMu + netMu;
    safeAmount(cumulative_mu, 'provider cumulative earning', { allowZero: true });
    earnCumMu += cumulative_mu;
    safeAmount(earnCumMu, 'earn_mu', { allowZero: true });
    earningEntries.push({ rail, provider, gross_mu: grossMu, net_mu: netMu, cumulative_mu });
  }
  const feeCumMu = priorFeeCumMu + feeMu;
  safeAmount(feeCumMu, 'fee_cum_mu', { allowZero: true });

  const roots = {
    dep: dep.root,
    use: await merkleRoot('use', usageLeaves),
    earn: await merkleRoot(
      'earn',
      await Promise.all(earningEntries.map((entry) => opaqueHash('mayhem-earn-leaf-v1', entry)))
    ),
    fee: await opaqueHash('mayhem-fee-root-v1', { epoch, fee_mu: feeMu, fee_cum_mu: feeCumMu }),
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
    .map(({ rail, id: user, mu }) => ({ rail, user, mu }));
  const earnings = sortedRailEntries(grossEarningMap)
    .map(({ rail, id: provider, mu: gross_mu }) => ({ rail, provider, gross_mu }));
  const market_usage = sortedMarketUsageEntries(marketUsageMap);
  const useMu = debits.reduce((sum, entry) => sum + entry.mu, 0);
  const totals = {
    dep_count: dep.count,
    dep_mu: dep.muTotal,
    use_count: sessions.size,
    use_mu: useMu,
    provider_count: grossEarningMap.size,
    earn_mu: earnCumMu,
    fee_mu: feeMu,
    fee_cum_mu: feeCumMu,
    price_count: priceDerivations.length,
  };

  return { epoch, params: { fee_bps: feeBps }, roots, totals, debits, earnings, market_usage };
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
  return safeAmount(feeBps, 'params.fee_bps', { allowZero: true });
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
