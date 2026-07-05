#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

import { distribution } from './merkle.mjs';

const scriptPath = fileURLToPath(import.meta.url);
const TAP_WEI = 1_000_000_000_000_000_000n;
const BPS = 10_000n;
const PROVIDER_BPS = 7_500n;

export const POOL_SETTLEMENT_ABI = [
  'function setRoot(bytes32 newRoot, uint256 newEpoch, uint256 newCumulativeSpent)',
  'function epoch() view returns (uint256)',
  'function cumulativeSpent() view returns (uint256)',
  'function totalDeposited() view returns (uint256)',
  'function maxEpochDelta() view returns (uint256)',
  'function merkleRoot() view returns (bytes32)',
];

function shellQuote(value) {
  const raw = String(value ?? '');
  if (raw.length === 0) return "''";
  return `'${raw.replaceAll("'", "'\\''")}'`;
}

function parseArgs(argv = process.argv.slice(2)) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) continue;
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) {
      out[key] = true;
    } else {
      out[key] = next;
      i += 1;
    }
  }
  return out;
}

function boolArg(value, fallback = false) {
  if (value === undefined) return fallback;
  if (value === true) return true;
  const text = String(value).trim().toLowerCase();
  return ['1', 'true', 'yes', 'on'].includes(text);
}

function parseNonNegativeInt(value, label, fallback = null) {
  if (value === undefined || value === null || value === '') {
    if (fallback !== null) return fallback;
    throw new Error(`Missing ${label}`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative safe integer`);
  }
  return parsed;
}

function parsePositiveInt(value, label, fallback = null) {
  const parsed = parseNonNegativeInt(value, label, fallback);
  if (parsed <= 0) throw new Error(`${label} must be positive`);
  return parsed;
}

function parseBigIntString(value, label, fallback = 0n) {
  if (value === undefined || value === null || value === '') return fallback;
  try {
    const parsed = BigInt(String(value).trim());
    if (parsed < 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a non-negative integer string`);
  }
}

function readJson(file, label) {
  const value = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (value === null || value === undefined) throw new Error(`${label} is empty`);
  return value;
}

function isObject(value) {
  return value && typeof value === 'object' && !Array.isArray(value);
}

function normalizeAddress(value, label) {
  try {
    return ethers.getAddress(String(value ?? '')).toLowerCase();
  } catch (_error) {
    throw new Error(`${label} must be an Ethereum address`);
  }
}

function maybeAddress(value) {
  try {
    return ethers.getAddress(String(value ?? '')).toLowerCase();
  } catch (_error) {
    return null;
  }
}

function safeMu(value, label, { allowZero = false } = {}) {
  if (!Number.isSafeInteger(value) || value < 0 || (!allowZero && value === 0)) {
    throw new Error(`${label} must be a ${allowZero ? 'non-negative' : 'positive'} safe integer`);
  }
  return value;
}

function addWei(map, account, amountWei) {
  if (amountWei <= 0n) return;
  map.set(account, (map.get(account) ?? 0n) + amountWei);
}

export function muToTapWei(mu, tapUsdE6) {
  const safe = safeMu(mu, 'mu', { allowZero: true });
  const rate = parsePositiveInt(tapUsdE6, 'tap_usd_e6');
  return (BigInt(safe) * TAP_WEI) / BigInt(rate);
}

export function providerShareWei(spentWei, weightBps = 10_000) {
  const spent = parseBigIntString(spentWei, 'spent wei');
  const weight = parseNonNegativeInt(weightBps, 'provider weight bps');
  if (weight > 10_000) throw new Error('provider weight bps must be <= 10000');
  return (spent * PROVIDER_BPS * BigInt(weight)) / (BPS * BPS);
}

function receiptEnvelope(entry) {
  const receipt = entry.receipt ?? entry;
  const body = receipt.body ?? receipt;
  if (!isObject(body)) throw new Error('receipt body must be an object');
  return { entry, body };
}

function receiptDeltaMu(entry, body, previousBySession) {
  const explicit = entry.settle_mu ?? entry.mu_delta;
  if (explicit !== undefined) return safeMu(explicit, 'receipt settle_mu');
  const current = safeMu(body.mu_owed_cum, 'receipt mu_owed_cum');
  const session = body.session_id;
  if (typeof session !== 'string' || session.length === 0) throw new Error('receipt session_id is required');
  const previous = entry.previous_mu_owed_cum ?? previousBySession.get(session) ?? 0;
  safeMu(previous, 'receipt previous_mu_owed_cum', { allowZero: true });
  if (current < previous) throw new Error('receipt cumulative mu regressed');
  previousBySession.set(session, current);
  return current - previous;
}

