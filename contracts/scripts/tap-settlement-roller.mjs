#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

import { distribution } from './merkle.mjs';
import {
  TAP_ROLLER_SIGNER_ENV,
  walletFromEnv,
} from './signer-env.mjs';

const scriptPath = fileURLToPath(import.meta.url);
let cliEthProvider = null;
export const TAP_WEI = 1_000_000_000_000_000_000n;
export const BPS = 10_000n;
export const PROVIDER_BPS = 7_500n;
export const OPERATOR_BPS = 1_500n;
export const BURN_BPS = 1_000n;
const PROVIDER_CAP_TOLERANCE_WEI = 0n;

export const POOL_SETTLEMENT_ABI = [
  'function setRoot(bytes32 newRoot, uint256 newEpoch, uint256 newCumulativeSpent)',
  'function epoch() view returns (uint256)',
  'function cumulativeSpent() view returns (uint256)',
  'function totalDeposited() view returns (uint256)',
  'function maxEpochDelta() view returns (uint256)',
  'function merkleRoot() view returns (bytes32)',
  'function claimed(address account) view returns (uint256)',
];
const POOL_SETTLEMENT_INTERFACE = new ethers.Interface(POOL_SETTLEMENT_ABI);
export const TAP_ROLLER_SIGNER_ENVS = [TAP_ROLLER_SIGNER_ENV];

export function tapRollerWallet(provider, env = process.env) {
  return walletFromEnv(provider, {
    env,
    names: TAP_ROLLER_SIGNER_ENVS,
    label: 'TAP settlement roller private key',
  });
}

function errorMessage(error) {
  return error?.shortMessage || error?.reason || error?.message || String(error);
}

export function encodeSetRootCalldata({
  root,
  epoch,
  cumulativeSpentWei,
} = {}) {
  if (!root) throw new Error('Missing setRoot root');
  const safeEpoch = parseNonNegativeInt(epoch, 'setRoot epoch');
  const spent = parseBigIntString(cumulativeSpentWei, 'setRoot cumulative spent wei');
  return POOL_SETTLEMENT_INTERFACE.encodeFunctionData('setRoot', [root, BigInt(safeEpoch), spent]);
}

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

