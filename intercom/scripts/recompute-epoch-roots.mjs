import fs from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';

const ROOT_KINDS = ['dep', 'use', 'earn', 'fee', 'pay'];

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

function addAmount(map, key, amount, label) {
  if (typeof key !== 'string' || key.length === 0) throw new Error(`${label} id is required`);
  const next = (map.get(key) ?? 0) + amount;
  safeAmount(next, label, { allowZero: true });
  map.set(key, next);
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
  return {
    body,
    enclave_sig: receipt.enclave_sig ?? entry.enclave_sig ?? null,
    user_sig: receipt.user_sig ?? entry.user_sig ?? null,
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

export async function recomputeEpoch(bundle) {
  if (!bundle || typeof bundle !== 'object' || Array.isArray(bundle)) {
    throw new Error('epoch bundle must be an object');
  }
  const epoch = safeAmount(bundle.epoch, 'epoch');
  const feeBps = adminFeeBps(bundle);
  if (feeBps > 10_000) throw new Error('fee_bps must be <= 10000');

  const deposits = Array.isArray(bundle.deposits) ? bundle.deposits : [];
  const receipts = Array.isArray(bundle.receipts) ? bundle.receipts : [];
  const payouts = Array.isArray(bundle.payouts) ? bundle.payouts : [];
  const priorEarnings = bundle.prior_earnings && typeof bundle.prior_earnings === 'object'
    ? bundle.prior_earnings
    : {};
  const priorFeeCumMu = bundle.prior_fee_cum_mu ?? 0;
  safeAmount(priorFeeCumMu, 'prior_fee_cum_mu', { allowZero: true });

  const dep = await depositRoot(deposits);
  const usageLeaves = [];
  const debitMap = new Map();
  const grossEarningMap = new Map();
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
    sessions.add(body.session_id);
    addAmount(debitMap, body.user, settleMu, 'debit');
    addAmount(grossEarningMap, body.provider, settleMu, 'earning');
    usageLeaves.push(await opaqueHash('mayhem-usage-leaf-v1', envelope));
  }

  const earningEntries = [];
  let feeMu = 0;
  let earnCumMu = 0;
  for (const [provider, grossMu] of Array.from(grossEarningMap.entries()).sort(([a], [b]) => a.localeCompare(b))) {
    const providerFeeMu = Math.floor((grossMu * feeBps) / 10_000);
    const netMu = grossMu - providerFeeMu;
    feeMu += providerFeeMu;
    safeAmount(feeMu, 'fee_mu', { allowZero: true });
    const priorMu = priorEarnings[provider] ?? 0;
    safeAmount(priorMu, 'prior provider earning', { allowZero: true });
    const cumulative_mu = priorMu + netMu;
    safeAmount(cumulative_mu, 'provider cumulative earning', { allowZero: true });
    earnCumMu += cumulative_mu;
    safeAmount(earnCumMu, 'earn_mu', { allowZero: true });
    earningEntries.push({ provider, gross_mu: grossMu, net_mu: netMu, cumulative_mu });
  }
  const feeCumMu = priorFeeCumMu + feeMu;
  safeAmount(feeCumMu, 'fee_cum_mu', { allowZero: true });

  const payoutLeaves = [];
  let payMu = 0;
  for (const payout of payouts) {
    const mu = safeAmount(payout.mu, 'payout mu');
    payMu += mu;
    safeAmount(payMu, 'pay_mu', { allowZero: true });
    payoutLeaves.push(await opaqueHash('mayhem-payout-leaf-v1', stableValue(payout)));
  }

  const roots = {
    dep: dep.root,
    use: await merkleRoot('use', usageLeaves),
    earn: await merkleRoot(
      'earn',
      await Promise.all(earningEntries.map((entry) => opaqueHash('mayhem-earn-leaf-v1', entry)))
    ),
    fee: await opaqueHash('mayhem-fee-root-v1', { epoch, fee_mu: feeMu, fee_cum_mu: feeCumMu }),
    pay: await merkleRoot('pay', payoutLeaves),
  };
  for (const key of ROOT_KINDS) {
    if (!/^[0-9a-f]{64}$/.test(roots[key])) throw new Error(`invalid ${key} root`);
  }

  const debits = Array.from(debitMap.entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([user, mu]) => ({ user, mu }));
  const earnings = Array.from(grossEarningMap.entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([provider, gross_mu]) => ({ provider, gross_mu }));
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
    pay_count: payouts.length,
    pay_mu: payMu,
  };

  return { epoch, params: { fee_bps: feeBps }, roots, totals, debits, earnings };
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