function providerAccountFor(providerId, entry, body, providerAccounts) {
  const direct = entry.provider_tap_address
    ?? entry.tap_address
    ?? body.provider_tap_address
    ?? body.tap_address;
  if (direct) return normalizeAddress(direct, 'provider TAP address');
  if (providerAccounts && providerAccounts[providerId]) {
    return normalizeAddress(providerAccounts[providerId], `provider account for ${providerId}`);
  }
  const providerAsAddress = maybeAddress(providerId);
  if (providerAsAddress) return providerAsAddress;
  throw new Error(`Missing TAP claim address for provider ${providerId}`);
}

function receiptProviders(entry, body, providerAccounts) {
  const refs = entry.provider_refs ?? body.provider_refs;
  if (refs !== undefined) {
    if (!Array.isArray(refs) || refs.length === 0) throw new Error('provider_refs must be a non-empty array');
    const weights = entry.contribution_weights_bps ?? body.contribution_weights_bps;
    if (weights !== undefined && (!Array.isArray(weights) || weights.length !== refs.length)) {
      throw new Error('contribution_weights_bps must match provider_refs length');
    }
    const evenWeight = Math.floor(10_000 / refs.length);
    return refs.map((providerId, index) => {
      if (typeof providerId !== 'string' || providerId.length === 0) {
        throw new Error('provider_refs entries must be non-empty strings');
      }
      return {
        provider_id: providerId,
        account: providerAccountFor(providerId, entry, body, providerAccounts),
        weight_bps: weights === undefined
          ? (index === refs.length - 1 ? 10_000 - evenWeight * (refs.length - 1) : evenWeight)
          : parseNonNegativeInt(weights[index], 'provider weight bps'),
      };
    });
  }
  const providerId = body.provider;
  if (typeof providerId !== 'string' || providerId.length === 0) {
    throw new Error('receipt provider is required');
  }
  return [{
    provider_id: providerId,
    account: providerAccountFor(providerId, entry, body, providerAccounts),
    weight_bps: 10_000,
  }];
}

function normalizedReceipts(bundleOrReceipts) {
  if (Array.isArray(bundleOrReceipts)) return bundleOrReceipts;
  if (Array.isArray(bundleOrReceipts?.receipts)) return bundleOrReceipts.receipts;
  if (Array.isArray(bundleOrReceipts?.data)) return bundleOrReceipts.data;
  throw new Error('settlement input must be a receipt array or object with receipts[]/data[]');
}

function priorProviderMap(prior) {
  const out = new Map();
  const source = prior?.providers ?? prior?.entries ?? {};
  if (Array.isArray(source)) {
    for (const entry of source) {
      const account = normalizeAddress(entry.account, 'prior account');
      out.set(account, parseBigIntString(entry.amount ?? entry.cumulative_wei, 'prior cumulative wei'));
    }
  } else if (isObject(source)) {
    for (const [account, amount] of Object.entries(source)) {
      out.set(normalizeAddress(account, 'prior account'), parseBigIntString(amount, 'prior cumulative wei'));
    }
  }
  return out;
}

export function buildTapSettlement({
  bundle,
  receipts,
  providerAccounts = {},
  tapUsdE6,
  prior = null,
} = {}) {
  const input = receipts ?? normalizedReceipts(bundle);
  const rate = parsePositiveInt(tapUsdE6, 'tap_usd_e6');
  const previousBySession = new Map();
  const perProvider = priorProviderMap(prior);
  let cumulativeSpentWei = parseBigIntString(prior?.cumulative_spent_wei ?? prior?.cumulativeSpentWei, 'prior cumulative spent wei');
  let receiptCount = 0;
  let spentMu = 0;

  const sorted = input
    .map(receiptEnvelope)
    .sort((a, b) => (
      String(a.body.session_id ?? '').localeCompare(String(b.body.session_id ?? '')) ||
      Number(a.body.seq ?? 0) - Number(b.body.seq ?? 0)
    ));

  for (const { entry, body } of sorted) {
    const deltaMu = receiptDeltaMu(entry, body, previousBySession);
    if (deltaMu === 0) continue;
    const spentWei = muToTapWei(deltaMu, rate);
    if (spentWei <= 0n) throw new Error('receipt converts to zero TAP wei');
    receiptCount += 1;
    spentMu += deltaMu;
    safeMu(spentMu, 'spent_mu', { allowZero: true });
    cumulativeSpentWei += spentWei;
    const providers = receiptProviders(entry, body, providerAccounts);
    let weightTotal = 0;
    for (const provider of providers) weightTotal += provider.weight_bps;
    if (weightTotal !== 10_000) throw new Error('provider weights must sum to 10000 bps');
    for (const provider of providers) {
      addWei(perProvider, provider.account, providerShareWei(spentWei, provider.weight_bps));
    }
  }

  const entries = Array.from(perProvider.entries())
    .filter(([, amount]) => amount > 0n)
    .map(([account, amount]) => ({ account, amount }))
    .sort((a, b) => a.account.localeCompare(b.account));
  if (entries.length === 0) {
    return {
      posted: false,
      reason: 'no claimable provider earnings',
      tap_usd_e6: rate,
      receipt_count: receiptCount,
      spent_mu: spentMu,
      cumulative_spent_wei: cumulativeSpentWei.toString(),
      entries: [],
    };
  }

  const dist = distribution(entries);
  const proofs = Object.fromEntries(entries.map((entry) => [
    entry.account,
    {
      account: entry.account,
      cumulative_wei: entry.amount.toString(),
      proof: dist.proofFor(entry.account),
      leaf: dist.leafFor(entry.account),
    },
  ]));
  const providerClaimedWei = entries.reduce((sum, entry) => sum + entry.amount, 0n);
  const providerCapWei = (cumulativeSpentWei * PROVIDER_BPS) / BPS;
  if (providerClaimedWei > providerCapWei) {
    throw new Error('provider distribution exceeds 75% TAP settlement cap');
  }

  return {
    posted: false,
    tap_usd_e6: rate,
    receipt_count: receiptCount,
    spent_mu: spentMu,
    cumulative_spent_wei: cumulativeSpentWei.toString(),
    provider_claimed_wei: providerClaimedWei.toString(),
    provider_cap_wei: providerCapWei.toString(),
    root: dist.root,
    entries: entries.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
    proofs,
    dist,
  };
}