function parseOptionalNonNegativeInt(value, label) {
  if (value === undefined || value === null || value === '') return null;
  return parseNonNegativeInt(value, label);
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

function releasePolicy({ settleThroughEpoch, challengeEpochs = 0, holdbackEpochs = 0 } = {}) {
  const through = parseOptionalNonNegativeInt(settleThroughEpoch, 'settle through epoch');
  const challenge = parseNonNegativeInt(challengeEpochs, 'challenge epochs', 0);
  const holdback = parseNonNegativeInt(holdbackEpochs, 'holdback epochs', 0);
  return {
    settle_through_epoch: through,
    challenge_epochs: challenge,
    holdback_epochs: holdback,
    release_delay_epochs: Math.max(challenge, holdback),
  };
}

function entryEpoch(entry, body, bundleEpoch) {
  return entry.release_epoch
    ?? entry.receipt_epoch
    ?? entry.epoch
    ?? body.release_epoch
    ?? body.receipt_epoch
    ?? body.epoch
    ?? bundleEpoch;
}

function receiptReleaseInfo(entry, body, bundleEpoch, policy) {
  const explicitReleaseAfter = entry.release_after_epoch
    ?? entry.eligible_epoch
    ?? body.release_after_epoch
    ?? body.eligible_epoch;
  let releaseAfterEpoch = parseOptionalNonNegativeInt(explicitReleaseAfter, 'release_after_epoch');
  let receiptEpoch = parseOptionalNonNegativeInt(entryEpoch(entry, body, bundleEpoch), 'receipt epoch');
  if (releaseAfterEpoch === null && policy.release_delay_epochs > 0) {
    if (receiptEpoch === null) {
      throw new Error('receipt epoch required for delayed TAP settlement');
    }
    releaseAfterEpoch = receiptEpoch + policy.release_delay_epochs;
  }
  if (releaseAfterEpoch !== null && policy.settle_through_epoch === null) {
    throw new Error('settle_through_epoch required for delayed TAP settlement');
  }
  return {
    receipt_epoch: receiptEpoch,
    release_after_epoch: releaseAfterEpoch,
    eligible: releaseAfterEpoch === null || releaseAfterEpoch <= policy.settle_through_epoch,
  };
}

function parseEntryAmount(entry, label) {
  if (isObject(entry)) {
    return parseBigIntString(entry.amount ?? entry.cumulative_wei, label);
  }
  return parseBigIntString(entry, label);
}

export function settlementDistributionEntries(report, label = 'settlement') {
  const out = new Map();
  const source = report?.providers ?? report?.entries ?? report?.proofs ?? {};
  if (Array.isArray(source)) {
    for (const entry of source) {
      const account = normalizeAddress(entry.account, `${label} account`);
      out.set(account, parseEntryAmount(entry, `${label} cumulative wei`));
    }
  } else if (isObject(source)) {
    for (const [account, entry] of Object.entries(source)) {
      out.set(normalizeAddress(account, `${label} account`), parseEntryAmount(entry, `${label} cumulative wei`));
    }
  }
  return Array.from(out.entries())
    .map(([account, amount]) => ({ account, amount }))
    .sort((a, b) => a.account.localeCompare(b.account));
}

function priorProviderMap(prior) {
  const out = new Map();
  for (const entry of settlementDistributionEntries(prior, 'prior')) {
    out.set(entry.account, entry.amount);
  }
  return out;
}

export function buildTapSettlement({
  bundle,
  receipts,
  providerAccounts = {},
  tapUsdE6,
  prior = null,
  settleThroughEpoch = null,
  challengeEpochs = 0,
  holdbackEpochs = 0,
} = {}) {
  const inputBundle = receipts ? { receipts } : bundle;
  const input = receipts ?? normalizedReceipts(bundle);
  const rate = parsePositiveInt(tapUsdE6, 'tap_usd_e6');
  const policy = releasePolicy({ settleThroughEpoch, challengeEpochs, holdbackEpochs });
  const bundleEpoch = inputBundle?.epoch ?? inputBundle?.receipt_epoch ?? inputBundle?.settlement_epoch;
  const previousBySession = new Map();
  const perProvider = priorProviderMap(prior);
  let cumulativeSpentWei = parseBigIntString(prior?.cumulative_spent_wei ?? prior?.cumulativeSpentWei, 'prior cumulative spent wei');
  let receiptCount = 0;
  let heldReceiptCount = 0;
  let spentMu = 0;
  let heldMu = 0;

  const sorted = input
    .map(receiptEnvelope)
    .sort((a, b) => (
      String(a.body.session_id ?? '').localeCompare(String(b.body.session_id ?? '')) ||
      Number(a.body.seq ?? 0) - Number(b.body.seq ?? 0)
    ));

  for (const { entry, body } of sorted) {
    const deltaMu = receiptDeltaMu(entry, body, previousBySession);
    if (deltaMu === 0) continue;
    const release = receiptReleaseInfo(entry, body, bundleEpoch, policy);
    if (!release.eligible) {
      heldReceiptCount += 1;
      heldMu += deltaMu;
      safeMu(heldMu, 'held_mu', { allowZero: true });
      continue;
    }
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
      payout_model: 'non_custodial_claim',
      custodial_wallet: false,
      reason: heldReceiptCount > 0 ? 'no matured provider earnings' : 'no claimable provider earnings',
      tap_usd_e6: rate,
      receipt_count: receiptCount,
      held_receipt_count: heldReceiptCount,
      spent_mu: spentMu,
      held_mu: heldMu,
      cumulative_spent_wei: cumulativeSpentWei.toString(),
      entries: [],
      release_policy: policy,
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
    payout_model: 'non_custodial_claim',
    custodial_wallet: false,
    tap_usd_e6: rate,
    receipt_count: receiptCount,
    held_receipt_count: heldReceiptCount,
    spent_mu: spentMu,
    held_mu: heldMu,
    cumulative_spent_wei: cumulativeSpentWei.toString(),
    provider_claimed_wei: providerClaimedWei.toString(),
    provider_cap_wei: providerCapWei.toString(),
    root: dist.root,
    entries: entries.map((entry) => ({ account: entry.account, cumulative_wei: entry.amount.toString() })),
    proofs,
    release_policy: policy,
    dist,
  };
}

export async function guardianScreenSettlement({
  settlement,
  pool,
  epoch,
  previous = null,
} = {}) {
  return guardianPreSignReport({ settlement, pool, epoch, previous });
}

export async function dryRunSetRoot({
  writablePool,
  root,
  epoch,
  cumulativeSpentWei,
} = {}) {
  if (!writablePool?.setRoot) throw new Error('Missing writable pool contract');
  const safeEpoch = parseNonNegativeInt(epoch, 'setRoot epoch');
  const spent = parseBigIntString(cumulativeSpentWei, 'setRoot cumulative spent wei');
  const out = { ok: false, static_call_ok: false, gas_estimate: null };
  try {
    if (writablePool.setRoot.staticCall) {
      await writablePool.setRoot.staticCall(root, safeEpoch, spent);
      out.static_call_ok = true;
    }
    if (writablePool.setRoot.estimateGas) {
      out.gas_estimate = (await writablePool.setRoot.estimateGas(root, safeEpoch, spent)).toString();
    }
    out.ok = true;
    return out;
  } catch (error) {
    return { ...out, error: errorMessage(error) };
  }
}

function bpsOf(value, bps) {
  return (value * BigInt(bps)) / BPS;
}

async function poolAddressOrNull(pool) {
  if (!pool?.getAddress) return null;
  try {
    return normalizeAddress(await pool.getAddress(), 'pool address');
  } catch (_error) {
    return null;
  }
}

export async function guardianPreSignReport({
  settlement,
  pool,
  epoch,
  previous = null,
  currentEpoch,
  prevSpentWei,
  totalDepositedWei,
  maxEpochDeltaWei,
} = {}) {
  if (!settlement?.root) return { ok: false, reasons: ['missing settlement root'] };
  const reasons = [];
  const flags = [];
  const priorEpoch = currentEpoch !== undefined
    ? parseNonNegativeInt(currentEpoch, 'current epoch')
    : pool
      ? Number(await pool.epoch())
      : parseNonNegativeInt(previous?.epoch ?? previous?.prev_epoch ?? 0, 'previous epoch', 0);
  const newEpoch = epoch ?? priorEpoch + 1;
  const cumulativeSpentWei = parseBigIntString(
    settlement.cumulative_spent_wei ?? settlement.cumulativeSpentWei,
    'settlement cumulative spent wei'
  );
  const previousSpentWei = prevSpentWei !== undefined
    ? parseBigIntString(prevSpentWei, 'previous spent wei')
    : pool
      ? await pool.cumulativeSpent()
      : parseBigIntString(previous?.cumulative_spent_wei ?? previous?.cumulativeSpentWei, 'previous spent wei');
  const depositedWei = totalDepositedWei !== undefined
    ? parseBigIntString(totalDepositedWei, 'total deposited wei')
    : pool
      ? await pool.totalDeposited()
      : cumulativeSpentWei;
  const epochDeltaCapWei = maxEpochDeltaWei !== undefined
    ? parseBigIntString(maxEpochDeltaWei, 'max epoch delta wei')
    : pool
      ? await pool.maxEpochDelta()
      : 0n;
  const poolAddress = await poolAddressOrNull(pool);

  if (!Number.isSafeInteger(newEpoch) || newEpoch <= priorEpoch) reasons.push('epoch !monotonic');
  if (cumulativeSpentWei < previousSpentWei) reasons.push('spent !monotonic');
  if (cumulativeSpentWei === previousSpentWei) reasons.push('no new spend since last root');
  if (cumulativeSpentWei > depositedWei) reasons.push('spent > deposited');
  if (epochDeltaCapWei > 0n && cumulativeSpentWei >= previousSpentWei && cumulativeSpentWei - previousSpentWei > epochDeltaCapWei) {
    reasons.push('epoch delta > cap');
  }

  let entries = [];
  try {
    entries = settlementDistributionEntries(settlement, 'settlement');
  } catch (error) {
    reasons.push(error?.message || 'invalid settlement entries');
  }
  const previousByAccount = priorProviderMap(previous);
  let totalOwedWei = 0n;
  for (const entry of entries) {
    if (entry.amount <= 0n) reasons.push(`non-positive amount for ${entry.account}`);
    if (entry.amount > depositedWei) reasons.push(`amount for ${entry.account} exceeds deposited`);
    if (entry.account === '0x0000000000000000000000000000000000000000') reasons.push('zero account in settlement');
    if (poolAddress && entry.account === poolAddress) reasons.push('pool account in settlement');
    const previousAmount = previousByAccount.get(entry.account);
    if (previousAmount !== undefined && entry.amount < previousAmount) {
      reasons.push(`cumulative for ${entry.account} decreased`);
    }
    totalOwedWei += entry.amount;
  }

  const providerCapWei = bpsOf(cumulativeSpentWei, PROVIDER_BPS);
  const operatorCapWei = bpsOf(cumulativeSpentWei, OPERATOR_BPS);
  const burnCapWei = bpsOf(cumulativeSpentWei, BURN_BPS);
  if (totalOwedWei > providerCapWei + PROVIDER_CAP_TOLERANCE_WEI) {
    reasons.push('provider owed > 75% spent cap');
  } else if (totalOwedWei > providerCapWei) {
    flags.push('provider owed exceeds exact 75% cap only by rounding tolerance');
  }
  if (totalOwedWei + operatorCapWei + burnCapWei > depositedWei) {
    reasons.push('owed + operator cap + burn cap > deposited');
  }

  return {
    ok: reasons.length === 0,
    reasons,
    flags,
    epoch: newEpoch,
    prev_epoch: priorEpoch,
    cumulative_spent_wei: cumulativeSpentWei.toString(),
    prev_spent_wei: previousSpentWei.toString(),
    total_deposited_wei: depositedWei.toString(),
    max_epoch_delta_wei: epochDeltaCapWei.toString(),
    total_owed_wei: totalOwedWei.toString(),
    provider_cap_wei: providerCapWei.toString(),
    operator_cap_wei: operatorCapWei.toString(),
    burn_cap_wei: burnCapWei.toString(),
    entries_count: entries.length,
  };
}

export async function rollTapSettlement({
  bundle,
  receipts,
  providerAccounts,
  tapUsdE6,
  prior,
  settleThroughEpoch,
  challengeEpochs,
  holdbackEpochs,
  pool,
  ownerSigner,
  epoch,
  post = true,
} = {}) {
  const settlement = buildTapSettlement({
    bundle,
    receipts,
    providerAccounts,
    tapUsdE6,
    prior,
    settleThroughEpoch,
    challengeEpochs,
    holdbackEpochs,
  });
  if (!settlement.root) return settlement;
  const screen = await guardianScreenSettlement({ settlement, pool, epoch, previous: prior });
  if (!screen.ok) {
    return { ...settlement, posted: false, blocked: true, reasons: screen.reasons, screen };
  }
  const setRootCalldata = encodeSetRootCalldata({
    root: settlement.root,
    epoch: screen.epoch,
    cumulativeSpentWei: settlement.cumulative_spent_wei,
  });
  let signingAddress = null;
  if (ownerSigner?.getAddress) {
    signingAddress = normalizeAddress(await ownerSigner.getAddress(), 'signing address');
  }
  let writable = null;
  let setRootDryRun = null;
  if (pool && ownerSigner) {
    writable = pool.connect ? pool.connect(ownerSigner) : pool;
    setRootDryRun = await dryRunSetRoot({
      writablePool: writable,
      root: settlement.root,
      epoch: screen.epoch,
      cumulativeSpentWei: settlement.cumulative_spent_wei,
    });
    if (!setRootDryRun.ok) {
      return {
        ...settlement,
        posted: false,
        blocked: true,
        reasons: [`setRoot dry-run failed: ${setRootDryRun.error}`],
        epoch: screen.epoch,
        signing_address: signingAddress,
        set_root_calldata: setRootCalldata,
        set_root_dry_run: setRootDryRun,
        screen,
      };
    }
  }
  let tx = null;
  if (post && pool) {
    if (!ownerSigner) throw new Error('Missing TAP settlement owner signer for broadcast');
    if (!writable) writable = pool.connect ? pool.connect(ownerSigner) : pool;
    const sent = await writable.setRoot(settlement.root, screen.epoch, BigInt(settlement.cumulative_spent_wei));
    await sent.wait();
    tx = sent.hash;
  }
  return {
    ...settlement,
    posted: Boolean(tx),
    epoch: screen.epoch,
    tx,
    signing_address: signingAddress,
    set_root_calldata: setRootCalldata,
    set_root_dry_run: setRootDryRun,
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
  settleThroughEpoch,
  challengeEpochs,
  holdbackEpochs,
  ethRpc,
  poolAddress,
  epoch,
  confirm,
  json,
} = {}) {
  const args = ['node', 'contracts/scripts/tap-settlement-roller.mjs'];
  if (bundlePath) args.push('--bundle', bundlePath);
  if (providerAccountsPath) args.push('--provider-accounts', providerAccountsPath);
  if (tapUsdE6) args.push('--tap-usd-e6', String(tapUsdE6));
  if (priorPath) args.push('--prior', priorPath);
  if (settleThroughEpoch !== undefined && settleThroughEpoch !== null) {
    args.push('--settle-through-epoch', String(settleThroughEpoch));
  }
  if (challengeEpochs !== undefined && challengeEpochs !== null) {
    args.push('--challenge-epochs', String(challengeEpochs));
  }
  if (holdbackEpochs !== undefined && holdbackEpochs !== null) {
    args.push('--holdback-epochs', String(holdbackEpochs));
  }
  if (ethRpc) args.push('--eth-rpc', ethRpc);
  if (poolAddress) args.push('--pool', poolAddress);
  if (epoch) args.push('--epoch', String(epoch));
  if (confirm) args.push('--confirm');
  if (json) args.push('--json');
  return args.map(shellQuote).join(' ');
}

async function main() {
  const args = parseArgs();
  if (args.post !== undefined) {
    throw new Error('--post has been retired. Run the dry-run without it, inspect the signer/calldata, then add --confirm to broadcast.');
  }
  if (args['owner-index'] !== undefined) {
    throw new Error(`--owner-index has been retired. Set ${TAP_ROLLER_SIGNER_ENV} in the environment instead.`);
  }
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
  const confirm = boolArg(args.confirm, false);
  const json = boolArg(args.json, false);
  let pool = null;
  let ownerSigner = null;
  let signerEnvName = null;
  if (confirm || ethRpc || poolAddress) {
    if (!ethRpc) throw new Error('Missing --eth-rpc or MAYHEM_TAP_ETH_RPC.');
    if (!poolAddress) throw new Error('Missing --pool or MAYHEM_TAP_POOL_ADDRESS.');
    cliEthProvider = new ethers.JsonRpcProvider(ethRpc);
    const signer = tapRollerWallet(cliEthProvider);
    ownerSigner = signer.wallet;
    signerEnvName = signer.envName;
    pool = new ethers.Contract(poolAddress, POOL_SETTLEMENT_ABI, cliEthProvider);
  }

  const report = await rollTapSettlement({
    bundle,
    providerAccounts,
    prior,
    tapUsdE6,
    settleThroughEpoch: args['settle-through-epoch'],
    challengeEpochs: args['challenge-epochs'] ?? 0,
    holdbackEpochs: args['holdback-epochs'] ?? 0,
    pool,
    ownerSigner,
    epoch: args.epoch ? parsePositiveInt(args.epoch, '--epoch') : undefined,
    post: confirm,
  });
  if (signerEnvName) report.signer_env = signerEnvName;
  report.copy_paste_replay_command = buildReplayCommand({
    bundlePath,
    providerAccountsPath,
    tapUsdE6,
    priorPath,
    settleThroughEpoch: args['settle-through-epoch'],
    challengeEpochs: args['challenge-epochs'],
    holdbackEpochs: args['holdback-epochs'],
    ethRpc,
    poolAddress,
    epoch: report.epoch,
    confirm,
    json: true,
  });
  if (!confirm && report.root && ethRpc && poolAddress) {
    report.copy_paste_confirm_command = buildReplayCommand({
      bundlePath,
      providerAccountsPath,
      tapUsdE6,
      priorPath,
      settleThroughEpoch: args['settle-through-epoch'],
      challengeEpochs: args['challenge-epochs'],
      holdbackEpochs: args['holdback-epochs'],
      ethRpc,
      poolAddress,
      epoch: report.epoch,
      confirm: true,
      json: true,
    });
  }
  delete report.dist;

  if (json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('[tap:settlement] root:', report.root ?? 'none');
    console.log('[tap:settlement] epoch:', report.epoch ?? 'none');
    console.log('[tap:settlement] cumulative_spent_wei:', report.cumulative_spent_wei);
    if (report.signing_address) console.log('[tap:settlement] signing_address:', report.signing_address);
    if (report.set_root_calldata) console.log('[tap:settlement] setRoot calldata:', report.set_root_calldata);
    if (report.set_root_dry_run) {
      console.log('[tap:settlement] setRoot dry-run:', JSON.stringify(report.set_root_dry_run));
    }
    console.log('[tap:settlement] posted:', report.posted);
    if (report.blocked) console.log('[tap:settlement] blocked:', report.reasons.join('; '));
    console.log('Copy/paste TAP settlement replay command:');
    console.log(report.copy_paste_replay_command);
    if (report.copy_paste_confirm_command) {
      console.log('Copy/paste TAP settlement broadcast command after inspecting the dry-run:');
      console.log(report.copy_paste_confirm_command);
    }
  }
  if (report.blocked) process.exitCode = 2;
  if (cliEthProvider?.destroy) {
    cliEthProvider.destroy();
    cliEthProvider = null;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    if (cliEthProvider?.destroy) cliEthProvider.destroy();
    console.error(error?.stack || error?.message || String(error));
    process.exit(1);
  });
}