export async function guardianScreenSettlement({
  settlement,
  pool,
  epoch,
} = {}) {
  if (!settlement?.root) return { ok: false, reasons: ['missing settlement root'] };
  const reasons = [];
  const currentEpoch = pool ? Number(await pool.epoch()) : 0;
  const newEpoch = epoch ?? currentEpoch + 1;
  const cumulativeSpentWei = BigInt(settlement.cumulative_spent_wei);
  const prevSpentWei = pool ? await pool.cumulativeSpent() : 0n;
  const totalDepositedWei = pool ? await pool.totalDeposited() : cumulativeSpentWei;
  const maxEpochDeltaWei = pool ? await pool.maxEpochDelta() : 0n;

  if (!Number.isSafeInteger(newEpoch) || newEpoch <= currentEpoch) reasons.push('epoch !monotonic');
  if (cumulativeSpentWei <= prevSpentWei) reasons.push('no new spend since last root');
  if (cumulativeSpentWei > totalDepositedWei) reasons.push('spent > deposited');
  if (maxEpochDeltaWei > 0n && cumulativeSpentWei - prevSpentWei > maxEpochDeltaWei) {
    reasons.push('epoch delta > cap');
  }
  return {
    ok: reasons.length === 0,
    reasons,
    epoch: newEpoch,
    prev_epoch: currentEpoch,
    prev_spent_wei: prevSpentWei.toString(),
    total_deposited_wei: totalDepositedWei.toString(),
    max_epoch_delta_wei: maxEpochDeltaWei.toString(),
  };
}

export async function rollTapSettlement({
  bundle,
  receipts,
  providerAccounts,
  tapUsdE6,
  prior,
  pool,
  ownerSigner,
  epoch,
  post = true,
} = {}) {
  const settlement = buildTapSettlement({ bundle, receipts, providerAccounts, tapUsdE6, prior });
  if (!settlement.root) return settlement;
  const screen = await guardianScreenSettlement({ settlement, pool, epoch });
  if (!screen.ok) {
    return { ...settlement, posted: false, blocked: true, reasons: screen.reasons, screen };
  }
  let tx = null;
  if (post && pool) {
    const writable = ownerSigner && pool.connect ? pool.connect(ownerSigner) : pool;
    const sent = await writable.setRoot(settlement.root, screen.epoch, BigInt(settlement.cumulative_spent_wei));
    await sent.wait();
    tx = sent.hash;
  }
  return {
    ...settlement,
    posted: Boolean(tx),
    epoch: screen.epoch,
    tx,
    screen,
  };
}

function ensureRpcBase(url) {
  const raw = String(url ?? '').trim();
  if (!raw) throw new Error('Missing peer RPC URL');
  return raw.endsWith('/') ? raw : `${raw}/`;
}

async function fetchJson(url, fetchImpl = globalThis.fetch) {
  if (typeof fetchImpl !== 'function') throw new Error('fetch is unavailable');
  const response = await fetchImpl(url);
  if (!response?.ok) {
    throw new Error(`GET ${url} failed with ${response?.status ?? 'unknown status'}`);
  }
  return await response.json();
}

async function readContractStateValue(rpcUrl, key, {
  confirmed = false,
  fetchImpl = globalThis.fetch,
} = {}) {
  const url = new URL('state', ensureRpcBase(rpcUrl));
  url.searchParams.set('key', key);
  url.searchParams.set('confirmed', confirmed ? 'true' : 'false');
  const body = await fetchJson(url, fetchImpl);
  return body?.value ?? null;
}

async function resolveTapUsdE6({ tapUsdE6, peerRpcUrl, fetchImpl } = {}) {
  if (tapUsdE6 !== undefined && tapUsdE6 !== null && tapUsdE6 !== '') {
    return parsePositiveInt(tapUsdE6, '--tap-usd-e6');
  }
  const rate = await readContractStateValue(peerRpcUrl, 'tap/rate/latest', { fetchImpl });
  return parsePositiveInt(rate?.tap_usd_e6, 'tap/rate/latest.tap_usd_e6');
}

function buildReplayCommand({
  bundlePath,
  providerAccountsPath,
  tapUsdE6,
  priorPath,
  ethRpc,
  poolAddress,
  epoch,
  post,
  json,
} = {}) {
  const args = ['node', 'contracts/scripts/tap-settlement-roller.mjs'];
  if (bundlePath) args.push('--bundle', bundlePath);
  if (providerAccountsPath) args.push('--provider-accounts', providerAccountsPath);
  if (tapUsdE6) args.push('--tap-usd-e6', String(tapUsdE6));
  if (priorPath) args.push('--prior', priorPath);
  if (ethRpc) args.push('--eth-rpc', ethRpc);
  if (poolAddress) args.push('--pool', poolAddress);
  if (epoch) args.push('--epoch', String(epoch));
  if (post) args.push('--post');
  if (json) args.push('--json');
  return args.map(shellQuote).join(' ');
}

async function main() {
  const args = parseArgs();
  const bundlePath = args.bundle || args['receipts-file'];
  if (!bundlePath) throw new Error('Missing --bundle/--receipts-file.');
  const bundle = readJson(path.resolve(bundlePath), 'receipt bundle');
  const providerAccountsPath = args['provider-accounts'];
  const providerAccounts = providerAccountsPath
    ? readJson(path.resolve(providerAccountsPath), 'provider accounts')
    : {};
  const priorPath = args.prior;
  const prior = priorPath ? readJson(path.resolve(priorPath), 'prior settlement') : null;
  const peerRpcUrl = args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC;
  const tapUsdE6 = await resolveTapUsdE6({
    tapUsdE6: args['tap-usd-e6'],
    peerRpcUrl,
  });

  const ethRpc = args['eth-rpc'] || args.rpc || process.env.MAYHEM_TAP_ETH_RPC;
  const poolAddress = args.pool || process.env.MAYHEM_TAP_POOL_ADDRESS;
  const post = boolArg(args.post, false);
  const json = boolArg(args.json, false);
  let pool = null;
  let ownerSigner = null;
  if (post || ethRpc || poolAddress) {
    if (!ethRpc) throw new Error('Missing --eth-rpc or MAYHEM_TAP_ETH_RPC.');
    if (!poolAddress) throw new Error('Missing --pool or MAYHEM_TAP_POOL_ADDRESS.');
    const provider = new ethers.JsonRpcProvider(ethRpc);
    ownerSigner = await provider.getSigner(parseNonNegativeInt(args['owner-index'] ?? 0, '--owner-index'));
    pool = new ethers.Contract(poolAddress, POOL_SETTLEMENT_ABI, provider);
  }

  const report = await rollTapSettlement({
    bundle,
    providerAccounts,
    prior,
    tapUsdE6,
    pool,
    ownerSigner,
    epoch: args.epoch ? parsePositiveInt(args.epoch, '--epoch') : undefined,
    post,
  });
  report.copy_paste_replay_command = buildReplayCommand({
    bundlePath,
    providerAccountsPath,
    tapUsdE6,
    priorPath,
    ethRpc,
    poolAddress,
    epoch: report.epoch,
    post,
    json: true,
  });
  delete report.dist;

  if (json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('[tap:settlement] root:', report.root ?? 'none');
    console.log('[tap:settlement] epoch:', report.epoch ?? 'none');
    console.log('[tap:settlement] cumulative_spent_wei:', report.cumulative_spent_wei);
    console.log('[tap:settlement] posted:', report.posted);
    if (report.blocked) console.log('[tap:settlement] blocked:', report.reasons.join('; '));
    console.log('Copy/paste TAP settlement replay command:');
    console.log(report.copy_paste_replay_command);
  }
  if (report.blocked) process.exitCode = 2;
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(error?.stack || error?.message || String(error));
    process.exit(1);
  });
}
